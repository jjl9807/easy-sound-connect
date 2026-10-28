//! One application-owned device bridge shared by all windows.
use crate::{
    gui_controls,
    notices::{DISPLAY_DURATION, Notices, Severity},
    preferences::{Language, Preferences},
    saved_devices::{SavedDevice, SavedDevices},
};
use bluetooth::{CancellationToken, Device, DeviceId, TransportKind};
use ezsc_core::{
    DeviceHandle, DeviceManager, DeviceSnapshot, OperationResult, PromptId, SessionConfig,
};
use gpui_kit::Context;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::{
    runtime::Handle,
    sync::{mpsc, watch},
};

pub(super) type SharedManager = Arc<Mutex<Option<Arc<DeviceManager>>>>;

pub(super) enum Event {
    NoticeExpired(u64),
    Backend(Result<Arc<DeviceManager>, String>),
    SavedLoaded(SavedDevices, Option<String>),
    SavedWritten,
    Devices(u64, Result<Vec<Device>, String>),
    PreviousReleased(u64),
    CleanupWarning(u64, String),
    Initialized(u64, SavedDevice),
    Connected(u64, DeviceHandle),
    Snapshot(u64, Box<DeviceSnapshot>),
    Finished(u64, Result<Completion, String>),
    Operation(u64, String, Result<OperationResult, String>),
    DisplayLanguage(u64, Language, Result<OperationResult, String>),
    PromptResponse(u64, PromptId, Result<OperationResult, String>),
}

pub(super) enum Completion {
    StateUpdated,
    Released,
    Forgotten(DeviceId),
}

pub(super) struct Session {
    pub(super) runtime: Handle,
    pub(super) manager: Option<Arc<DeviceManager>>,
    pub(super) events: mpsc::Sender<Event>,
    pub(super) lifetime: CancellationToken,
    pub(super) devices: Vec<Device>,
    pub(super) saved_devices: SavedDevices,
    pub(super) focused_device: Option<DeviceId>,
    pub(super) adding_device: bool,
    pub(super) device_storage_error: Option<String>,
    pub(super) selected: Option<DeviceId>,
    pub(super) handle: Option<DeviceHandle>,
    pub(super) snapshot: Option<DeviceSnapshot>,
    pub(super) epoch: u64,
    pub(super) discovering: bool,
    pub(super) busy: bool,
    pub(super) message: String,
    pub(super) diagnostics: bool,
    pub(super) replying_to: Option<PromptId>,
    pub(super) preferences: Preferences,
    pub(super) activity: std::collections::VecDeque<String>,
    pub(super) notices: Notices,
    pub(super) pending_controls: crate::gui_controls::PendingControls,
    pub(super) preview: bool,
    pub(super) prompt_error: Option<(PromptId, String)>,
    display_language_request: Option<(u64, Language)>,
    display_language_deferred: bool,
    saved_loaded: bool,
    saved_writer: Option<SavedWriter>,
    saved_revision: u64,
    saved_result_revision: u64,
    discovery_generation: u64,
    discovery_cancel: Option<CancellationToken>,
    cleanup_device: Option<DeviceId>,
    connecting_name: Option<String>,
    attempted_transport: Option<TransportKind>,
}

