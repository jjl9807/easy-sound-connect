//! Shared GPUI desktop and tray surfaces for SONY MDR v2 controls.
extern crate gpui_kit as gpui;

mod config;
mod desktop_integration;
mod device_status;
mod gui_controls;
mod gui_daily_controls;
mod gui_device_picker;
mod gui_device_prompts;
mod gui_notices;
mod gui_operation_feedback;
mod gui_pages;
mod gui_preview;
mod gui_product_images;
mod gui_session;
mod gui_shell;
mod gui_theme;
mod gui_titlebar;
mod gui_tray_menu;
mod notices;
mod preferences;
mod product_assets;
mod product_media;
mod saved_devices;

use gpui_kit::component::{
    ActiveTheme, Selectable, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::{prelude::*, *};
use gui_session::{Event, Session};
use preferences::Language;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Device,
    Sound,
    Scenes,
    Settings,
    Activity,
    Preferences,
    About,
}
impl Page {
    const ALL: [Self; 7] = [
        Self::Device,
        Self::Sound,
        Self::Scenes,
        Self::Settings,
        Self::Activity,
        Self::Preferences,
        Self::About,
    ];
    fn title(self, language: Language) -> &'static str {
        match self {
            Self::Device => language.text("My Device", "我的设备"),
            Self::Sound => language.text("Sound", "声音"),
            Self::Scenes => language.text("Scenes", "场景"),
            Self::Settings => language.text("Settings", "设置"),
            Self::Activity => language.text("Activity", "活动"),
            Self::Preferences => language.text("Preferences", "首选项"),
            Self::About => language.text("About", "关于"),
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Device => "icons/headphones.svg",
            Self::Sound => "icons/sliders-horizontal.svg",
            Self::Scenes => "icons/sparkles.svg",
            Self::Settings => "icons/settings.svg",
            Self::Activity => "icons/activity.svg",
            Self::Preferences => "icons/settings-2.svg",
            Self::About => "icons/info.svg",
        }
    }
}

