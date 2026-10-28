//! Application lifecycle: windows are views; closing one never owns a connection.
use crate::{
    Desktop, Session,
    desktop_integration::{self, DesktopEvent, Instance, ScreenPoint, TrayService},
    gui_theme,
    notices::Severity,
    preferences::{self, Language, Preferences, ThemeMode},
};
use gpui_kit::{prelude::*, *};
use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

pub(super) struct Shell {
    session: Entity<Session>,
    main: Option<AnyWindowHandle>,
    panel: Option<AnyWindowHandle>,
    tray: Option<TrayService>,
    tray_available: bool,
    pub(super) tray_error: Option<String>,
    events: mpsc::Sender<DesktopEvent>,
    pub(super) saving_preferences: bool,
    background_requested: bool,
    preview_tray: bool,
    last_prompt: Option<ezsc_core::PromptId>,
    last_connection: Option<ezsc_core::ConnectionState>,
}
impl Global for Shell {}
gpui_kit::assets::icon_assets!(
    AppIcons,
    [
        Headphones,
        SlidersHorizontal,
        Sparkles,
        Activity,
        Link,
        RefreshCw,
        Unlink,
        Trash,
        Plus,
        TriangleAlert,
        CircleAlert,
        X
    ]
);
struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let local: Option<&'static [u8]> = match path {
            "model_image_default_dark.png" => {
                Some(include_bytes!("../assets/model_image_default_dark.png"))
            }
            "model_image_default_light.png" => {
                Some(include_bytes!("../assets/model_image_default_light.png"))
            }
            "tray_icon_dark.svg" => Some(include_bytes!("../assets/tray_icon_dark.svg")),
            "tray_icon_light.svg" => Some(include_bytes!("../assets/tray_icon_light.svg")),
            _ => None,
        };
        if let Some(bytes) = local {
            Ok(Some(Cow::Borrowed(bytes)))
        } else if let Some(icon) = AppIcons.load(path)? {
            Ok(Some(icon))
        } else {
            gpui_kit::assets::Assets.load(path)
        }
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

