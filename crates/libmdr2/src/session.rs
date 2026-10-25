//! A transport-independent stop-and-wait session for reliable MDR v2 frames.
//!
//! The caller owns transport I/O and a monotonic clock. Each [`SessionEvent::Transmit`]
//! requests one complete frame write. Report its success with
//! [`Session::complete_write`] or failure with [`Session::fail_write`], using the
//! supplied token. The session never requests overlapping writes, including ACKs.
//! A transport must enforce its own write timeout; the ACK timeout starts only
//! after a complete write succeeds.
//!
//! Link acknowledgement is separate from command responses and device state.
//! Reconnection requires a new session and isolation from the old connection's
//! callbacks. Unacknowledged SHOT frames and unknown frame types are unsupported.

#![doc = include_str!("../docs/session.md")]

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

use crate::command::ProtocolInfo;
use crate::frame::{DataType, Frame, Sequence};
use crate::types::CommandTable;

/// Maximum queued ACK writes, excluding a write already in progress.
pub const MAX_PENDING_ACKS: usize = 32;

/// ACK waiting policy for ordinary reliable MDR v2 data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetryPolicy {
    ack_timeout: Duration,
    max_retries: u32,
}

impl RetryPolicy {
    /// Creates a policy. `max_retries` counts retransmissions after the first send.
    pub fn new(ack_timeout: Duration, max_retries: u32) -> Result<Self, SessionError> {
        if ack_timeout.is_zero() {
            return Err(SessionError::ZeroAckTimeout);
        }
        Ok(Self {
            ack_timeout,
            max_retries,
        })
    }

    /// Time to wait after each successful frame write.
    pub fn ack_timeout(self) -> Duration {
        self.ack_timeout
    }

    /// Maximum number of retransmissions, excluding the first send.
    pub fn max_retries(self) -> u32 {
        self.max_retries
    }
}

impl Default for RetryPolicy {
    /// Waits 750 ms per attempt and permits ten additional transmissions.
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_millis(750),
            max_retries: 10,
        }
    }
}

/// Identifies one write attempt within a session.
///
/// Tokens from different sessions are not interchangeable. The caller must keep
/// callbacks associated with their original session/connection generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteToken(u64);

/// A terminal reason. A closed session cannot be reused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CloseReason {
    /// The caller cancelled the connection/session.
    Cancelled,
    /// A complete frame write failed, possibly after a partial transport write.
    WriteFailed,
    /// No matching ACK arrived within the configured number of attempts.
    RetriesExhausted,
    /// The caller could not drain ACK writes before the bounded queue filled.
    AckQueueFull,
    /// The requested next deadline cannot be represented by `Duration`.
    ClockOverflow,
    /// The write-token counter cannot represent another attempt.
    WriteTokenExhausted,
}

/// An effect produced by one state-machine input.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// Write this frame in full, then report completion using `token`.
    Transmit {
        /// Pass this token back when the complete write succeeds or fails.
        token: WriteToken,
        /// The frame to encode and write without interleaving other frames.
        frame: Frame,
    },
    /// An incoming reliable frame delivered independently of any outgoing request.
    Received(Frame),
    /// The outgoing frame was link-acknowledged; this is not command success.
    LinkAcknowledged {
        /// The outgoing command's table.
        table: CommandTable,
        /// The sequence of the acknowledged data frame, not the ACK sequence.
        sequence: Sequence,
    },
    /// All outstanding work was abandoned. Close the underlying transport.
    Closed(CloseReason),
}

