//! Product pages built from shared device state and client preferences.
use crate::{
    Desktop, button, gui_shell, gui_style,
    preferences::{Language, ThemeMode},
};
use ezsc_core::{BatteryComponent, ConnectionState, Freshness};
use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariants},
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

fn action_button(id: &'static str, icon: IconName, label: &'static str) -> Button {
    Button::new(id)
        .icon(icon)
        .tooltip(label)
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
                    action_button(
                        "refresh",
                        IconName::RefreshCw,
                        language.text("Refresh", "刷新"),
                    )
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
                    action_button(
                        "reconnect",
                        IconName::Link,
                        language.text("Connect", "连接"),
                    )
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
                        language.text("Release control", "释放控制连接"),
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
                        language.text("Remove headphones", "删除耳机"),
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
            .unwrap_or_else(|| language.text("Your headphones", "你的耳机").into());
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
            let mut batteries = div().flex().flex_wrap().justify_center().gap(px(24.));
            for (index, endpoint) in snapshot.batteries.iter().enumerate() {
                let label = match endpoint.component {
                    BatteryComponent::Headset => language.text("Battery", "电量"),
                    BatteryComponent::Left => language.text("Left", "左耳"),
                    BatteryComponent::Right => language.text("Right", "右耳"),
                    BatteryComponent::Case => language.text("Case", "充电盒"),
                };
                let current = ready && endpoint.freshness == Freshness::Current;
                let level = endpoint.reading.map(|r| r.level.as_u8());
                let charging = endpoint
                    .reading
                    .is_some_and(|r| r.charging == libmdr2::battery::ChargingStatus::Charging);
                let tint = if !current {
                    cx.theme().muted_foreground
                } else if level.is_some_and(|level| level <= 20) {
                    cx.theme().danger
                } else {
                    cx.theme().foreground
                };
                let icon = if charging {
                    IconName::BatteryCharging
                } else if level.is_some_and(|level| level <= 20) {
                    IconName::BatteryLow
                } else {
                    IconName::Battery
                };
                let value = level.map_or_else(|| "—".to_owned(), |v| format!("{v}%"));
                let charging_label =
                    endpoint
                        .reading
                        .map_or("", |reading| match reading.charging {
                            libmdr2::battery::ChargingStatus::Charging => {
                                language.text(" · charging", " · 充电中")
                            }
                            libmdr2::battery::ChargingStatus::Charged => {
                                language.text(" · fully charged", " · 已充满")
                            }
                            libmdr2::battery::ChargingStatus::NotCharging => "",
                            _ => language.text(" · charging status unknown", " · 充电状态未知"),
                        });
                let age = if current || level.is_none() {
                    ""
                } else {
                    language.text(" · last known", " · 上次读取")
                };
                let mut battery = div()
                    .id(("hero-battery", index))
                    .role(accesskit::Role::Group)
                    .aria_label(format!("{label}: {value}{charging_label}{age}"))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                svg()
                                    .path(icon.path())
                                    .w(px(19.))
                                    .h(px(19.))
                                    .text_color(tint),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(tint)
                                    .child(value),
                            ),
                    );
                if endpoint.component != BatteryComponent::Headset {
                    battery = battery.child(caption(label, cx).text_size(px(11.)));
                }
                if !current && endpoint.reading.is_some() {
                    battery = battery.child(
                        caption(language.text("Last known", "上次读取"), cx).text_size(px(11.)),
                    );
                }
                batteries = batteries.child(battery);
            }
            status = status.child(batteries);
            let mut badges = div().flex().flex_wrap().justify_center().gap(px(8.));
            if let Some(codec) = &snapshot.audio_codec {
                let value: String = codec
                    .codec
                    .map(|c| match c {
                        ezsc_core::common::AudioCodec::SBC => "SBC".into(),
                        ezsc_core::common::AudioCodec::AAC => "AAC".into(),
                        ezsc_core::common::AudioCodec::LDAC => "LDAC".into(),
                        ezsc_core::common::AudioCodec::LC3 => "LC3".into(),
                        ezsc_core::common::AudioCodec::APT_X => "aptX".into(),
                        ezsc_core::common::AudioCodec::APT_X_HD => "aptX HD".into(),
                        ezsc_core::common::AudioCodec::UNSETTLED => {
                            language.text("Not determined", "尚未确定").into()
                        }
                        _ => language.text("Unknown", "未知").into(),
                    })
                    .unwrap_or_else(|| "—".into());
                let suffix = match codec.freshness {
                    Freshness::Unknown => language.text(" · not read", " · 未读取"),
                    Freshness::Current if ready => "",
                    _ => language.text(" · last known", " · 上次读取"),
                };
                badges = badges.child(badge(format!("{value}{suffix}"), cx));
            }
            status = status.child(badges);
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
            language.text("Connect your headphones", "连接耳机"),
            if state.saved_devices.entries.is_empty() {
                language.text(
                    "Pair in Bluetooth settings, then add your headphones.",
                    "先在系统蓝牙设置中配对，再添加耳机。",
                )
            } else {
                language.text(
                    "Choose your headphones from the device menu.",
                    "从左上角设备菜单选择耳机。",
                )
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
        let mut details = gui_style::card(language.text("Headphones", "耳机信息"), cx);
        let rows = [
            (
                language.text("Model", "型号"),
                snapshot
                    .model_name
                    .as_ref()
                    .and_then(|s| s.as_utf8().ok())
                    .unwrap_or("—")
                    .to_owned(),
            ),
            (
                language.text("Firmware", "固件版本"),
                snapshot
                    .firmware_version
                    .as_ref()
                    .and_then(|s| s.as_utf8().ok())
                    .unwrap_or("—")
                    .to_owned(),
            ),
            (
                language.text("Connection type", "连接类型"),
                match snapshot.transport {
                    bluetooth::TransportKind::Rfcomm => {
                        language.text("Bluetooth Classic", "经典蓝牙")
                    }
                    bluetooth::TransportKind::Gatt => language.text("Bluetooth LE", "低功耗蓝牙"),
                }
                .to_owned(),
            ),
            (
                language.text("Status", "状态"),
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
                            .child(title(language.text("Connection", "连接")))
                            .child(self.connection_actions(cx)),
                    )
                    .child(caption(
                        language.text(
                            "Releasing control keeps Bluetooth audio connected.",
                            "释放控制连接后，蓝牙音频仍保持连接。",
                        ),
                        cx,
                    )),
            )
    }
    pub(super) fn scenes_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        empty_state(
            IconName::Sparkles,
            l.text("Scenes aren't available yet", "场景暂未开放"),
            "",
            cx,
        )
    }
    pub(super) fn activity_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let state = self.session.read(cx);
        let mut list = surface(cx).w_full().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(title(l.text("Recent activity", "最近活动")))
                .child(
                    action_button(
                        "copy-diagnostics",
                        IconName::Copy,
                        l.text("Copy diagnostics", "复制诊断信息"),
                    )
                    .ghost()
                    .on_click(cx.listener(|view, _, _, cx| {
                        let state = view.session.read(cx);
                        let mut text = String::from("Easy Sound Connect diagnostics\n");
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
            entries =
                entries.child(caption(l.text("No recent activity", "暂无活动"), cx).py(px(20.)));
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
    pub(super) fn preferences_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let prefs = self.session.read(cx).preferences.clone();
        let saving = cx.global::<gui_shell::Shell>().saving_preferences;
        let mut languages = div().flex().flex_wrap().justify_end().gap(px(6.));
        for (index, (value, label)) in [
            (Language::English, "English"),
            (Language::Chinese, "简体中文"),
        ]
        .into_iter()
        .enumerate()
        {
            languages = languages.child(
                gui_style::chip(
                    ("language", index),
                    label,
                    prefs.language == value,
                    !saving,
                    cx,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    gui_shell::change_preferences(cx, move |p| p.language = value);
                })),
            );
        }
        let mut themes = div().flex().flex_wrap().justify_end().gap(px(6.));
        for (index, (value, label)) in [
            (ThemeMode::System, l.text("System", "跟随系统")),
            (ThemeMode::Light, l.text("Light", "明亮")),
            (ThemeMode::Dark, l.text("Dark", "暗色")),
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
        let appearance = gui_style::card(l.text("Appearance", "外观"), cx).child(
            div()
                .flex()
                .flex_col()
                .child(gui_style::setting_row(
                    l.text("Language", "语言"),
                    languages,
                    cx,
                ))
                .child(gui_style::divider(cx))
                .child(gui_style::setting_row(l.text("Theme", "主题"), themes, cx)),
        );
        let mut desktop_rows = div().flex().flex_col();
        for (index, (key, label, checked, disabled)) in [
            (
                "tray",
                l.text("Show tray icon", "显示托盘图标"),
                prefs.tray_enabled,
                false,
            ),
            (
                "close",
                l.text("Keep running when window closes", "关闭窗口后继续运行"),
                prefs.close_to_tray,
                !prefs.tray_enabled,
            ),
            (
                "autostart",
                l.text("Start at login", "登录时启动"),
                prefs.start_at_login,
                false,
            ),
            (
                "hidden",
                l.text("Start in tray at login", "登录启动时仅显示托盘"),
                prefs.start_hidden,
                !prefs.tray_enabled,
            ),
            (
                "notifications",
                l.text("Desktop notifications", "桌面通知"),
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
                label,
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
        let mut desktop = gui_style::card(l.text("Desktop", "桌面"), cx).child(desktop_rows);
        if let Some(reason) = &cx.global::<gui_shell::Shell>().tray_error {
            desktop = desktop.child(
                caption(
                    format!("{}: {reason}", l.text("Tray unavailable", "托盘不可用")),
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
            (
                format!("{modifier}1–7"),
                l.text("Navigate pages", "切换页面"),
            ),
            (format!("{modifier}R"), l.text("Refresh", "刷新")),
            (format!("{modifier},"), l.text("Preferences", "首选项")),
            (format!("{modifier}Q"), l.text("Quit", "退出")),
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
            .child(gui_style::card(l.text("Keyboard shortcuts", "键盘快捷键"), cx).child(shortcuts))
    }
    pub(super) fn about_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let identity = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .py(px(28.))
            .child(
                div()
                    .size(px(76.))
                    .rounded(px(20.))
                    .bg(cx.theme().accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        svg()
                            .path(IconName::Headphones.path())
                            .size(px(40.))
                            .text_color(cx.theme().primary),
                    ),
            )
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::BOLD)
                    .child("Easy Sound Connect"),
            )
            .child(caption(
                format!(
                    "{} {}",
                    l.text("Version", "版本"),
                    env!("CARGO_PKG_VERSION")
                ),
                cx,
            ));
        let links = div()
            .flex()
            .justify_center()
            .gap(px(10.))
            .child(
                button("project", "GitHub")
                    .h(px(32.))
                    .px(px(14.))
                    .on_click(|_, _, cx| {
                        cx.open_url("https://github.com/jjl9807/easy-sound-connect")
                    }),
            )
            .child(
                button("license", l.text("License", "许可证"))
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
            .child(
                caption(
                    l.text(
                        "Independent software. Not affiliated with SONY.",
                        "独立开发的软件，与 SONY 无隶属关系。",
                    ),
                    cx,
                )
                .text_center(),
            )
            .child(caption("Easy Sound Connect · MIT", cx).text_size(px(11.)))
            .child(caption("GPUI / GPUI Kit · Apache-2.0     Lucide · ISC", cx).text_size(px(11.)))
    }
}
fn connection_text(state: ConnectionState, l: Language) -> &'static str {
    match state {
        ConnectionState::Ready => l.text("Connected", "已连接"),
        ConnectionState::Connected => l.text("Connecting…", "正在连接…"),
        ConnectionState::Negotiating => l.text("Discovering capabilities…", "正在协商能力…"),
        ConnectionState::Synchronizing => l.text("Synchronizing…", "正在同步…"),
        ConnectionState::Closing => l.text("Releasing control…", "正在释放控制…"),
        ConnectionState::Disconnected => l.text("Disconnected", "已断开"),
        ConnectionState::Failed => l.text("Connection failed", "连接失败"),
    }
}
