//! Shared surfaces and controls for the desktop's visual hierarchy.

use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::{Button, ButtonCustomVariant, ButtonVariants},
};
use gpui_kit::{prelude::*, *};

pub(super) fn card_shadow(cx: &App) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: hsla(0., 0., 0., if cx.theme().is_dark() { 0.28 } else { 0.035 }),
        offset: point(px(0.), px(6.)),
        blur_radius: px(14.),
        spread_radius: px(0.),
        inset: false,
    }]
}

pub(super) fn surface(cx: &App) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .gap(px(16.))
        .p(px(20.))
        .rounded(px(20.))
        .bg(cx.theme().group_box)
        .border_1()
        .border_color(cx.theme().border)
        .shadow(card_shadow(cx))
}

pub(super) fn card(title: impl Into<SharedString>, cx: &App) -> Div {
    surface(cx).child(
        div()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD)
            .child(title.into()),
    )
}

pub(super) fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    enabled: bool,
    cx: &App,
) -> Button {
    let label = label.into();
    Button::new(id)
        .when(!label.is_empty(), |button| button.label(label))
        .when(selected, |button| {
            button.custom(
                ButtonCustomVariant::new(cx)
                    .color(cx.theme().primary)
                    .hover(cx.theme().primary_hover)
                    .active(cx.theme().primary)
                    .foreground(cx.theme().primary_foreground),
            )
        })
        .small()
        .h(px(30.))
        .px(px(13.))
        .text_size(px(12.5))
        .font_weight(if selected {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .rounded(px(15.))
        .border_0()
        .selected(selected)
        .disabled(!enabled)
}

pub(super) fn setting_row(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    labelled_row(label, control, px(13.5), FontWeight::MEDIUM, cx)
}

pub(super) fn setting_heading(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> Div {
    labelled_row(label, control, px(15.), FontWeight::BOLD, cx)
}

fn labelled_row(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    size: Pixels,
    weight: FontWeight,
    cx: &App,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .min_h(px(38.))
        .gap(px(16.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(size)
                .font_weight(weight)
                .text_color(cx.theme().foreground)
                .child(label.into()),
        )
        .child(div().flex().items_center().flex_shrink_0().child(control))
}

pub(super) fn divider(cx: &App) -> Div {
    div().h(px(1.)).w_full().bg(cx.theme().border)
}
