use super::{Appearance, DesktopEvent, MenuAction, MenuEntry, ScreenPoint, action, icon_rgba};
use std::sync::{Mutex, Once};
use tokio::{runtime::Handle, sync::mpsc};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

static INSTALL_HANDLERS: Once = Once::new();
static ROUTE: Mutex<Option<(String, mpsc::Sender<DesktopEvent>)>> = Mutex::new(None);

pub(super) struct NativeTray {
    // Retain the native item; on macOS AppKit owns template recoloring.
    icon: TrayIcon,
    appearance: Appearance,
    #[cfg(target_os = "windows")]
    dark: bool,
    id: String,
    generation: u64,
    events: mpsc::Sender<DesktopEvent>,
    available: Option<bool>,
}

impl NativeTray {
    pub(super) fn start(
        _runtime: &Handle,
        appearance: Appearance,
        generation: u64,
        events: mpsc::Sender<DesktopEvent>,
    ) -> Result<Self, String> {
        INSTALL_HANDLERS.call_once(|| {
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    id,
                    position,
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                    && let Some((active, events)) =
                        &*ROUTE.lock().unwrap_or_else(|e| e.into_inner())
                    && active == id.as_ref()
                {
                    action(
                        events,
                        DesktopEvent::OpenPanel(Some(normalize_point(ScreenPoint {
                            x: position.x,
                            y: position.y,
                            physical: true,
                            display_id: None,
                        }))),
                    );
                }
            }));
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                if let Some((active, events)) = &*ROUTE.lock().unwrap_or_else(|e| e.into_inner()) {
                    let Some(suffix) = event.id.as_ref().strip_prefix(active) else {
                        return;
                    };
                    match suffix {
                        "-main" => action(events, DesktopEvent::Menu(MenuAction::OpenMain)),
                        "-quit" => action(events, DesktopEvent::Menu(MenuAction::Quit)),
                        _ => {}
                    }
                }
            }));
        });
        let id = format!("ezsc-tray-{generation}");
        let menu = native_menu(&id, &appearance.menu)?;
        let builder = TrayIconBuilder::new()
            .with_id(id.clone())
            .with_tooltip("Easy Sound Connect")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false);
        #[cfg(target_os = "windows")]
        let dark = taskbar_dark().unwrap_or_else(|error| {
            eprintln!("Unable to read Windows taskbar color mode: {error}; using light tray artwork until the setting becomes available");
            true
        });
        #[cfg(target_os = "windows")]
        let builder = builder.with_icon(native_icon(dark)?);
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(native_template_icon()?);
        let icon = builder.build().map_err(|error| error.to_string())?;
        *ROUTE.lock().unwrap_or_else(|e| e.into_inner()) = Some((id.clone(), events.clone()));
        let mut tray = Self {
            icon,
            appearance,
            #[cfg(target_os = "windows")]
            dark,
            id,
            generation,
            events,
            available: None,
        };
        tray.refresh_availability();
        Ok(tray)
    }

    pub(super) fn update(&mut self, appearance: Appearance) -> Result<(), String> {
        if !self.refresh_availability() {
            // Keep the native tray window alive: tray-icon re-registers after
            // Explorer broadcasts TaskbarCreated. Retry appearance then.
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            // Windows supports independent app and shell modes. Read the shell
            // setting on each refresh; a temporary read failure keeps the last
            // usable icon instead of replacing it with an app-theme guess.
            let dark = taskbar_dark().unwrap_or(self.dark);
            if self.dark != dark {
                self.icon
                    .set_icon(Some(native_icon(dark)?))
                    .map_err(|error| error.to_string())?;
                self.dark = dark;
            }
        }
        if self.appearance != appearance {
            self.icon
                .set_menu(Some(Box::new(native_menu(&self.id, &appearance.menu)?)));
        }
        self.appearance = appearance;
        Ok(())
    }

    fn refresh_availability(&mut self) -> bool {
        // tray-icon intentionally returns Ok from build even if Explorer is not
        // ready. NIM_MODIFY checks whether the icon is actually registered; it
        // also detects Explorer exit without relying on the overflow geometry.
        #[cfg(target_os = "windows")]
        let status = self.icon.set_tooltip(Some("Easy Sound Connect"));
        #[cfg(target_os = "macos")]
        let status: Result<(), tray_icon::Error> = Ok(());
        let available = status.is_ok();
        if self.available != Some(available) {
            let reason = status
                .err()
                .map(|error| format!("The system tray host is unavailable: {error}"));
            // All native updates happen on the UI thread. A full queue retries
            // on the next refresh, preserving order without detached senders.
            if self
                .events
                .try_send(DesktopEvent::TrayAvailability {
                    generation: self.generation,
                    available,
                    reason,
                })
                .is_ok()
            {
                self.available = Some(available);
            }
        }
        available
    }
}

