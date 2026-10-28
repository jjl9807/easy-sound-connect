//! Native desktop services. GUI windows and device sessions remain owned by the app.

mod instance;
pub use instance::Instance;
mod startup;
pub use startup::{autostart_enabled, set_autostart};
mod tray_menu;
pub use tray_menu::{MenuAction, MenuEntry};

#[cfg(target_os = "linux")]
mod x11_focus;
#[cfg(target_os = "linux")]
pub use x11_focus::focus_x11_panel;

#[cfg(target_os = "linux")]
mod linux_tray;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod native_tray;

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::{runtime::Handle, sync::mpsc};

static NEXT_TRAY: AtomicU64 = AtomicU64::new(1);

/// A hint supplied by the tray host, not a guaranteed window position.
#[derive(Clone, Copy, Debug)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
    pub physical: bool,
    /// GPUI's native display identity, when the platform can resolve it.
    pub display_id: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum DesktopEvent {
    OpenPanel(Option<ScreenPoint>),
    OpenMain,
    Menu(MenuAction),
    TrayAvailability {
        generation: u64,
        available: bool,
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Appearance {
    menu: Vec<MenuEntry>,
}

/// Created, updated, and dropped on the GPUI thread. Linux's D-Bus service runs
/// on Tokio; Windows and macOS retain native objects on their owning UI thread.
pub struct TrayService {
    generation: u64,
    #[cfg(target_os = "linux")]
    inner: linux_tray::LinuxTray,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    inner: native_tray::NativeTray,
}

impl TrayService {
    pub fn start(
        runtime: &Handle,
        menu: Vec<MenuEntry>,
        events: mpsc::Sender<DesktopEvent>,
    ) -> Result<Self, String> {
        let generation = NEXT_TRAY.fetch_add(1, Ordering::Relaxed);
        let appearance = Appearance { menu };
        #[cfg(target_os = "linux")]
        let inner = linux_tray::LinuxTray::start(runtime, appearance, generation, events)?;
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let inner = native_tray::NativeTray::start(runtime, appearance, generation, events)?;
        Ok(Self { generation, inner })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn update(&mut self, menu: Vec<MenuEntry>) -> Result<(), String> {
        self.inner.update(Appearance { menu })
    }
}

/// Render the supplied SVG resource without changing its source. Native APIs
/// require straight-alpha RGBA; tiny-skia's output is premultiplied.
#[cfg(any(test, target_os = "windows", target_os = "macos"))]
fn icon_rgba(dark: bool, size: u32) -> Result<Vec<u8>, String> {
    let data: &[u8] = if dark {
        include_bytes!("../assets/tray_icon_dark.svg")
    } else {
        include_bytes!("../assets/tray_icon_light.svg")
    };
    render_icon(data, size)
}

fn render_icon(data: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default())
        .map_err(|error| format!("Invalid tray image: {error}"))?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| "Unable to allocate the tray image".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            size as f32 / tree.size().width(),
            size as f32 / tree.size().height(),
        ),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect())
}

/// UI actions may be coalesced when the bounded queue is full. Availability
/// changes await capacity on Linux and retry on the next native refresh.
fn action(events: &mpsc::Sender<DesktopEvent>, event: DesktopEvent) {
    let _ = events.try_send(event);
}

/// Deliver a notification without blocking the GPUI thread. Native notification
/// permissions and desktop service failures remain visible to the caller.
pub async fn notify(summary: String, body: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        prepare_notification_identity()?;

        let mut notification = notify_rust::Notification::new();
        notification
            .appname(app::APP_NAME)
            .summary(&summary)
            .body(&body)
            .timeout(6000);
        #[cfg(target_os = "linux")]
        notification.hint(notify_rust::Hint::DesktopEntry(app::APP_ID.into()));
        #[cfg(target_os = "windows")]
        notification.app_id(app::APP_ID);
        notification
            .show()
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn prepare_notification_identity() -> Result<(), String> {
    // macOS accepts its application identity only once. Cache failures too,
    // so retries preserve the original error rather than reporting AlreadySet.
    static IDENTITY: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    IDENTITY
        .get_or_init(|| {
            #[cfg(target_os = "windows")]
            {
                // Unpackaged desktop apps need a registered display name for
                // their own AUMID; never fall back to another application's ID.
                winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
                    .create_subkey(format!(r"Software\Classes\AppUserModelId\{}", app::APP_ID))
                    .and_then(|(key, _)| key.set_value("DisplayName", &app::APP_NAME))
                    .map_err(|error| format!("Unable to register notification identity: {error}"))
            }
            #[cfg(target_os = "macos")]
            {
                notify_rust::set_application(app::APP_ID)
                    .map_err(|error| format!("Unable to select notification identity: {error}"))
            }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplied_icons_have_transparency_and_theme_specific_pixels() {
        let light = icon_rgba(false, 32).unwrap();
        let dark = icon_rgba(true, 32).unwrap();
        assert_eq!(light.len(), 32 * 32 * 4);
        assert_eq!(dark.len(), light.len());
        assert!(light.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
        assert!(light.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert_ne!(light, dark);
    }
}
