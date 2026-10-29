//! Brief, important messages anchored to the bottom of the sidebar.

use crate::{Desktop, Page, notices::Severity};
use gpui_kit::component::{
    ActiveTheme, Icon,
    button::{Button, ButtonCustomVariant, ButtonVariants},
};
use gpui_kit::{assets::IconName, prelude::*, *};

impl Desktop {
    pub(super) fn sidebar_notice(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        let state = self.session.read(cx);
        let notice = state.notices.current()?;
        let language = self.language(cx);
        let (title, icon, color) = match notice.severity {
            Severity::Warning => (
                language.tr("ui.warning"),
                IconName::TriangleAlert,
                cx.theme().warning,
            ),
            Severity::Error => (
                language.tr("ui.error"),
                IconName::CircleAlert,
                cx.theme().danger,
            ),
        };
        let id = notice.id;
        Some(
            div()
                .id("sidebar-notice")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .w_full()
                .min_w_0()
                .gap_1()
                .p_2()
                .rounded(px(9.))
                .border_1()
                .border_color(color.opacity(0.35))
                .bg(color.opacity(0.1))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_color(color)
                        .child(Icon::new(icon).size(px(15.)))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            Button::new("dismiss-sidebar-notice")
                                .ghost()
                                .icon(IconName::X)
                                .w(px(20.))
                                .h(px(20.))
                                .accessibility_label(language.tr("ui.dismiss_message"))
                                .tooltip(language.tr("ui.dismiss_message"))
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.session.update(cx, |state, cx| {
                                        state.notices.dismiss(id);
                                        cx.notify();
                                    });
                                })),
                        ),
                )
                .child(
                    Button::new("sidebar-notice-details")
                        .custom(ButtonCustomVariant::new(cx))
                        .w_full()
                        .h_auto()
                        .p_0()
                        .accessibility_label(notice.message.clone())
                        .tooltip(language.tr("ui.view_activity_details"))
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .text_left()
                                .text_size(px(12.))
                                .text_color(cx.theme().foreground)
                                .whitespace_normal()
                                .line_clamp(4)
                                .child(notice.message.clone()),
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.select_page(Page::Activity, window, cx);
                        })),
                ),
        )
    }
}
