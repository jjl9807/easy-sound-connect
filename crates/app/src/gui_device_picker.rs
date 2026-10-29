//! The sidebar's selected headset and saved-device popover.

use crate::{Desktop, Session, device_status, gui_product_images, product_assets::ModelKey};
use bluetooth::{DeviceId, TransportKind};
use ezsc_core::ConnectionState;
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable,
    button::{Button, ButtonCustomVariant, ButtonVariants},
    popover::Popover,
};
use gpui_kit::{assets::IconName, prelude::*, *};

pub(super) const SIDEBAR_WIDTH: f32 = 228.;
pub(super) const SIDEBAR_PADDING: f32 = 14.;
// The sidebar's one-pixel right border is inside its width.
const PICKER_WIDTH: f32 = SIDEBAR_WIDTH - SIDEBAR_PADDING * 2. - 1.;

impl Desktop {
    pub(super) fn device_picker(&self, cx: &Context<Self>) -> Popover {
        let session = self.session.clone();
        let state = session.read(cx);
        let language = state.preferences.language;
        let name = state.focused_name().unwrap_or_else(|| {
            if state.focused_device.is_some() {
                language.tr("ui.your_sony_headphones")
            } else if !state.saved_devices.entries.is_empty() {
                language.tr("ui.choose_headphones")
            } else {
                language.tr("ui.add_headphones")
            }
        });
        let (status, color, battery) = card_status(state, state.focused_device.as_ref(), cx);
        let trigger = Button::new("device-picker-trigger")
            .accessibility_label(language.tr("ui.choose_headphones"))
            .ghost()
            .w_full()
            .h(px(64.))
            .p_0()
            .rounded(px(14.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().group_box)
            .child(card(
                name,
                status,
                color,
                battery,
                gui_product_images::model_key(state, state.focused_device.as_ref()),
                language,
                cx,
            ));
        let view = cx.entity().downgrade();
        let changed = view.clone();
        Popover::new("device-picker")
            .w(px(PICKER_WIDTH))
            .p_1()
            .trigger(trigger)
            .trigger_style(StyleRefinement::default().w_full())
            .open(self.device_picker_open)
            .on_open_change(move |open, _, cx| {
                let _ = changed.update(cx, |view, cx| {
                    view.device_picker_open = *open;
                    if *open && view.session.read(cx).saved_devices.entries.is_empty() {
                        view.session.update(cx, |state, cx| {
                            state.begin_add_device();
                            cx.notify();
                        });
                    } else if !open {
                        view.session.update(cx, |state, cx| {
                            state.finish_add_device();
                            cx.notify();
                        });
                    }
                    cx.notify();
                });
            })
            .content(move |_, _, cx| picker_content(&session, view.clone(), cx))
    }
}

fn card(
    name: String,
    status: String,
    color: Hsla,
    battery: Option<String>,
    model: Option<ModelKey>,
    language: crate::preferences::Language,
    cx: &App,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .p(px(9.))
        .w_full()
        .min_w_0()
        .child(
            img(gui_product_images::source(model, None, cx))
                .object_fit(ObjectFit::Contain)
                .size(px(32.))
                .flex_shrink_0(),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(cx.theme().foreground)
                        .truncate()
                        .child(name),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .min_w_0()
                        .child(div().size(px(6.)).rounded_full().bg(color).flex_shrink_0())
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(battery.map_or_else(
                                    || status.to_owned(),
                                    |battery| {
                                        language.tr_args(
                                            "ui.connection_battery",
                                            &[("status", status.clone()), ("battery", battery)],
                                        )
                                    },
                                )),
                        ),
                ),
        )
}

