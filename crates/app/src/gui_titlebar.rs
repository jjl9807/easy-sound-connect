//! Window controls integrated into the content, without a separate title bar.

use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable};
use gpui_kit::{
    AnyElement, App, Decorations, MouseButton, MouseDownEvent, MouseMoveEvent, TitlebarOptions,
    Window, WindowButton, WindowButtonLayout, WindowControlArea, WindowDecorations, WindowOptions,
    div, prelude::*, px,
};

pub(super) fn window_options() -> WindowOptions {
    WindowOptions {
        // Keep native window semantics and macOS traffic lights while allowing
        // the application content to extend through the title-bar area.
        titlebar: Some(TitlebarOptions {
            title: Some(app::APP_NAME.into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        app_owns_titlebar_drag: true,
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn button_layout(cx: &App) -> WindowButtonLayout {
    #[cfg(target_os = "linux")]
    {
        cx.button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        WindowButtonLayout {
            left: [None; 3],
            right: if cfg!(target_os = "windows") {
                [
                    Some(WindowButton::Minimize),
                    Some(WindowButton::Maximize),
                    Some(WindowButton::Close),
                ]
            } else {
                // AppKit supplies its own controls and their placement.
                [None; 3]
            },
        }
    }
}

fn control_button(button: WindowButton, window: &Window, cx: &App) -> AnyElement {
    let (icon, area) = match button {
        WindowButton::Minimize => (IconName::WindowMinimize, WindowControlArea::Min),
        WindowButton::Maximize => (
            if window.is_maximized() {
                IconName::WindowRestore
            } else {
                IconName::WindowMaximize
            },
            WindowControlArea::Max,
        ),
        WindowButton::Close => (IconName::WindowClose, WindowControlArea::Close),
    };
    let (hover_background, hover_foreground) = if button == WindowButton::Close {
        (cx.theme().danger, cx.theme().danger_foreground)
    } else {
        (cx.theme().secondary_hover, cx.theme().foreground)
    };
    div()
        .id(button.id())
        .flex()
        .items_center()
        .justify_center()
        .w(px(34.))
        .h_full()
        .flex_shrink_0()
        .text_color(cx.theme().foreground)
        .hover(move |style| style.bg(hover_background).text_color(hover_foreground))
        // Windows dispatches these as native caption actions, including the
        // maximize button's system interactions. Do not dispatch twice.
        .when(cfg!(target_os = "windows"), |control| {
            control.window_control_area(area)
        })
        .when(cfg!(target_os = "linux"), |control| {
            control
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match button {
                        WindowButton::Minimize => window.minimize_window(),
                        WindowButton::Maximize => window.zoom_window(),
                        WindowButton::Close => window.remove_window(),
                    }
                })
        })
        .child(Icon::new(icon).small())
        .into_any_element()
}

#[derive(Default)]
struct DragState {
    armed: bool,
}

pub(super) fn render(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    // A compositor can grant server decorations even when client decorations
    // were requested. Its existing controls must not be duplicated.
    if cfg!(target_os = "linux")
        && !matches!(window.window_decorations(), Decorations::Client { .. })
    {
        return None;
    }

    let layout = button_layout(cx);
    let supported = window.window_controls();
    let group = |buttons: [Option<WindowButton>; 3]| {
        div().flex().h_full().flex_shrink_0().children(
            buttons
                .into_iter()
                .flatten()
                .filter(|button| match button {
                    WindowButton::Minimize => supported.minimize,
                    WindowButton::Maximize => supported.maximize,
                    WindowButton::Close => true,
                })
                .map(|button| control_button(button, window, cx)),
        )
    };
    let left = group(layout.left).when(cfg!(target_os = "macos"), |group| {
        // Leave the native traffic lights untouched and keep drag handling
        // outside their usual content inset.
        group.w(px(80.))
    });
    let right = group(layout.right);
    let drag_state = window.use_state(cx, |_, _| DragState::default());
    let drag = div()
        .id("window-drag-region")
        .flex_1()
        .min_w_0()
        .h_full()
        .when(cfg!(target_os = "windows"), |region| {
            region.window_control_area(WindowControlArea::Drag)
        })
        .when(!cfg!(target_os = "windows"), |region| {
            region
                .on_mouse_down(
                    MouseButton::Left,
                    window.listener_for(&drag_state, |state, event: &MouseDownEvent, window, _| {
                        state.armed = event.click_count != 2;
                        if event.click_count == 2 {
                            if cfg!(target_os = "macos") {
                                window.titlebar_double_click();
                            } else if window.window_controls().maximize {
                                window.zoom_window();
                            }
                        }
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    window.listener_for(&drag_state, |state, _, _, _| state.armed = false),
                )
                .on_mouse_down_out(
                    window.listener_for(&drag_state, |state, _, _, _| state.armed = false),
                )
                .on_mouse_move(window.listener_for(
                    &drag_state,
                    |state, event: &MouseMoveEvent, window, _| {
                        if !event.dragging() {
                            state.armed = false;
                        } else if std::mem::take(&mut state.armed) {
                            window.start_window_move();
                        }
                    },
                ))
        })
        .when(
            cfg!(target_os = "linux") && supported.window_menu,
            |region| {
                region.on_mouse_down(MouseButton::Right, |event, window, _| {
                    window.show_window_menu(event.position);
                })
            },
        );

    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(34.))
            .flex()
            .child(left)
            .child(drag)
            .child(right)
            .into_any_element(),
    )
}