/// Invalid input to the session. Rejected inputs do not change session state.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionError {
    /// A retry policy requires a positive ACK wait duration.
    ZeroAckTimeout,
    /// This session has already terminated; create a new one after reconnecting.
    Closed,
    /// An outgoing data frame is already queued or awaiting acknowledgement.
    Busy,
    /// Reliable sessions support only command tables 1 and 2.
    UnsupportedTable(CommandTable),
    /// The frame type has no supported session semantics.
    UnsupportedDataType(DataType),
    /// Reliable data must contain a command payload.
    EmptyPayload,
    /// Link ACK frames have no payload.
    NonEmptyAckPayload,
    /// The callback does not identify the currently active transport write.
    UnknownWriteToken(WriteToken),
    /// The caller supplied a timestamp older than its last accepted input.
    TimeWentBackwards {
        /// Timestamp of the previous accepted input.
        previous: Duration,
        /// Timestamp of the rejected input.
        received: Duration,
    },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroAckTimeout => f.write_str("ACK timeout must be greater than zero"),
            Self::Closed => f.write_str("session is closed"),
            Self::Busy => f.write_str("a reliable frame is already outstanding"),
            Self::UnsupportedTable(table) => write!(f, "unsupported command table: {table:?}"),
            Self::UnsupportedDataType(data_type) => {
                write!(f, "unsupported session data type: {data_type:?}")
            }
            Self::EmptyPayload => f.write_str("reliable data requires a command payload"),
            Self::NonEmptyAckPayload => f.write_str("ACK payload must be empty"),
            Self::UnknownWriteToken(token) => write!(f, "write token is not active: {token:?}"),
            Self::TimeWentBackwards { previous, received } => {
                write!(f, "clock moved backwards from {previous:?} to {received:?}")
            }
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Debug)]
enum OutboundPhase {
    Queued { retry: bool },
    Writing { acknowledged: bool },
    AwaitingAck { deadline: Duration },
}

#[derive(Debug)]
struct Outbound {
    table: CommandTable,
    frame: Frame,
    retries: u32,
    phase: OutboundPhase,
}

#[derive(Clone, Copy, Debug)]
enum WriteKind {
    Ack,
    Data,
}

#[derive(Debug)]
struct ActiveWrite {
    token: WriteToken,
    kind: WriteKind,
}

/// One reliable MDR v2 session, with no I/O or runtime dependency.
///
/// At most one outgoing data frame and [`MAX_PENDING_ACKS`] queued ACKs are
/// retained. Each input returns only its immediate effects; there is no unbounded
/// internal event queue. Pass elapsed monotonic time from a caller-chosen origin.
#[derive(Debug)]
pub struct Session {
    policy: RetryPolicy,
    tx_sequence: Sequence,
    last_rx_sequence: Option<Sequence>,
    outbound: Option<Outbound>,
    active_write: Option<ActiveWrite>,
    pending_acks: VecDeque<Frame>,
    last_time: Duration,
    next_token: u64,
    closed: bool,
}

