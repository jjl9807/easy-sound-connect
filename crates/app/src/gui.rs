//! Desktop controls driven by current device capabilities and confirmed state.

mod gui_controls;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bluetooth::{CancellationToken, Device, DeviceId, TransportKind};
use ezsc_core::{DeviceHandle, DeviceManager, DeviceSnapshot, OperationResult, SessionConfig};
use gpui::{
    App, Bounds, Context, IntoElement, Render, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use tokio::{runtime::Handle, sync::mpsc};

type SharedManager = Arc<Mutex<Option<Arc<DeviceManager>>>>;

enum Event {
    Backend(Result<Arc<DeviceManager>, String>),
    Devices(Result<Vec<Device>, String>),
    Connected(u64, DeviceHandle),
    Snapshot(u64, Box<DeviceSnapshot>),
    Finished(u64, Result<String, String>),
    Operation(u64, String, Result<OperationResult, String>),
}

struct Desktop {
    runtime: Handle,
    manager: Option<Arc<DeviceManager>>,
    events: mpsc::Sender<Event>,
    lifetime: CancellationToken,
    devices: Vec<Device>,
    selected: Option<DeviceId>,
    handle: Option<DeviceHandle>,
    snapshot: Option<DeviceSnapshot>,
    epoch: u64,
    discovering: bool,
    busy: bool,
    message: String,
    diagnostics: bool,
}

impl Desktop {
    fn new(
        runtime: Handle,
        shared: SharedManager,
        lifetime: CancellationToken,
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
            }
        })
        .detach();
        let sender = events.clone();
        let cancel = lifetime.clone();
        runtime.spawn(async move {
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
        Self {
            runtime,
            manager: None,
            events,
            lifetime,
            devices: Vec::new(),
            selected: None,
            handle: None,
            snapshot: None,
            epoch: 0,
            discovering: false,
            busy: false,
            message: "Starting Bluetooth…".into(),
            diagnostics: std::env::args().any(|arg| arg == "--diagnostics"),
        }
    }

    fn receive(&mut self, event: Event) {
        match event {
            Event::Backend(Ok(manager)) => {
                self.manager = Some(manager);
                self.discover();
            }
            Event::Backend(Err(error)) => self.message = error,
            Event::Devices(result) => {
                self.discovering = false;
                match result {
                    Ok(devices) => {
                        if self.selected.is_none() && !self.busy {
                            self.message = if devices.is_empty() {
                                "No paired MDR v2 devices found. Pair your headset in system settings, then refresh.".into()
                            } else {
                                "Select a control connection.".into()
                            };
                        }
                        self.devices = devices;
                    }
                    Err(error) => self.message = error,
                }
            }
            Event::Connected(epoch, handle) if epoch == self.epoch => self.handle = Some(handle),
            Event::Snapshot(epoch, snapshot) if epoch == self.epoch => {
                // Queued notifications are wakeups: consult the latest watch
                // value so a delayed snapshot cannot undo an operation readback.
                self.snapshot = Some(
                    self.handle
                        .as_ref()
                        .map(DeviceHandle::snapshot)
                        .unwrap_or_else(|| *snapshot),
                );
            }
            Event::Finished(epoch, result) if epoch == self.epoch => {
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                self.busy = false;
                self.message = match result {
                    Ok(message) => message,
                    Err(error) => format!("Unable to complete: {error}"),
                };
            }
            Event::Operation(epoch, label, result) if epoch == self.epoch => {
                if let Some(handle) = &self.handle {
                    self.snapshot = Some(handle.snapshot());
                }
                self.busy = false;
                self.message = match result {
                    Ok(OperationResult::Confirmed) => {
                        format!("Confirmed: {label}. The headset confirmed the requested value.")
                    }
                    Ok(OperationResult::Adjusted) => format!(
                        "Adjusted: {label}. The headset reported a different value; the controls show its actual state."
                    ),
                    Ok(OperationResult::Synchronized) => format!("Updated: {label}."),
                    Err(error) => format!("Could not confirm {label}: {error}"),
                };
            }
            _ => {}
        }
        if let Some(snapshot) = &self.snapshot
            && matches!(
                snapshot.connection,
                ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed
            )
        {
            self.message = match &snapshot.last_error {
                Some(error) => format!(
                    "Control connection ended: {error}. Release control before reconnecting."
                ),
                None => "Control connection ended. Release control before reconnecting.".into(),
            };
        }
        if self.diagnostics {
            eprintln!("UI: {}", self.message);
            if let Some(snapshot) = &self.snapshot {
                eprintln!("Connection: {:?}", snapshot.connection);
                if let Some(error) = &snapshot.last_error {
                    eprintln!("Connection/operation error: {error}");
                }
                for line in app::control_lines(snapshot) {
                    eprintln!("{line}");
                }
            }
        }
    }

    fn discover(&mut self) {
        if self.discovering || self.busy {
            return;
        }
        let Some(manager) = self.manager.clone() else {
            return;
        };
        self.discovering = true;
        self.message = "Looking for paired MDR v2 devices…".into();
        let sender = self.events.clone();
        self.runtime.spawn(async move {
            let result = manager.discover().await.map_err(|error| error.to_string());
            let _ = sender.send(Event::Devices(result)).await;
        });
    }

    fn connect(&mut self, device: DeviceId, transport: TransportKind) {
        if self.busy || self.selected.is_some() {
            return;
        }
        let Some(manager) = self.manager.clone() else {
            return;
        };
        self.epoch += 1;
        let epoch = self.epoch;
        self.selected = Some(device.clone());
        self.snapshot = None;
        self.handle = None;
        self.busy = true;
        self.message = "Connecting…".into();
        let sender = self.events.clone();
        let cancel = self.lifetime.clone();
        self.runtime.spawn(async move {
            let result = async {
                let handle = manager.connect(device, transport, SessionConfig::default()).await.map_err(|error| error.to_string())?;
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
                gui_controls::refresh_controls(&handle).await
            }.await;
            let _ = sender.send(Event::Finished(epoch, result.map(|_| "Device state is up to date.".into()))).await;
        });
    }

    fn release(&mut self) {
        let Some(device) = self.selected.take() else {
            return;
        };
        let Some(manager) = self.manager.clone() else {
            return;
        };
        self.epoch += 1;
        let epoch = self.epoch;
        self.busy = true;
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.snapshot = None;
        self.message = "Releasing control…".into();
        let sender = self.events.clone();
        self.runtime.spawn(async move {
            let result = manager
                .disconnect(&device)
                .await
                .map(|_| "Control released.".into())
                .map_err(|error| error.to_string());
            let _ = sender.send(Event::Finished(epoch, result)).await;
        });
    }

    fn refresh(&mut self) {
        if self.busy {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        self.busy = true;
        self.message = "Reading battery and sound controls…".into();
        let sender = self.events.clone();
        let epoch = self.epoch;
        self.runtime.spawn(async move {
            let result = async {
                handle
                    .refresh_battery()
                    .await
                    .map_err(|error| error.to_string())?;
                gui_controls::refresh_controls(&handle).await
            }
            .await
            .map(|_| "Device state is up to date.".into());
            let _ = sender.send(Event::Finished(epoch, result)).await;
        });
    }
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(0x36364a))
        .hover(|style| style.bg(rgb(0x494963)))
        .cursor_pointer()
        .child(label.into())
}

