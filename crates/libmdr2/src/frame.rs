//! Incremental MDR framing with explicit payload limits.
//!
//! Frames use `3e | type | sequence | length:u32be | payload | sum8 | 3c`.
//! The body, including its checksum, escapes `3c`, `3d`, and `3e` as
//! `3d 2c`, `3d 2d`, and `3d 2e`. Only these escape pairs are accepted.
//! Unknown data types are preserved; interpreting their reliability semantics
//! is the session's responsibility.

#![doc = include_str!("../docs/framing.md")]

use std::{error::Error, fmt};

use crate::types::CommandTable;

const START: u8 = 0x3e;
const END: u8 = 0x3c;
const ESCAPE: u8 = 0x3d;
const HEADER_LEN: usize = 6;
const OVERHEAD: usize = HEADER_LEN + 1;

/// Raw frame type. Unknown values round-trip without being interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataType(u8);

impl DataType {
    /// Link acknowledgement, with an empty payload.
    pub const ACK: Self = Self(0x01);
    /// Reliable MDR v2 command-table-1 data.
    pub const DATA_MDR: Self = Self(0x0c);
    /// Reliable MDR v2 command-table-2 data.
    pub const DATA_MDR_NO2: Self = Self(0x0e);
    /// Unacknowledged table-1 data; unsupported by the reliable session.
    pub const SHOT_MDR: Self = Self(0x1c);
    /// Unacknowledged table-2 data; unsupported by the reliable session.
    pub const SHOT_MDR_NO2: Self = Self(0x1e);

    /// Preserves an uninterpreted frame type.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original type byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Returns the command table of the known MDR data and SHOT types.
    pub const fn command_table(self) -> Option<CommandTable> {
        match self.0 {
            0x0c | 0x1c => Some(CommandTable::TABLE_1),
            0x0e | 0x1e => Some(CommandTable::TABLE_2),
            _ => None,
        }
    }
}

/// The one-bit sequence used by MDR data and acknowledgement frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sequence {
    /// Sequence zero.
    Zero,
    /// Sequence one.
    One,
}

impl Sequence {
    /// Returns the wire representation.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::One => 1,
        }
    }

    /// Returns the opposite sequence, including the ACK for this sequence.
    pub const fn toggled(self) -> Self {
        match self {
            Self::Zero => Self::One,
            Self::One => Self::Zero,
        }
    }
}

impl TryFrom<u8> for Sequence {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            _ => Err(FrameError::InvalidSequence(value)),
        }
    }
}

/// A decoded frame, without transport-specific fragmentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The original wire data type.
    pub data_type: DataType,
    /// The validated one-bit sequence.
    pub sequence: Sequence,
    /// Unescaped payload; the checksum is not included.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Builds an acknowledgement for the received sequence.
    pub fn acknowledgement(received: Sequence) -> Self {
        Self {
            data_type: DataType::ACK,
            sequence: received.toggled(),
            payload: Vec::new(),
        }
    }

    /// Encodes a complete frame, enforcing the caller's payload limit.
    ///
    /// The limit is an application policy, not a maximum promised by a device.
    /// An ACK with a nonempty payload is rejected. Unknown data types remain raw.
    pub fn encode(&self, max_payload: usize) -> Result<Vec<u8>, FrameError> {
        let mut encoded = Vec::new();
        self.encode_into(&mut encoded, max_payload)?;
        Ok(encoded)
    }

    /// Appends the encoded frame to a buffer, preserving existing contents.
    ///
    /// Validation errors leave the buffer unchanged. This method does not split
    /// frames into Bluetooth writes; the transport must do that in order.
    pub fn encode_into(&self, output: &mut Vec<u8>, max_payload: usize) -> Result<(), FrameError> {
        validate_limit(max_payload)?;
        validate_length(self.data_type, self.payload.len(), max_payload)?;
        let length = u32::try_from(self.payload.len())
            .map_err(|_| FrameError::InvalidLimit(self.payload.len()))?
            .to_be_bytes();
        output.push(START);
        let mut checksum = 0u8;
        for byte in [self.data_type.as_raw(), self.sequence.as_u8()]
            .into_iter()
            .chain(length)
            .chain(self.payload.iter().copied())
        {
            checksum = checksum.wrapping_add(byte);
            append_escaped(output, byte);
        }
        append_escaped(output, checksum);
        output.push(END);
        Ok(())
    }
}