pub(super) fn run() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Tokio runtime");
    let args: Vec<String> = std::env::args().collect();
    let preview = args.iter().any(|a| a == "--preview");
    let preview_tray = preview && args.iter().any(|a| a == "--with-tray");
    let (mut prefs, warning) = if preview {
        (Preferences::default(), None)
    } else {
        Preferences::load()
    };
    if preview {
        prefs.language = if args.iter().any(|a| a == "--zh") {
            Language::Chinese
        } else {
            Language::English
        };
        prefs.theme = if args.iter().any(|a| a == "--light") {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
    }
    if !preview && let Ok(enabled) = desktop_integration::autostart_enabled() {
        prefs.start_at_login = enabled;
    }
    let (events, mut incoming) = mpsc::channel(32);
    // Preview uses an isolated lifetime directory even when registering a tray.
    let preview_directory = preview_tray
        .then(tempfile::tempdir)
        .transpose()
        .expect("temporary preview directory");
    let _instance = if preview && !preview_tray {
        None
    } else {
        let directory = match preview_directory
            .as_ref()
            .map(|dir| Ok(dir.path().to_path_buf()))
            .unwrap_or_else(|| {
                preferences::config_path().map(|path| path.parent().unwrap().to_owned())
            }) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        };
        match runtime.block_on(Instance::acquire(&directory, events.clone())) {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => return,
            Err(e) => {
                eprintln!("Unable to coordinate the desktop instance: {e}");
                return;
            }
        }
    };
    let background = args.iter().any(|a| a == "--background") && prefs.start_hidden;
    let shared = Arc::new(Mutex::new(None));
    let lifetime = bluetooth::CancellationToken::new();
    let handle = runtime.handle().clone();
    let application = gpui_kit::application()
        .with_assets(Assets)
        .with_quit_mode(QuitMode::Explicit);
    application.on_reopen(open_main);
    application.run(move |cx| {
        gpui_kit::init(cx);
        gpui_kit::component::set_locale(prefs.language.locale());
        gui_theme::apply(prefs.theme, cx);
        crate::gui_product_images::init(handle.clone(), lifetime.clone(), preview, cx);
        let shutdown_shared = shared.clone();
        let shutdown_runtime = handle.clone();
        let shutdown_cancel = lifetime.clone();
        cx.on_app_quit(move |cx| {
            let saved = cx.global::<Shell>().session.read(cx).flush_saved_devices();
            shutdown_cancel.cancel();
            let shared = shutdown_shared.clone();
            let task = shutdown_runtime.spawn(async move {
                let manager: Option<Arc<ezsc_core::DeviceManager>> =
                    shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(manager) = manager {
                    let _ = manager.shutdown().await;
                }
            });
            async move {
                saved.await;
                let _ = task.await;
            }
        })
        .detach();
        let session = cx.new(|cx| Session::new(handle, shared, lifetime, prefs, preview, cx));
        if preview && !args.iter().any(|arg| arg == "--empty") {
            session.update(cx, |session, _| {
                session.snapshot = Some(crate::gui_preview::snapshot(
                    !args.iter().any(|arg| arg == "--headband"),
                ));
                for tws in [true, false] {
                    let snapshot = crate::gui_preview::snapshot(tws);
                    session
                        .saved_devices
                        .upsert(crate::saved_devices::SavedDevice {
                            id: snapshot.device.0,
                            name: Some(
                                if tws {
                                    "Preview earbuds"
                                } else {
                                    "Preview headphones"
                                }
                                .into(),
                            ),
                            model_name: None,
                            model_color: None,
                            transport: bluetooth::TransportKind::Rfcomm.into(),
                        });
                }
                let current = session.snapshot.as_ref().unwrap().device.clone();
                session.saved_devices.last_selected = Some(current.0.clone());
                session.focused_device = Some(current.clone());
                session.selected = Some(current);
            });
        }
        if preview {
            session.update(cx, |session, _| {
                session.message = session
                    .preferences
                    .language
                    .text(
                        "Preview · Device controls are disabled",
                        "预览 · 设备控制已禁用",
                    )
                    .into();
            });
        }
        if let Some(warning) = warning {
            session.update(cx, |state, _| {
                state.show_notice(Severity::Warning, warning.clone());
                state.message = warning;
            });
        }
        if preview {
            let notice = args.iter().find_map(|arg| match arg.as_str() {
                "--preview-notice=warning" => Some(Severity::Warning),
                "--preview-notice=error" => Some(Severity::Error),
                _ => None,
            });
            if let Some(severity) = notice {
                session.update(cx, |state, _| {
                    let message = match severity {
                        Severity::Warning => state.preferences.language.text(
                            "The headset adjusted the requested setting. Review the current value.",
                            "耳机调整了请求的设置，请检查当前值。",
                        ),
                        Severity::Error => state.preferences.language.text(
                            "Could not connect to the headphones. Check Bluetooth and try again.",
                            "无法连接耳机，请检查蓝牙连接后重试。",
                        ),
                    };
                    state.show_notice(severity, message.into());
                });
            }
        }
        cx.set_global(Shell {
            session: session.clone(),
            main: None,
            panel: None,
            tray: None,
            tray_available: false,
            tray_error: None,
            events,
            saving_preferences: false,
            background_requested: background,
            preview_tray,
            last_prompt: None,
            last_connection: None,
        });
        // Native menu hosts observe the same state as GPUI windows, including
        // pending operations, unsolicited headset updates and prompt gating.
        cx.observe(&session, |_, cx| refresh_tray(cx)).detach();
        cx.on_window_closed(|cx, id| {
            if !cx.has_global::<Shell>() {
                return;
            }
            let was_main = cx
                .global::<Shell>()
                .main
                .is_some_and(|w| w.window_id() == id);
            cx.update_global::<Shell, _>(|shell, _| {
                if was_main {
                    shell.main = None;
                }
                if shell.panel.is_some_and(|w| w.window_id() == id) {
                    shell.panel = None;
                }
            });
            if was_main {
                let shell = cx.global::<Shell>();
                let keep = shell.tray_available && shell.session.read(cx).preferences.close_to_tray;
                if !keep {
                    cx.quit();
                }
            }
        })
        .detach();
        cx.spawn(async move |cx| {
            while let Some(event) = incoming.recv().await {
                cx.update(|cx| handle_event(event, cx));
            }
        })
        .detach();
        // Display immediately; hide only after the tray host confirms availability.
        show_main(cx);
        if !preview || preview_tray {
            refresh_tray(cx);
            cx.spawn(async |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    cx.update(refresh_tray);
                }
            })
            .detach();
        }
        if preview && args.iter().any(|a| a == "--panel") {
            open_panel(None, cx);
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(5));
}

