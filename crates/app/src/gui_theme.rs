//! Shared appearance for the main window, tray panel and dialogs.

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode as KitThemeMode, theme::ThemeToken};
use gpui_kit::{App, Hsla, WindowAppearance, linear_color_stop, linear_gradient, px, rgb, rgba};

use crate::preferences::ThemeMode;

/// Apply a client preference after the component library has been initialized.
/// Call again for a system-appearance event only when the preference is System.
pub fn apply(mode: ThemeMode, cx: &mut App) {
    let appearance = cx.window_appearance();
    apply_appearance(mode, appearance, cx);
}

/// Uses the reporting window's appearance, which is more precise on Linux.
pub fn apply_appearance(mode: ThemeMode, appearance: WindowAppearance, cx: &mut App) {
    let mode = match mode {
        ThemeMode::System => KitThemeMode::from(appearance),
        ThemeMode::Light => KitThemeMode::Light,
        ThemeMode::Dark => KitThemeMode::Dark,
    };
    Theme::change(mode, None, cx);
    #[cfg(target_os = "linux")]
    let font_family = weighted_ui_font(cx);
    let dark = mode.is_dark();
    Theme::update(cx, |theme| {
        #[cfg(target_os = "linux")]
        {
            theme.font_family = font_family;
        }
        theme.font_size = px(13.5);
        theme.radius = px(9.);
        theme.radius_lg = px(20.);
        theme.shadow = true;

        let palette = &mut theme.colors;
        let foreground = rgb(if dark { 0xf4f4f7 } else { 0x242428 }).into();
        let secondary_text = rgb(if dark { 0xa2a2ac } else { 0x67676e }).into();
        let surface = rgb(if dark { 0x222227 } else { 0xffffff }).into();
        let muted = rgb(if dark { 0x33333a } else { 0xededf0 }).into();
        let hover = rgb(if dark { 0x3c3c44 } else { 0xe4e4e9 }).into();
        let active = rgb(if dark { 0x45454d } else { 0xd9d9e0 }).into();
        let button = rgb(if dark { 0x3b3b45 } else { 0xe3e3eb }).into();
        let button_hover = rgb(if dark { 0x494954 } else { 0xd8d8e3 }).into();
        let button_active = rgb(if dark { 0x555562 } else { 0xcccdda }).into();
        let border: Hsla = rgba(if dark { 0xffffff12 } else { 0x00000012 }).into();
        let primary = rgb(if dark { 0xa7a7f6 } else { 0x5e58b7 }).into();
        let primary_hover = rgb(if dark { 0xbcbcfb } else { 0x716ac8 }).into();
        let primary_active = rgb(if dark { 0x9292e0 } else { 0x514c9e }).into();
        let primary_text = rgb(if dark { 0x14142e } else { 0xffffff }).into();
        let selection: Hsla = rgba(if dark { 0xa7a7f621 } else { 0x5e58b71a }).into();

        palette.background = rgb(if dark { 0x17171b } else { 0xf6f6f8 }).into();
        palette.foreground = foreground;
        palette.border = border;
        palette.input = border;
        palette.group_box = surface;
        palette.group_box_foreground = foreground;
        palette.popover = surface;
        palette.popover_foreground = foreground;
        palette.accordion = surface;
        palette.muted = muted;
        palette.muted_foreground = secondary_text;
        palette.accent = selection;
        palette.accent_foreground = primary;
        palette.primary = primary;
        palette.primary_hover = primary_hover;
        palette.primary_active = primary_active;
        palette.primary_foreground = primary_text;
        palette.secondary = muted;
        palette.secondary_hover = hover;
        palette.secondary_active = active;
        palette.secondary_foreground = foreground;

        palette.button = button;
        palette.button_hover = button_hover;
        palette.button_active = button_active;
        palette.button_foreground = foreground;
        palette.button_primary = primary;
        palette.button_primary_hover = primary_hover;
        palette.button_primary_active = primary_active;
        palette.button_primary_foreground = primary_text;
        palette.button_secondary = button;
        palette.button_secondary_hover = button_hover;
        palette.button_secondary_active = button_active;
        palette.button_secondary_foreground = foreground;

        palette.sidebar = rgb(if dark { 0x111114 } else { 0xeeeef2 }).into();
        palette.sidebar_foreground = foreground;
        palette.sidebar_border = border;
        palette.sidebar_accent = selection;
        palette.sidebar_accent_foreground = primary;
        palette.sidebar_primary = primary;
        palette.sidebar_primary_foreground = primary_text;

        palette.list = surface;
        palette.list_head = surface;
        palette.list_hover = hover;
        palette.list_active = selection;
        palette.list_active_border = primary;
        palette.list_even = surface;
        palette.slider_bar = primary;
        palette.slider_thumb = rgb(if dark { 0xdad9ff } else { 0xffffff }).into();
        palette.switch = muted;
        palette.switch_thumb = rgb(0xffffff).into();
        palette.progress_bar = primary;
        palette.ring = primary;
        palette.caret = primary;
        palette.selection = selection;
        palette.link = primary;
        palette.link_hover = primary_hover;
        palette.link_active = primary_active;
        palette.scrollbar_thumb = border;
        palette.scrollbar_thumb_hover = secondary_text;
        palette.success = rgb(if dark { 0x8bcb9b } else { 0x287a45 }).into();
        palette.warning = rgb(if dark { 0xe7c46a } else { 0xa56912 }).into();
        palette.danger = rgb(if dark { 0xec8a80 } else { 0xc73b36 }).into();
    });
    Theme::update(cx, |theme| {
        let top = theme.colors.background;
        let bottom: gpui_kit::Hsla = rgb(if dark { 0x0e0e11 } else { 0xf0f0f4 }).into();
        theme.tokens.background = ThemeToken::new(
            top,
            linear_gradient(
                180.,
                linear_color_stop(top, 0.),
                linear_color_stop(bottom, 1.),
            ),
        );
    });
}

