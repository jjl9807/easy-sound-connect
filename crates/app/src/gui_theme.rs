//! Shared appearance for the main window, tray panel and dialogs.

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode as KitThemeMode, theme::ThemeToken};
use gpui_kit::{App, WindowAppearance, linear_color_stop, linear_gradient, px, rgb};

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
    let dark = mode.is_dark();
    Theme::update(cx, |theme| {
        theme.font_size = px(13.5);
        theme.radius = px(9.);
        theme.radius_lg = px(20.);
        theme.shadow = true;

        let palette = &mut theme.colors;
        let foreground = rgb(if dark { 0xf0f0f4 } else { 0x25252f }).into();
        let secondary_text = rgb(if dark { 0xa8a8b7 } else { 0x676777 }).into();
        let surface = rgb(if dark { 0x222227 } else { 0xffffff }).into();
        let muted = rgb(if dark { 0x30303a } else { 0xececf3 }).into();
        let hover = rgb(if dark { 0x3a3a47 } else { 0xe3e3ee }).into();
        let active = rgb(if dark { 0x454554 } else { 0xd8d8e8 }).into();
        let border = rgb(if dark { 0x363640 } else { 0xdcdce5 }).into();
        let primary = rgb(if dark { 0xa7a7f6 } else { 0x5e58b7 }).into();
        let primary_hover = rgb(if dark { 0xbcbcfb } else { 0x716ac8 }).into();
        let primary_active = rgb(if dark { 0x9292e0 } else { 0x514c9e }).into();
        let primary_text = rgb(if dark { 0x20202c } else { 0xffffff }).into();
        let selection = rgb(if dark { 0x3b3859 } else { 0xe7e4fa }).into();

        palette.background = rgb(if dark { 0x17171b } else { 0xf5f5f9 }).into();
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

        palette.button = muted;
        palette.button_hover = hover;
        palette.button_active = active;
        palette.button_foreground = foreground;
        palette.button_primary = primary;
        palette.button_primary_hover = primary_hover;
        palette.button_primary_active = primary_active;
        palette.button_primary_foreground = primary_text;
        palette.button_secondary = muted;
        palette.button_secondary_hover = hover;
        palette.button_secondary_active = active;
        palette.button_secondary_foreground = foreground;

        palette.sidebar = rgb(if dark { 0x1b1b20 } else { 0xeeeef5 }).into();
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
    });
    Theme::update(cx, |theme| {
        let top = theme.colors.background;
        let bottom: gpui_kit::Hsla = rgb(if dark { 0x0e0e11 } else { 0xeeeef5 }).into();
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