impl Session {
    pub(super) fn new(
        runtime: Handle,
        shared: SharedManager,
        lifetime: CancellationToken,
        preferences: Preferences,
        preview: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let (events, mut incoming) = mpsc::channel(32);
        cx.spawn(async move |view, cx| {
            while let Some(event) = incoming.recv().await {
                if view
                    .update(cx, |view, cx| {
                        view.receive(event);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                cx.update(crate::gui_shell::session_changed);
            }
        })
        .detach();
        let sender = events.clone();
        let cancel = lifetime.clone();
        let saved_writer = (!preview).then(|| SavedWriter::new(&runtime, events.clone()));
        if !preview {
            runtime.spawn(async move {
            let (saved, warning) = tokio::task::spawn_blocking(SavedDevices::load)
                .await
                .unwrap_or_else(|error| (SavedDevices::default(), Some(format!("Unable to load saved devices: {error}"))));
            if sender.send(Event::SavedLoaded(saved, warning)).await.is_err() || cancel.is_cancelled() {
                return;
            }
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = tokio::time::timeout(Duration::from_secs(15), bluetooth::system_backend()) => {
                    result.map_err(|_| "Bluetooth startup timed out".to_owned())
                        .and_then(|result| result.map_err(|error| error.to_string()))
                        .map(|backend| Arc::new(DeviceManager::new(backend)))
                }
            };
            if let Ok(manager) = &result {
                *shared.lock().unwrap_or_else(|error| error.into_inner()) = Some(manager.clone());
                if cancel.is_cancelled() { let _ = manager.shutdown().await; return; }
            }
            let _ = sender.send(Event::Backend(result)).await;
        });
        }
        Self {
            runtime,
            manager: None,
            events,
            lifetime,
            devices: Vec::new(),
            saved_devices: SavedDevices::default(),
            focused_device: None,
            adding_device: false,
            device_storage_error: None,
            selected: None,
            handle: None,
            snapshot: None,
            epoch: 0,
            discovering: false,
            busy: false,
            message: preferences
                .language
                .text("Starting Bluetooth…", "正在启动蓝牙…")
                .into(),
            diagnostics: std::env::args().any(|arg| arg == "--diagnostics"),
            replying_to: None,
            preferences,
            activity: Default::default(),
            notices: Default::default(),
            pending_controls: Default::default(),
            preview,
            prompt_error: None,
            display_language_request: None,
            display_language_deferred: false,
            saved_loaded: preview,
            saved_writer,
            saved_revision: 0,
            saved_result_revision: 0,
            discovery_generation: 0,
            discovery_cancel: None,
            cleanup_device: None,
            connecting_name: None,
            attempted_transport: None,
        }
    }

    fn receive(&mut self, event: Event) {
        if let Event::NoticeExpired(id) = &event {
            self.notices.dismiss(*id);
            return;
        }
        let language = self.preferences.language;
        let previous_connection = self
            .snapshot
            .as_ref()
            .map(|snapshot| (snapshot.device.clone(), snapshot.connection));
        match event {
            Event::SavedLoaded(saved, warning) => {
                self.focused_device = saved.last_selected.clone().map(DeviceId);
                self.saved_devices = saved;
                self.saved_loaded = true;
                self.device_storage_error = warning;
                if let Some(error) = &self.device_storage_error {
                    self.show_notice(Severity::Warning, error.clone());
                }
            }
            Event::SavedWritten => {}
            Event::Backend(Ok(manager)) => {
                self.manager = Some(manager);
                self.message = language
                    .text(
                        "Choose a saved headset or add a device.",
                        "请选择已保存的耳机，或添加设备。",
                    )
                    .into();
                if self.adding_device {
                    self.discover();
                }
            }
            Event::Backend(Err(error)) => {
                self.message = format!(
                    "{}: {error}",
                    language.text("Bluetooth startup failed", "蓝牙启动失败")
                );
                self.show_notice(Severity::Error, self.message.clone());
            }
            Event::Devices(generation, result)
                if generation == self.discovery_generation && self.adding_device =>
            {
                self.discovering = false;
                self.discovery_cancel = None;
                match result {
                    Ok(devices) => {
                        if self.selected.is_none() && !self.busy {
                            self.message = if devices.is_empty() {
                                language.text("No paired MDR v2 devices found. Pair your headset in system settings, then refresh.", "未发现已配对的 MDR v2 耳机。请先在系统设置中配对，然后刷新。").into()
                            } else {
                                language
                                    .text("Select a headset to connect.", "请选择要连接的耳机。")
                                    .into()
                            };
                        }
                        self.devices = devices
                            .into_iter()
                            .filter(|device| self.saved_devices.find(&device.id).is_none())
                            .collect();
                    }
                    Err(error) => {
                        self.message = format!(
                            "{}: {error}",
                            language.text("Device discovery failed", "设备发现失败")
                        );
                        self.show_notice(Severity::Error, self.message.clone());
                    }
                }
            }
            Event::CleanupWarning(epoch, error) if epoch == self.epoch => {
                self.show_notice(
                    Severity::Warning,
                    format!(
                        "{}: {error}",
                        language.text(
                            "Control cleanup completed with an error",
                            "控制连接清理已完成，但报告了错误"
                        )
                    ),
                );
            }
            Event::PreviousReleased(epoch) if epoch == self.epoch => {
                self.cleanup_device = None;
                self.message = language.text("Connecting…", "正在连接…").into();
            }
            Event::Initialized(epoch, device) if epoch == self.epoch => {
                self.saved_devices.upsert(device);
                self.persist_saved_devices();
                self.finish_add_device();
            }
            Event::Connected(epoch, handle) if epoch == self.epoch => self.handle = Some(handle),
            Event::Connected(_, handle) => handle.cancel(),
            Event::Snapshot(epoch, snapshot) if epoch == self.epoch => {
                // Queued notifications are wakeups: consult the latest watch
                // value so a delayed snapshot cannot undo an operation readback.
                self.snapshot = Some(
                    self.handle
                        .as_ref()
                        .map(DeviceHandle::snapshot)
                        .unwrap_or_else(|| *snapshot),
                );
                if let Some(snapshot) = &self.snapshot {
                    let phase = match snapshot.connection {
                        ezsc_core::ConnectionState::Connected => {
                            Some(language.text("Control channel connected…", "控制通道已连接…"))
                        }
                        ezsc_core::ConnectionState::Negotiating => Some(
                            language.text("Reading headset capabilities…", "正在读取耳机能力…"),
                        ),
                        ezsc_core::ConnectionState::Synchronizing => {
                            Some(language.text("Synchronizing headset state…", "正在同步耳机状态…"))
                        }
                        ezsc_core::ConnectionState::Closing => {
                            Some(language.text("Closing control connection…", "正在关闭控制连接…"))
                        }
                        _ => None,
                    };
                    if let Some(phase) = phase {
                        self.message = phase.into();
                    }
                }
            }
            Event::Finished(epoch, result) if epoch == self.epoch => {
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                self.busy = false;
                let successful = result.is_ok();
                self.message = match result {
                    Ok(Completion::StateUpdated) => language
                        .text("Device state is up to date.", "耳机状态已更新。")
                        .into(),
                    Ok(Completion::Released) => {
                        self.cleanup_device = None;
                        language
                            .text("Control released.", "已释放控制连接。")
                            .into()
                    }
                    Ok(Completion::Forgotten(device)) => {
                        self.cleanup_device = None;
                        self.remove_saved_device(&device);
                        language
                            .text(
                                "Device removed from the saved list.",
                                "已从已保存列表移除设备。",
                            )
                            .into()
                    }
                    Err(error) => format!(
                        "{}: {error}",
                        language.text("Unable to complete", "无法完成操作")
                    ),
                };
                if successful {
                    self.drain_pending_control();
                } else {
                    self.pending_controls.clear();
                    self.show_notice(Severity::Error, self.message.clone());
                }
                self.start_display_language_refresh();
            }
            Event::Operation(epoch, label, result) if epoch == self.epoch => {
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                self.busy = false;
                let successful = result.as_ref().is_ok_and(|r| {
                    matches!(
                        r,
                        OperationResult::Confirmed
                            | OperationResult::Adjusted
                            | OperationResult::Synchronized
                    )
                });
                let severity = operation_severity(&result);
                self.message = operation_message(&label, result, language);
                if let Some(severity) = severity {
                    self.show_notice(severity, self.message.clone());
                }
                if successful {
                    self.drain_pending_control();
                } else {
                    self.pending_controls.clear();
                }
            }
            Event::DisplayLanguage(epoch, requested, result)
                if epoch == self.epoch
                    && self.display_language_request == Some((epoch, requested)) =>
            {
                self.display_language_request = None;
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                let severity = operation_severity(&result);
                let message = operation_message(
                    language.text("Device display labels", "耳机显示标签"),
                    result,
                    language,
                );
                if let Some(severity) = severity {
                    self.show_notice(severity, message.clone());
                }
                if requested == language && !self.busy && self.replying_to.is_none() {
                    self.message = message;
                } else {
                    self.record_activity(message);
                }
                // UI changes made while this request ran are coalesced. A
                // failed current request is reported once without replay.
                self.display_language_deferred = requested != language;
                self.start_display_language_refresh();
            }
            Event::PromptResponse(epoch, id, result)
                if epoch == self.epoch && self.replying_to == Some(id) =>
            {
                self.replying_to = None;
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                if !self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.pending_prompt.as_ref())
                    .is_some_and(|prompt| prompt.id != id)
                {
                    if let Err(error) = &result {
                        self.prompt_error = Some((id, error.clone()));
                    }
                    let severity = operation_severity(&result);
                    self.message = operation_message(
                        language.text("Device decision", "设备确认"),
                        result,
                        language,
                    );
                    if let Some(severity) = severity {
                        self.show_notice(severity, self.message.clone());
                    }
                }
            }
            _ => {}
        }
        if let Some(prompt) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.pending_prompt.as_ref())
            && self.replying_to.is_some_and(|id| id != prompt.id)
        {
            self.replying_to = None;
            self.prompt_error = None;
        }
        if let Some(snapshot) = &self.snapshot
            && matches!(
                snapshot.connection,
                ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed
            )
        {
            self.replying_to = None;
            self.prompt_error = None;
            self.message = match &snapshot.last_error {
                Some(error) => format!(
                    "{}: {error}. {}",
                    language.text("Control connection ended", "控制连接已结束"),
                    language.text("Select the headset to reconnect.", "请选择耳机重新连接。")
                ),
                None => language
                    .text(
                        "Control connection ended. Select the headset to reconnect.",
                        "控制连接已结束。请选择耳机重新连接。",
                    )
                    .into(),
            };
            let previously_ended = previous_connection.as_ref().is_some_and(|(device, state)| {
                device == &snapshot.device
                    && matches!(
                        state,
                        ezsc_core::ConnectionState::Disconnected
                            | ezsc_core::ConnectionState::Failed
                    )
            });
            // The selected identity is cleared before intentional release.
            // A terminal snapshot can be repeated after the notice has expired.
            if !previously_ended && self.selected.as_ref() == Some(&snapshot.device) {
                let severity = if snapshot.last_error.is_some()
                    || snapshot.connection == ezsc_core::ConnectionState::Failed
                {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                self.show_notice(severity, self.message.clone());
            }
        }
        if self.snapshot.as_ref().is_some_and(|s| {
            s.pending_prompt.is_some()
                || matches!(
                    s.connection,
                    ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed
                )
        }) {
            self.pending_controls.clear();
        }
        self.observe_saved_result();
        self.record_activity(self.message.clone());
        if self.diagnostics {
            eprintln!("UI: {}", self.message);
            if let Some(snapshot) = &self.snapshot {
                eprintln!("Connection: {:?}", snapshot.connection);
                if let Some(error) = &snapshot.last_error {
                    eprintln!("Connection/operation error: {error}");
                }
                if let Some(codec) = &snapshot.audio_codec {
                    eprintln!("{}", app::audio_codec_line(codec));
                }
                for line in app::control_lines(snapshot) {
                    eprintln!("{line}");
                }
            }
        }
    }