fn card_status(state: &Session, id: Option<&DeviceId>, cx: &App) -> (String, Hsla, Option<String>) {
    let language = state.preferences.language;
    let snapshot = state
        .snapshot
        .as_ref()
        .filter(|snapshot| Some(&snapshot.device) == id);
    let connection = id.map_or(ConnectionState::Disconnected, |id| {
        state.device_connection(id)
    });
    let (status, color) = match connection {
        ConnectionState::Ready => (language.tr("ui.connected"), cx.theme().success),
        ConnectionState::Connected | ConnectionState::Negotiating => {
            (language.tr("ui.connecting"), cx.theme().warning)
        }
        ConnectionState::Synchronizing => (language.tr("ui.syncing"), cx.theme().warning),
        ConnectionState::Closing => (language.tr("ui.disconnecting"), cx.theme().warning),
        ConnectionState::Failed => (language.tr("ui.connection_failed"), cx.theme().danger),
        ConnectionState::Disconnected => {
            (language.tr("ui.not_connected"), cx.theme().muted_foreground)
        }
    };
    let battery = (connection == ConnectionState::Ready).then(|| {
        snapshot
            .and_then(device_status::battery_level)
            .map_or_else(|| "—".into(), |level| format!("{level}%"))
    });
    (status, color, battery)
}