pub(super) fn open_main(cx: &mut App) {
    if !cx.has_global::<Shell>() {
        return;
    }
    cx.global_mut::<Shell>().background_requested = false;
    show_main(cx);
}
fn show_main(cx: &mut App) {
    if let Some(handle) = cx.global::<Shell>().main
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let session = cx.global::<Shell>().session.clone();
    gui_theme::apply(session.read(cx).preferences.theme, cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1160.), px(820.)),
            cx,
        ))),
        window_min_size: Some(size(px(920.), px(640.))),
        app_id: Some("io.github.jjl9807.easy-sound-connect".into()),
        ..crate::gui_titlebar::window_options()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Desktop::new(session, false, window, cx))
    }) {
        Ok((handle, view)) => {
            cx.global_mut::<Shell>().main = Some(handle);
            view.update(cx, |view, cx| {
                if view.session.read(cx).preview {
                    view.page = match std::env::args()
                        .find_map(|arg| arg.strip_prefix("--page=").map(str::to_owned))
                        .as_deref()
                    {
                        Some("sound") => crate::Page::Sound,
                        Some("settings") => crate::Page::Settings,
                        Some("preferences") => crate::Page::Preferences,
                        Some("activity") => crate::Page::Activity,
                        Some("scenes") => crate::Page::Scenes,
                        Some("about") => crate::Page::About,
                        _ => crate::Page::Device,
                    };
                }
            });
            cx.activate(true);
        }
        Err(error) => {
            eprintln!("Unable to open main window: {error}");
            if !cx.global::<Shell>().tray_available {
                cx.quit();
            }
        }
    }
}
fn open_panel(anchor: Option<ScreenPoint>, cx: &mut App) {
    cx.global_mut::<Shell>().background_requested = false;
    if let Some(handle) = cx.global_mut::<Shell>().panel.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return;
    }
    let session = cx.global::<Shell>().session.clone();
    let connected = session.read(cx).snapshot.as_ref().is_some_and(|s| {
        !matches!(
            s.connection,
            ezsc_core::ConnectionState::Disconnected
                | ezsc_core::ConnectionState::Failed
                | ezsc_core::ConnectionState::Closing
        )
    });
    let height = if connected { 680. } else { 360. };
    let display = anchor
        .and_then(|anchor| {
            cx.displays().into_iter().find(|display| {
                if let Some(id) = anchor.display_id {
                    u64::from(display.id()) == id
                } else {
                    !anchor.physical
                        && display
                            .bounds()
                            .contains(&point(px(anchor.x as f32), px(anchor.y as f32)))
                }
            })
        })
        .or_else(|| cx.primary_display());
    let mut bounds = Bounds::centered(None, size(px(320.), px(height)), cx);
    if let Some(display) = &display {
        let area = display.visible_bounds();
        bounds.size.height = bounds.size.height.min(area.size.height - px(24.));
        // Native coordinates are hints. Ignore unconvertible physical coordinates
        // rather than placing the only panel off-screen on a scaled display.
        if let Some(anchor) = anchor.filter(|p| !p.physical) {
            bounds.origin.x = px(anchor.x as f32 - 160.)
                .max(area.left() + px(8.))
                .min(area.right() - px(328.));
            bounds.origin.y = px(anchor.y as f32 + 8.)
                .max(area.top() + px(8.))
                .min(area.bottom() - bounds.size.height - px(8.));
        } else {
            bounds.origin.x = area.right() - px(336.);
            bounds.origin.y = area.top() + px(16.);
        }
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(320.), px(220.))),
        titlebar: None,
        is_resizable: false,
        is_minimizable: false,
        // X11 PopUp windows bypass the window manager and do not receive
        // normal keyboard focus, so they cannot report focus loss reliably.
        kind: if cfg!(target_os = "linux") {
            WindowKind::Floating
        } else {
            WindowKind::PopUp
        },
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        display_id: display.map(|d| d.id()),
        app_id: Some("io.github.jjl9807.easy-sound-connect".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title("Easy Sound Connect — Quick controls");
        cx.new(|cx| Desktop::new(session, true, window, cx))
    }) {
        Ok((handle, _)) => {
            cx.global_mut::<Shell>().panel = Some(handle);
            #[cfg(target_os = "linux")]
            let runtime = cx.global::<Shell>().session.read(cx).runtime.clone();
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
                #[cfg(target_os = "linux")]
                window.on_next_frame(move |window, _| {
                    let native = raw_window_handle::HasWindowHandle::window_handle(window)
                        .map(|handle| handle.as_raw());
                    let window_id = match native {
                        Ok(raw_window_handle::RawWindowHandle::Xcb(handle)) => {
                            Some(handle.window.get())
                        }
                        Ok(raw_window_handle::RawWindowHandle::Xlib(handle)) => {
                            u32::try_from(handle.window).ok()
                        }
                        Ok(_) => None,
                        Err(error) => {
                            eprintln!("Unable to obtain tray panel window handle: {error}");
                            None
                        }
                    };
                    if let Some(window_id) = window_id {
                        runtime.spawn_blocking(move || {
                            if let Err(error) = desktop_integration::focus_x11_panel(window_id) {
                                eprintln!("Unable to focus X11 tray panel: {error}");
                            }
                        });
                    }
                });
            });
            cx.activate(true);
        }
        Err(error) => {
            eprintln!("Unable to open tray panel: {error}");
            open_main(cx);
        }
    }
}
fn handle_event(event: DesktopEvent, cx: &mut App) {
    match event {
        DesktopEvent::OpenMain => open_main(cx),
        DesktopEvent::OpenPanel(anchor) => open_panel(anchor, cx),
        DesktopEvent::Menu(action) => {
            handle_menu_action(action, cx);
        }
        DesktopEvent::TrayAvailability {
            generation,
            available,
            reason,
        } => {
            if !cx
                .global::<Shell>()
                .tray
                .as_ref()
                .is_some_and(|t| t.generation() == generation)
            {
                return;
            }
            let previous_error = cx.global::<Shell>().tray_error.clone();
            let reason = if available {
                None
            } else {
                Some(reason.unwrap_or_else(|| {
                    cx.global::<Shell>()
                        .session
                        .read(cx)
                        .preferences
                        .language
                        .text(
                            "Keep the main window open to access the app.",
                            "请保留主窗口，以便继续访问应用。",
                        )
                        .into()
                }))
            };
            cx.update_global::<Shell, _>(|s, _| {
                s.tray_available = available;
                s.tray_error = reason;
            });
            report_tray_warning(previous_error, cx);
            if !available {
                cx.global_mut::<Shell>().background_requested = false;
                if cx.global::<Shell>().main.is_none() {
                    open_main(cx);
                }
            } else if cx.global::<Shell>().background_requested {
                let main = cx.global_mut::<Shell>().main.take();
                cx.global_mut::<Shell>().background_requested = false;
                if let Some(main) = main {
                    let _ = main.update(cx, |_, window, _| window.remove_window());
                }
            }
            cx.refresh_windows();
        }
    }
}
fn handle_menu_action(action: desktop_integration::MenuAction, cx: &mut App) {
    use desktop_integration::MenuAction;
    match action {
        MenuAction::OpenMain => open_main(cx),
        MenuAction::Quit => cx.quit(),
    }
}
pub(super) fn refresh_tray(cx: &mut App) {
    if !cx.has_global::<Shell>() {
        return;
    }
    let session = cx.global::<Shell>().session.clone();
    let state = session.read(cx);
    let prefs = state.preferences.clone();
    let runtime = state.runtime.clone();
    let preview = state.preview;
    let menu = crate::gui_tray_menu::build(state.snapshot.as_ref(), prefs.language);
    if preview && !cx.global::<Shell>().preview_tray {
        return;
    }
    if !prefs.tray_enabled {
        cx.update_global::<Shell, _>(|shell, _| {
            shell.tray = None;
            shell.tray_available = false;
            shell.tray_error = None;
            shell.background_requested = false;
        });
        if cx.global::<Shell>().main.is_none() {
            open_main(cx);
        }
        return;
    }
    let previous_error = cx.global::<Shell>().tray_error.clone();
    cx.update_global::<Shell, _>(|shell, _| {
        let result = if let Some(tray) = &mut shell.tray {
            tray.update(menu)
        } else {
            TrayService::start(&runtime, menu, shell.events.clone())
                .map(|tray| shell.tray = Some(tray))
        };
        if let Err(error) = result {
            shell.tray_available = false;
            shell.tray_error = Some(error);
        }
    });
    report_tray_warning(previous_error, cx);
    if cx.global::<Shell>().tray_error.is_some() {
        cx.global_mut::<Shell>().background_requested = false;
        if cx.global::<Shell>().main.is_none() {
            open_main(cx);
        }
    }
}