    pub(super) fn show_notice(&mut self, severity: Severity, message: String) {
        self.record_activity(message.clone());
        let Some(id) = self.notices.show(severity, message) else {
            return;
        };
        let sender = self.events.clone();
        let cancel = self.lifetime.clone();
        self.runtime.spawn(async move {
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = async {
                    tokio::time::sleep(DISPLAY_DURATION).await;
                    let _ = sender.send(Event::NoticeExpired(id)).await;
                } => {}
            }
        });
    }

    pub(super) fn record_activity(&mut self, message: String) {
        if self.activity.back() != Some(&message) {
            if self.activity.len() == 256 {
                self.activity.pop_front();
            }
            self.activity.push_back(message);
        }
    }

    pub(super) fn refresh_display_language(&mut self, cx: &mut Context<Self>) {
        self.display_language_deferred = true;
        self.start_display_language_refresh();
        cx.notify();
    }

    fn start_display_language_refresh(&mut self) {
        if self.preview
            || !self.display_language_deferred
            || self.display_language_request.is_some()
        {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        if handle.snapshot().connection != ezsc_core::ConnectionState::Ready {
            return;
        }
        let requested = self.preferences.language;
        let epoch = self.epoch;
        self.display_language_request = Some((epoch, requested));
        self.display_language_deferred = false;
        let sender = self.events.clone();
        self.runtime.spawn(async move {
            let result = handle
                .set_display_language(display_language(requested))
                .await
                .map_err(|error| error.to_string());
            let _ = sender
                .send(Event::DisplayLanguage(epoch, requested, result))
                .await;
        });
    }

    pub(super) fn begin_add_device(&mut self) {
        if self.settings_blocked() || !self.saved_loaded {
            return;
        }
        self.adding_device = true;
        self.discover();
    }

    pub(super) fn finish_add_device(&mut self) {
        self.adding_device = false;
        self.discovery_generation += 1;
        if let Some(cancel) = self.discovery_cancel.take() {
            cancel.cancel();
        }
        self.discovering = false;
        self.devices.clear();
    }

    pub(super) fn discover(&mut self) {
        if self.preview || !self.adding_device || self.discovering || self.settings_blocked() {
            return;
        }
        let Some(manager) = self.manager.clone() else {
            return;
        };
        self.discovery_generation += 1;
        let generation = self.discovery_generation;
        let cancel = self.lifetime.child_token();
        self.discovery_cancel = Some(cancel.clone());
        self.discovering = true;
        self.message = self
            .preferences
            .language
            .text(
                "Looking for paired MDR v2 devices…",
                "正在查找已配对的 MDR v2 耳机…",
            )
            .into();
        let sender = self.events.clone();
        self.runtime.spawn(async move {
            let result = tokio::select! {
                _ = cancel.cancelled() => return,
                result = manager.discover() => result.map_err(|error| error.to_string()),
            };
            let _ = sender.send(Event::Devices(generation, result)).await;
        });
    }

    pub(super) fn select_saved_device(&mut self, device: DeviceId) {
        if self.settings_blocked() {
            return;
        }
        let Some(saved) = self.saved_devices.find(&device).cloned() else {
            return;
        };
        if self.preview {
            self.focused_device = Some(device.clone());
            self.saved_devices.last_selected = Some(device.0.clone());
            self.finish_add_device();
            if matches!(device.0.as_str(), "preview-tws" | "preview-headband") {
                self.snapshot = Some(crate::gui_preview::snapshot(device.0 == "preview-tws"));
                self.selected = Some(device);
            } else {
                self.snapshot = None;
                self.selected = None;
            }
            return;
        }
        self.connect(device, saved.transport_kind());
    }

    pub(super) fn focused_name(&self) -> Option<String> {
        let device = self.focused_device.as_ref()?;
        self.snapshot
            .as_ref()
            .filter(|snapshot| &snapshot.device == device)
            .and_then(|snapshot| snapshot.model_name.as_ref())
            .and_then(|name| name.as_utf8().ok())
            .map(str::to_owned)
            .or_else(|| {
                self.saved_devices
                    .find(device)
                    .map(|saved| saved.display_name().to_owned())
            })
            .or_else(|| self.connecting_name.clone())
    }

    pub(super) fn device_connection(&self, device: &DeviceId) -> ezsc_core::ConnectionState {
        use ezsc_core::ConnectionState;
        if self.cleanup_device.as_ref() == Some(device) {
            return if self.busy {
                ConnectionState::Closing
            } else {
                ConnectionState::Failed
            };
        }
        if let Some(snapshot) = self
            .snapshot
            .as_ref()
            .filter(|snapshot| &snapshot.device == device)
        {
            return snapshot.connection;
        }
        if self.selected.as_ref() == Some(device) {
            return if self.busy {
                ConnectionState::Connected
            } else {
                ConnectionState::Failed
            };
        }
        ConnectionState::Disconnected
    }

    pub(super) fn connect_focused_device(&mut self) {
        let Some(device) = self.focused_device.clone() else {
            return;
        };
        if self.saved_devices.find(&device).is_some() {
            self.select_saved_device(device);
        } else if let Some(transport) = self.attempted_transport {
            // An unsuccessful candidate stays out of the saved directory, but
            // the current view retains enough information for an explicit retry.
            self.connect(device, transport);
        }
    }

    pub(super) fn connect(&mut self, device: DeviceId, transport: TransportKind) {
        if self.preview || self.settings_blocked() || !self.saved_loaded {
            return;
        }
        let Some(manager) = self.manager.clone() else {
            return;
        };
        if self.selected.as_ref() == Some(&device)
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.connection == ezsc_core::ConnectionState::Ready)
        {
            self.finish_add_device();
            return;
        }
        let name = self
            .devices
            .iter()
            .find(|entry| entry.id == device)
            .and_then(|entry| entry.name.clone())
            .or_else(|| {
                self.saved_devices
                    .find(&device)
                    .and_then(|saved| saved.name.clone())
            })
            .or_else(|| {
                (self.focused_device.as_ref() == Some(&device))
                    .then(|| self.connecting_name.clone())
                    .flatten()
            });
        let previous = self
            .cleanup_device
            .clone()
            .or_else(|| self.selected.clone());
        self.cleanup_device = previous.clone();
        self.pending_controls.clear();
        self.display_language_request = None;
        self.display_language_deferred = false;
        self.epoch += 1;
        let epoch = self.epoch;
        self.focused_device = Some(device.clone());
        self.selected = Some(device.clone());
        self.connecting_name = name.clone();
        self.attempted_transport = Some(transport);
        self.snapshot = None;
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.replying_to = None;
        self.prompt_error = None;
        self.busy = true;
        self.message = self
            .preferences
            .language
            .text("Connecting…", "正在连接…")
            .into();
        if self.saved_devices.find(&device).is_some() {
            self.saved_devices.last_selected = Some(device.0.clone());
            self.persist_saved_devices();
        }
        self.finish_add_device();
        let sender = self.events.clone();
        let cancel = self.lifetime.clone();
        let display_language = display_language(self.preferences.language);
        self.runtime.spawn(async move {
            let result = async {
                if let Some(previous) = previous {
                    if let Some(error) = disconnect_control(&manager, &previous).await? {
                        let _ = sender.send(Event::CleanupWarning(epoch, error)).await;
                    }
                    sender.send(Event::PreviousReleased(epoch)).await.map_err(|_| "Application closed".to_owned())?;
                }
                if cancel.is_cancelled() {
                    return Ok(());
                }
                let config = SessionConfig { report_foreground: true, language: display_language, ..SessionConfig::default() };
                let handle = manager.connect(device.clone(), transport, config).await.map_err(|error| error.to_string())?;
                let mut updates = handle.subscribe();
                if sender.send(Event::Connected(epoch, handle.clone())).await.is_err() { handle.cancel(); return Ok(()); }
                let snapshots = sender.clone();
                tokio::spawn(async move {
                    loop {
                        let snapshot = updates.borrow_and_update().clone();
                        let terminal = matches!(snapshot.connection, ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed);
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            result = snapshots.send(Event::Snapshot(epoch, Box::new(snapshot))) => if result.is_err() { break; },
                        }
                        if terminal { break; }
                        tokio::select! { _ = cancel.cancelled() => break, result = updates.changed() => if result.is_err() { break; } }
                    }
                });
                handle.initialize().await.map_err(|error| error.to_string())?;
                let snapshot = handle.snapshot();
                let saved = SavedDevice {
                    id: device.0,
                    name,
                    model_name: snapshot.model_name.as_ref().and_then(|name| name.as_utf8().ok()).map(str::to_owned),
                    model_color: snapshot.appearance.map(|appearance| appearance.color.as_raw()),
                    transport: transport.into(),
                };
                sender.send(Event::Initialized(epoch, saved)).await.map_err(|_| "Application closed".to_owned())?;
                gui_controls::refresh_controls(&handle).await
            }.await;
            let _ = sender.send(Event::Finished(epoch, result.map(|_| Completion::StateUpdated))).await;
        });
    }

    pub(super) fn forget_device(&mut self, device: DeviceId) {
        if self.settings_blocked() || self.saved_devices.find(&device).is_none() {
            return;
        }
        if self.preview {
            if self.selected.as_ref() == Some(&device) {
                self.selected = None;
                self.snapshot = None;
            }
            self.remove_saved_device(&device);
        } else if self.selected.as_ref() == Some(&device)
            || self.cleanup_device.as_ref() == Some(&device)
        {
            self.release_device(Some(device));
        } else {
            self.remove_saved_device(&device);
        }
    }

    fn remove_saved_device(&mut self, device: &DeviceId) {
        self.saved_devices.remove(device);
        if self.focused_device.as_ref() == Some(device) {
            self.focused_device = None;
            self.connecting_name = None;
            self.attempted_transport = None;
        }
        self.persist_saved_devices();
    }

    pub(super) fn release(&mut self) {
        if self.settings_blocked() {
            return;
        }
        if self.preview {
            self.selected = None;
            self.snapshot = None;
            return;
        }
        self.release_device(None);
    }

    fn release_device(&mut self, forget: Option<DeviceId>) {
        let Some(device) = self
            .cleanup_device
            .clone()
            .or_else(|| self.selected.clone())
        else {
            return;
        };
        let Some(manager) = self.manager.clone() else {
            return;
        };
        self.cleanup_device = Some(device.clone());
        self.selected = None;
        self.pending_controls.clear();
        self.display_language_request = None;
        self.display_language_deferred = false;
        self.epoch += 1;
        let epoch = self.epoch;
        self.busy = true;
        self.replying_to = None;
        self.prompt_error = None;
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.snapshot = None;
        self.message = self
            .preferences
            .language
            .text("Releasing control…", "正在释放控制连接…")
            .into();
        let sender = self.events.clone();
        self.runtime.spawn(async move {
            let result = async {
                if let Some(error) = disconnect_control(&manager, &device).await? {
                    let _ = sender.send(Event::CleanupWarning(epoch, error)).await;
                }
                Ok(forget.map_or(Completion::Released, Completion::Forgotten))
            }
            .await;
            let _ = sender.send(Event::Finished(epoch, result)).await;
        });
    }

    fn persist_saved_devices(&mut self) {
        if self.preview {
            return;
        }
        self.saved_revision += 1;
        if let Some(writer) = &self.saved_writer {
            writer
                .updates
                .send_replace(Some((self.saved_revision, self.saved_devices.clone())));
        }
    }

    fn observe_saved_result(&mut self) {
        let outcome = self
            .saved_writer
            .as_ref()
            .map(|writer| writer.completed.borrow().clone());
        if let Some((revision, error)) = outcome
            && revision > self.saved_result_revision
        {
            self.saved_result_revision = revision;
            self.device_storage_error = error;
            if let Some(error) = &self.device_storage_error {
                self.show_notice(Severity::Error, error.clone());
            }
        }
    }

    pub(super) fn flush_saved_devices(
        &self,
    ) -> impl std::future::Future<Output = ()> + Send + 'static + use<> {
        let target = self.saved_revision;
        let completed = self
            .saved_writer
            .as_ref()
            .map(|writer| writer.completed.clone());
        async move {
            if let Some(mut completed) = completed {
                loop {
                    if completed.borrow_and_update().0 >= target {
                        return;
                    }
                    if completed.changed().await.is_err() {
                        return;
                    }
                }
            }
        }
    }

    pub(super) fn refresh(&mut self) {
        if self.settings_blocked() {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        self.busy = true;
        self.message = self
            .preferences
            .language
            .text(
                "Reading battery, audio codec and sound controls…",
                "正在读取电量、音频编码和声音设置…",
            )
            .into();
        let sender = self.events.clone();
        let epoch = self.epoch;
        self.runtime.spawn(async move {
            let result = async {
                handle
                    .refresh_battery()
                    .await
                    .map_err(|error| error.to_string())?;
                gui_controls::refresh_controls(&handle).await?;
                if handle.snapshot().audio_codec.is_some() {
                    handle
                        .refresh_audio_codec()
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
            .await
            .map(|_| Completion::StateUpdated);
            let _ = sender.send(Event::Finished(epoch, result)).await;
        });
    }
}

// A completed cleanup may report a transport error forever through close().
// Only the explicit completion flag permits a replacement after that error;
// a timed-out cleanup still owns its native resources and must remain blocked.
async fn disconnect_control(
    manager: &DeviceManager,
    device: &DeviceId,
) -> Result<Option<String>, String> {
    match manager.disconnect(device).await {
        Ok(()) => Ok(None),
        Err(error) => {
            let finished = manager
                .devices()
                .iter()
                .any(|handle| handle.snapshot().device == *device && handle.cleanup_finished());
            if finished {
                Ok(Some(error.to_string()))
            } else {
                Err(error.to_string())
            }
        }
    }
}

struct SavedWriter {
    updates: watch::Sender<Option<(u64, SavedDevices)>>,
    completed: watch::Receiver<(u64, Option<String>)>,
}

impl SavedWriter {
    fn new(runtime: &Handle, events: mpsc::Sender<Event>) -> Self {
        let (updates, mut incoming) = watch::channel::<Option<(u64, SavedDevices)>>(None);
        let (completed, results) = watch::channel((0, None));
        runtime.spawn(async move {
            while incoming.changed().await.is_ok() {
                let Some((revision, saved)) = incoming.borrow_and_update().clone() else {
                    continue;
                };
                let error = tokio::task::spawn_blocking(move || {
                    saved.save().map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| format!("Unable to save devices: {error}"))
                .and_then(|result| result)
                .err();
                completed.send_replace((revision, error));
                // Never block persistence behind the UI event queue during shutdown.
                let _ = events.try_send(Event::SavedWritten);
            }
        });
        Self {
            updates,
            completed: results,
        }
    }
}

fn display_language(language: Language) -> libmdr2::types::DisplayLanguage {
    match language {
        Language::English => libmdr2::types::DisplayLanguage::ENGLISH,
        Language::Chinese => libmdr2::types::DisplayLanguage::SIMPLIFIED_CHINESE,
    }
}

fn operation_severity(result: &Result<OperationResult, String>) -> Option<Severity> {
    match result {
        Err(_) => Some(Severity::Error),
        Ok(OperationResult::Adjusted) => Some(Severity::Warning),
        // Pending device decisions already have a dedicated confirmation dialog.
        Ok(
            OperationResult::Synchronized
            | OperationResult::Confirmed
            | OperationResult::AwaitingConfirmation
            | OperationResult::Cancelled
            | OperationResult::Dismissed,
        ) => None,
    }
}

pub(super) fn operation_message(
    label: &str,
    result: Result<OperationResult, String>,
    language: Language,
) -> String {
    match result {
        Ok(OperationResult::Confirmed) => format!("{}: {label}. {}", language.text("Confirmed", "已确认"), language.text("The headset confirmed the requested value.", "耳机已确认请求的设置。")),
        Ok(OperationResult::Adjusted) => format!("{}: {label}. {}", language.text("Adjusted", "设备已调整"), language.text("The headset reported a different value; controls show its actual state.", "耳机返回了不同的值；界面已显示实际状态。")),
        Ok(OperationResult::Synchronized) => format!("{}: {label}.", language.text("Updated", "已更新")),
        Ok(OperationResult::AwaitingConfirmation) => language.text("A headset message interrupted this change. Review it, then check the reported settings before trying again.", "耳机提示中断了此次更改。请先处理提示并检查实际设置，再决定是否重试。").into(),
        Ok(OperationResult::Cancelled) => language.text("Cancelled. Your decision was sent to the headset.", "已取消，并已将决定发送给耳机。").into(),
        Ok(OperationResult::Dismissed) => language.text("Device message closed locally.", "已在本地关闭设备提示。").into(),
        Err(error) => format!("{} {label}: {error}", language.text("Could not confirm", "无法确认")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bluetooth::{Backend, BoxFuture, Connection};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingBackend {
        inner: bluetooth::fake::FakeBackend,
        discoveries: Arc<AtomicUsize>,
    }

    impl Backend for CountingBackend {
        fn discover(
            &self,
            cancel: CancellationToken,
        ) -> BoxFuture<'_, bluetooth::Result<Vec<Device>>> {
            self.discoveries.fetch_add(1, Ordering::SeqCst);
            self.inner.discover(cancel)
        }
        fn connect(
            &self,
            device: DeviceId,
            transport: TransportKind,
            cancel: CancellationToken,
        ) -> BoxFuture<'_, bluetooth::Result<Connection>> {
            self.inner.connect(device, transport, cancel)
        }
    }

    fn descriptor(id: &str) -> Device {
        Device {
            id: DeviceId(id.into()),
            name: Some(id.into()),
            transports: vec![TransportKind::Rfcomm],
        }
    }

    fn saved(id: &str) -> SavedDevice {
        SavedDevice {
            id: id.into(),
            name: Some(id.into()),
            model_name: None,
            model_color: None,
            transport: TransportKind::Rfcomm.into(),
        }
    }

    fn session() -> (Session, mpsc::Receiver<Event>) {
        let (events, incoming) = mpsc::channel(32);
        (
            Session {
                runtime: Handle::current(),
                manager: None,
                events,
                lifetime: CancellationToken::new(),
                devices: Vec::new(),
                saved_devices: SavedDevices::default(),
                focused_device: None,
                adding_device: false,
                device_storage_error: None,
                selected: None,
                handle: None,
                snapshot: None,
                epoch: 0,
                discovering: false,
                busy: false,
                message: String::new(),
                diagnostics: false,
                replying_to: None,
                preferences: Preferences::default(),
                activity: Default::default(),
                notices: Default::default(),
                pending_controls: Default::default(),
                preview: false,
                prompt_error: None,
                display_language_request: None,
                display_language_deferred: false,
                saved_loaded: true,
                saved_writer: None,
                saved_revision: 0,
                saved_result_revision: 0,
                discovery_generation: 0,
                discovery_cancel: None,
                cleanup_device: None,
                connecting_name: None,
                attempted_transport: None,
            },
            incoming,
        )
    }

    async fn until_finished(state: &mut Session, incoming: &mut mpsc::Receiver<Event>) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let event = incoming.recv().await.unwrap();
                let finished = matches!(event, Event::Finished(_, _));
                state.receive(event);
                if finished {
                    break;
                }
            }
        })
        .await
        .expect("connection completes after fake peer closes");
    }

    #[tokio::test]
    async fn normal_results_stay_quiet_and_operation_outcomes_have_explicit_severity() {
        let (mut state, _incoming) = session();
        for completion in [Completion::StateUpdated, Completion::Released] {
            state.receive(Event::Finished(0, Ok(completion)));
            assert!(state.notices.current().is_none());
        }
        for result in [
            OperationResult::Confirmed,
            OperationResult::Synchronized,
            OperationResult::AwaitingConfirmation,
            OperationResult::Cancelled,
            OperationResult::Dismissed,
        ] {
            state.receive(Event::Operation(0, "Sound".into(), Ok(result)));
            assert!(state.notices.current().is_none());
        }
        state.receive(Event::Operation(
            0,
            "Sound".into(),
            Ok(OperationResult::Adjusted),
        ));
        let notice = state.notices.current().unwrap();
        assert_eq!(notice.severity, Severity::Warning);
        let id = notice.id;
        state.receive(Event::NoticeExpired(id));
        state.receive(Event::Operation(
            0,
            "Sound".into(),
            Err("Readback failed".into()),
        ));
        let error = state.notices.current().unwrap();
        assert_eq!(error.severity, Severity::Error);
        assert!(error.message.contains("Readback failed"));
        let id = error.id;
        state.receive(Event::Finished(0, Ok(Completion::StateUpdated)));
        assert_eq!(state.notices.current().unwrap().id, id);
        state.receive(Event::CleanupWarning(0, "Cleanup warning".into()));
        assert_eq!(state.notices.current().unwrap().id, id);
        assert!(
            state
                .activity
                .iter()
                .any(|line| line.contains("Cleanup warning"))
        );
    }

    #[tokio::test]
    async fn stale_epochs_and_closed_discovery_do_not_raise_notices() {
        let (mut state, _incoming) = session();
        state.epoch = 2;
        state.discovery_generation = 3;
        for event in [
            Event::Finished(1, Err("Old failure".into())),
            Event::Operation(1, "Sound".into(), Err("Old failure".into())),
            Event::CleanupWarning(1, "Old warning".into()),
            Event::Devices(3, Err("Closed discovery".into())),
        ] {
            state.receive(event);
            assert!(state.notices.current().is_none());
        }
        state.adding_device = true;
        state.receive(Event::Devices(2, Err("Old discovery".into())));
        assert!(state.notices.current().is_none());
        state.receive(Event::Devices(3, Err("Current discovery".into())));
        let notice = state.notices.current().unwrap();
        assert_eq!(notice.severity, Severity::Error);
        assert!(notice.message.contains("Current discovery"));
    }

    #[tokio::test]
    async fn notice_timer_expires_without_further_device_events() {
        let (mut state, mut incoming) = session();
        state.show_notice(Severity::Warning, "Temporary warning".into());
        let id = state.notices.current().unwrap().id;
        let event =
            tokio::time::timeout(DISPLAY_DURATION + Duration::from_secs(2), incoming.recv())
                .await
                .expect("notice timer completes")
                .expect("session event channel remains open");
        assert!(matches!(&event, Event::NoticeExpired(expired) if *expired == id));
        state.receive(event);
        assert!(state.notices.current().is_none());
        assert!(
            state
                .activity
                .iter()
                .any(|entry| entry == "Temporary warning")
        );
    }

    #[tokio::test]
    async fn old_notice_timer_cannot_clear_new_notice_or_mutate_device_state() {
        let (mut state, _incoming) = session();
        state.show_notice(Severity::Warning, "First warning".into());
        let first = state.notices.current().unwrap().id;
        state.show_notice(Severity::Error, "New error".into());
        let current = state.notices.current().unwrap().id;
        state.busy = true;
        state.message = "Reading settings".into();
        let mut failed = crate::gui_preview::snapshot(true);
        failed.connection = ezsc_core::ConnectionState::Failed;
        state.selected = Some(failed.device.clone());
        state.snapshot = Some(failed);
        let history = state.activity.clone();
        state.receive(Event::NoticeExpired(first));
        assert_eq!(state.notices.current().unwrap().id, current);
        state.receive(Event::NoticeExpired(current));
        assert!(state.notices.current().is_none());
        assert_eq!(state.message, "Reading settings");
        assert_eq!(state.activity, history);
        assert!(state.busy);
    }

    #[tokio::test]
    async fn disconnect_notices_appear_once_and_intentional_release_stays_quiet() {
        for connection in [
            ezsc_core::ConnectionState::Disconnected,
            ezsc_core::ConnectionState::Failed,
        ] {
            let (mut state, _incoming) = session();
            let ready = crate::gui_preview::snapshot(true);
            state.selected = Some(ready.device.clone());
            state.snapshot = Some(ready.clone());
            let mut ended = ready.clone();
            ended.connection = connection;
            state.receive(Event::Snapshot(0, Box::new(ended.clone())));
            let notice = state.notices.current().unwrap();
            assert_eq!(
                notice.severity,
                if connection == ezsc_core::ConnectionState::Failed {
                    Severity::Error
                } else {
                    Severity::Warning
                }
            );
            let id = notice.id;
            state.receive(Event::Snapshot(0, Box::new(ended.clone())));
            assert_eq!(state.notices.current().unwrap().id, id);
            state.receive(Event::NoticeExpired(id));
            ended.connection = ezsc_core::ConnectionState::Disconnected;
            state.receive(Event::Snapshot(0, Box::new(ended.clone())));
            assert!(state.notices.current().is_none());

            state.preview = true;
            state.snapshot = Some(ready);
            state.release();
            state.receive(Event::Snapshot(0, Box::new(ended)));
            assert!(state.notices.current().is_none());
        }
    }

    #[tokio::test]
    async fn storage_and_startup_errors_use_notice_severity_without_replaying_saved_results() {
        let (mut state, _incoming) = session();
        state.receive(Event::SavedLoaded(
            SavedDevices::default(),
            Some("Invalid saved directory".into()),
        ));
        let notice = state.notices.current().unwrap();
        assert_eq!(notice.severity, Severity::Warning);
        let id = notice.id;
        state.receive(Event::NoticeExpired(id));
        state.receive(Event::Backend(Err("Bluetooth unavailable".into())));
        assert_eq!(state.notices.current().unwrap().severity, Severity::Error);

        let (updates, _) = watch::channel(None);
        let (complete, completed) = watch::channel((1, Some("Cannot save devices".into())));
        state.saved_writer = Some(SavedWriter { updates, completed });
        state.receive(Event::SavedWritten);
        let notice = state.notices.current().unwrap();
        assert_eq!(notice.severity, Severity::Error);
        assert_eq!(notice.message, "Cannot save devices");
        let id = notice.id;
        state.receive(Event::NoticeExpired(id));
        state.receive(Event::SavedWritten);
        assert!(state.notices.current().is_none());
        complete.send_replace((2, None));
        state.receive(Event::SavedWritten);
        assert!(state.notices.current().is_none());
        assert!(state.device_storage_error.is_none());
    }

    #[tokio::test]
    async fn display_label_and_prompt_failures_respect_their_request_identity() {
        let (mut state, _incoming) = session();
        let language = state.preferences.language;
        state.receive(Event::DisplayLanguage(
            0,
            language,
            Err("Unrequested label failure".into()),
        ));
        assert!(state.notices.current().is_none());
        state.display_language_request = Some((0, language));
        state.receive(Event::DisplayLanguage(
            0,
            language,
            Err("Current label failure".into()),
        ));
        let notice = state.notices.current().unwrap();
        assert!(notice.message.contains("Current label failure"));
        let id = notice.id;
        state.receive(Event::NoticeExpired(id));

        let prompt = PromptId {
            generation: 1,
            serial: 2,
        };
        state.receive(Event::PromptResponse(
            0,
            prompt,
            Err("Stale decision".into()),
        ));
        assert!(state.notices.current().is_none());
        state.replying_to = Some(prompt);
        state.receive(Event::PromptResponse(
            0,
            prompt,
            Err("Current decision failed".into()),
        ));
        assert_eq!(state.notices.current().unwrap().severity, Severity::Error);
        assert!(
            state
                .notices
                .current()
                .unwrap()
                .message
                .contains("Current decision failed")
        );
        assert_eq!(
            state.prompt_error,
            Some((prompt, "Current decision failed".into()))
        );
    }

    #[tokio::test]
    async fn startup_does_not_discover_and_closed_add_flow_ignores_late_results() {
        let (backend, _openings) =
            bluetooth::fake::FakeBackend::new(vec![descriptor("new")], 8, 32).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let manager = Arc::new(DeviceManager::new(Arc::new(CountingBackend {
            inner: backend,
            discoveries: count.clone(),
        })));
        let (mut state, mut incoming) = session();
        let mut directory = SavedDevices::default();
        directory.upsert(saved("known"));
        state.receive(Event::SavedLoaded(directory, None));
        state.receive(Event::Backend(Ok(manager.clone())));
        tokio::task::yield_now().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(state.focused_device, Some(DeviceId("known".into())));
        assert!(state.selected.is_none());
        assert!(incoming.try_recv().is_err());

        state.begin_add_device();
        let event = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        state.finish_add_device();
        state.receive(event);
        assert!(state.devices.is_empty());
        assert!(!state.discovering);
        assert!(!state.adding_device);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn switching_waits_for_cleanup_and_failed_candidates_remain_unsaved_and_retryable() {
        let (backend, mut openings) =
            bluetooth::fake::FakeBackend::new(vec![descriptor("old"), descriptor("new")], 8, 32)
                .unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let manager = Arc::new(DeviceManager::new(Arc::new(CountingBackend {
            inner: backend,
            discoveries: count.clone(),
        })));
        let old = manager
            .connect(
                DeviceId("old".into()),
                TransportKind::Rfcomm,
                SessionConfig::default(),
            )
            .await
            .unwrap();
        let old_peer = openings.recv().await.unwrap();
        let (mut state, mut incoming) = session();
        state.manager = Some(manager.clone());
        state.selected = Some(DeviceId("old".into()));
        state.focused_device = state.selected.clone();
        state.handle = Some(old);
        state.saved_devices.upsert(saved("old"));
        state.devices = vec![descriptor("new")];
        state.adding_device = true;
        state.connect(DeviceId("new".into()), TransportKind::Rfcomm);
        tokio::time::timeout(Duration::from_secs(2), old_peer.cancel.cancelled())
            .await
            .unwrap();
        assert!(
            openings.try_recv().is_err(),
            "target must wait for native cleanup acknowledgement"
        );
        assert!(state.saved_devices.find(&DeviceId("new".into())).is_none());
        old_peer.finish(Ok(()));
        let new_peer = tokio::time::timeout(Duration::from_secs(2), openings.recv())
            .await
            .unwrap()
            .unwrap();
        new_peer.finish(Err(bluetooth::Error::new(
            bluetooth::ErrorKind::Disconnected,
            "synthetic peer stopped before MDR initialization",
        )));
        until_finished(&mut state, &mut incoming).await;
        assert!(state.saved_devices.find(&DeviceId("new".into())).is_none());
        assert!(!state.busy);
        assert_eq!(state.focused_device, Some(DeviceId("new".into())));

        assert_eq!(state.focused_name().as_deref(), Some("new"));
        state.connect_focused_device();
        let retry = tokio::time::timeout(Duration::from_secs(2), openings.recv())
            .await
            .unwrap()
            .unwrap();
        retry.finish(Ok(()));
        until_finished(&mut state, &mut incoming).await;
        assert!(state.saved_devices.find(&DeviceId("new".into())).is_none());
        assert_eq!(state.focused_name().as_deref(), Some("new"));
        state.release();
        until_finished(&mut state, &mut incoming).await;
        assert!(state.selected.is_none());
        state.connect_focused_device();
        let retry_after_release = tokio::time::timeout(Duration::from_secs(2), openings.recv())
            .await
            .unwrap()
            .unwrap();
        retry_after_release.finish(Ok(()));
        until_finished(&mut state, &mut incoming).await;
        assert!(state.saved_devices.find(&DeviceId("new".into())).is_none());
        assert_eq!(
            count.load(Ordering::SeqCst),
            0,
            "switch/retry must never enumerate devices"
        );
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn timed_out_cleanup_blocks_switch_until_native_completion() {
        let (backend, mut openings) =
            bluetooth::fake::FakeBackend::new(vec![descriptor("old"), descriptor("new")], 8, 32)
                .unwrap();
        let manager = Arc::new(DeviceManager::new(Arc::new(backend)));
        let old = manager
            .connect(
                DeviceId("old".into()),
                TransportKind::Rfcomm,
                SessionConfig {
                    close_timeout: Duration::from_millis(20),
                    ..SessionConfig::default()
                },
            )
            .await
            .unwrap();
        let old_peer = openings.recv().await.unwrap();
        let (mut state, mut incoming) = session();
        state.manager = Some(manager.clone());
        state.selected = Some(DeviceId("old".into()));
        state.handle = Some(old.clone());
        state.connect(DeviceId("new".into()), TransportKind::Rfcomm);
        until_finished(&mut state, &mut incoming).await;
        assert!(!old.cleanup_finished());
        assert!(
            openings.try_recv().is_err(),
            "timed-out cleanup must not overlap a new device"
        );
        assert_eq!(state.cleanup_device, Some(DeviceId("old".into())));

        old_peer.finish(Ok(()));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !old.cleanup_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        state.connect(DeviceId("new".into()), TransportKind::Rfcomm);
        let new_peer = tokio::time::timeout(Duration::from_secs(2), openings.recv())
            .await
            .unwrap()
            .unwrap();
        new_peer.finish(Ok(()));
        until_finished(&mut state, &mut incoming).await;
        assert!(state.cleanup_device.is_none());
        assert!(
            state
                .activity
                .iter()
                .any(|entry| entry.contains("cleanup completed") || entry.contains("清理已完成"))
        );
        // close() keeps its original timeout result, even after cleanup finishes.
        assert!(manager.shutdown().await.is_err());
    }

    #[tokio::test]
    async fn forgetting_active_device_removes_record_only_after_cleanup() {
        let (backend, mut openings) =
            bluetooth::fake::FakeBackend::new(vec![descriptor("known")], 8, 32).unwrap();
        let manager = Arc::new(DeviceManager::new(Arc::new(backend)));
        let handle = manager
            .connect(
                DeviceId("known".into()),
                TransportKind::Rfcomm,
                SessionConfig::default(),
            )
            .await
            .unwrap();
        let peer = openings.recv().await.unwrap();
        let (mut state, mut incoming) = session();
        state.manager = Some(manager.clone());
        state.saved_devices.upsert(saved("known"));
        state.selected = Some(DeviceId("known".into()));
        state.focused_device = state.selected.clone();
        state.handle = Some(handle);
        state.forget_device(DeviceId("known".into()));
        tokio::time::timeout(Duration::from_secs(2), peer.cancel.cancelled())
            .await
            .unwrap();
        assert!(
            state
                .saved_devices
                .find(&DeviceId("known".into()))
                .is_some()
        );
        peer.finish(Ok(()));
        until_finished(&mut state, &mut incoming).await;
        assert!(state.saved_devices.entries.is_empty());
        assert!(state.saved_devices.last_selected.is_none());
        assert!(state.selected.is_none());
        assert!(state.focused_device.is_none());
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_initialization_cannot_remember_old_device_and_optional_refresh_failure_keeps_success()
     {
        let (mut state, _incoming) = session();
        state.epoch = 4;
        state.focused_device = Some(DeviceId("current".into()));
        state.receive(Event::Initialized(3, saved("old")));
        state.receive(Event::Snapshot(
            3,
            Box::new(crate::gui_preview::snapshot(true)),
        ));
        assert!(state.saved_devices.entries.is_empty());
        assert!(state.snapshot.is_none());
        state.receive(Event::Initialized(4, saved("current")));
        state.receive(Event::Finished(4, Err("Optional EQ refresh failed".into())));
        assert!(
            state
                .saved_devices
                .find(&DeviceId("current".into()))
                .is_some()
        );
        assert_eq!(
            state.saved_devices.last_selected.as_deref(),
            Some("current")
        );
        assert!(!state.busy);
    }
}