impl Render for Desktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut device_list = div().flex().flex_col().flex_shrink_0().gap_3();
        for (index, device) in self.devices.iter().enumerate() {
            let mut row = div()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .rounded_lg()
                .bg(rgb(0x222227))
                .child(
                    device
                        .name
                        .clone()
                        .unwrap_or_else(|| "Unnamed headset".into()),
                );
            if self.selected.is_none() && !self.busy {
                let mut controls = div().flex().gap_2();
                for (transport_index, transport) in device.transports.iter().copied().enumerate() {
                    let id = device.id.clone();
                    controls = controls.child(
                        button(
                            ("connect", index * 2 + transport_index),
                            format!("Connect via {transport:?}"),
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.connect(id.clone(), transport);
                            cx.notify();
                        })),
                    );
                }
                row = row.child(controls);
            }
            device_list = device_list.child(row);
        }
        let mut controls = div().flex().gap_2();
        if self.manager.is_some() && !self.discovering && !self.busy {
            controls = controls.child(button("discover", "Refresh devices").on_click(cx.listener(
                |view, _, _, cx| {
                    view.discover();
                    cx.notify();
                },
            )));
        }
        if self.selected.is_some() {
            controls = controls.child(button("release", "Release control").on_click(cx.listener(
                |view, _, _, cx| {
                    view.release();
                    cx.notify();
                },
            )));
        }
        if self.handle.is_some()
            && !self.busy
            && self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.connection == ezsc_core::ConnectionState::Ready)
        {
            controls = controls.child(button("battery", "Refresh state").on_click(cx.listener(
                |view, _, _, cx| {
                    view.refresh();
                    cx.notify();
                },
            )));
        }
        let mut state = div().flex().flex_col().flex_shrink_0().gap_2();
        if let Some(snapshot) = &self.snapshot {
            for line in app::snapshot_lines(snapshot) {
                state = state.child(line);
            }
        }
        div()
            .id("desktop")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .bg(rgb(0x17171b))
            .text_color(rgb(0xe8eef4))
            .font_family("sans-serif")
            .text_size(px(15.))
            .child(div().text_size(px(25.)).child("Easy Sound Connect"))
            .child(if self.selected.is_some() {
                "Controls show the state reported by your headset."
            } else {
                "Pair your SONY headset in system settings to get started."
            })
            .child(controls)
            .child(device_list)
            .child(state)
            .child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(rgb(0x292936))
                    .text_color(rgb(0xc8c8f3))
                    .child(self.message.clone()),
            )
            .child(self.render_sound_controls(cx))
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime");
    let handle = runtime.handle().clone();
    let lifetime = CancellationToken::new();
    let manager: SharedManager = Arc::new(Mutex::new(None));
    gpui_platform::application().run(move |cx: &mut App| {
        let shutdown_manager = manager.clone();
        let shutdown_runtime = handle.clone();
        let shutdown_cancel = lifetime.clone();
        cx.on_app_quit(move |_| {
            shutdown_cancel.cancel();
            let manager = shutdown_manager
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            let task = shutdown_runtime.spawn(async move {
                if let Some(manager) = manager {
                    let _ = manager.shutdown().await;
                }
            });
            async move {
                let _ = task.await;
            }
        })
        .detach();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1160.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Easy Sound Connect".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Desktop::new(handle, manager, lifetime, cx)),
        )
        .expect("desktop window");
        cx.activate(true);
    });
    runtime.shutdown_timeout(Duration::from_secs(5));
}
