use std::{
    collections::VecDeque,
    future::{Future, pending},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use bluetooth::{CancellationToken, Connection};
use libmdr2::{
    alert::{AlertControl, AlertDecision, AlertMessage},
    audio::{AudioCapability, AudioKind, AudioParameters, AudioSetting, AudioStatus},
    battery::BatteryKind,
    command::{CapabilityQuery, ExpectedResponse, Notification, Request, Response},
    common::{AudioCodec, CommonKind, CommonStatus},
    device::{DeviceInfo, DeviceInfoKind},
    equalizer::{EqualizerKind, EqualizerSetting, PresetId},
    frame::{Decoder, Frame},
    noise::{NoiseAvailability, NoiseCapability, NoiseKind, NoiseParameters},
    session::{CloseReason, RetryPolicy, Session, SessionEvent, WriteToken},
    system::{SpeakToChatSettings, SystemKind, SystemParameters, SystemStatus},
    types::{CommandTable, DisplayLanguage},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, timeout},
};

use crate::controls::{EqualizerEdit, audio_matches, equalizer_matches, noise_matches};
use crate::prompts::SettingReadback;
use crate::{
    AudioChange, ConnectionState, DeviceSnapshot, Error, Freshness, NoiseChange, Result,
    SystemChange, TimeoutStage,
};
use crate::{DevicePrompt, PromptId, PromptReply};

/// Local budgets. ACK policy is independent from write and response deadlines.
#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub queue_capacity: usize,
    pub payload_limit: usize,
    pub connect_timeout: Duration,
    pub queue_timeout: Duration,
    pub operation_timeout: Duration,
    pub write_timeout: Duration,
    pub response_timeout: Duration,
    pub close_timeout: Duration,
    pub retry_policy: RetryPolicy,
    /// The standard handshake is the default known layout. App-type requests
    /// must be selected explicitly; no version/model heuristic or fallback runs.
    pub capability_query: CapabilityQuery,
    pub language: DisplayLanguage,
    /// Report application foreground state during initialization after alert delivery is enabled.
    pub report_foreground: bool,
    /// A prompt that expires closes the session; no implicit device decision is sent.
    pub prompt_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 32,
            payload_limit: 16 * 1024,
            connect_timeout: Duration::from_secs(15),
            queue_timeout: Duration::from_secs(60),
            operation_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(5),
            close_timeout: Duration::from_secs(3),
            retry_policy: RetryPolicy::default(),
            capability_query: CapabilityQuery::Standard,
            language: DisplayLanguage::ENGLISH,
            report_foreground: false,
            prompt_timeout: Duration::from_secs(120),
        }
    }
}

impl SessionConfig {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.queue_capacity == 0 || self.queue_capacity > 4096 {
            return Err(Error::InvalidConfiguration(
                "operation queue must hold 1..=4096 items",
            ));
        }
        if self.payload_limit == 0 || self.payload_limit > (bluetooth::MAX_WRITE_BYTES - 16) / 2 {
            return Err(Error::InvalidConfiguration(
                "payload limit exceeds transport frame budget",
            ));
        }
        for duration in [
            self.connect_timeout,
            self.queue_timeout,
            self.operation_timeout,
            self.write_timeout,
            self.response_timeout,
            self.close_timeout,
            self.prompt_timeout,
            self.retry_policy.ack_timeout(),
        ] {
            if duration.is_zero() || Instant::now().checked_add(duration).is_none() {
                return Err(Error::InvalidConfiguration(
                    "timeouts must be positive and representable",
                ));
            }
        }
        Ok(())
    }
}

/// A state synchronization or a setting followed by an explicit readback.
/// `Adjusted` means the returned state differs; it does not infer a rejection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationResult {
    Synchronized,
    Confirmed,
    Adjusted,
    /// A device prompt interrupted the operation. Reconcile reported state and
    /// retry any unsent steps explicitly; responding does not resume the operation.
    AwaitingConfirmation,
    /// A rejected setting was read back at its preflight value.
    Cancelled,
    /// An informational or unsupported message was closed locally.
    Dismissed,
}

