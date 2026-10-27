use super::{Appearance, DesktopEvent, MenuEntry, ScreenPoint, action, render_icon};
use ksni::{Tray, TrayMethods};
use std::time::Duration;
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot, watch},
};

pub(super) struct LinuxTray {
    appearance: watch::Sender<Appearance>,
    stop: Option<oneshot::Sender<()>>,
}

impl LinuxTray {
    pub(super) fn start(
        runtime: &Handle,
        appearance: Appearance,
        generation: u64,
        events: mpsc::Sender<DesktopEvent>,
    ) -> Result<Self, String> {
        // SNI exposes no portable panel-color query. A named symbolic icon lets
        // the host apply its own panel palette, including live theme changes.
        let icon = HostIcon::new()?;
        let (appearance_tx, mut appearance_rx) = watch::channel(appearance.clone());
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let (host_tx, mut host_rx) = watch::channel(None);
        let tray = StatusItem {
            appearance,
            icon,
            events: events.clone(),
            host: host_tx,
        };
        runtime.spawn(async move {
            // Finish the bounded registration attempt even when dropped during
            // startup, so a just-created service is always explicitly shut down.
            let handle = match tokio::time::timeout(Duration::from_secs(5), tray.spawn()).await {
                    Ok(Ok(handle)) => handle,
                    Ok(Err(error)) => {
                        let _ = events.send(DesktopEvent::TrayAvailability {
                            generation, available: false, reason: Some(error.to_string()),
                        }).await;
                        return;
                    }
                    Err(_) => {
                        let _ = events.send(DesktopEvent::TrayAvailability {
                            generation, available: false,
                            reason: Some("The system tray host did not respond".into()),
                        }).await;
                        return;
                    }
            };
            if !matches!(stop_rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
                let _ = tokio::time::timeout(Duration::from_secs(2), handle.shutdown()).await;
                return;
            }
            // spawn() used the normal host check. A host transition may already
            // have followed it; publish only the latest state from this worker.
            let (available, reason) = host_rx.borrow_and_update().clone().unwrap_or((true, None));
            let _ = events.send(DesktopEvent::TrayAvailability {
                generation, available, reason,
            }).await;
            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    result = host_rx.changed() => {
                        if result.is_err() {
                            let _ = events.send(DesktopEvent::TrayAvailability {
                                generation, available: false,
                                reason: Some("The system tray service stopped".into()),
                            }).await;
                            break;
                        }
                        let Some(mut state) = host_rx.borrow_and_update().clone() else { continue; };
                        if state.0 {
                            // ksni calls watcher_online before re-registering.
                            // An update is processed after that registration,
                            // so a failed registration's offline callback wins.
                            if !matches!(tokio::time::timeout(Duration::from_secs(2), handle.update(|_| ())).await, Ok(Some(()))) {
                                state = (false, Some("The system tray service stopped".into()));
                            } else if let Some(latest) = host_rx.borrow_and_update().clone() {
                                state = latest;
                            }
                        }
                        let _ = events.send(DesktopEvent::TrayAvailability {
                            generation, available: state.0, reason: state.1,
                        }).await;
                    }
                    result = appearance_rx.changed() => {
                        if result.is_err() { break; }
                        let next = appearance_rx.borrow_and_update().clone();
                        if !matches!(tokio::time::timeout(Duration::from_secs(2), handle.update(|tray| tray.appearance = next)).await, Ok(Some(()))) {
                            let _ = events.send(DesktopEvent::TrayAvailability {
                                generation, available: false,
                                reason: Some("The system tray service stopped".into()),
                            }).await;
                            break;
                        }
                    }
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), handle.shutdown()).await;
        });
        Ok(Self {
            appearance: appearance_tx,
            stop: Some(stop_tx),
        })
    }

    pub(super) fn update(&mut self, appearance: Appearance) -> Result<(), String> {
        self.appearance.send_if_modified(|current| {
            if *current == appearance {
                false
            } else {
                *current = appearance;
                true
            }
        });
        Ok(())
    }
}

impl Drop for LinuxTray {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn argb(rgba: Vec<u8>) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}

struct StatusItem {
    appearance: Appearance,
    icon: HostIcon,
    events: mpsc::Sender<DesktopEvent>,
    host: watch::Sender<Option<(bool, Option<String>)>>,
}

impl Tray for StatusItem {
    fn id(&self) -> String {
        "easy-sound-connect".into()
    }
    fn title(&self) -> String {
        "Easy Sound Connect".into()
    }
    fn activate(&mut self, x: i32, y: i32) {
        let hint = (x != 0 || y != 0).then_some(ScreenPoint {
            x: f64::from(x),
            y: f64::from(y),
            physical: false,
            display_id: None,
        });
        action(&self.events, DesktopEvent::OpenPanel(hint));
    }
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        action(&self.events, DesktopEvent::OpenMain);
    }
    fn icon_name(&self) -> String {
        HostIcon::NAME.into()
    }
    fn icon_theme_path(&self) -> String {
        self.icon.theme_path.clone()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![ksni::Icon {
            width: 32,
            height: 32,
            data: self.icon.fallback.clone(),
        }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Easy Sound Connect".into(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        native_menu(&self.appearance.menu)
    }
    fn watcher_online(&self) {
        self.availability(true, None);
    }
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        self.availability(
            false,
            Some(format!("The system tray host is unavailable: {reason:?}")),
        );
        true
    }
}