impl Drop for NativeTray {
    fn drop(&mut self) {
        let mut route = ROUTE.lock().unwrap_or_else(|e| e.into_inner());
        if route.as_ref().is_some_and(|(id, _)| id == &self.id) {
            *route = None;
        }
    }
}

fn native_menu(id: &str, entries: &[MenuEntry]) -> Result<Menu, String> {
    let menu = Menu::new();
    for (index, entry) in entries.iter().enumerate() {
        let result = match entry {
            MenuEntry::Info(label) => menu.append(&MenuItem::with_id(
                format!("{id}-info-{index}"),
                label.replace('&', "&&"),
                false,
                None,
            )),
            MenuEntry::Action { label, action } => {
                let suffix = match action {
                    MenuAction::OpenMain => "main",
                    MenuAction::Quit => "quit",
                };
                menu.append(&MenuItem::with_id(
                    format!("{id}-{suffix}"),
                    label.replace('&', "&&"),
                    true,
                    None,
                ))
            }
            MenuEntry::Separator => menu.append(&PredefinedMenuItem::separator()),
        };
        result.map_err(|error| error.to_string())?;
    }
    Ok(menu)
}

#[cfg(target_os = "windows")]
fn native_icon(dark: bool) -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_rgba(icon_rgba(dark, 32)?, 32, 32).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn taskbar_dark() -> std::io::Result<bool> {
    // Microsoft documents this shell mode separately from AppsUseLightTheme:
    // https://learn.microsoft.com/windows/apps/develop/settings/settings-common#personalization---colors
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")?;
    match key.get_value::<u32, _>("SystemUsesLightTheme")? {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "SystemUsesLightTheme must be 0 or 1",
        )),
    }
}

#[cfg(target_os = "macos")]
fn native_template_icon() -> Result<tray_icon::Icon, String> {
    // AppKit recolors an NSImage template for its actual menu-bar host. Keep
    // the supplied artwork's alpha and make its rendered pixels black/clear,
    // as required by https://developer.apple.com/documentation/appkit/nsimage/istemplate.
    let mut rgba = icon_rgba(false, 32)?;
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel[..3].fill(0);
    }
    tray_icon::Icon::from_rgba(rgba, 32, 32).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn normalize_point(point: ScreenPoint) -> ScreenPoint {
    use windows::Win32::{
        Foundation::POINT,
        Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint},
        UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
    };

    // GPUI identifies a Windows display by HMONITOR and divides that display's
    // global physical origin by its own DPI scale. Match that representation,
    // including monitors to the left/above the primary monitor.
    let monitor = unsafe {
        MonitorFromPoint(
            POINT {
                x: point.x.round() as i32,
                y: point.y.round() as i32,
            },
            MONITOR_DEFAULTTONEAREST,
        )
    };
    let (mut dpi_x, mut dpi_y) = (0, 0);
    if monitor.0.is_null()
        || unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_err()
        || dpi_x == 0
        || dpi_y == 0
    {
        return point;
    }
    ScreenPoint {
        x: point.x * 96.0 / f64::from(dpi_x),
        y: point.y * 96.0 / f64::from(dpi_y),
        physical: false,
        display_id: Some(monitor.0 as u64),
    }
}

#[cfg(target_os = "macos")]
fn normalize_point(fallback: ScreenPoint) -> ScreenPoint {
    use objc2::{MainThreadMarker, msg_send};
    use objc2_app_kit::{NSEvent, NSScreen};
    use objc2_foundation::ns_string;

    let Some(main_thread) = MainThreadMarker::new() else {
        return fallback;
    };
    // AppKit already supplies global logical points. tray-icon's physical
    // point combines the primary screen's flip with the status bar's scale,
    // so ask AppKit directly before converting to GPUI's display-local space.
    let mouse = NSEvent::mouseLocation();
    for screen in NSScreen::screens(main_thread) {
        let frame = screen.frame();
        if mouse.x < frame.origin.x
            || mouse.x >= frame.origin.x + frame.size.width
            || mouse.y < frame.origin.y
            || mouse.y >= frame.origin.y + frame.size.height
        {
            continue;
        }
        let description = screen.deviceDescription();
        let Some(number) = description.objectForKey(ns_string!("NSScreenNumber")) else {
            continue;
        };
        // NSScreenNumber is documented to be an NSNumber containing the
        // CGDirectDisplayID, which is also GPUI's macOS DisplayId.
        let display_id: u32 = unsafe { msg_send![&*number, unsignedIntValue] };
        return ScreenPoint {
            x: mouse.x - frame.origin.x,
            y: frame.origin.y + frame.size.height - mouse.y,
            physical: false,
            display_id: Some(u64::from(display_id)),
        };
    }
    fallback
}
