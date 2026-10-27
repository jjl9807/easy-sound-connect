//! A modal surface for explicit decisions requested by the current device.

use app::device_prompts::PromptPresentation;
use ezsc_core::{ConnectionState, PromptId, PromptReply};
use gpui::{Context, Div, MouseButton, Window, div, prelude::*, px, rgb, rgba};

use crate::{Desktop, Event, button};

impl Desktop {
    pub(super) fn settings_blocked(&self) -> bool {
        self.busy
            || self.replying_to.is_some()
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
        self.message = match reply {
            PromptReply::Accept => "Sending your confirmation to the headset…",
            PromptReply::Reject => "Sending your cancellation to the headset…",
            PromptReply::Dismiss => "Closing the device message…",
        }
        .into();
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

    pub(super) fn render_device_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let prompt = self
            .snapshot
            .as_ref()
            .filter(|s| s.connection == ConnectionState::Ready)?
            .pending_prompt
            .clone()?;
        let presentation = PromptPresentation::from_prompt(&prompt);
        if self.prompt_seen != Some(prompt.id) || !self.prompt_focus.contains_focused(window, cx) {
            if self.prompt_seen != Some(prompt.id) {
                self.prompt_error = None;
            }
            self.prompt_seen = Some(prompt.id);
            window.focus(&self.prompt_focus, cx);
        }
        let waiting = self.replying_to == Some(prompt.id);
        let id = prompt.id;
        let mut content = div().flex().flex_col().gap_3().child(presentation.body);
        if let Some((error_id, error)) = &self.prompt_error
            && *error_id == id
        {
            content = content.child(
                div()
                    .text_color(rgb(0xffb7b7))
                    .child(format!("Could not complete your decision: {error}")),
            );
        }
        if !presentation.affected_functions.is_empty() {
            content = content.child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Affected functions"),
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
        for (index, action) in presentation.actions.iter().copied().enumerate() {
            let focus = self.prompt_action_focus[index].clone();
            actions = actions.child(
                button(("device-prompt-action", index), action.label)
                    .track_focus(&focus)
                    .border_1()
                    .border_color(rgb(0x50505c))
                    .focus(|style| style.border_color(rgb(0xa5b9ff)).bg(rgb(0x494963)))
                    .when(waiting, |button| button.opacity(0.5).cursor_default())
                    .on_click(cx.listener(move |view, _, window, cx| {
                        window.focus(&focus, cx);
                        view.respond_prompt(id, action.reply);
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
                .track_focus(&self.prompt_focus)
                .on_key_down(
                    cx.listener(move |view, event: &gpui::KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "tab" => {
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
                            "escape" => {
                                if let Some(reply) = keyboard.escape_reply() {
                                    view.respond_prompt(id, reply);
                                }
                            }
                            "enter" | "space" => {
                                // There is no default affirmative action. Keyboard activation
                                // requires deliberately focusing a particular action first.
                                if let Some(index) = view.prompt_action_focus
                                    [..keyboard.actions.len()]
                                    .iter()
                                    .position(|handle| handle.is_focused(window))
                                {
                                    view.respond_prompt(id, keyboard.actions[index].reply);
                                }
                            }
                            _ => {}
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
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
                        .bg(rgb(0x292930))
                        .border_1()
                        .border_color(rgb(0x50505c))
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
                                    .text_color(rgb(0xc8c8f3))
                                    .child("Waiting for the headset…"),
                            )
                        })
                        .child(actions),
                ),
        )
    }
}
