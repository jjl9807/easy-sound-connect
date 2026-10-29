//! Product pages built from shared device state and client preferences.
use crate::{
    Desktop, button, gui_shell, gui_style,
    preferences::{Language, ThemeMode},
};
use ezsc_core::ConnectionState;
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
    select::Select,
    switch::Switch,
};
use gpui_kit::{assets::IconName, prelude::*, *};

fn surface(cx: &App) -> Div {
    gui_style::surface(cx)
}
fn detail_row(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    gui_style::setting_row(
        label,
        div()
            .text_color(cx.theme().muted_foreground)
            .child(value.into()),
        cx,
    )
}
fn badge(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px(px(8.))
        .py(px(3.))
        .rounded(px(5.))
        .border_1()
        .border_color(cx.theme().muted_foreground.opacity(0.35))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
fn empty_state(
    icon: IconName,
    heading: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    cx: &App,
) -> Div {
    let detail = detail.into();
    div()
        .w_full()
        .min_h(px(280.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.))
        .child(
            svg()
                .path(icon.path())
                .size(px(36.))
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::BOLD)
                .child(heading.into()),
        )
        .when(!detail.is_empty(), |view| {
            view.child(caption(detail, cx).max_w(px(360.)).text_center())
        })
}
fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
fn title(text: impl Into<SharedString>) -> Div {
    div()
        .font_weight(FontWeight::BOLD)
        .text_size(px(15.))
        .child(text.into())
}

fn action_button(id: &'static str, icon: IconName, label: String) -> Button {
    Button::new(id)
        .icon(icon)
        .tooltip(label.clone())
        .accessibility_label(label)
        .small()
        .w(px(32.))
        .h(px(32.))
        .rounded(px(9.))
}