impl Session {
    /// Starts a new session with transmit sequence zero and no receive history.
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            policy,
            tx_sequence: Sequence::Zero,
            last_rx_sequence: None,
            outbound: None,
            active_write: None,
            pending_acks: VecDeque::new(),
            last_time: Duration::ZERO,
            next_token: 0,
            closed: false,
        }
    }

    /// Whether the session has entered a terminal state.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether an outgoing reliable frame is queued, being written or awaiting ACK.
    pub fn is_busy(&self) -> bool {
        self.outbound.is_some()
    }

    /// The next ACK deadline, if a completed write is waiting for acknowledgement.
    ///
    /// Call [`Self::advance`] at this time. The outgoing data frame has no ACK
    /// deadline until its write completes. Writing an ACK for incoming data does
    /// not suspend an existing deadline. Enforce transport-write timeouts separately.
    pub fn next_deadline(&self) -> Option<Duration> {
        match &self.outbound {
            Some(Outbound {
                phase: OutboundPhase::AwaitingAck { deadline },
                ..
            }) => Some(*deadline),
            _ => None,
        }
    }

    /// Queues one reliable T1 or T2 command payload.
    ///
    /// A pending ACK write takes precedence. A second data command returns
    /// [`SessionError::Busy`] until the first is link-acknowledged.
    pub fn send(
        &mut self,
        table: CommandTable,
        payload: Vec<u8>,
        now: Duration,
    ) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        if self.outbound.is_some() {
            return Err(SessionError::Busy);
        }
        let data_type = match table {
            CommandTable::TABLE_1 => DataType::DATA_MDR,
            CommandTable::TABLE_2 => DataType::DATA_MDR_NO2,
            _ => return Err(SessionError::UnsupportedTable(table)),
        };
        if payload.is_empty() {
            return Err(SessionError::EmptyPayload);
        }
        self.last_time = now;
        self.outbound = Some(Outbound {
            table,
            frame: Frame {
                data_type,
                sequence: self.tx_sequence,
                payload,
            },
            retries: 0,
            phase: OutboundPhase::Queued { retry: false },
        });
        let mut events = Vec::new();
        self.start_write(&mut events);
        Ok(events)
    }

    /// Receives a previously validated frame from the incremental frame decoder.
    ///
    /// Reliable duplicates are ACKed again but normally not delivered twice.
    /// A structurally valid T1 protocol-info response is the documented exception.
    /// Unknown payload commands remain available to the caller via `Received`.
    pub fn receive(
        &mut self,
        frame: Frame,
        now: Duration,
    ) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        if frame.data_type == DataType::ACK {
            if !frame.payload.is_empty() {
                return Err(SessionError::NonEmptyAckPayload);
            }
            self.last_time = now;
            let mut events = Vec::new();
            self.receive_ack(frame.sequence, &mut events);
            self.start_write(&mut events);
            return Ok(events);
        }
        let table = match frame.data_type {
            DataType::DATA_MDR => CommandTable::TABLE_1,
            DataType::DATA_MDR_NO2 => CommandTable::TABLE_2,
            other => return Err(SessionError::UnsupportedDataType(other)),
        };
        if frame.payload.is_empty() {
            return Err(SessionError::EmptyPayload);
        }
        self.last_time = now;
        let mut events = Vec::new();
        if self.pending_acks.len() == MAX_PENDING_ACKS {
            self.close(CloseReason::AckQueueFull, &mut events);
            return Ok(events);
        }
        self.pending_acks.push_back(Frame {
            data_type: DataType::ACK,
            sequence: frame.sequence.toggled(),
            payload: Vec::new(),
        });
        let duplicate = self.last_rx_sequence == Some(frame.sequence);
        self.last_rx_sequence = Some(frame.sequence);
        let protocol_info = ProtocolInfo::decode(table, &frame.payload).is_ok();
        self.start_write(&mut events);
        if !self.closed && (!duplicate || protocol_info) {
            events.push(SessionEvent::Received(frame));
        }
        Ok(events)
    }

    /// Reports successful completion of the requested transport write.
    ///
    /// A matching ACK observed before this callback is retained, but is not
    /// reported as `LinkAcknowledged` until this write succeeds.
    pub fn complete_write(
        &mut self,
        token: WriteToken,
        now: Duration,
    ) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        self.check_token(token)?;
        self.last_time = now;
        let mut events = Vec::new();
        let active = self.active_write.take();
        if matches!(active.map(|write| write.kind), Some(WriteKind::Data)) {
            let acknowledged = matches!(
                self.outbound.as_ref().map(|outbound| &outbound.phase),
                Some(OutboundPhase::Writing { acknowledged: true })
            );
            if acknowledged {
                self.finish_outbound(&mut events);
            } else if let Some(deadline) = now.checked_add(self.policy.ack_timeout) {
                if let Some(outbound) = &mut self.outbound {
                    outbound.phase = OutboundPhase::AwaitingAck { deadline };
                }
            } else {
                self.close(CloseReason::ClockOverflow, &mut events);
            }
        }
        self.start_write(&mut events);
        Ok(events)
    }

    /// Reports a failed data or ACK write and terminates the session.
    pub fn fail_write(
        &mut self,
        token: WriteToken,
        now: Duration,
    ) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        self.check_token(token)?;
        self.last_time = now;
        let mut events = Vec::new();
        self.close(CloseReason::WriteFailed, &mut events);
        Ok(events)
    }

    /// Advances the caller's clock, scheduling at most one retransmission.
    ///
    /// Retries retain the original sequence and payload. A write already in
    /// progress is never interrupted or followed by overlapping writes.
    pub fn advance(&mut self, now: Duration) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        self.last_time = now;
        let mut events = Vec::new();
        if self.next_deadline().is_some_and(|deadline| now >= deadline)
            && let Some(outbound) = &mut self.outbound
        {
            if outbound.retries >= self.policy.max_retries {
                self.close(CloseReason::RetriesExhausted, &mut events);
            } else {
                outbound.phase = OutboundPhase::Queued { retry: true };
            }
        }
        self.start_write(&mut events);
        Ok(events)
    }

    /// Cancels all work. Close the transport and create a new session to reconnect.
    pub fn cancel(&mut self, now: Duration) -> Result<Vec<SessionEvent>, SessionError> {
        self.check_time(now)?;
        self.last_time = now;
        let mut events = Vec::new();
        self.close(CloseReason::Cancelled, &mut events);
        Ok(events)
    }

    fn check_time(&self, now: Duration) -> Result<(), SessionError> {
        if self.closed {
            return Err(SessionError::Closed);
        }
        if now < self.last_time {
            return Err(SessionError::TimeWentBackwards {
                previous: self.last_time,
                received: now,
            });
        }
        Ok(())
    }

    fn check_token(&self, token: WriteToken) -> Result<(), SessionError> {
        if self.active_write.as_ref().map(|write| write.token) != Some(token) {
            return Err(SessionError::UnknownWriteToken(token));
        }
        Ok(())
    }

    fn receive_ack(&mut self, sequence: Sequence, events: &mut Vec<SessionEvent>) {
        let Some(outbound) = &mut self.outbound else {
            return;
        };
        if sequence != outbound.frame.sequence.toggled() {
            return;
        }
        match &mut outbound.phase {
            OutboundPhase::Queued { retry: false } => {}
            OutboundPhase::Writing { acknowledged } => *acknowledged = true,
            OutboundPhase::Queued { retry: true } | OutboundPhase::AwaitingAck { .. } => {
                self.finish_outbound(events);
            }
        }
    }

    fn finish_outbound(&mut self, events: &mut Vec<SessionEvent>) {
        if let Some(outbound) = self.outbound.take() {
            self.tx_sequence = outbound.frame.sequence.toggled();
            events.push(SessionEvent::LinkAcknowledged {
                table: outbound.table,
                sequence: outbound.frame.sequence,
            });
        }
    }

    fn start_write(&mut self, events: &mut Vec<SessionEvent>) {
        if self.closed || self.active_write.is_some() {
            return;
        }
        let next_data = self
            .outbound
            .as_ref()
            .is_some_and(|outbound| matches!(outbound.phase, OutboundPhase::Queued { .. }));
        if self.pending_acks.is_empty() && !next_data {
            return;
        }
        let Some(next_token) = self.next_token.checked_add(1) else {
            self.close(CloseReason::WriteTokenExhausted, events);
            return;
        };
        let token = WriteToken(self.next_token);
        self.next_token = next_token;
        let (kind, frame) = if let Some(frame) = self.pending_acks.pop_front() {
            (WriteKind::Ack, frame)
        } else if let Some(outbound) = &mut self.outbound {
            if matches!(outbound.phase, OutboundPhase::Queued { retry: true }) {
                // advance() only queues a retry while retries < max_retries.
                outbound.retries += 1;
            }
            outbound.phase = OutboundPhase::Writing {
                acknowledged: false,
            };
            (WriteKind::Data, outbound.frame.clone())
        } else {
            return;
        };
        self.active_write = Some(ActiveWrite { token, kind });
        events.push(SessionEvent::Transmit { token, frame });
    }

    fn close(&mut self, reason: CloseReason, events: &mut Vec<SessionEvent>) {
        self.closed = true;
        self.outbound = None;
        self.active_write = None;
        self.pending_acks.clear();
        events.push(SessionEvent::Closed(reason));
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new(RetryPolicy::default())
    }
}
