use std::{
    collections::VecDeque,
    future::{Future, pending},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use bluetooth::{CancellationToken, Connection};
use libmdr2::{
    battery::BatteryKind,
    command::{CapabilityQuery, ExpectedResponse, Notification, Request, Response},
    device::{DeviceInfo, DeviceInfoKind},
    equalizer::{EqualizerKind, EqualizerSetting, PresetId},
    frame::{Decoder, Frame},
    noise::{NoiseAvailability, NoiseCapability, NoiseKind, NoiseParameters},
    session::{CloseReason, RetryPolicy, Session, SessionEvent, WriteToken},
    types::{CommandTable, DisplayLanguage},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, timeout},
};

use crate::controls::{EqualizerEdit, equalizer_matches, noise_matches};
use crate::{ConnectionState, DeviceSnapshot, Error, Freshness, NoiseChange, Result, TimeoutStage};

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
            write: None,
            terminal: None,
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

    /// Performs a complete standard/app-type handshake and initial battery sync.
    pub async fn initialize(&self) -> Result<OperationResult> {
        self.submit(Operation::Initialize).await
    }
    pub async fn refresh_battery(&self) -> Result<OperationResult> {
        self.submit(Operation::Battery).await
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
    Initialize,
    Battery,
    Noise(NoiseKind),
    SetNoise(NoiseKind, NoiseChange),
    Equalizer(EqualizerKind),
    EqualizerBandInfo(EqualizerKind),
    SetEqualizer(EqualizerKind, EqualizerSetting),
    EditEqualizer(EqualizerKind, EqualizerEdit),
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
    outcome: OperationResult,
    last_response: Option<Response>,
}

enum Step {
    Query(Request),
    FunctionLists,
    BatteryQueries,
    Send(Vec<u8>),
    BuildNoise(NoiseKind, NoiseChange),
    VerifyNoise(NoiseKind, NoiseParameters),
    BuildEqualizer(EqualizerKind, EqualizerSetting, bool),
    BuildEqualizerEdit(EqualizerKind, EqualizerEdit),
    VerifyEqualizer(EqualizerKind, EqualizerSetting),
    RequireEqualizerPreset(EqualizerKind, libmdr2::equalizer::PresetId),
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
    write: Option<WriteFuture>,
    terminal: Option<Error>,
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
                    if self
                        .active
                        .as_ref()
                        .is_some_and(|active| now >= active.deadline)
                    {
                        self.terminal = Some(Error::Timeout(TimeoutStage::Operation));
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
        let result = self.plan(command.operation);
        match result {
            Ok(steps) => {
                self.active = Some(Active {
                    reply: command.reply,
                    steps,
                    deadline: Instant::now() + self.config.operation_timeout,
                    initializing,
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
        let mut steps = VecDeque::new();
        match operation {
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
                ]);
            }
            Operation::Battery => steps.push_back(Step::BatteryQueries),
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
        }
        Ok(steps)
    }

    fn drive(&mut self) {
        if self.terminal.is_some() {
            return;
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
        while self.active.is_some() && self.exchange.is_none() && self.terminal.is_none() {
            if self
                .active
                .as_ref()
                .is_some_and(|active| active.reply.is_closed())
            {
                self.terminal = Some(Error::Cancelled);
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
                    Step::Send(payload),
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
                        Step::Send(selection),
                        Step::Query(Request::EqualizerParameters(kind)),
                        Step::RequireEqualizerPreset(kind, preset),
                        Step::Query(Request::EqualizerStatus(kind)),
                        Step::BuildEqualizer(kind, setting, false),
                    ]);
                } else {
                    self.prepend([
                        Step::Send(payload),
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
        }
        Ok(())
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
            Notification::BatteryStatus(status) => self.snapshot.battery(status),
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
            // Transport requests require caller policy; never silently act on an
            // address supplied by a protocol notification.
            _ => {
                self.snapshot.unhandled_messages =
                    self.snapshot.unhandled_messages.saturating_add(1)
            }
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
}
