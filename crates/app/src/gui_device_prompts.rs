//! A modal surface for explicit decisions requested by the current device.

use app::device_prompts::PromptPresentation;
use ezsc_core::{ConnectionState, PromptId, PromptReply};
use gpui::{Context, Div, MouseButton, Window, div, prelude::*, px, rgba};

use crate::{Desktop, Event, Session, button};
use gpui_kit::component::{ActiveTheme, Disableable};

impl Session {
    pub(super) fn settings_blocked(&self) -> bool {
        self.busy || self.settings_unavailable()
    }

    /// Temporary command serialization keeps controls' existing appearance.
    pub(super) fn settings_unavailable(&self) -> bool {
        self.replying_to.is_some()
            || self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.pending_prompt.is_some())
    }

    pub(super) fn respond_prompt(&mut self, id: PromptId, reply: PromptReply) {
        let Some(handle) = self.handle.clone() else {
            return;
        };
        let snapshot = handle.snapshot();
        let Some(prompt) = snapshot.pending_prompt else {
            return;
        };
        if self.replying_to.is_some()
            || snapshot.connection != ConnectionState::Ready
            || prompt.id != id
            || !prompt.allows(reply)
            || !PromptPresentation::from_prompt(&prompt)
                .actions
                .iter()
                .any(|action| action.reply == reply)
        {
            return;
        }
        self.replying_to = Some(id);
        self.prompt_error = None;
        let language = self.preferences.language;
        self.message = match reply {
            PromptReply::Accept => language.tr("prompt.sending_confirmation"),
            PromptReply::Reject => language.tr("prompt.sending_cancellation"),
            PromptReply::Dismiss => language.tr("prompt.closing_message"),
        };
        let sender = self.events.clone();
        let epoch = self.epoch;
        self.runtime.spawn(async move {
            let result = handle
                .respond_prompt(id, reply)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(Event::PromptResponse(epoch, id, result)).await;
        });
    }
}

impl Desktop {
    pub(super) fn render_device_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let waiting_id = state.replying_to;
        let prompt_error = state.prompt_error.clone();
        let prompt = state
            .snapshot
            .as_ref()
            .filter(|s| s.connection == ConnectionState::Ready)
            .and_then(|snapshot| snapshot.pending_prompt.clone());
        let Some(prompt) = prompt else {
            if self.prompt_seen.take().is_some() {
                window.focus(&self.focus, cx);
            }
            return None;
        };
        let presentation = PromptPresentation::from_prompt_localized(&prompt, language.locale());
        if self.prompt_seen != Some(prompt.id) || !self.prompt_focus.contains_focused(window, cx) {
            self.prompt_seen = Some(prompt.id);
            window.focus(&self.prompt_focus, cx);
        }
        let waiting = waiting_id == Some(prompt.id);
        let id = prompt.id;
        let mut content = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(presentation.body.clone());
        if let Some((error_id, _)) = &prompt_error
            && *error_id == id
        {
            content = content.child(
                div()
                    .text_color(cx.theme().danger)
                    .child(language.tr("prompt.result_unconfirmed")),
            );
        }
        if !presentation.affected_functions.is_empty() {
            content = content.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(language.tr("prompt.affected_functions")),
            );
            for item in &presentation.affected_functions {
                content = content.child(format!("• {item}"));
            }
        }
        let mut actions = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .flex_shrink_0();
        for (index, action) in presentation.actions.iter().enumerate() {
            let focus = self.prompt_action_focus[index].clone();
            let reply = action.reply;
            actions = actions.child(
                button(("device-prompt-action", index), action.label.clone())
                    .track_focus(&focus)
                    .border_1()
                    .border_color(cx.theme().border)
                    .disabled(waiting)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        window.focus(&focus, cx);
                        view.session.update(cx, |session, cx| {
                            session.respond_prompt(id, reply);
                            cx.notify();
                        });
                        cx.stop_propagation();
                        cx.notify();
                    })),
            );
        }
        let keyboard = presentation.clone();
        let max_height = (window.viewport_size().height - px(40.)).max(px(80.));
        Some(
            div()
                .id("device-prompt-overlay")
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p_4()
                .bg(rgba(0x000000b8))
                .occlude()
                .role(gpui::accesskit::Role::Dialog)
                .aria_label(presentation.title.clone())
                .track_focus(&self.prompt_focus)
                .on_key_down(
                    cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "tab" => {
                                if waiting {
                                    window.focus(&view.prompt_focus, cx);
                                    cx.stop_propagation();
                                    return;
                                }
                                let count = keyboard.actions.len();
                                let current = view.prompt_action_focus[..count]
                                    .iter()
                                    .position(|handle| handle.is_focused(window));
                                let next = match (current, event.keystroke.modifiers.shift) {
                                    (Some(0), true) | (None, true) => count - 1,
                                    (Some(index), true) => index - 1,
                                    (Some(index), false) => (index + 1) % count,
                                    (None, false) => 0,
                                };
                                window.focus(&view.prompt_action_focus[next], cx);
                            }
                            "escape" if !event.keystroke.modifiers.modified() => {
                                if let Some(reply) = keyboard.escape_reply() {
                                    view.session.update(cx, |session, cx| {
                                        session.respond_prompt(id, reply);
                                        cx.notify();
                                    });
                                }
                            }
                            // Kit buttons dispatch one click on Enter/Space release only
                            // while explicitly focused. The container has no default action.
                            _ => {}
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_key_up(|_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w_full()
                        .max_w(px(540.))
                        .max_h(max_height)
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .gap_4()
                        .p_5()
                        .rounded_lg()
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(cx.theme().border)
                        .shadow_lg()
                        .child(
                            div()
                                .text_size(px(21.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .flex_shrink_0()
                                .child(presentation.title),
                        )
                        .child(
                            div()
                                .id("device-prompt-content")
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(content),
                        )
                        .when(waiting, |panel| {
                            panel.child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(language.tr("prompt.waiting")),
                            )
                        })
                        .child(actions),
                ),
        )
    }
}