impl Desktop {
    pub(super) fn connection_actions(&self, cx: &Context<Self>) -> Div {
        let state = self.session.read(cx);
        let language = self.language(cx);
        let ready = state.snapshot.as_ref().is_some_and(|s| {
            s.connection == ConnectionState::Ready
                && state.focused_device.as_ref() == Some(&s.device)
        });
        let blocked = state.settings_unavailable() || (!state.preview && state.manager.is_none());
        let has_focused_device = state.focused_device.is_some();
        let removable_device = state
            .focused_device
            .clone()
            .filter(|id| ready && state.saved_devices.find(id).is_some());
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .when(ready, |row| {
                row.child(
                    action_button("refresh", IconName::RefreshCw, language.tr("ui.refresh"))
                        .ghost()
                        .disabled(blocked || state.preview)
                        .loading_icon(IconName::RefreshCw)
                        .loading(state.operation_feedback_visible(
                            crate::gui_operation_feedback::OperationTarget::Refresh,
                        ))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.session.update(cx, |state, cx| {
                                state.refresh();
                                cx.notify();
                            });
                        })),
                )
            })
            .when(!ready && has_focused_device, |row| {
                row.child(
                    action_button("reconnect", IconName::Link, language.tr("ui.connect"))
                        .primary()
                        .disabled(blocked)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.session.update(cx, |state, cx| {
                                state.connect_focused_device();
                                cx.notify();
                            });
                        })),
                )
            })
            .when(state.selected.is_some(), |row| {
                row.child(
                    action_button(
                        "release",
                        IconName::Unlink,
                        language.tr("ui.release_control"),
                    )
                    .ghost()
                    .disabled(blocked)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.session.update(cx, |state, cx| {
                            state.release();
                            cx.notify();
                        });
                    })),
                )
            })
            .when_some(removable_device, |row, device| {
                row.child(
                    action_button(
                        "forget-device",
                        IconName::Trash,
                        language.tr("ui.remove_headphones"),
                    )
                    .ghost()
                    .when(!blocked, |button| {
                        button
                            .danger()
                            .bg(cx.theme().transparent)
                            .text_color(cx.theme().secondary_foreground)
                    })
                    .disabled(blocked)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.session.update(cx, |state, cx| {
                            state.forget_device(device.clone());
                            cx.notify();
                        });
                    })),
                )
            })
    }
    pub(super) fn product_header(&self, compact: bool, cx: &Context<Self>) -> Div {
        let state = self.session.read(cx);
        let language = self.language(cx);
        let snapshot = state
            .snapshot
            .as_ref()
            .filter(|snapshot| state.focused_device.as_ref() == Some(&snapshot.device));
        let connection = state
            .focused_device
            .as_ref()
            .map_or(ConnectionState::Disconnected, |device| {
                state.device_connection(device)
            });
        let name = state
            .focused_name()
            .unwrap_or_else(|| language.tr("ui.your_headphones"));
        let ready = connection == ConnectionState::Ready;
        let playback = (!compact)
            .then_some(snapshot)
            .flatten()
            .filter(|s| {
                s.connection == ConnectionState::Ready
                    && state.focused_device.as_ref() == Some(&s.device)
            })
            .map(|s| (s.device.0.clone(), state.epoch));
        let image = img(crate::gui_product_images::source(
            crate::gui_product_images::model_key(state, state.focused_device.as_ref()),
            playback,
            cx,
        ))
        .object_fit(ObjectFit::Contain)
        .w(px(if compact { 84. } else { 260. }))
        .h(px(if compact { 76. } else { 196. }))
        .flex_shrink_0();
        let mut status = div()
            .flex()
            .flex_col()
            .items_center()
            .min_w_0()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(if compact { 20. } else { 30. }))
                    .font_weight(FontWeight::BOLD)
                    .text_center()
                    .child(name),
            );
        if !ready {
            status = status.child(caption(connection_text(connection, language), cx));
        }
        if let Some(snapshot) = snapshot {
            status = status
                .child(crate::gui_status_icons::batteries(
                    snapshot, ready, language, cx,
                ))
                .child(crate::gui_status_icons::audio_marks(
                    snapshot, ready, language, cx,
                ));
        }
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .w_full()
            .pt(px(if compact { 0. } else { 4. }))
            .pb(px(if compact { 0. } else { 16. }))
            .child(image)
            .child(status)
    }
    pub(super) fn connection_page(&self, cx: &Context<Self>) -> Div {
        let state = self.session.read(cx);
        let language = self.language(cx);
        empty_state(
            IconName::Headphones,
            language.tr("ui.connect_your_headphones"),
            if state.saved_devices.entries.is_empty() {
                language.tr("ui.pair_before_adding")
            } else {
                language.tr("ui.choose_from_device_menu")
            },
            cx,
        )
        .when(self.compact, |view| view.child(self.device_picker(cx)))
    }
    pub(super) fn device_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let page = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w_full()
            .max_w(px(1100.));
        if self.session.read(cx).snapshot.is_none()
            && self.session.read(cx).focused_device.is_none()
        {
            return page.child(self.connection_page(cx));
        }
        let page = page.child(self.product_header(false, cx));
        if self.session.read(cx).snapshot.is_none() {
            return page;
        }
        let narrow = window.viewport_size().width < px(1060.);
        page.child(
            div()
                .flex()
                .gap(px(16.))
                .items_start()
                .when(narrow, |row| row.flex_col())
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .when(!narrow, |col| col.flex_1())
                        .child(self.render_noise_controls(window, cx)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .when(!narrow, |col| col.flex_1())
                        .child(self.render_speak_controls(cx)),
                ),
        )
    }
    pub(super) fn sound_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.session.read(cx).snapshot.is_none() {
            return div().child(self.connection_page(cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .max_w(px(980.))
            .child(self.render_equalizer_controls(window, cx))
            .child(self.render_audio_controls(cx))
    }
    pub(super) fn settings_page(&self, cx: &Context<Self>) -> Div {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let Some(snapshot) = &state.snapshot else {
            return div().child(self.connection_page(cx));
        };
        let mut details = gui_style::card(language.tr("ui.headphones"), cx);
        let rows = [
            (
                language.tr("ui.model"),
                snapshot
                    .model_name
                    .as_ref()
                    .and_then(|s| s.as_utf8().ok())
                    .unwrap_or("—")
                    .to_owned(),
            ),
            (
                language.tr("ui.firmware"),
                snapshot
                    .firmware_version
                    .as_ref()
                    .and_then(|s| s.as_utf8().ok())
                    .unwrap_or("—")
                    .to_owned(),
            ),
            (
                language.tr("ui.connection_type"),
                match snapshot.transport {
                    bluetooth::TransportKind::Rfcomm => language.tr("ui.bluetooth_classic"),
                    bluetooth::TransportKind::Gatt => language.tr("ui.bluetooth_le"),
                }
                .to_owned(),
            ),
            (
                language.tr("ui.status"),
                connection_text(snapshot.connection, language).to_owned(),
            ),
        ];
        let mut information = div().flex().flex_col();
        for (index, (label, value)) in rows.into_iter().enumerate() {
            if index > 0 {
                information = information.child(gui_style::divider(cx));
            }
            information = information.child(detail_row(label, value, cx));
        }
        details = details.child(information);
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w_full()
            .max_w(px(980.))
            .child(details)
            .child(
                surface(cx)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(title(language.tr("ui.connection")))
                            .child(self.connection_actions(cx)),
                    )
                    .child(caption(language.tr("ui.release_control_description"), cx)),
            )
    }
    pub(super) fn scenes_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        empty_state(IconName::Sparkles, l.tr("ui.scenes_unavailable"), "", cx)
    }
    pub(super) fn activity_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let state = self.session.read(cx);
        let mut list = surface(cx).w_full().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(title(l.tr("ui.recent_activity")))
                .child(
                    action_button(
                        "copy-diagnostics",
                        IconName::Copy,
                        l.tr("ui.copy_diagnostics"),
                    )
                    .ghost()
                    .on_click(cx.listener(|view, _, _, cx| {
                        let state = view.session.read(cx);
                        let mut text = state
                            .preferences
                            .language
                            .tr_args("ui.diagnostics_title", &[("app", app::APP_NAME.into())]);
                        if let Some(snapshot) = &state.snapshot {
                            text.push_str(&app::snapshot_lines(snapshot).join("\n"));
                        }
                        text.push_str("\n\n");
                        text.push_str(
                            &state
                                .activity
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n"),
                        );
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    })),
                ),
        );
        let mut entries = div().flex().flex_col();
        if state.activity.is_empty() {
            entries = entries.child(caption(l.tr("ui.no_recent_activity"), cx).py(px(20.)));
        }
        for (index, line) in state.activity.iter().rev().enumerate() {
            if index > 0 {
                entries = entries.child(gui_style::divider(cx));
            }
            entries = entries.child(
                div()
                    .id(("activity", index))
                    .py(px(11.))
                    .text_size(px(12.5))
                    .line_height(px(19.))
                    .child(line.clone()),
            );
        }
        list = list.child(entries);
        list
    }
    pub(super) fn preferences_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let l = self.language(cx);
        let prefs = self.session.read(cx).preferences.clone();
        let saving = cx.global::<gui_shell::Shell>().saving_preferences;
        // Follow committed preferences after a save succeeds or fails, including
        // changes made from another window. Keep the pending choice while saving.
        if !saving && self.language_select.read(cx).selected_value() != Some(&prefs.language) {
            self.language_select.update(cx, |select, cx| {
                select.set_selected_value(&prefs.language, window, cx);
            });
        }
        let languages = Select::new(&self.language_select)
            .id("language-select")
            .accessibility_label(l.tr("ui.language"))
            .small()
            .w(px(160.))
            .disabled(saving);
        let mut themes = div().flex().flex_wrap().justify_end().gap(px(6.));
        for (index, (value, label)) in [
            (ThemeMode::System, l.tr("ui.system")),
            (ThemeMode::Light, l.tr("ui.light")),
            (ThemeMode::Dark, l.tr("ui.dark")),
        ]
        .into_iter()
        .enumerate()
        {
            themes = themes.child(
                gui_style::chip(("theme", index), label, prefs.theme == value, !saving, cx)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        gui_shell::change_preferences(cx, move |p| p.theme = value);
                    })),
            );
        }
        let appearance = gui_style::card(l.tr("ui.appearance"), cx).child(
            div()
                .flex()
                .flex_col()
                .child(gui_style::setting_row(l.tr("ui.language"), languages, cx))
                .child(gui_style::divider(cx))
                .child(gui_style::setting_row(l.tr("ui.theme"), themes, cx)),
        );
        let mut desktop_rows = div().flex().flex_col();
        for (index, (key, label, checked, disabled)) in [
            ("tray", l.tr("ui.show_tray_icon"), prefs.tray_enabled, false),
            (
                "close",
                l.tr("ui.keep_running_on_close"),
                prefs.close_to_tray,
                !prefs.tray_enabled,
            ),
            (
                "autostart",
                l.tr("ui.start_at_login"),
                prefs.start_at_login,
                false,
            ),
            (
                "hidden",
                l.tr("ui.start_in_tray_at_login"),
                prefs.start_hidden,
                !prefs.tray_enabled,
            ),
            (
                "notifications",
                l.tr("ui.desktop_notifications"),
                prefs.notifications,
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                desktop_rows = desktop_rows.child(gui_style::divider(cx));
            }
            desktop_rows = desktop_rows.child(gui_style::setting_row(
                label.clone(),
                Switch::new(key)
                    .accessibility_label(label)
                    .checked(checked)
                    .disabled(disabled || saving)
                    .on_click(cx.listener(move |_, value, _, cx| {
                        let value = *value;
                        gui_shell::change_preferences(cx, move |p| match key {
                            "tray" => p.tray_enabled = value,
                            "close" => p.close_to_tray = value,
                            "autostart" => p.start_at_login = value,
                            "hidden" => p.start_hidden = value,
                            _ => p.notifications = value,
                        });
                    })),
                cx,
            ));
        }
        let mut desktop = gui_style::card(l.tr("ui.desktop"), cx).child(desktop_rows);
        if let Some(reason) = &cx.global::<gui_shell::Shell>().tray_error {
            desktop = desktop.child(
                caption(
                    l.tr_args("ui.tray_unavailable_reason", &[("reason", reason.clone())]),
                    cx,
                )
                .text_color(cx.theme().warning),
            );
        }
        let modifier = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        let mut shortcuts = div().flex().flex_col();
        for (index, (keys, label)) in [
            (format!("{modifier}1–7"), l.tr("ui.navigate_pages")),
            (format!("{modifier}R"), l.tr("ui.refresh")),
            (format!("{modifier},"), l.tr("ui.preferences")),
            (format!("{modifier}Q"), l.tr("ui.quit")),
        ]
        .into_iter()
        .enumerate()
        {
            if index > 0 {
                shortcuts = shortcuts.child(gui_style::divider(cx));
            }
            shortcuts = shortcuts.child(gui_style::setting_row(label, badge(keys, cx), cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .w_full()
            .max_w(px(980.))
            .child(appearance)
            .child(desktop)
            .child(gui_style::card(l.tr("ui.keyboard_shortcuts"), cx).child(shortcuts))
    }
    pub(super) fn about_page(&self, window: &Window, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let identity = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .py(px(28.))
            .child(crate::gui_app_logo::app_logo(108., window))
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::BOLD)
                    .child(app::APP_NAME),
            )
            .child(caption(
                l.tr_args(
                    "ui.version_number",
                    &[("version", env!("CARGO_PKG_VERSION").into())],
                ),
                cx,
            ));
        let links = div()
            .flex()
            .justify_center()
            .gap(px(10.))
            .child(
                button("project", l.tr("ui.github"))
                    .h(px(32.))
                    .px(px(14.))
                    .on_click(|_, _, cx| {
                        cx.open_url("https://github.com/jjl9807/easy-sound-connect")
                    }),
            )
            .child(
                button("license", l.tr("ui.license"))
                    .h(px(32.))
                    .px(px(14.))
                    .on_click(|_, _, cx| {
                        cx.open_url(
                            "https://github.com/jjl9807/easy-sound-connect/blob/main/LICENSE",
                        )
                    }),
            );
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(20.))
            .w_full()
            .child(identity)
            .child(links)
            .child(caption(l.tr("ui.independent_software"), cx).text_center())
            .child(
                caption(
                    l.tr_args("ui.copyright_notice", &[("app", app::APP_NAME.into())]),
                    cx,
                )
                .text_size(px(11.)),
            )
            .child(caption(l.tr("ui.third_party_licenses"), cx).text_size(px(11.)))
    }
}
fn connection_text(state: ConnectionState, l: Language) -> String {
    match state {
        ConnectionState::Ready => l.tr("ui.connected"),
        ConnectionState::Connected => l.tr("ui.connecting_detail"),
        ConnectionState::Negotiating => l.tr("ui.discovering_capabilities"),
        ConnectionState::Synchronizing => l.tr("ui.synchronizing"),
        ConnectionState::Closing => l.tr("ui.releasing_control"),
        ConnectionState::Disconnected => l.tr("ui.disconnected"),
        ConnectionState::Failed => l.tr("ui.connection_failed"),
    }
}
