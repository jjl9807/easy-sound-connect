//! Product pages built from shared device state and client preferences.
use crate::{
    Desktop, Page, button, gui_shell,
    preferences::{Language, ThemeMode},
};
use ezsc_core::{BatteryComponent, ConnectionState, Freshness};
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
    switch::Switch,
};
use gpui_kit::{assets::IconName, prelude::*, *};

fn surface(cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_4()
        .p_5()
        .rounded(px(20.))
        .bg(cx.theme().group_box)
        .border_1()
        .border_color(cx.theme().border)
}
fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
fn title(text: impl Into<SharedString>) -> Div {
    div()
        .font_weight(FontWeight::SEMIBOLD)
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
        let blocked = state.settings_blocked() || (!state.preview && state.manager.is_none());
        let has_focused_device = state.focused_device.is_some();
        let removable_device = state
            .focused_device
            .clone()
            .filter(|id| ready && state.saved_devices.find(id).is_some());
        div()
            .flex()
            .items_center()
            .gap_2()
            .when(ready, |row| {
                row.child(
                    action_button(
                        "refresh",
                        IconName::RefreshCw,
                        language.text("Refresh", "刷新"),
                    )
                    .ghost()
                    .disabled(blocked || state.preview)
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
        let snapshot = state.snapshot.as_ref();
        let name = state.focused_name().unwrap_or_else(|| {
            language
                .text("Your SONY headphones", "你的 SONY 耳机")
                .into()
        });
        let connection = snapshot
            .map(|s| connection_text(s.connection, language))
            .unwrap_or(language.text("Not connected", "未连接"));
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
        .size(px(if compact { 64. } else { 220. }))
        .flex_shrink_0();
        let mut status = div()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_2()
            .child(
                div()
                    .text_size(px(if compact { 16. } else { 30. }))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(name),
            )
            .child(caption(connection, cx));
        if let Some(snapshot) = snapshot {
            if let Some(codec) = &snapshot.audio_codec {
                let codec = codec
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
                        _ => format!(
                            "{} (0x{:02X})",
                            language.text("Unknown", "未知"),
                            c.as_raw()
                        ),
                    })
                    .unwrap_or_else(|| "—".into());
                let suffix = match snapshot.audio_codec.as_ref().unwrap().freshness {
                    Freshness::Current => "",
                    Freshness::Unknown => language.text(" · not read", " · 未读取"),
                    Freshness::Stale => language.text(" · stale", " · 已过期"),
                };
                status = status.child(caption(format!("{codec}{suffix}"), cx));
            }
            let mut batteries = div().flex().flex_wrap().gap_3();
            for endpoint in &snapshot.batteries {
                let label = match endpoint.component {
                    BatteryComponent::Headset => language.text("Battery", "电量"),
                    BatteryComponent::Left => language.text("Left", "左耳"),
                    BatteryComponent::Right => language.text("Right", "右耳"),
                    BatteryComponent::Case => language.text("Case", "充电盒"),
                };
                let value = endpoint
                    .reading
                    .map(|r| {
                        let charge = match r.charging {
                            libmdr2::battery::ChargingStatus::Charging => {
                                language.text(" · charging", " · 充电中")
                            }
                            libmdr2::battery::ChargingStatus::Charged => {
                                language.text(" · charged", " · 已充满")
                            }
                            libmdr2::battery::ChargingStatus::NotCharging => "",
                            _ => language.text(" · charging unknown", " · 充电状态未知"),
                        };
                        format!("{}%{charge}", r.level.as_u8())
                    })
                    .unwrap_or_else(|| "—".into());
                let freshness = match endpoint.freshness {
                    Freshness::Current => "",
                    Freshness::Stale => language.text(" · stale", " · 已过期"),
                    Freshness::Unknown => language.text(" · not read", " · 未读取"),
                };
                batteries = batteries.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(caption(label, cx))
                        .child(format!("{value}{freshness}")),
                );
            }
            status = status.child(batteries);
        }
        surface(cx)
            .when(compact, |card| {
                card.p_0().border_0().bg(cx.theme().background)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_6()
                    .child(image)
                    .child(status),
            )
    }
    pub(super) fn connection_page(&self, cx: &Context<Self>) -> Div {
        let state = self.session.read(cx);
        let language = self.language(cx);
        surface(cx)
            .child(title(language.text("Connect your headphones", "连接耳机")))
            .child(caption(if state.saved_devices.entries.is_empty() {
                language.text(
                    "Add headphones to save them here. Pair new headphones in system Bluetooth settings first.",
                    "添加耳机后会保存在这里。新耳机请先在系统蓝牙设置中完成配对。")
            } else {
                language.text(
                    "Choose a saved headset from the device card, or add another pair.",
                    "点击设备卡片选择已保存的耳机，或添加其他耳机。")
            }, cx))
            .when(self.compact, |card| card.child(self.device_picker(cx)))
    }
    pub(super) fn device_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut page = div()
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(1100.))
            .child(self.product_header(false, cx));
        if self.session.read(cx).snapshot.is_none() {
            return page.child(self.connection_page(cx));
        }
        let language = self.language(cx);
        page = page.child(
            div()
                .flex()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.render_noise_controls(window, cx)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.render_speak_controls(cx)),
                ),
        );
        page.child(
            surface(cx)
                .child(title(language.text("Your sound", "你的声音")))
                .child(caption(
                    language.text(
                        "Fine-tune equalizer, DSEE and connection quality in Sound.",
                        "在“声音”中调整均衡器、DSEE 和连接质量。",
                    ),
                    cx,
                ))
                .child(
                    button("go-sound", language.text("Open Sound", "打开声音设置")).on_click(
                        cx.listener(|view, _, _, cx| {
                            view.select_page(Page::Sound, cx);
                        }),
                    ),
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
            .gap_4()
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
        let mut details =
            surface(cx).child(title(language.text("Headphone information", "耳机信息")));
        for (label, value) in [
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
                language.text("Control connection", "控制连接"),
                format!("{:?}", snapshot.transport),
            ),
            (
                language.text("Status", "状态"),
                connection_text(snapshot.connection, language).to_owned(),
            ),
        ] {
            details = details.child(
                div()
                    .flex()
                    .justify_between()
                    .child(caption(label, cx))
                    .child(value),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(980.))
            .child(details)
            .child(
                surface(cx)
                    .child(title(language.text("Control connection", "控制连接")))
                    .child(caption(
                        language.text(
                            "Releasing this connection leaves system audio connected.",
                            "释放控制连接不会断开系统音频连接。",
                        ),
                        cx,
                    ))
                    .child(self.connection_actions(cx)),
            )
    }
    pub(super) fn scenes_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        surface(cx).child(title(l.text("Local scenes","本地场景"))).child(caption(l.text("Scene editing and automation are not available in this version. Use My Device and Sound to adjust your headphones.","此版本尚未提供场景编辑与自动触发。可在“我的设备”和“声音”中调整耳机。"),cx))
    }
    pub(super) fn activity_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let state = self.session.read(cx);
        let mut list = surface(cx).child(
            div()
                .flex()
                .justify_between()
                .child(title(l.text("Recent activity", "最近活动")))
                .child(
                    button(
                        "copy-diagnostics",
                        l.text("Copy diagnostics", "复制诊断信息"),
                    )
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
        if state.activity.is_empty() {
            list = list.child(caption(
                l.text(
                    "Connection and setting results appear here.",
                    "连接事件和设置结果会显示在这里。",
                ),
                cx,
            ));
        }
        for (index, line) in state.activity.iter().rev().enumerate() {
            list = list.child(
                div()
                    .id(("activity", index))
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(line.clone()),
            );
        }
        list
    }
    pub(super) fn preferences_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        let prefs = self.session.read(cx).preferences.clone();
        let saving = cx.global::<gui_shell::Shell>().saving_preferences;
        let mut languages = div().flex().gap_2();
        for (index, (value, label)) in [
            (Language::English, "English"),
            (Language::Chinese, "简体中文"),
        ]
        .into_iter()
        .enumerate()
        {
            languages = languages.child(
                button(("language", index), label)
                    .selected(prefs.language == value)
                    .disabled(saving)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        gui_shell::change_preferences(cx, move |p| p.language = value);
                    })),
            );
        }
        let mut themes = div().flex().gap_2();
        for (index, (value, label)) in [
            (ThemeMode::System, l.text("System", "跟随系统")),
            (ThemeMode::Light, l.text("Light", "明亮")),
            (ThemeMode::Dark, l.text("Dark", "暗色")),
        ]
        .into_iter()
        .enumerate()
        {
            themes = themes.child(
                button(("theme", index), label)
                    .selected(prefs.theme == value)
                    .disabled(saving)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        gui_shell::change_preferences(cx, move |p| p.theme = value);
                    })),
            );
        }
        let appearance = surface(cx)
            .child(title(l.text("Appearance", "外观")))
            .child(caption(l.text("Language", "语言"), cx))
            .child(languages)
            .child(caption(l.text("Theme", "主题"), cx))
            .child(themes);
        let mut desktop = surface(cx).child(title(l.text("Desktop", "桌面行为")));
        for (key, label, checked, disabled) in [
            (
                "tray",
                l.text("Show tray icon", "显示托盘图标"),
                prefs.tray_enabled,
                false,
            ),
            (
                "close",
                l.text("Close window to tray", "关闭窗口后保留托盘"),
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
        ] {
            desktop = desktop.child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(label)
                    .child(
                        Switch::new(key)
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
                    ),
            );
        }
        let shell = cx.global::<gui_shell::Shell>();
        if let Some(reason) = &shell.tray_error {
            desktop = desktop.child(caption(
                format!("{}: {reason}", l.text("Tray unavailable", "托盘不可用")),
                cx,
            ));
        }
        desktop = desktop.child(caption(
            l.text(
                "If no tray host is available, the main window stays accessible.",
                "系统未提供托盘时，会保留可访问的主窗口。",
            ),
            cx,
        ));
        let modifier = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .max_w(px(980.))
            .child(appearance)
            .child(desktop)
            .child(
                surface(cx)
                    .child(title(l.text("Keyboard shortcuts", "键盘快捷键")))
                    .child(format!(
                        "{modifier}1–7 · {}",
                        l.text("Navigate pages", "切换页面")
                    ))
                    .child(format!("{modifier}R · {}", l.text("Refresh", "刷新")))
                    .child(format!("{modifier}, · {}", l.text("Preferences", "首选项")))
                    .child(format!("{modifier}Q · {}", l.text("Quit", "退出"))),
            )
    }
    pub(super) fn about_page(&self, cx: &Context<Self>) -> Div {
        let l = self.language(cx);
        surface(cx)
            .max_w(px(980.))
            .child(title("Easy Sound Connect"))
            .child(format!(
                "{} {}",
                l.text("Version", "版本"),
                env!("CARGO_PKG_VERSION")
            ))
            .child(caption(
                l.text(
                    "An independent Rust desktop client for SONY MDR v2 headphones.",
                    "独立使用 Rust 实现的 SONY MDR v2 耳机桌面客户端。",
                ),
                cx,
            ))
            .child(caption(
                l.text(
                    "Built with GPUI Kit. This project is not affiliated with SONY.",
                    "基于 GPUI Kit 构建。本项目与 SONY 无隶属关系。",
                ),
                cx,
            ))
            .child(
                button("project", "GitHub").on_click(|_, _, cx| {
                    cx.open_url("https://github.com/jjl9807/easy-sound-connect")
                }),
            )
            .child(caption(
                "Easy Sound Connect: MIT · GPUI / GPUI Kit: Apache-2.0 · Lucide icons: ISC",
                cx,
            ))
    }
}
fn connection_text(state: ConnectionState, l: Language) -> &'static str {
    match state {
        ConnectionState::Ready => l.text("Connected", "已连接"),
        ConnectionState::Connected => l.text("Connected · preparing", "已连接 · 正在准备"),
        ConnectionState::Negotiating => l.text("Discovering capabilities…", "正在协商能力…"),
        ConnectionState::Synchronizing => l.text("Synchronizing…", "正在同步…"),
        ConnectionState::Closing => l.text("Releasing control…", "正在释放控制…"),
        ConnectionState::Disconnected => l.text("Disconnected", "已断开"),
        ConnectionState::Failed => l.text("Connection failed", "连接失败"),
    }
}