fn picker_content(session: &Entity<Session>, view: WeakEntity<Desktop>, cx: &mut App) -> Div {
    let state = session.read(cx);
    let language = state.preferences.language;
    let blocked = state.settings_unavailable() || (!state.preview && state.manager.is_none());
    let adding = state.adding_device;
    let mut content = div().flex().flex_col().gap_2().w_full().min_w_0();
    content = content.when(adding, |content| {
        content.child(
            div()
                .px_2()
                .py_1()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .child(language.tr("ui.add_headphones")),
        )
    });
    let mut list = div()
        .id("saved-device-list")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(350.))
        .overflow_y_scroll();
    if adding {
        list = list.child(hint(language.tr("ui.pair_before_selecting"), cx));
        let candidates: Vec<_> = state
            .devices
            .iter()
            .filter(|device| state.saved_devices.find(&device.id).is_none())
            .cloned()
            .collect();
        if candidates.is_empty() {
            list = list.child(hint(
                if state.discovering {
                    language.tr("ui.looking_for_paired_headphones")
                } else {
                    language.tr("ui.no_new_headphones")
                },
                cx,
            ));
        }
        for (index, device) in candidates.into_iter().enumerate() {
            let transport = if device.transports.contains(&TransportKind::Rfcomm) {
                Some(TransportKind::Rfcomm)
            } else {
                device.transports.first().copied()
            };
            let Some(transport) = transport else {
                continue;
            };
            let id = device.id.clone();
            let name = device
                .name
                .unwrap_or_else(|| language.tr("ui.unnamed_headphones"));
            let connect_session = session.clone();
            let connect_view = view.clone();
            list = list.child(
                Button::new(("add-headset", index))
                    .ghost()
                    .h(px(64.))
                    .p_0()
                    .w_full()
                    .disabled(blocked || state.discovering)
                    .accessibility_label(
                        language.tr_args("ui.connect_named_headphones", &[("name", name.clone())]),
                    )
                    .child(card(
                        name,
                        language.tr("ui.not_connected"),
                        cx.theme().muted_foreground,
                        None,
                        None,
                        language,
                        cx,
                    ))
                    .on_click(move |_, _, cx| {
                        if connect_session.read(cx).settings_blocked() {
                            return;
                        }
                        connect_session.update(cx, |state, cx| {
                            state.connect(id.clone(), transport);
                            state.finish_add_device();
                            cx.notify();
                        });
                        close_picker(&connect_view, cx);
                    }),
            );
            // Preserve an explicit alternative when the headset advertises
            // more than one v2 service; never silently change transports.
            for (option, alternate) in device.transports.into_iter().enumerate() {
                if alternate == transport {
                    continue;
                }
                let id = device.id.clone();
                let session = session.clone();
                let view = view.clone();
                let label = language.tr_args(
                    "ui.connect_via_transport",
                    &[(
                        "transport",
                        match alternate {
                            TransportKind::Rfcomm => "RFCOMM",
                            TransportKind::Gatt => "GATT",
                        }
                        .into(),
                    )],
                );
                list = list.child(
                    Button::new(("add-headset-transport", index * 2 + option))
                        .ghost()
                        .label(label)
                        .w_full()
                        .disabled(blocked || state.discovering)
                        .on_click(move |_, _, cx| {
                            if session.read(cx).settings_blocked() {
                                return;
                            }
                            session.update(cx, |state, cx| {
                                state.connect(id.clone(), alternate);
                                state.finish_add_device();
                                cx.notify();
                            });
                            close_picker(&view, cx);
                        }),
                );
            }
        }
    } else {
        if state.saved_devices.entries.is_empty() {
            list = list.child(hint(language.tr("ui.no_saved_headphones_yet"), cx));
        }
        for (index, device) in state.saved_devices.entries.iter().enumerate() {
            let id = device.device_id();
            let selected = state.focused_device.as_ref() == Some(&id);
            let (status, color, battery) = card_status(state, Some(&id), cx);
            let select_id = id.clone();
            let select_session = session.clone();
            let select_view = view.clone();
            let name = device.display_name().to_owned();
            list = list.child(
                Button::new(("saved-headset", index))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if selected {
                                cx.theme().sidebar_accent
                            } else {
                                cx.theme().transparent
                            })
                            .hover(cx.theme().sidebar_accent)
                            .active(cx.theme().sidebar_accent),
                    )
                    .selected(selected)
                    .h(px(64.))
                    .p_0()
                    .w_full()
                    .min_w_0()
                    .disabled(blocked)
                    .accessibility_label(
                        language.tr_args("ui.select_named_headphones", &[("name", name.clone())]),
                    )
                    .child(card(
                        name,
                        status,
                        color,
                        battery,
                        gui_product_images::model_key(state, Some(&id)),
                        language,
                        cx,
                    ))
                    .on_click(move |_, _, cx| {
                        if select_session.read(cx).settings_blocked() {
                            return;
                        }
                        select_session.update(cx, |state, cx| {
                            state.select_saved_device(select_id.clone());
                            cx.notify();
                        });
                        close_picker(&select_view, cx);
                    }),
            );
        }
    }
    content = content.child(list);
    if let Some(error) = &state.device_storage_error {
        content = content.child(hint(error.clone(), cx).text_color(cx.theme().danger));
    }
    let footer_session = session.clone();
    let back_session = session.clone();
    content.child(
        div()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .flex()
            .items_center()
            .gap_2()
            .when(adding, |row| {
                row.child(
                    Button::new("device-picker-back")
                        .ghost()
                        .flex_1()
                        .min_w_0()
                        .h(px(32.))
                        .label(language.tr("ui.back"))
                        .on_click(move |_, _, cx| {
                            back_session.update(cx, |state, cx| {
                                state.finish_add_device();
                                cx.notify();
                            })
                        }),
                )
            })
            .child(
                Button::new("device-picker-add")
                    .ghost()
                    .flex_1()
                    .min_w_0()
                    .h(px(32.))
                    .when(!adding, |button| button.icon(IconName::Plus))
                    .label(if adding {
                        language.tr("ui.refresh")
                    } else {
                        language.tr("ui.add_headphones")
                    })
                    .disabled(blocked || state.discovering)
                    .on_click(move |_, _, cx| {
                        footer_session.update(cx, |state, cx| {
                            state.begin_add_device();
                            cx.notify();
                        })
                    }),
            ),
    )
}

fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .p_2()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .whitespace_normal()
        .child(text.into())
}

fn close_picker(view: &WeakEntity<Desktop>, cx: &mut App) {
    let _ = view.update(cx, |view, cx| {
        view.device_picker_open = false;
        cx.notify();
    });
}