// Querying the menu and activating its informational rows are read-only.
fn native_menu(entries: &[MenuEntry]) -> Vec<ksni::MenuItem<StatusItem>> {
    entries
        .iter()
        .map(|entry| match entry {
            MenuEntry::Info(label) => ksni::menu::StandardItem {
                label: menu_label(label),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuEntry::Separator => ksni::MenuItem::Separator,
            MenuEntry::Action {
                label,
                action: command,
            } => {
                let command = *command;
                ksni::menu::StandardItem {
                    label: menu_label(label),
                    activate: Box::new(move |tray: &mut StatusItem| {
                        action(&tray.events, DesktopEvent::Menu(command));
                    }),
                    ..Default::default()
                }
                .into()
            }
        })
        .collect()
}

fn menu_label(label: &str) -> String {
    // D-Bus menus treat a single underscore as a mnemonic marker. Device text
    // must remain literal, including user-supplied underscores.
    label.replace('_', "__")
}

/// Keep the private icon directory alive until the SNI worker has shut down.
/// Hosts can load the resource by name without depending on the install path.
struct HostIcon {
    _directory: tempfile::TempDir,
    theme_path: String,
    fallback: Vec<u8>,
}

impl HostIcon {
    const NAME: &str = "easy-sound-connect-symbolic";
    const SVG: &[u8] = include_bytes!("../../assets/tray_icon_symbolic.svg");

    fn new() -> Result<Self, String> {
        let directory = tempfile::Builder::new()
            .prefix("easy-sound-connect-")
            .tempdir()
            .map_err(|error| error.to_string())?;
        let icons = directory.path().join("icons");
        std::fs::create_dir(&icons).map_err(|error| error.to_string())?;
        std::fs::write(icons.join(format!("{}.svg", Self::NAME)), Self::SVG)
            .map_err(|error| error.to_string())?;
        let theme_path = icons
            .to_str()
            .ok_or("The tray icon directory is not valid Unicode")?
            .to_owned();
        Ok(Self {
            _directory: directory,
            theme_path,
            // Legacy hosts that cannot load named SVGs get neutral pixels,
            // never a color inferred from the application or desktop theme.
            fallback: argb(render_icon(Self::SVG, 32)?),
        })
    }
}

impl StatusItem {
    fn availability(&self, available: bool, reason: Option<String>) {
        self.host.send_replace(Some((available, reason)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_menu_information_is_inert_and_application_actions_dispatch() {
        use super::super::MenuAction;

        let (events, mut received) = mpsc::channel(4);
        let (host, _) = watch::channel(None);
        let mut tray = StatusItem {
            appearance: Appearance {
                menu: vec![
                    MenuEntry::Info("🎧 Test_headset".into()),
                    MenuEntry::Action {
                        label: "Open app".into(),
                        action: MenuAction::OpenMain,
                    },
                    MenuEntry::Action {
                        label: "Quit".into(),
                        action: MenuAction::Quit,
                    },
                ],
            },
            icon: HostIcon::new().unwrap(),
            events,
            host,
        };
        // Hosts can query AboutToShow during registration, before any click.
        tray.menu_about_to_show();
        let menu = tray.menu();
        assert!(received.try_recv().is_err());
        let ksni::MenuItem::Standard(info) = &menu[0] else {
            panic!("expected an informational row");
        };
        assert_eq!(info.label, "🎧 Test__headset");
        assert!(!info.enabled);
        (info.activate)(&mut tray);
        assert!(received.try_recv().is_err());

        for (index, expected) in [(1, MenuAction::OpenMain), (2, MenuAction::Quit)] {
            let ksni::MenuItem::Standard(item) = &menu[index] else {
                panic!("expected an application action");
            };
            assert!(item.enabled);
            (item.activate)(&mut tray);
            assert!(
                matches!(received.try_recv().unwrap(), DesktopEvent::Menu(action) if action == expected)
            );
            assert!(received.try_recv().is_err());
        }
    }

    #[test]
    fn named_icon_is_readable_for_the_service_lifetime_and_keeps_the_supplied_shape() {
        let icon = HostIcon::new().unwrap();
        let path = std::path::Path::new(&icon.theme_path).join(format!("{}.svg", HostIcon::NAME));
        assert_eq!(std::fs::read(&path).unwrap(), HostIcon::SVG);
        let supplied = super::super::icon_rgba(false, 32).unwrap();
        let symbolic = render_icon(HostIcon::SVG, 32).unwrap();
        assert_eq!(symbolic.len(), supplied.len());
        for (original, adapted) in supplied
            .as_chunks::<4>()
            .0
            .iter()
            .zip(symbolic.as_chunks::<4>().0)
        {
            assert_eq!(original[3], adapted[3], "only color may change");
        }
        drop(icon);
        assert!(!path.exists());
    }
}