fn append_escaped(output: &mut Vec<u8>, byte: u8) {
    if matches!(byte, END | ESCAPE | START) {
        output.extend_from_slice(&[ESCAPE, byte & !0x10]);
    } else {
        output.push(byte);
    }
}

/// A malformed frame or an invalid codec configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameError {
    /// The configured limit cannot fit the wire length and local buffer size.
    InvalidLimit(usize),
    /// A payload exceeds the configured limit.
    PayloadTooLarge {
        /// Declared or supplied payload length.
        length: usize,
        /// Caller-selected maximum payload length.
        maximum: usize,
    },
    /// A sequence other than zero or one was received.
    InvalidSequence(u8),
    /// An ACK declares or contains a payload.
    InvalidAckPayload(usize),
    /// A byte following the escape marker was not 2c, 2d, or 2e.
    InvalidEscape(u8),
    /// A new start delimiter replaced an unfinished frame; the new frame starts here.
    UnexpectedStart,
    /// End of input was declared while a frame was incomplete.
    TruncatedFrame,
    /// The decoded frame body is shorter than its header and checksum.
    FrameTooShort(usize),
    /// The payload length does not match the frame body.
    LengthMismatch {
        /// Length declared in the header.
        declared: usize,
        /// Length implied by the body, excluding header and checksum.
        actual: usize,
    },
    /// The checksum does not match the unescaped header and payload.
    ChecksumMismatch {
        /// Computed checksum.
        expected: u8,
        /// Received checksum.
        actual: u8,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimit(limit) => write!(f, "unrepresentable payload limit: {limit}"),
            Self::PayloadTooLarge { length, maximum } => {
                write!(f, "payload length {length} exceeds limit {maximum}")
            }
            Self::InvalidSequence(value) => write!(f, "invalid sequence: {value}"),
            Self::InvalidAckPayload(length) => write!(f, "ACK has payload length {length}"),
            Self::InvalidEscape(value) => write!(f, "invalid escape suffix: {value:#04x}"),
            Self::UnexpectedStart => f.write_str("new start delimiter interrupted a frame"),
            Self::TruncatedFrame => f.write_str("incomplete frame at end of input"),
            Self::FrameTooShort(length) => write!(f, "frame body too short: {length}"),
            Self::LengthMismatch { declared, actual } => {
                write!(
                    f,
                    "declared payload length {declared}, actual length {actual}"
                )
            }
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "checksum mismatch: expected {expected:#04x}, got {actual:#04x}"
                )
            }
        }
    }
}

impl Error for FrameError {}

fn validate_limit(max_payload: usize) -> Result<(), FrameError> {
    if u32::try_from(max_payload).is_err() || max_payload.checked_add(OVERHEAD).is_none() {
        return Err(FrameError::InvalidLimit(max_payload));
    }
    Ok(())
}

fn validate_length(
    data_type: DataType,
    length: usize,
    max_payload: usize,
) -> Result<(), FrameError> {
    if length > max_payload {
        return Err(FrameError::PayloadTooLarge {
            length,
            maximum: max_payload,
        });
    }
    if data_type == DataType::ACK && length != 0 {
        return Err(FrameError::InvalidAckPayload(length));
    }
    Ok(())
}

/// A bounded incremental decoder accepting arbitrary transport fragments.
///
/// Bytes outside frame delimiters are ignored. An error discards the current
/// frame and resumes seeking a start delimiter; an unexpected start delimiter
/// itself starts a replacement frame. Buffer contents never exceed the
/// configured payload limit plus the seven-byte header/checksum overhead.
/// Declared lengths are checked before any payload-dependent allocation.
#[derive(Debug)]
pub struct Decoder {
    max_payload: usize,
    body: Vec<u8>,
    receiving: bool,
    escaped: bool,
}

impl Decoder {
    /// Creates a decoder with a caller-selected maximum payload length.
    pub fn new(max_payload: usize) -> Result<Self, FrameError> {
        validate_limit(max_payload)?;
        Ok(Self {
            max_payload,
            body: Vec::new(),
            receiving: false,
            escaped: false,
        })
    }