#[cfg(target_os = "linux")]
pub(super) fn source_han_sans(cx: &App) -> Option<gpui_kit::SharedString> {
    let text = cx.text_system();
    let installed = text.all_font_names();
    // Both regional subsets (CN) and full Simplified Chinese fonts (SC) are
    // distributed under these names. Do not mistake a Fontconfig fallback
    // for an installed family, or use the Japanese/Traditional Chinese face.
    [
        "Source Han Sans SC",
        "Source Han Sans CN",
        "思源黑体",
        "思源黑体 CN",
        "思源黑体 SC",
    ]
    .into_iter()
    .find(|family| {
        if !installed.iter().any(|name| name == family) {
            return false;
        }
        let regular = gpui_kit::font(*family);
        // The current Linux backend can collapse variable-only families to
        // one regular face. Keep the existing fallback for unusable weights.
        text.resolve_font(&regular) != text.resolve_font(&regular.bold())
    })
    .map(Into::into)
}

#[cfg(target_os = "linux")]
fn weighted_ui_font(cx: &App) -> gpui_kit::SharedString {
    if let Some(family) = source_han_sans(cx) {
        return family;
    }
    let current = cx.theme().font_family.clone();
    let text = cx.text_system();
    let has_distinct_weights = |family: gpui_kit::SharedString| {
        let regular = gpui_kit::font(family);
        text.resolve_font(&regular) != text.resolve_font(&regular.bold())
    };
    if has_distinct_weights(current.clone()) {
        return current;
    }
    // Cosmic currently resolves variable-only families to one regular face.
    // Use an installed sans family with real weights so headings stay distinct.
    let installed = text.all_font_names();
    for family in ["Noto Sans", "DejaVu Sans", "Liberation Sans"] {
        if installed.iter().any(|name| name == family) && has_distinct_weights(family.into()) {
            return family.into();
        }
    }
    current
}

pub fn is_dark(cx: &App) -> bool {
    cx.theme().is_dark()
}

/// Theme-aware embedded placeholder while product artwork is unavailable.
pub fn product_image(cx: &App) -> &'static str {
    if is_dark(cx) {
        "model_image_default_dark.png"
    } else {
        "model_image_default_light.png"
    }
}