#[derive(Clone)]
pub struct DeviceHandle {
    requests: mpsc::Sender<Envelope>,
    state: watch::Receiver<DeviceSnapshot>,
    done: watch::Receiver<Option<Result<()>>>,
    cleanup: watch::Receiver<bool>,
    lifetime: Arc<Lifetime>,
    queue_timeout: Duration,
}
struct Lifetime(CancellationToken);
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl DeviceHandle {
    pub(crate) fn start(
        connection: Connection,
        generation: u64,
        config: SessionConfig,
        cancel: CancellationToken,
    ) -> Result<Self> {
        config.validate()?;
        let decoder = Decoder::new(config.payload_limit)
            .map_err(|error| Error::Protocol(error.to_string()))?;
        let snapshot = DeviceSnapshot::new(connection.info(), generation, config.language);
        let (state_tx, state) = watch::channel(snapshot.clone());
        let (done_tx, done) = watch::channel(None);
        let (cleanup_tx, cleanup) = watch::channel(false);
        let (requests, commands) = mpsc::channel(config.queue_capacity);
        let handle = Self {
            requests,
            state,
            done,
            cleanup,
            lifetime: Arc::new(Lifetime(cancel.clone())),
            queue_timeout: config.queue_timeout,
        };
        let actor = Actor {
            connection,
            decoder,
            session: Session::new(config.retry_policy),
            config,
            snapshot,
            state_tx,
            done_tx,
            cleanup_tx,
            commands,
            cancel,
            origin: Instant::now(),
            active: None,
            exchange: None,
            held_prompt_query: None,
            write: None,
            terminal: None,
            prompt_serial: 0,
            prompt_deadline: None,
            prompt_readback: None,
        };
        tokio::spawn(actor.run());
        Ok(handle)
    }

    pub fn snapshot(&self) -> DeviceSnapshot {
        self.state.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<DeviceSnapshot> {
        self.state.clone()
    }
    pub fn cancel(&self) {
        self.lifetime.0.cancel();
    }

    pub(crate) fn cleanup_finished(&self) -> bool {
        *self.cleanup.borrow()
    }

    /// Performs a complete handshake and initial advertised battery/codec sync.
    pub async fn initialize(&self) -> Result<OperationResult> {
        self.submit(Operation::Initialize).await
    }
    /// Responds to the actor's current received prompt, never to a caller-built message.
    pub async fn respond_prompt(
        &self,
        id: PromptId,
        reply: PromptReply,
    ) -> Result<OperationResult> {
        let snapshot = self.snapshot();
        if snapshot
            .pending_prompt
            .as_ref()
            .is_none_or(|prompt| prompt.id != id)
        {
            return Err(Error::StalePrompt);
        }
        self.submit(Operation::RespondPrompt(id, reply)).await
    }

    pub async fn refresh_battery(&self) -> Result<OperationResult> {
        self.submit(Operation::Battery).await
    }
    /// Reads the current device-reported codec when its function is advertised.
    pub async fn refresh_audio_codec(&self) -> Result<OperationResult> {
        self.submit(Operation::AudioCodec).await
    }
    pub async fn refresh_noise(&self, kind: NoiseKind) -> Result<OperationResult> {
        self.submit(Operation::Noise(kind)).await
    }
    pub async fn set_noise(&self, kind: NoiseKind, change: NoiseChange) -> Result<OperationResult> {
        self.submit(Operation::SetNoise(kind, change)).await
    }
    pub async fn refresh_equalizer(&self, kind: EqualizerKind) -> Result<OperationResult> {
        self.submit(Operation::Equalizer(kind)).await
    }
    /// Explicitly reads optional band labels and associates them with a freshly
    /// queried capability. Unsupported metadata does not authorize fixed labels.
    pub async fn refresh_equalizer_band_info(
        &self,
        kind: EqualizerKind,
    ) -> Result<OperationResult> {
        self.submit(Operation::EqualizerBandInfo(kind)).await
    }
    /// Selects a preset while preserving the ULT mode from the fresh preflight.
    pub async fn set_equalizer_preset(
        &self,
        kind: EqualizerKind,
        preset: PresetId,
    ) -> Result<OperationResult> {
        self.submit(Operation::EditEqualizer(
            kind,
            EqualizerEdit::Preset(preset),
        ))
        .await
    }
    /// Replaces one native value, retaining other bands and ULT from fresh state.
    /// The expected preset guards against editing a different externally selected
    /// slot; `None` is only appropriate for standalone custom EQ.
    pub async fn set_equalizer_band(
        &self,
        kind: EqualizerKind,
        expected_preset: Option<PresetId>,
        index: usize,
        value: u8,
    ) -> Result<OperationResult> {
        self.submit(Operation::EditEqualizer(
            kind,
            EqualizerEdit::Band {
                expected_preset,
                index,
                value,
            },
        ))
        .await
    }
    pub async fn set_equalizer(
        &self,
        kind: EqualizerKind,
        setting: EqualizerSetting,
    ) -> Result<OperationResult> {
        self.submit(Operation::SetEqualizer(kind, setting)).await
    }

    /// Reads a supported DSEE or connection-priority capability, status and parameters.
    pub async fn refresh_audio(&self, kind: AudioKind) -> Result<OperationResult> {
        self.submit(Operation::Audio(kind)).await
    }
    /// Applies an audio edit after a fresh preflight and confirms by GET readback.
    /// A connection-priority transition that disconnects returns an error; no
    /// operation replay or automatic reconnection is performed.
    pub async fn set_audio(&self, kind: AudioKind, change: AudioChange) -> Result<OperationResult> {
        self.submit(Operation::SetAudio(kind, change)).await
    }
    /// Reads Speak-to-Chat capability, status, ordinary and extended parameters.
    pub async fn refresh_system(&self, kind: SystemKind) -> Result<OperationResult> {
        self.submit(Operation::System(kind)).await
    }
    /// Changes one Speak-to-Chat field while preserving the other fields in its
    /// freshly read parameter set, then confirms that complete set by readback.
    pub async fn set_system(
        &self,
        kind: SystemKind,
        change: SystemChange,
    ) -> Result<OperationResult> {
        self.submit(Operation::SetSystem(kind, change)).await
    }

    async fn submit(&self, operation: Operation) -> Result<OperationResult> {
        if self.lifetime.0.is_cancelled() {
            return Err(Error::Disconnected);
        }
        let (reply, result) = oneshot::channel();
        let (started, start) = oneshot::channel();
        let deadline = Instant::now() + self.queue_timeout;
        let envelope = Envelope {
            operation,
            reply,
            started,
            deadline,
        };
        self.requests
            .try_send(envelope)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => Error::QueueFull,
                mpsc::error::TrySendError::Closed(_) => Error::Disconnected,
            })?;
        // The queue budget ends when the actor accepts this operation. Expiring
        // a queued item drops its reply without cancelling another operation.
        tokio::select! {
            biased;
            _ = start => result.await.unwrap_or(Err(Error::Disconnected)),
            _ = tokio::time::sleep_until(deadline) => Err(Error::Timeout(TimeoutStage::Queue)),
        }
    }

    /// Idempotently cancels all operations and waits for transport cleanup.
    pub async fn close(&self) -> Result<()> {
        self.cancel();
        let mut done = self.done.clone();
        loop {
            if let Some(result) = done.borrow().clone() {
                return result;
            }
            done.changed().await.map_err(|_| Error::Disconnected)?;
        }
    }
}

enum Operation {
    RespondPrompt(PromptId, PromptReply),
    Initialize,
    Battery,
    AudioCodec,
    Noise(NoiseKind),
    SetNoise(NoiseKind, NoiseChange),
    Equalizer(EqualizerKind),
    EqualizerBandInfo(EqualizerKind),
    SetEqualizer(EqualizerKind, EqualizerSetting),
    EditEqualizer(EqualizerKind, EqualizerEdit),
    Audio(AudioKind),
    SetAudio(AudioKind, AudioChange),
    System(SystemKind),
    SetSystem(SystemKind, SystemChange),
}
impl Operation {
    fn is_setting(&self) -> bool {
        matches!(
            self,
            Self::SetNoise(..)
                | Self::SetEqualizer(..)
                | Self::EditEqualizer(..)
                | Self::SetAudio(..)
                | Self::SetSystem(..)
        )
    }
}

struct Envelope {
    operation: Operation,
    reply: oneshot::Sender<Result<OperationResult>>,
    started: oneshot::Sender<()>,
    deadline: Instant,
}
struct Active {
    reply: oneshot::Sender<Result<OperationResult>>,
    steps: VecDeque<Step>,
    deadline: Instant,
    initializing: bool,
    setting: bool,
    responding: bool,
    prompt_consumed: bool,
    prompt_at_start: Option<PromptId>,
    readback: Option<SettingReadback>,
    outcome: OperationResult,
    last_response: Option<Response>,
}

enum Step {
    EnableAlerts,
    RespondPrompt(PromptId, PromptReply),
    VerifyPrompt(SettingReadback, PromptReply),
    Setting(Vec<u8>, SettingReadback),
    Query(Request),
    FunctionLists,
    BatteryQueries,
    AudioCodecQuery,
    Send(Vec<u8>),
    BuildNoise(NoiseKind, NoiseChange),
    VerifyNoise(NoiseKind, NoiseParameters),
    BuildEqualizer(EqualizerKind, EqualizerSetting, bool),
    BuildEqualizerEdit(EqualizerKind, EqualizerEdit),
    VerifyEqualizer(EqualizerKind, EqualizerSetting),
    RequireEqualizerPreset(EqualizerKind, libmdr2::equalizer::PresetId),
    BuildAudio(AudioKind, AudioChange),
    VerifyAudio(AudioKind, AudioSetting),
    BuildSystem(SystemKind, SystemChange),
    VerifySystemParameters(SystemKind, SystemParameters),
    VerifySystemExtended(SystemKind, SpeakToChatSettings),
}
struct Exchange {
    expected: Option<ExpectedResponse>,
    response: Option<Response>,
    failure: Option<Error>,
    critical: bool,
    acknowledged: bool,
    response_deadline: Option<Instant>,
}
impl Exchange {
    fn complete(&self) -> bool {
        self.acknowledged
            && (self.expected.is_none() || self.response.is_some() || self.failure.is_some())
    }
}
type WriteFuture = Pin<Box<dyn Future<Output = (WriteToken, bool, Result<()>)> + Send>>;
struct Actor {
    connection: Connection,
    decoder: Decoder,
    session: Session,
    config: SessionConfig,
    snapshot: DeviceSnapshot,
    state_tx: watch::Sender<DeviceSnapshot>,
    done_tx: watch::Sender<Option<Result<()>>>,
    cleanup_tx: watch::Sender<bool>,
    commands: mpsc::Receiver<Envelope>,
    cancel: CancellationToken,
    origin: Instant,
    active: Option<Active>,
    exchange: Option<Exchange>,
    held_prompt_query: Option<Exchange>,
    write: Option<WriteFuture>,
    terminal: Option<Error>,
    prompt_serial: u64,
    prompt_deadline: Option<Instant>,
    prompt_readback: Option<SettingReadback>,
}