    /// Discards incomplete input, for example when the physical link closes.
    pub fn reset(&mut self) {
        self.body.clear();
        self.receiving = false;
        self.escaped = false;
    }

    /// Reports a truncated frame at end of stream, then clears the decoder.
    ///
    /// Do not call this between transport fragments. Use it when no more bytes
    /// will arrive on the current stream.
    pub fn finish(&mut self) -> Result<(), FrameError> {
        let incomplete = self.receiving;
        self.reset();
        if incomplete {
            Err(FrameError::TruncatedFrame)
        } else {
            Ok(())
        }
    }

    /// Consumes one byte and optionally produces a frame or a framing error.
    pub fn push(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        if byte == START {
            let error = self.receiving.then_some(if self.escaped {
                FrameError::InvalidEscape(byte)
            } else {
                FrameError::UnexpectedStart
            });
            self.reset();
            self.receiving = true;
            return error.map(Err);
        }
        if !self.receiving {
            return None;
        }
        if self.escaped {
            self.escaped = false;
            if !(0x2c..=0x2e).contains(&byte) {
                self.reset();
                return Some(Err(FrameError::InvalidEscape(byte)));
            }
            return self.append(byte | 0x10);
        }
        match byte {
            ESCAPE => {
                self.escaped = true;
                None
            }
            END => {
                let result = self.decode_body();
                self.reset();
                Some(result)
            }
            _ => self.append(byte),
        }
    }

    /// Iterates over frames and errors from a fragment without buffering events.
    ///
    /// The iterator is lazy: consume it fully to process every input byte.
    /// Dropping it early leaves the remaining bytes unconsumed. An incomplete
    /// frame stays in the decoder for the next fragment.
    pub fn feed<'a>(&'a mut self, input: &'a [u8]) -> DecodeIter<'a> {
        DecodeIter {
            decoder: self,
            input: input.iter(),
        }
    }

    fn append(&mut self, byte: u8) -> Option<Result<Frame, FrameError>> {
        // Check before pushing so stored contents stay within the stated bound.
        if self.body.len() >= OVERHEAD {
            let declared = self.declared_length();
            if self.body.len() - OVERHEAD >= declared {
                let actual = self.body.len() - OVERHEAD + 1;
                self.reset();
                return Some(Err(FrameError::LengthMismatch { declared, actual }));
            }
        }
        self.body.push(byte);
        let error = if self.body.len() == 2 {
            Sequence::try_from(byte).err()
        } else if self.body.len() == HEADER_LEN {
            validate_length(
                DataType::from_raw(self.body[0]),
                self.declared_length(),
                self.max_payload,
            )
            .err()
        } else {
            None
        };
        if let Some(error) = error {
            self.reset();
            Some(Err(error))
        } else {
            None
        }
    }

    fn declared_length(&self) -> usize {
        u32::from_be_bytes([self.body[2], self.body[3], self.body[4], self.body[5]]) as usize
    }

    fn decode_body(&self) -> Result<Frame, FrameError> {
        if self.body.len() < OVERHEAD {
            return Err(FrameError::FrameTooShort(self.body.len()));
        }
        let declared = self.declared_length();
        let actual = self.body.len() - OVERHEAD;
        if declared != actual {
            return Err(FrameError::LengthMismatch { declared, actual });
        }
        let checksum_index = self.body.len() - 1;
        let expected = self.body[..checksum_index]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        let actual = self.body[checksum_index];
        if expected != actual {
            return Err(FrameError::ChecksumMismatch { expected, actual });
        }
        Ok(Frame {
            data_type: DataType::from_raw(self.body[0]),
            sequence: Sequence::try_from(self.body[1])?,
            payload: self.body[HEADER_LEN..checksum_index].to_vec(),
        })
    }
}

/// Lazy results from [`Decoder::feed`]. No result queue is retained by the decoder.
#[must_use = "the iterator must be consumed to process input"]
pub struct DecodeIter<'a> {
    decoder: &'a mut Decoder,
    input: std::slice::Iter<'a, u8>,
}

impl Iterator for DecodeIter<'_> {
    type Item = Result<Frame, FrameError>;

    fn next(&mut self) -> Option<Self::Item> {
        for byte in self.input.by_ref() {
            if let Some(result) = self.decoder.push(*byte) {
                return Some(result);
            }
        }
        None
    }
}