fn report_tray_warning(previous: Option<String>, cx: &mut App) {
    let shell = cx.global::<Shell>();
    if shell.tray_error == previous {
        return;
    }
    let Some(error) = shell.tray_error.clone() else {
        return;
    };
    let session = shell.session.clone();
    session.update(cx, |state, cx| {
        let message = format!(
            "{}: {error}",
            state
                .preferences
                .language
                .text("System tray unavailable", "系统托盘不可用")
        );
        state.show_notice(Severity::Warning, message);
        cx.notify();
    });
}

pub(super) fn change_preferences(cx: &mut App, change: impl FnOnce(&mut Preferences)) {
    if cx.global::<Shell>().saving_preferences {
        return;
    }
    let session = cx.global::<Shell>().session.clone();
    let state = session.read(cx);
    let old = state.preferences.clone();
    let mut next = old.clone();
    change(&mut next);
    let preview = state.preview;
    let runtime = state.runtime.clone();
    if old == next {
        return;
    }
    cx.global_mut::<Shell>().saving_preferences = true;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        let saved = next.clone();
        let (result, observed_autostart) = runtime
            .spawn_blocking(move || {
                let result = (|| -> Result<(), String> {
                    if preview {
                        return Ok(());
                    }
                    if old.start_at_login != saved.start_at_login {
                        desktop_integration::set_autostart(saved.start_at_login)?;
                    }
                    if let Err(error) = saved.save() {
                        if old.start_at_login != saved.start_at_login
                            && let Err(rollback) =
                                desktop_integration::set_autostart(old.start_at_login)
                        {
                            return Err(format!(
                                "{error}; could not restore login startup: {rollback}"
                            ));
                        }
                        return Err(error.to_string());
                    }
                    Ok(())
                })();
                let observed = if result.is_err() && !preview {
                    desktop_integration::autostart_enabled().ok()
                } else {
                    None
                };
                (result, observed)
            })
            .await
            .unwrap_or_else(|e| (Err(e.to_string()), None));
        cx.update(|cx| {
            cx.global_mut::<Shell>().saving_preferences = false;
            match result {
                Ok(()) => {
                    session.update(cx, |state, cx| {
                        let language_changed = state.preferences.language != next.language;
                        state.preferences = next.clone();
                        state.message = next
                            .language
                            .text("Preferences saved.", "首选项已保存。")
                            .into();
                        state.record_activity(state.message.clone());
                        if language_changed {
                            state.refresh_display_language(cx);
                        }
                        cx.notify();
                    });
                    gpui_kit::component::set_locale(next.language.locale());
                    gui_theme::apply(next.theme, cx);
                    refresh_tray(cx);
                }
                Err(error) => session.update(cx, |state, cx| {
                    if let Some(enabled) = observed_autostart {
                        state.preferences.start_at_login = enabled;
                    }
                    state.message = format!(
                        "{}: {error}",
                        state
                            .preferences
                            .language
                            .text("Could not save preferences", "无法保存首选项")
                    );
                    state.show_notice(Severity::Error, state.message.clone());
                    cx.notify();
                }),
            }
            cx.refresh_windows();
        });
    })
    .detach();
}
pub(super) fn session_changed(cx: &mut App) {
    if !cx.has_global::<Shell>() {
        return;
    }
    let session = cx.global::<Shell>().session.clone();
    let state = session.read(cx);
    let prompt = state
        .snapshot
        .as_ref()
        .and_then(|s| s.pending_prompt.as_ref())
        .map(|p| p.id);
    let connection = state.snapshot.as_ref().map(|s| s.connection);
    let l = state.preferences.language;
    let notifications = state.preferences.notifications;
    let runtime = state.runtime.clone();
    let new_prompt = prompt.is_some() && prompt != cx.global::<Shell>().last_prompt;
    let disconnected = matches!(
        connection,
        Some(ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed)
    ) && connection != cx.global::<Shell>().last_connection;
    cx.update_global::<Shell, _>(|shell, _| {
        shell.last_prompt = prompt;
        shell.last_connection = connection;
    });
    if new_prompt {
        if let Some(panel) = cx.global::<Shell>().panel {
            let _ = panel.update(cx, |_, window, _| window.activate_window());
        } else if cx.global::<Shell>().main.is_some() {
            open_main(cx);
        } else {
            open_panel(None, cx);
        }
    }
    if notifications && (new_prompt || disconnected) {
        let summary = l
            .text("Easy Sound Connect", "Easy Sound Connect")
            .to_owned();
        let body = if new_prompt {
            l.text(
                "Your headphones need confirmation. Open the app to review.",
                "耳机需要你的确认，请打开应用查看。",
            )
        } else {
            l.text(
                "Headphone control connection ended.",
                "耳机控制连接已断开。",
            )
        }
        .to_owned();
        runtime.spawn(async move {
            if let Err(error) = desktop_integration::notify(summary, body).await {
                eprintln!("Notification unavailable: {error}");
            }
        });
    }
}