enum Input {
    Stop,
    Abandoned,
    Command(Option<Envelope>),
    Read(Option<bluetooth::Result<Vec<u8>>>),
    Written(WriteToken, bool, Result<()>),
    Timer,
}

impl Actor {
    async fn run(mut self) {
        loop {
            self.drive();
            if self.terminal.is_some() {
                break;
            }
            let deadline = self.deadline();
            let input = tokio::select! {
                _ = self.cancel.cancelled() => Input::Stop,
                _ = async { if let Some(active) = &mut self.active { active.reply.closed().await } else { pending().await } } => Input::Abandoned,
                command = self.commands.recv(), if self.active.is_none() => Input::Command(command),
                read = self.connection.recv() => Input::Read(read),
                (token, data, result) = async { match &mut self.write { Some(write) => write.await, None => pending().await } } => Input::Written(token, data, result),
                _ = async { match deadline { Some(deadline) => tokio::time::sleep_until(deadline).await, None => pending().await } } => Input::Timer,
            };
            match input {
                Input::Stop | Input::Abandoned | Input::Command(None) => {
                    self.terminal = Some(Error::Cancelled);
                }
                Input::Command(Some(command)) => self.start(command),
                Input::Read(Some(Ok(bytes))) => {
                    let frames: Vec<_> = self.decoder.feed(&bytes).collect();
                    for frame in frames {
                        match frame {
                            Ok(frame) => match self.session.receive(frame, self.origin.elapsed()) {
                                Ok(events) => self.effects(events),
                                Err(error) => {
                                    self.terminal = Some(Error::Protocol(error.to_string()));
                                }
                            },
                            Err(error) => {
                                self.terminal = Some(Error::Protocol(error.to_string()));
                            }
                        }
                        if self.terminal.is_some() {
                            break;
                        }
                    }
                }
                Input::Read(Some(Err(error))) => self.terminal = Some(error.into()),
                Input::Read(None) => self.terminal = Some(Error::Disconnected),
                Input::Written(token, data, result) => {
                    self.write = None;
                    match result {
                        Ok(()) => {
                            if data
                                && let Some(exchange) = &mut self.exchange
                                && exchange.expected.is_some()
                                && exchange.response_deadline.is_none()
                            {
                                exchange.response_deadline =
                                    Some(Instant::now() + self.config.response_timeout);
                            }
                            match self.session.complete_write(token, self.origin.elapsed()) {
                                Ok(events) => self.effects(events),
                                Err(error) => {
                                    self.terminal = Some(Error::Protocol(error.to_string()))
                                }
                            }
                        }
                        Err(error) => {
                            let _ = self.session.fail_write(token, self.origin.elapsed());
                            self.terminal = Some(error);
                        }
                    }
                }
                Input::Timer => {
                    let now = Instant::now();
                    if self.prompt_deadline.is_some_and(|deadline| now >= deadline) {
                        self.terminal = Some(Error::Timeout(TimeoutStage::Prompt));
                    } else if self
                        .active
                        .as_ref()
                        .is_some_and(|active| now >= active.deadline)
                    {
                        self.terminal = Some(Error::Timeout(TimeoutStage::Operation));
                    } else if self.held_prompt_query.as_ref().is_some_and(|query| {
                        query
                            .response_deadline
                            .is_some_and(|deadline| now >= deadline)
                    }) {
                        self.terminal = Some(Error::Timeout(TimeoutStage::Response));
                    } else if self.exchange.as_ref().is_some_and(|exchange| {
                        exchange.response.is_none()
                            && exchange.failure.is_none()
                            && exchange
                                .response_deadline
                                .is_some_and(|deadline| now >= deadline)
                    }) {
                        // A later identical GET cannot distinguish a stale reply.
                        self.terminal = Some(Error::Timeout(TimeoutStage::Response));
                    } else {
                        match self.session.advance(self.origin.elapsed()) {
                            Ok(events) => self.effects(events),
                            Err(error) => self.terminal = Some(Error::Protocol(error.to_string())),
                        }
                    }
                }
            }
        }
        let reason = self.terminal.take().unwrap_or(Error::Disconnected);
        self.cancel.cancel();
        let _ = self.session.cancel(self.origin.elapsed());
        self.write = None;
        self.snapshot.connection = ConnectionState::Closing;
        self.clear_prompt();
        self.snapshot.stale();
        self.publish();
        if let Some(active) = self.active.take() {
            let _ = active.reply.send(Err(reason.clone()));
        }
        self.commands.close();
        while let Some(command) = self.commands.recv().await {
            let _ = command.reply.send(Err(reason.clone()));
        }
        let cleanup_attempt = timeout(self.config.close_timeout, self.connection.close()).await;
        let cleanup_finished = cleanup_attempt.is_ok();
        let cleanup = cleanup_attempt
            .map_err(|_| Error::Timeout(TimeoutStage::Close))
            .and_then(|result| result.map_err(Error::from));
        self.snapshot.connection = if reason == Error::Cancelled && cleanup.is_ok() {
            ConnectionState::Disconnected
        } else {
            ConnectionState::Failed
        };
        self.snapshot.last_error = if reason != Error::Cancelled {
            Some(reason)
        } else {
            cleanup.as_ref().err().cloned()
        };
        self.publish();
        if cleanup_finished {
            self.cleanup_tx.send_replace(true);
        }
        self.done_tx.send_replace(Some(cleanup));
        if !cleanup_finished {
            // The close budget limits the caller's wait, not resource ownership.
            // Keep the connection and reconnect gate alive until native cleanup
            // completes; a timed-out driver must not overlap its replacement.
            let _ = self.connection.close().await;
            self.cleanup_tx.send_replace(true);
        }
    }

    fn publish(&self) {
        self.state_tx.send_replace(self.snapshot.clone());
    }
    fn deadline(&self) -> Option<Instant> {
        let mut result = self.active.as_ref().map(|active| active.deadline);
        if let Some(deadline) = self.prompt_deadline {
            result = Some(result.map_or(deadline, |old| old.min(deadline)));
        }
        if let Some(deadline) = self
            .held_prompt_query
            .as_ref()
            .and_then(|query| query.response_deadline)
        {
            result = Some(result.map_or(deadline, |old| old.min(deadline)));
        }
        if let Some(duration) = self.session.next_deadline() {
            let deadline = self.origin + duration;
            result = Some(result.map_or(deadline, |old| old.min(deadline)));
        }
        if let Some(exchange) = &self.exchange
            && exchange.response.is_none()
            && exchange.failure.is_none()
            && let Some(deadline) = exchange.response_deadline
        {
            result = Some(result.map_or(deadline, |old| old.min(deadline)));
        }
        result
    }