struct Desktop {
    session: Entity<Session>,
    compact: bool,
    page: Page,
    focus: FocusHandle,
    controls: gui_controls::ControlWidgets,
    prompt_seen: Option<ezsc_core::PromptId>,
    prompt_focus: FocusHandle,
    prompt_action_focus: [FocusHandle; 2],
    device_picker_open: bool,
    _subscriptions: Vec<Subscription>,
}
impl Desktop {
    fn new(
        session: Entity<Session>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&session, |_, _, cx| cx.notify());
        let theme = cx.observe_global::<gpui_kit::component::Theme>(|_, cx| cx.notify());
        let button_layout = cx.observe_button_layout_changed(window, |_, _, cx| cx.notify());
        let appearance = cx.observe_window_appearance(window, |view, window, cx| {
            let mode = view.session.read(cx).preferences.theme;
            gui_theme::apply_appearance(mode, window.appearance(), cx);
            gui_shell::refresh_tray(cx);
            cx.notify();
        });
        let activation = cx.observe_window_activation(window, |view, window, cx| {
            if view.compact && !window.is_window_active() {
                window.remove_window();
            } else if !view.compact && window.is_window_active() && view.page == Page::Device {
                gui_product_images::restart(cx);
                cx.notify();
            }
        });
        if !compact {
            gui_product_images::restart(cx);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            session,
            compact,
            page: Page::Device,
            focus,
            controls: Default::default(),
            prompt_seen: None,
            prompt_focus: cx.focus_handle(),
            prompt_action_focus: [cx.focus_handle(), cx.focus_handle()],
            device_picker_open: std::env::args().any(|arg| arg == "--preview")
                && std::env::args().any(|arg| arg == "--device-picker"),
            _subscriptions: vec![observe, theme, button_layout, appearance, activation],
        }
    }
    fn language(&self, cx: &App) -> Language {
        self.session.read(cx).preferences.language
    }
    fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if !self.compact && self.page != page && page == Page::Device {
            gui_product_images::restart(cx);
        }
        self.page = page;
        cx.notify();
    }
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .session
            .read(cx)
            .snapshot
            .as_ref()
            .is_some_and(|s| s.pending_prompt.is_some())
        {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform {
            if let Ok(index) = event.keystroke.key.parse::<usize>() {
                if let Some(page) = index.checked_sub(1).and_then(|index| Page::ALL.get(index)) {
                    self.select_page(*page, cx);
                    cx.stop_propagation();
                }
            } else if event.keystroke.key == "r" {
                self.session.update(cx, |session, cx| {
                    if session.handle.is_some() {
                        session.refresh();
                    } else if session.adding_device {
                        session.discover();
                    }
                    cx.notify();
                });
            } else if event.keystroke.key == "," {
                self.select_page(Page::Preferences, cx);
            } else if event.keystroke.key == "q" {
                cx.quit();
            }
        } else if self.compact && event.keystroke.key == "escape" {
            window.remove_window();
        }
    }
    fn sidebar(&self, cx: &Context<Self>) -> Div {
        let language = self.language(cx);
        let mut navigation = div().flex().flex_col().gap_1();
        for (index, page) in Page::ALL.into_iter().enumerate() {
            navigation = navigation.child(
                Button::new(("page", index))
                    .accessibility_label(page.title(language))
                    .ghost()
                    .selected(self.page == page)
                    .w_full()
                    .h(px(38.))
                    .rounded(px(9.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .gap_3()
                            .text_size(px(14.))
                            .child(
                                svg()
                                    .path(page.icon())
                                    .size(px(17.))
                                    .text_color(cx.theme().foreground),
                            )
                            .child(page.title(language)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.select_page(page, cx);
                    })),
            );
        }
        div()
            .w(px(gui_device_picker::SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .p(px(gui_device_picker::SIDEBAR_PADDING))
            .gap_6()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(div().pt_6().child(self.device_picker(cx)))
            .child(navigation)
            .child(div().flex_1())
            .children(self.sidebar_notice(cx))
    }
}
impl Render for Desktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.language(cx);
        let body = if self.compact {
            let connected = self.session.read(cx).snapshot.as_ref().is_some_and(|s| {
                !matches!(
                    s.connection,
                    ezsc_core::ConnectionState::Disconnected
                        | ezsc_core::ConnectionState::Failed
                        | ezsc_core::ConnectionState::Closing
                )
            });
            let mut panel = div()
                .id("tray-scroll")
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .size_full()
                .overflow_y_scroll()
                .child(self.product_header(true, cx));
            if connected {
                panel = panel
                    .child(self.render_noise_controls(window, cx))
                    .child(self.render_equalizer_controls(window, cx))
                    .child(self.render_daily_controls(cx));
            } else {
                panel = panel
                    .child(self.connection_actions(cx))
                    .child(self.connection_page(cx));
            }
            panel
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button("open-main", language.text("Open app", "打开主窗口"))
                                .on_click(|_, _, cx| gui_shell::open_main(cx)),
                        )
                        .child(
                            button("quit", language.text("Quit", "退出"))
                                .ghost()
                                .on_click(|_, _, cx| cx.quit()),
                        ),
                )
                .into_any_element()
        } else {
            let page = self.page;
            let content = div()
                .flex()
                .flex_col()
                .gap_4()
                .p(px(28.))
                .pt(px(40.))
                .w_full()
                .h_full()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(24.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(page.title(language)),
                        )
                        .when(page == Page::Device, |header| {
                            header.child(self.connection_actions(cx))
                        }),
                )
                .child(
                    div()
                        .id(("page-content", page as usize))
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .gap_4()
                        .overflow_y_scroll()
                        .child(match page {
                            Page::Device => self.device_page(window, cx),
                            Page::Sound => self.sound_page(window, cx),
                            Page::Scenes => self.scenes_page(cx),
                            Page::Settings => self.settings_page(cx),
                            Page::Activity => self.activity_page(cx),
                            Page::Preferences => self.preferences_page(cx),
                            Page::About => self.about_page(cx),
                        }),
                );
            div()
                .flex()
                .size_full()
                .child(self.sidebar(cx))
                .child(content)
                .into_any_element()
        };
        let prompt = self.render_device_prompt(window, cx);
        if prompt.is_none() {
            self.prompt_seen = None;
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .font_family(cx.theme().font_family.clone())
            .text_size(px(13.5))
            .bg(cx.theme().tokens.background.background)
            .text_color(cx.theme().foreground)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(body)
                    .children(prompt),
            )
            .when(!self.compact, |root| {
                root.children(gui_titlebar::render(window, cx))
            })
    }
}
fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).label(label).rounded(px(9.)).small()
}
fn main() {
    gui_shell::run();
}