    fn start(&mut self, command: Envelope) {
        if command.reply.is_closed() {
            return;
        }
        if Instant::now() >= command.deadline {
            let _ = command.reply.send(Err(Error::Timeout(TimeoutStage::Queue)));
            return;
        }
        if command.started.send(()).is_err() {
            return;
        }
        let initializing = matches!(command.operation, Operation::Initialize);
        let setting = command.operation.is_setting();
        let responding = matches!(command.operation, Operation::RespondPrompt(..));
        let result = self.plan(command.operation);
        match result {
            Ok(steps) => {
                self.active = Some(Active {
                    reply: command.reply,
                    steps,
                    deadline: Instant::now() + self.config.operation_timeout,
                    initializing,
                    setting,
                    responding,
                    prompt_consumed: false,
                    prompt_at_start: self
                        .snapshot
                        .pending_prompt
                        .as_ref()
                        .map(|prompt| prompt.id),
                    readback: None,
                    outcome: OperationResult::Synchronized,
                    last_response: None,
                });
                self.snapshot.last_error = None;
                self.publish();
            }
            Err(error) => {
                let _ = command.reply.send(Err(error));
            }
        }
    }

    fn plan(&mut self, operation: Operation) -> Result<VecDeque<Step>> {
        if !matches!(operation, Operation::Initialize)
            && self.snapshot.connection != ConnectionState::Ready
        {
            return Err(Error::NotReady);
        }
        if self.held_prompt_query.is_some() && !matches!(operation, Operation::RespondPrompt(..)) {
            return Err(Error::PromptPending);
        }
        if operation.is_setting() && self.snapshot.pending_prompt.is_some() {
            return Err(Error::PromptPending);
        }
        let mut steps = VecDeque::new();
        match operation {
            Operation::RespondPrompt(id, reply) => steps.push_back(Step::RespondPrompt(id, reply)),
            Operation::Initialize => {
                if self.snapshot.connection == ConnectionState::Ready {
                    return Ok(steps);
                }
                self.snapshot.connection = ConnectionState::Negotiating;
                steps.extend([
                    Step::Query(Request::ProtocolInfo),
                    Step::Query(Request::CapabilityInfo(self.config.capability_query)),
                    Step::FunctionLists,
                    Step::Query(Request::DeviceInfo(DeviceInfoKind::ModelName)),
                    Step::Query(Request::DeviceInfo(DeviceInfoKind::FirmwareVersion)),
                    Step::BatteryQueries,
                    Step::AudioCodecQuery,
                    Step::EnableAlerts,
                ]);
            }
            Operation::Battery => steps.push_back(Step::BatteryQueries),
            Operation::AudioCodec => {
                if self.snapshot.audio_codec.is_none() {
                    return Err(Error::Unsupported("audio codec report not advertised"));
                }
                steps.push_back(Step::AudioCodecQuery);
            }
            Operation::Noise(kind) | Operation::SetNoise(kind, _) => {
                if !self.snapshot.noise.iter().any(|state| state.kind == kind) {
                    return Err(Error::Unsupported("noise layout not advertised"));
                }
                if kind.has_capability_query() {
                    steps.push_back(Step::Query(Request::NoiseCapability(kind)));
                } else if let Some(state) = self
                    .snapshot
                    .noise
                    .iter_mut()
                    .find(|state| state.kind == kind)
                {
                    state.capability = Some(NoiseCapability::nc_only());
                }
                steps.extend([
                    Step::Query(Request::NoiseStatus(kind)),
                    Step::Query(Request::NoiseParameters(kind)),
                ]);
                if let Operation::SetNoise(_, change) = operation {
                    steps.push_back(Step::BuildNoise(kind, change));
                }
            }
            Operation::Equalizer(kind)
            | Operation::SetEqualizer(kind, _)
            | Operation::EditEqualizer(kind, _)
            | Operation::EqualizerBandInfo(kind) => {
                if !self
                    .snapshot
                    .equalizers
                    .iter()
                    .any(|state| state.kind == kind)
                {
                    return Err(Error::Unsupported("equalizer layout not advertised"));
                }
                steps.push_back(Step::Query(Request::EqualizerCapability {
                    kind,
                    language: self.config.language,
                }));
                if matches!(operation, Operation::EqualizerBandInfo(_)) {
                    if let Some(state) = self
                        .snapshot
                        .equalizers
                        .iter_mut()
                        .find(|state| state.kind == kind)
                    {
                        state.band_info = None;
                    }
                    steps.push_back(Step::Query(Request::EqualizerBandInfo(kind)));
                    return Ok(steps);
                }
                steps.extend([
                    Step::Query(Request::EqualizerStatus(kind)),
                    Step::Query(Request::EqualizerParameters(kind)),
                ]);
                if let Operation::SetEqualizer(_, setting) = operation {
                    steps.push_back(Step::BuildEqualizer(kind, setting, true));
                } else if let Operation::EditEqualizer(_, edit) = operation {
                    steps.push_back(Step::BuildEqualizerEdit(kind, edit));
                }
            }
            Operation::Audio(kind) | Operation::SetAudio(kind, _) => {
                let state = self
                    .snapshot
                    .audio
                    .iter_mut()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::Unsupported(
                        "audio layout is not advertised or not implemented by the core",
                    ))?;
                if kind.has_capability_query() {
                    steps.push_back(Step::Query(Request::AudioCapability(kind)));
                } else {
                    state.capability = Some(AudioCapability::static_connection(kind)?);
                }
                steps.extend([
                    Step::Query(Request::AudioStatus(kind)),
                    Step::Query(Request::AudioParameters(kind)),
                ]);
                if let Operation::SetAudio(_, change) = operation {
                    steps.push_back(Step::BuildAudio(kind, change));
                }
            }
            Operation::System(kind) | Operation::SetSystem(kind, _) => {
                if !self.snapshot.system.iter().any(|state| state.kind == kind) {
                    return Err(Error::Unsupported(
                        "system layout is not advertised or not implemented by the core",
                    ));
                }
                steps.extend([
                    Step::Query(Request::SystemCapability(kind)),
                    Step::Query(Request::SystemStatus(kind)),
                ]);
                if let Operation::SetSystem(_, change) = operation {
                    steps.push_back(Step::Query(if change.is_extended() {
                        Request::SystemExtendedParameters(kind)
                    } else {
                        Request::SystemParameters(kind)
                    }));
                    steps.push_back(Step::BuildSystem(kind, change));
                } else {
                    steps.extend([
                        Step::Query(Request::SystemParameters(kind)),
                        Step::Query(Request::SystemExtendedParameters(kind)),
                    ]);
                }
            }
        }
        Ok(steps)
    }

    fn prompt_interrupts_active(&self) -> bool {
        self.snapshot.pending_prompt.as_ref().is_some_and(|prompt| {
            self.active.as_ref().is_some_and(|active| {
                !active.initializing && active.prompt_at_start != Some(prompt.id)
            })
        })
    }

    fn drive(&mut self) {
        if self.terminal.is_some() {
            return;
        }
        if self.snapshot.pending_prompt.is_some()
            && self.active.as_ref().is_some_and(|active| {
                !active.initializing && (!active.responding || active.prompt_consumed)
            })
            && self.exchange.as_ref().is_some_and(|exchange| {
                exchange.acknowledged && exchange.expected.is_some() && !exchange.complete()
            })
        {
            let mut query = self.exchange.take().expect("acknowledged prompt query");
            query.response_deadline = None;
            self.held_prompt_query = Some(query);
            if let Some(active) = &mut self.active {
                active.prompt_at_start = None;
            }
        }
        if let Some(exchange) = &self.exchange {
            if !exchange.complete() {
                return;
            }
            let exchange = self.exchange.take().expect("completed exchange");
            if let Some(response) = exchange.response
                && let Some(active) = &mut self.active
            {
                active.last_response = Some(response);
            }
            if let Some(error) = exchange.failure {
                // A structurally unsupported feature response was received and
                // consumed. It cannot leave a late reply for the next operation.
                let optional_initial_read = !exchange.critical
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.initializing);
                if !optional_initial_read && let Some(active) = self.active.take() {
                    let _ = active.reply.send(Err(error.clone()));
                }
                self.snapshot.last_error = Some(error);
                self.publish();
            }
        }
        if self.exchange.is_none() && self.prompt_interrupts_active() {
            let active = self
                .active
                .take()
                .expect("active operation interrupted by prompt");
            if let Some(query) = &mut self.held_prompt_query {
                query.response_deadline = None;
            }
            self.publish();
            let _ = active.reply.send(Ok(OperationResult::AwaitingConfirmation));
        }
        if self.held_prompt_query.is_some()
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.prompt_consumed)
        {
            if let Some(query) = &mut self.held_prompt_query {
                query
                    .response_deadline
                    .get_or_insert(Instant::now() + self.config.response_timeout);
            }
            return;
        }
        while self.active.is_some() && self.exchange.is_none() && self.terminal.is_none() {
            if self
                .active
                .as_ref()
                .is_some_and(|active| active.reply.is_closed())
            {
                self.terminal = Some(Error::Cancelled);
                break;
            }
            if self.held_prompt_query.is_some()
                && self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.prompt_consumed)
                && self
                    .active
                    .as_ref()
                    .is_some_and(|active| !matches!(active.steps.front(), Some(Step::Send(_))))
            {
                if let Some(query) = &mut self.held_prompt_query {
                    query
                        .response_deadline
                        .get_or_insert(Instant::now() + self.config.response_timeout);
                }
                break;
            }
            let step = self
                .active
                .as_mut()
                .and_then(|active| active.steps.pop_front());
            match step {
                Some(step) => {
                    if let Err(error) = self.step(step) {
                        // These errors happen before a new frame is sent, so a failed
                        // validation does not make the following operation ambiguous.
                        // A send failure after installing a matcher instead means
                        // the protocol session cannot accept this exchange.
                        if self.exchange.take().is_some() {
                            self.terminal = Some(error.clone());
                        }
                        if let Some(active) = self.active.take() {
                            if active.initializing {
                                self.terminal = Some(error.clone());
                            }
                            let _ = active.reply.send(Err(error.clone()));
                        }
                        self.snapshot.last_error = Some(error);
                        self.publish();
                    }
                }
                None => {
                    if let Some(active) = self.active.take() {
                        if active.initializing {
                            self.snapshot.connection = ConnectionState::Ready;
                        }
                        self.publish();
                        let _ = active.reply.send(Ok(active.outcome));
                    }
                }
            }
        }
    }

    fn prepend(&mut self, steps: impl IntoIterator<Item = Step>) {
        if let Some(active) = &mut self.active {
            let mut steps: VecDeque<_> = steps.into_iter().collect();
            steps.append(&mut active.steps);
            active.steps = steps;
        }
    }

    fn step(&mut self, step: Step) -> Result<()> {
        match step {
            Step::EnableAlerts => {
                if self.snapshot.functions.iter().any(|entry| {
                    entry.advertised.id.table == CommandTable::TABLE_1
                        && entry.advertised.id.code == 0x90
                }) {
                    let mut steps = vec![Step::Send(
                        AlertControl::EnableFixedMessages.encode().to_vec(),
                    )];
                    if self.config.report_foreground {
                        steps.push(Step::Send(
                            AlertControl::ApplicationForeground.encode().to_vec(),
                        ));
                    }
                    self.prepend(steps);
                }
            }
            Step::RespondPrompt(id, reply) => {
                if self
                    .prompt_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    let error = Error::Timeout(TimeoutStage::Prompt);
                    self.terminal = Some(error.clone());
                    return Err(error);
                }
                let prompt = self
                    .snapshot
                    .pending_prompt
                    .as_ref()
                    .ok_or(Error::StalePrompt)?;
                if id.generation != self.snapshot.generation || prompt.id != id {
                    return Err(Error::StalePrompt);
                }
                if !prompt.allows(reply) {
                    return Err(Error::InvalidPromptReply);
                }
                let payload = match reply {
                    PromptReply::Accept => {
                        Some(prompt.message.encode_reply(AlertDecision::Accept)?)
                    }
                    PromptReply::Reject => {
                        Some(prompt.message.encode_reply(AlertDecision::Reject)?)
                    }
                    PromptReply::Dismiss => None,
                };
                let readback = self.prompt_readback.take();
                self.clear_prompt();
                self.publish();
                if let Some(active) = &mut self.active {
                    active.prompt_consumed = true;
                }
                if let Some(payload) = payload {
                    let mut steps = vec![Step::Send(payload)];
                    if let Some(readback) = readback {
                        steps.push(Step::Query(readback.query()));
                        steps.push(Step::VerifyPrompt(readback, reply));
                    } else {
                        steps.extend(self.prompt_refresh_steps());
                    }
                    self.prepend(steps);
                } else {
                    if let Some(active) = &mut self.active {
                        active.outcome = OperationResult::Dismissed;
                    }
                    self.prepend(self.prompt_refresh_steps());
                }
            }
            Step::VerifyPrompt(readback, reply) => {
                let active = self.active.as_mut().ok_or(Error::NotReady)?;
                active.outcome = readback
                    .verify(active.last_response.as_ref().ok_or(Error::NotReady)?, reply)?;
            }
            Step::Setting(payload, readback) => {
                if self.snapshot.pending_prompt.is_some() {
                    return Err(Error::PromptPending);
                }
                if let Some(active) = &mut self.active {
                    active.readback = Some(readback);
                }
                self.step(Step::Send(payload))?;
            }
            Step::Query(request) => {
                let critical = matches!(
                    &request,
                    Request::ProtocolInfo
                        | Request::CapabilityInfo(_)
                        | Request::SupportedFunctions(_)
                );
                let encoded = request.encode()?;
                // Register before send, including when send immediately emits a write.
                self.exchange = Some(Exchange {
                    expected: Some(encoded.expected_response),
                    response: None,
                    failure: None,
                    critical,
                    acknowledged: false,
                    response_deadline: None,
                });
                let events = self
                    .session
                    .send(encoded.table, encoded.payload, self.origin.elapsed())
                    .map_err(|error| Error::Protocol(error.to_string()))?;
                self.effects(events);
            }
            Step::Send(payload) => {
                self.exchange = Some(Exchange {
                    expected: None,
                    response: None,
                    failure: None,
                    critical: false,
                    acknowledged: false,
                    response_deadline: None,
                });
                let events = self
                    .session
                    .send(CommandTable::TABLE_1, payload, self.origin.elapsed())
                    .map_err(|error| Error::Protocol(error.to_string()))?;
                self.effects(events);
            }
            Step::FunctionLists => {
                let protocol = self.snapshot.protocol.as_ref().ok_or(Error::NotReady)?;
                let mut steps = Vec::new();
                if protocol.table_1_supported {
                    steps.push(Step::Query(Request::SupportedFunctions(
                        CommandTable::TABLE_1,
                    )));
                }
                if protocol.table_2_supported {
                    steps.push(Step::Query(Request::SupportedFunctions(
                        CommandTable::TABLE_2,
                    )));
                }
                if steps.is_empty() {
                    return Err(Error::Unsupported("no supported MDR v2 function table"));
                }
                self.prepend(steps);
            }
            Step::AudioCodecQuery => {
                if self.snapshot.audio_codec.is_some() {
                    self.prepend([Step::Query(Request::CommonStatus(CommonKind::AudioCodec))]);
                }
            }
            Step::BatteryQueries => {
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.initializing)
                {
                    self.snapshot.connection = ConnectionState::Synchronizing;
                    self.publish();
                }
                let mut kinds = Vec::new();
                for entry in &self.snapshot.functions {
                    if let Some(kind) = BatteryKind::from_function(entry.advertised.id)
                        && !kinds.contains(&kind)
                    {
                        kinds.push(kind);
                    }
                }
                self.prepend(
                    kinds
                        .into_iter()
                        .map(|kind| Step::Query(Request::BatteryStatus(kind))),
                );
            }
            Step::BuildNoise(kind, change) => {
                let state = self
                    .snapshot
                    .noise
                    .iter()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::NotReady)?;
                if state
                    .status
                    .is_none_or(|status| status.availability != NoiseAvailability::Enabled)
                {
                    return Err(Error::Unavailable("noise control"));
                }
                let desired = change.apply(state.parameters.ok_or(Error::NotReady)?)?;
                let payload =
                    desired.encode_set(state.capability.as_ref().ok_or(Error::NotReady)?)?;
                self.prepend([
                    Step::Setting(
                        payload,
                        SettingReadback::Noise(
                            kind,
                            desired,
                            state.parameters.ok_or(Error::NotReady)?,
                        ),
                    ),
                    Step::Query(Request::NoiseParameters(kind)),
                    Step::VerifyNoise(kind, desired),
                ]);
            }
            Step::VerifyNoise(kind, desired) => {
                let actual = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::NoiseParameters(parameters)) if parameters.kind() == kind => {
                        *parameters
                    }
                    _ => return Err(Error::NotReady),
                };
                if let Some(active) = &mut self.active {
                    active.outcome = if noise_matches(desired, actual) {
                        OperationResult::Confirmed
                    } else {
                        OperationResult::Adjusted
                    };
                }
            }
            Step::BuildEqualizer(kind, setting, may_select) => {
                let state = self
                    .snapshot
                    .equalizers
                    .iter()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::NotReady)?;
                if state.status.as_ref().is_none_or(|status| !status.enabled) {
                    return Err(Error::Unavailable("equalizer"));
                }
                let capability = state.capability.as_ref().ok_or(Error::NotReady)?;
                let payload = setting.encode_set(capability)?;
                let current = state.parameters.as_ref().ok_or(Error::NotReady)?;
                let preselection = match &setting {
                    EqualizerSetting::Preset {
                        preset,
                        levels: Some(_),
                    } if current.preset() != Some(*preset) => Some((
                        *preset,
                        EqualizerSetting::Preset {
                            preset: *preset,
                            levels: None,
                        },
                    )),
                    EqualizerSetting::PresetWithUlt {
                        preset,
                        ult_mode,
                        levels: Some(_),
                    } if current.preset() != Some(*preset) => Some((
                        *preset,
                        EqualizerSetting::PresetWithUlt {
                            preset: *preset,
                            ult_mode: *ult_mode,
                            levels: None,
                        },
                    )),
                    _ => None,
                };
                if let Some((preset, select)) = preselection {
                    if !may_select {
                        return Err(Error::Unavailable(
                            "device did not select the editable preset",
                        ));
                    }
                    let selection = select.encode_set(capability)?;
                    self.prepend([
                        Step::Setting(
                            selection,
                            SettingReadback::Equalizer(kind, setting.clone(), current.clone()),
                        ),
                        Step::Query(Request::EqualizerParameters(kind)),
                        Step::RequireEqualizerPreset(kind, preset),
                        Step::Query(Request::EqualizerStatus(kind)),
                        Step::BuildEqualizer(kind, setting, false),
                    ]);
                } else {
                    self.prepend([
                        Step::Setting(
                            payload,
                            SettingReadback::Equalizer(kind, setting.clone(), current.clone()),
                        ),
                        Step::Query(Request::EqualizerParameters(kind)),
                        Step::VerifyEqualizer(kind, setting),
                    ]);
                }
            }
            Step::BuildEqualizerEdit(kind, edit) => {
                let state = self
                    .snapshot
                    .equalizers
                    .iter()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::NotReady)?;
                let desired = edit.apply(
                    state.parameters.as_ref().ok_or(Error::NotReady)?,
                    state.capability.as_ref().ok_or(Error::NotReady)?,
                )?;
                self.prepend([Step::BuildEqualizer(kind, desired, false)]);
            }
            Step::RequireEqualizerPreset(kind, preset) => {
                let matched = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::EqualizerParameters(parameters))
                        if parameters.kind() == kind =>
                    {
                        parameters.preset()
                    }
                    _ => None,
                };
                if matched != Some(preset) {
                    return Err(Error::Unavailable(
                        "device did not select the editable preset",
                    ));
                }
            }
            Step::VerifyEqualizer(kind, desired) => {
                let actual = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::EqualizerParameters(parameters))
                        if parameters.kind() == kind =>
                    {
                        parameters
                    }
                    _ => return Err(Error::NotReady),
                };
                let confirmed = equalizer_matches(&desired, actual);
                if let Some(active) = &mut self.active {
                    active.outcome = if confirmed {
                        OperationResult::Confirmed
                    } else {
                        OperationResult::Adjusted
                    };
                }
            }
            Step::BuildAudio(kind, change) => {
                let state = self
                    .snapshot
                    .audio
                    .iter()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::NotReady)?;
                if state.status.as_ref().is_none_or(|status| !status.enabled) {
                    return Err(Error::Unavailable("audio control"));
                }
                let desired = change.apply(state.parameters.as_ref().ok_or(Error::NotReady)?)?;
                let payload =
                    desired.encode_set(state.capability.as_ref().ok_or(Error::NotReady)?)?;
                self.prepend([
                    Step::Setting(
                        payload,
                        SettingReadback::Audio(
                            kind,
                            desired.clone(),
                            state.parameters.as_ref().ok_or(Error::NotReady)?.clone(),
                        ),
                    ),
                    Step::Query(Request::AudioParameters(kind)),
                    Step::VerifyAudio(kind, desired),
                ]);
            }
            Step::VerifyAudio(kind, desired) => {
                let actual = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::AudioParameters(parameters)) if parameters.kind() == kind => {
                        parameters
                    }
                    _ => return Err(Error::NotReady),
                };
                self.record_confirmation(audio_matches(&desired, actual));
            }
            Step::BuildSystem(kind, change) => {
                let state = self
                    .snapshot
                    .system
                    .iter()
                    .find(|state| state.kind == kind)
                    .ok_or(Error::NotReady)?;
                if state.status.is_none_or(|status| !status.enabled) {
                    return Err(Error::Unavailable("Speak-to-Chat"));
                }
                let capability = state.capability.as_ref().ok_or(Error::NotReady)?;
                if change.is_extended() {
                    let desired = change.apply_extended(state.extended.ok_or(Error::NotReady)?)?;
                    let payload = desired.encode_set(capability)?;
                    self.prepend([
                        Step::Setting(
                            payload,
                            SettingReadback::Extended(
                                kind,
                                desired,
                                state.extended.ok_or(Error::NotReady)?,
                            ),
                        ),
                        Step::Query(Request::SystemExtendedParameters(kind)),
                        Step::VerifySystemExtended(kind, desired),
                    ]);
                } else {
                    let desired =
                        change.apply_parameters(state.parameters.ok_or(Error::NotReady)?)?;
                    let payload = desired.encode_set(capability)?;
                    self.prepend([
                        Step::Setting(
                            payload,
                            SettingReadback::System(
                                kind,
                                desired,
                                state.parameters.ok_or(Error::NotReady)?,
                            ),
                        ),
                        Step::Query(Request::SystemParameters(kind)),
                        Step::VerifySystemParameters(kind, desired),
                    ]);
                }
            }
            Step::VerifySystemParameters(kind, desired) => {
                let actual = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::SystemParameters(parameters)) if parameters.kind() == kind => {
                        *parameters
                    }
                    _ => return Err(Error::NotReady),
                };
                self.record_confirmation(desired == actual);
            }
            Step::VerifySystemExtended(kind, desired) => {
                let actual = match self
                    .active
                    .as_ref()
                    .and_then(|active| active.last_response.as_ref())
                {
                    Some(Response::SystemExtendedParameters(parameters))
                        if parameters.kind() == kind =>
                    {
                        *parameters
                    }
                    _ => return Err(Error::NotReady),
                };
                self.record_confirmation(desired == actual);
            }
        }
        Ok(())
    }

    fn clear_prompt(&mut self) {
        self.snapshot.pending_prompt = None;
        self.prompt_deadline = None;
        self.prompt_readback = None;
    }

    fn receive_prompt(&mut self, message: AlertMessage) {
        let Some(serial) = self.prompt_serial.checked_add(1) else {
            self.terminal = Some(Error::Protocol("prompt identity exhausted".into()));
            return;
        };
        self.prompt_serial = serial;
        // Only an active setting already transmitted on this connection can be
        // associated. Preflight, late and replacement prompts remain independent.
        self.prompt_readback = if self.snapshot.pending_prompt.is_none() {
            self.active
                .as_ref()
                .filter(|active| active.setting && !active.responding)
                .and_then(|active| active.readback.as_ref())
                .filter(|readback| readback.relates_to(&message))
                .cloned()
        } else {
            None
        };
        self.snapshot.pending_prompt = Some(DevicePrompt {
            id: PromptId {
                generation: self.snapshot.generation,
                serial,
            },
            message,
        });
        self.prompt_deadline = Some(Instant::now() + self.config.prompt_timeout);
    }

    fn prompt_refresh_steps(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        for state in &self.snapshot.noise {
            steps.push(Step::Query(Request::NoiseParameters(state.kind)));
        }
        for state in &self.snapshot.equalizers {
            steps.push(Step::Query(Request::EqualizerParameters(state.kind)));
        }
        for state in &self.snapshot.audio {
            steps.push(Step::Query(Request::AudioParameters(state.kind)));
        }
        for state in &self.snapshot.system {
            steps.push(Step::Query(Request::SystemParameters(state.kind)));
            steps.push(Step::Query(Request::SystemExtendedParameters(state.kind)));
        }
        if self.snapshot.audio_codec.is_some() {
            steps.push(Step::Query(Request::CommonStatus(CommonKind::AudioCodec)));
        }
        steps
    }

    fn record_confirmation(&mut self, confirmed: bool) {
        if let Some(active) = &mut self.active {
            active.outcome = if confirmed {
                OperationResult::Confirmed
            } else {
                OperationResult::Adjusted
            };
        }
    }

    fn effects(&mut self, events: Vec<SessionEvent>) {
        for event in events {
            match event {
                SessionEvent::Transmit { token, frame } => {
                    if self.write.is_some() {
                        self.terminal =
                            Some(Error::Protocol("overlapping transport writes".into()));
                        return;
                    }
                    let data = frame.data_type.command_table().is_some();
                    let bytes = match frame.encode(self.config.payload_limit) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            self.terminal = Some(Error::Protocol(error.to_string()));
                            return;
                        }
                    };
                    let writer = self.connection.writer();
                    let duration = self.config.write_timeout;
                    self.write = Some(Box::pin(async move {
                        let result = timeout(duration, writer.write(bytes))
                            .await
                            .map_err(|_| Error::Timeout(TimeoutStage::Write))
                            .and_then(|result| result.map_err(Error::from));
                        (token, data, result)
                    }));
                }
                SessionEvent::LinkAcknowledged { .. } => {
                    if let Some(exchange) = &mut self.exchange {
                        exchange.acknowledged = true;
                    }
                }
                SessionEvent::Received(frame) => self.received(frame),
                SessionEvent::Closed(reason) => {
                    self.terminal = Some(match reason {
                        CloseReason::RetriesExhausted => Error::Timeout(TimeoutStage::Ack),
                        CloseReason::Cancelled => Error::Cancelled,
                        _ => Error::Protocol(format!("session closed: {reason:?}")),
                    });
                }
                _ => {
                    self.terminal = Some(Error::Protocol("unsupported session event".into()));
                }
            }
            if self.terminal.is_some() {
                return;
            }
        }
    }

    fn received(&mut self, frame: Frame) {
        let Some(table) = frame.data_type.command_table() else {
            return;
        };
        if let Some(expected) = self
            .held_prompt_query
            .as_ref()
            .and_then(|query| query.expected)
            && expected.matches(table, &frame.payload)
        {
            self.held_prompt_query = None;
            match expected.decode(table, &frame.payload) {
                Ok(response) => self.project_response(response),
                Err(error) => self.snapshot.last_error = Some(error.into()),
            }
            self.publish();
            return;
        }
        if let Some(exchange) = &mut self.exchange
            && let Some(expected) = exchange.expected
            && expected.matches(table, &frame.payload)
            && exchange.response.is_none()
            && exchange.failure.is_none()
        {
            match expected.decode(table, &frame.payload) {
                Ok(response) => {
                    if let Response::EqualizerBandInfo(info) = &response {
                        let validation = self
                            .snapshot
                            .equalizers
                            .iter()
                            .find(|state| state.kind == info.kind())
                            .and_then(|state| state.capability.as_ref())
                            .ok_or(Error::NotReady)
                            .and_then(|capability| {
                                info.validate_capability(capability).map_err(Error::from)
                            });
                        if let Err(error) = validation {
                            exchange.failure = Some(error);
                            return;
                        }
                    }
                    // A response is not complete until the outgoing data frame is
                    // also link-acknowledged. An early response is retained.
                    exchange.response = Some(response.clone());
                    self.project_response(response);
                }
                Err(error) => {
                    if exchange.critical {
                        self.terminal = Some(error.into());
                    } else {
                        exchange.failure = Some(error.into());
                    }
                }
            }
        } else {
            match Notification::decode(table, &frame.payload) {
                Ok(notification) => self.project_notification(notification),
                Err(_) => {
                    self.snapshot.unhandled_messages =
                        self.snapshot.unhandled_messages.saturating_add(1)
                }
            }
        }
        self.publish();
    }

    fn project_response(&mut self, response: Response) {
        self.snapshot.message_counters.responses =
            self.snapshot.message_counters.responses.saturating_add(1);
        match response {
            Response::ProtocolInfo(info) => self.snapshot.protocol = Some(info),
            Response::CapabilityInfo(info) => self.snapshot.capability_info = Some(info),
            Response::SupportedFunctions(functions) => {
                for function in functions.functions {
                    self.snapshot.add_function(function);
                }
            }
            Response::DeviceInfo(DeviceInfo::ModelName(name)) => {
                self.snapshot.model_name = Some(name)
            }
            Response::DeviceInfo(DeviceInfo::FirmwareVersion(version)) => {
                self.snapshot.firmware_version = Some(version)
            }
            Response::BatteryStatus(status) => self.snapshot.battery(status),
            Response::CommonStatus(CommonStatus::AudioCodec(codec)) => self.audio_codec(codec),
            Response::NoiseCapability(capability) => {
                if let Some(state) = self
                    .snapshot
                    .noise
                    .iter_mut()
                    .find(|state| state.kind == capability.kind())
                {
                    state.capability = Some(capability);
                }
            }
            Response::NoiseStatus(status) => self.noise_status(status),
            Response::NoiseParameters(parameters) => self.noise_parameters(parameters),
            Response::EqualizerCapability(capability) => {
                if let Some(state) = self
                    .snapshot
                    .equalizers
                    .iter_mut()
                    .find(|state| state.kind == capability.kind())
                {
                    if state
                        .band_info
                        .as_ref()
                        .is_some_and(|info| info.validate_capability(&capability).is_err())
                    {
                        state.band_info = None;
                    }
                    state.capability = Some(capability);
                }
            }
            Response::EqualizerBandInfo(info) => {
                if let Some(state) = self
                    .snapshot
                    .equalizers
                    .iter_mut()
                    .find(|state| state.kind == info.kind())
                {
                    state.band_info = Some(info);
                }
            }
            Response::EqualizerStatus(status) => self.eq_status(status),
            Response::EqualizerParameters(parameters) => self.eq_parameters(parameters),
            Response::AudioCapability(capability) => {
                if let Some(state) = self
                    .snapshot
                    .audio
                    .iter_mut()
                    .find(|state| state.kind == capability.kind())
                {
                    state.capability = Some(capability);
                }
            }
            Response::AudioStatus(status) => self.audio_status(status),
            Response::AudioParameters(parameters) => self.audio_parameters(parameters),
            Response::SystemCapability(capability) => {
                if let Some(state) = self
                    .snapshot
                    .system
                    .iter_mut()
                    .find(|state| state.kind == capability.kind())
                {
                    state.capability = Some(capability);
                }
            }
            Response::SystemStatus(status) => self.system_status(status),
            Response::SystemParameters(parameters) => self.system_parameters(parameters),
            Response::SystemExtendedParameters(parameters) => self.system_extended(parameters),
            _ => {
                self.snapshot.unhandled_messages =
                    self.snapshot.unhandled_messages.saturating_add(1)
            }
        }
    }

    fn project_notification(&mut self, notification: Notification) {
        self.snapshot.message_counters.notifications = self
            .snapshot
            .message_counters
            .notifications
            .saturating_add(1);
        match notification {
            Notification::AlertMessage(message) => self.receive_prompt(message),
            Notification::BatteryStatus(status) => self.snapshot.battery(status),
            Notification::CommonStatus(CommonStatus::AudioCodec(codec)) => self.audio_codec(codec),
            Notification::NoiseStatus(status) => self.noise_status(status),
            Notification::NoiseParameters(parameters) => {
                self.snapshot
                    .message_counters
                    .noise_parameters_notifications = self
                    .snapshot
                    .message_counters
                    .noise_parameters_notifications
                    .saturating_add(1);
                self.noise_parameters(parameters);
            }
            Notification::EqualizerStatus(status) => self.eq_status(status),
            Notification::EqualizerParameters(parameters) => {
                self.snapshot
                    .message_counters
                    .equalizer_parameters_notifications = self
                    .snapshot
                    .message_counters
                    .equalizer_parameters_notifications
                    .saturating_add(1);
                self.eq_parameters(parameters);
            }
            Notification::AudioStatus(status) => self.audio_status(status),
            Notification::AudioParameters(parameters) => {
                self.snapshot
                    .message_counters
                    .audio_parameters_notifications = self
                    .snapshot
                    .message_counters
                    .audio_parameters_notifications
                    .saturating_add(1);
                self.audio_parameters(parameters);
            }
            Notification::SystemStatus(status) => self.system_status(status),
            Notification::SystemParameters(parameters) => {
                self.snapshot
                    .message_counters
                    .system_parameters_notifications = self
                    .snapshot
                    .message_counters
                    .system_parameters_notifications
                    .saturating_add(1);
                self.system_parameters(parameters);
            }
            Notification::SystemExtendedParameters(parameters) => {
                self.snapshot.message_counters.system_extended_notifications = self
                    .snapshot
                    .message_counters
                    .system_extended_notifications
                    .saturating_add(1);
                self.system_extended(parameters);
            }
            // Transport requests require caller policy; never silently act on an
            // address supplied by a protocol notification.
            _ => {
                self.snapshot.unhandled_messages =
                    self.snapshot.unhandled_messages.saturating_add(1)
            }
        }
    }
    fn audio_codec(&mut self, codec: AudioCodec) {
        if let Some(state) = &mut self.snapshot.audio_codec {
            state.codec = Some(codec);
            state.freshness = Freshness::Current;
        }
    }
    fn noise_status(&mut self, status: libmdr2::noise::NoiseStatus) {
        if let Some(state) = self
            .snapshot
            .noise
            .iter_mut()
            .find(|state| state.kind == status.kind())
        {
            state.status = Some(status);
        }
    }
    fn noise_parameters(&mut self, parameters: NoiseParameters) {
        if let Some(state) = self
            .snapshot
            .noise
            .iter_mut()
            .find(|state| state.kind == parameters.kind())
        {
            state.parameters = Some(parameters);
            state.freshness = Freshness::Current;
        }
    }
    fn eq_status(&mut self, status: libmdr2::equalizer::EqualizerStatus) {
        if let Some(state) = self
            .snapshot
            .equalizers
            .iter_mut()
            .find(|state| state.kind == status.kind())
        {
            state.status = Some(status);
        }
    }
    fn eq_parameters(&mut self, parameters: libmdr2::equalizer::EqualizerParameters) {
        if let Some(state) = self
            .snapshot
            .equalizers
            .iter_mut()
            .find(|state| state.kind == parameters.kind())
        {
            state.parameters = Some(parameters);
            state.freshness = Freshness::Current;
        }
    }
    fn audio_status(&mut self, status: AudioStatus) {
        if let Some(state) = self
            .snapshot
            .audio
            .iter_mut()
            .find(|state| state.kind == status.kind())
        {
            state.status = Some(status);
        }
    }
    fn audio_parameters(&mut self, parameters: AudioParameters) {
        if let Some(state) = self
            .snapshot
            .audio
            .iter_mut()
            .find(|state| state.kind == parameters.kind())
        {
            state.parameters = Some(parameters);
            state.freshness = Freshness::Current;
        }
    }
    fn system_status(&mut self, status: SystemStatus) {
        if let Some(state) = self
            .snapshot
            .system
            .iter_mut()
            .find(|state| state.kind == status.kind())
        {
            state.status = Some(status);
        }
    }
    fn system_parameters(&mut self, parameters: SystemParameters) {
        if let Some(state) = self
            .snapshot
            .system
            .iter_mut()
            .find(|state| state.kind == parameters.kind())
        {
            state.parameters = Some(parameters);
            state.freshness = Freshness::Current;
        }
    }
    fn system_extended(&mut self, parameters: SpeakToChatSettings) {
        if let Some(state) = self
            .snapshot
            .system
            .iter_mut()
            .find(|state| state.kind == parameters.kind())
        {
            state.extended = Some(parameters);
            state.extended_freshness = Freshness::Current;
        }
    }
}
