//! User-facing descriptions and explicit actions for device messages.

use crate::i18n::translate;

use ezsc_core::{
    DevicePrompt, PromptReply,
    alert::{AlertActionType, AlertContents},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptAction {
    pub label: String,
    pub reply: PromptReply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPresentation {
    pub title: String,
    pub body: String,
    pub affected_functions: Vec<String>,
    pub actions: Vec<PromptAction>,
}

impl PromptPresentation {
    pub fn from_prompt(prompt: &DevicePrompt) -> Self {
        Self::from_prompt_localized(prompt, "en")
    }

    /// Localizes presentation without changing the device's permitted actions.
    pub fn from_prompt_localized(prompt: &DevicePrompt, locale: &str) -> Self {
        let (description_key, action, affected_functions, known_scope) =
            match prompt.message.contents() {
                AlertContents::Fixed { message, action } => (
                    fixed_description_key(message.as_raw()),
                    Some(*action),
                    Vec::new(),
                    message.supports_reply(),
                ),
                AlertContents::Flexible {
                    message,
                    items,
                    action,
                } => (
                    flexible_description_key(message.as_raw()),
                    Some(*action),
                    items
                        .iter()
                        .map(|item| item_label(item.as_raw(), locale))
                        .collect(),
                    message.supports_reply() && items.iter().all(|item| item.supports_reply()),
                ),
                _ => (None, None, Vec::new(), false),
            };
        let supported = known_scope
            && description_key.is_some()
            && (prompt.allows(PromptReply::Accept)
                || prompt.allows(PromptReply::Reject)
                || action == Some(AlertActionType::ConfirmationOnly));
        let body = if supported {
            translate(
                locale,
                description_key.expect("supported prompts have a semantic key"),
            )
        } else {
            translate(locale, "prompt.unsupported_request")
        };
        let actions = if supported && action != Some(AlertActionType::ConfirmationOnly) {
            let mut actions = Vec::new();
            if prompt.allows(PromptReply::Reject) {
                actions.push(PromptAction {
                    label: translate(locale, "prompt.cancel"),
                    reply: PromptReply::Reject,
                });
            }
            if prompt.allows(PromptReply::Accept) {
                actions.push(PromptAction {
                    label: if action == Some(AlertActionType::ConfirmWithReply) {
                        translate(locale, "prompt.ok")
                    } else {
                        translate(locale, "prompt.continue")
                    },
                    reply: PromptReply::Accept,
                });
            }
            actions
        } else {
            vec![PromptAction {
                label: translate(locale, "prompt.close"),
                reply: PromptReply::Dismiss,
            }]
        };
        Self {
            title: if supported && action != Some(AlertActionType::ConfirmationOnly) {
                translate(locale, "prompt.confirmation_title")
            } else {
                translate(locale, "prompt.message_title")
            },
            body,
            affected_functions,
            actions,
        }
    }

    /// Escape never accepts a request or pretends to reject a positive-only one.
    pub fn escape_reply(&self) -> Option<PromptReply> {
        self.actions.iter().find_map(|action| match action.reply {
            PromptReply::Reject | PromptReply::Dismiss => Some(action.reply),
            PromptReply::Accept => None,
        })
    }
}

/// Maps protocol identities to semantic keys; locale coverage never authorizes replies.
fn fixed_description_key(message: u8) -> Option<&'static str> {
    Some(match message {
        0x00 => "prompt.fixed.connection_quality_disconnect",
        0x01 => "prompt.fixed.button_assignment_disconnect",
        0x06 => "prompt.fixed.multipoint_disable_ldac_disconnect",
        0x07 => "prompt.fixed.multipoint_disconnect",
        0x08 => "prompt.fixed.equalizer_dsee_battery",
        0x09 => "prompt.fixed.duplicate_playback_assignment",
        0x0a => "prompt.fixed.touch_panel_disable",
        0x0b => "prompt.fixed.touch_panel_disable_reconnect",
        0x0c => "prompt.fixed.touch_panel_enable",
        0x0d => "prompt.fixed.touch_panel_enable_reconnect",
        0x1c => "prompt.fixed.sound_quality_confirmation",
        0x2a => "prompt.fixed.ldac_unavailable",
        0x2b => "prompt.fixed.sound_functions_battery",
        0x30 => "prompt.fixed.classic_audio_disconnect",
        0x53 => "prompt.fixed.ldac_990_confirmation",
        0x59 => "prompt.fixed.automatic_volume_enable",
        0x5a => "prompt.fixed.automatic_volume_safe_control",
        0x5b => "prompt.fixed.automatic_volume_safe_limit",
        0x5c => "prompt.fixed.equalizer_bgm_conflict",
        0x5e => "prompt.fixed.playback_left",
        0x5f => "prompt.fixed.playback_right",
        0x65 => "prompt.fixed.repeated_tap_enable",
        0x66 => "prompt.fixed.repeated_tap_disable",
        0x70 => "prompt.fixed.multipoint_ldac_confirmation",
        0x71 => "prompt.fixed.multipoint_sound_quality_confirmation",
        0x72 => "prompt.fixed.multipoint_ldac_notice",
        0x73 => "prompt.fixed.multipoint_ldac_990_confirmation",
        0x74 => "prompt.fixed.le_to_classic_sound_quality",
        0x75 => "prompt.fixed.le_to_classic_stable_connection",
        0x76 => "prompt.fixed.classic_sound_quality_disconnect",
        0x77 => "prompt.fixed.classic_stable_connection_disconnect",
        0x86 => "prompt.fixed.stamina_sound_effect",
        0x87 => "prompt.fixed.stamina_sound_effect_lighting_off",
        0x90 => "prompt.fixed.bgm_disable",
        _ => return None,
    })
}

fn flexible_description_key(message: u8) -> Option<&'static str> {
    Some(match message {
        0x00 | 0x07 | 0x0b => "prompt.flexible.battery_consumption",
        0x01 => "prompt.flexible.multipoint_conflict",
        0x08 => "prompt.flexible.bgm_conflict",
        0x09 => "prompt.flexible.equalizer_conflict",
        0x0d => "prompt.flexible.connection_le_restrictions",
        0x11 => "prompt.flexible.le_audio_restrictions",
        _ => return None,
    })
}

fn item_label(item: u8, locale: &str) -> String {
    let key = match item {
        0x00 => "prompt.function.equalizer",
        0x01 => "prompt.function.dsee",
        0x02 => "prompt.function.speak_to_chat",
        0x03 => "prompt.function.automatic_volume",
        0x06 => "prompt.function.ldac",
        0x07 => "prompt.function.sound_quality",
        0x0b => "prompt.function.multipoint",
        0x0d => "prompt.function.bgm",
        0x10 => "prompt.function.le_audio",
        0x14 => "prompt.function.noise_cancelling",
        0x18 => "prompt.function.connection_mode",
        _ => "prompt.function.unsupported",
    };
    translate(locale, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezsc_core::{PromptId, alert::AlertMessage};

    fn prompt(bytes: &[u8]) -> DevicePrompt {
        DevicePrompt {
            id: PromptId {
                generation: 3,
                serial: 7,
            },
            message: AlertMessage::decode_notification(
                ezsc_core::audio::AudioKind::ClassicLePriority
                    .function_id()
                    .table,
                bytes,
            )
            .unwrap(),
        }
    }

    #[test]
    fn priority_prompts_require_an_explicit_choice_and_explain_disconnection() {
        for (message, phrase) in [(0x76, "sound-quality"), (0x77, "stable-connection")] {
            let view = PromptPresentation::from_prompt(&prompt(&[0x99, 0, message, 1]));
            assert!(view.body.contains(phrase));
            assert!(view.body.contains("disconnect"));
            assert_eq!(
                view.actions.iter().map(|a| a.reply).collect::<Vec<_>>(),
                [PromptReply::Reject, PromptReply::Accept]
            );
            assert_eq!(view.escape_reply(), Some(PromptReply::Reject));
        }
    }

    #[test]
    fn display_only_and_positive_only_have_distinct_actions() {
        let display = PromptPresentation::from_prompt(&prompt(&[0x99, 4, 0x08, 0]));
        assert_eq!(
            display.actions,
            [PromptAction {
                label: "Close".into(),
                reply: PromptReply::Dismiss
            }]
        );
        let positive = PromptPresentation::from_prompt(&prompt(&[0x99, 0, 0x08, 2]));
        assert_eq!(
            positive.actions,
            [PromptAction {
                label: "OK".into(),
                reply: PromptReply::Accept
            }]
        );
        assert_eq!(positive.escape_reply(), None);
    }

    #[test]
    fn unknown_and_service_messages_are_read_only() {
        for bytes in [
            &[0x99, 0, 0xff, 1][..],
            &[0x99, 0, 0xff, 0],
            &[0x99, 4, 0x02, 2],
            &[0x99, 2, 0, 1],
        ] {
            let view = PromptPresentation::from_prompt(&prompt(bytes));
            assert!(view.body.contains("SONY | Sound Connect"));
            assert_eq!(view.actions[0].reply, PromptReply::Dismiss);
            assert_eq!(view.actions.len(), 1);
        }
    }

    #[test]
    fn flexible_messages_preserve_affected_functions_and_unknown_restrictions() {
        let known = PromptPresentation::from_prompt(&prompt(&[0x99, 6, 1, 2, 0, 1, 1]));
        assert_eq!(known.affected_functions, ["Equalizer", "DSEE"]);
        assert_eq!(known.actions.len(), 2);
        let unknown = PromptPresentation::from_prompt(&prompt(&[0x99, 6, 1, 3, 0, 0xfe, 0, 1]));
        assert_eq!(unknown.affected_functions.len(), 3);
        assert!(unknown.affected_functions[1].contains("Unsupported"));
        assert_eq!(unknown.actions[0].reply, PromptReply::Dismiss);
        let service = PromptPresentation::from_prompt(&prompt(&[0x99, 6, 2, 1, 0, 1]));
        assert_eq!(service.actions[0].reply, PromptReply::Dismiss);
        let unknown_display = PromptPresentation::from_prompt(&prompt(&[0x99, 6, 1, 1, 0xfe, 0]));
        assert!(unknown_display.body.contains("SONY | Sound Connect"));
        assert_eq!(unknown_display.actions[0].reply, PromptReply::Dismiss);
    }

    #[test]
    fn localization_preserves_all_fixed_message_action_boundaries() {
        for message in 0..=u8::MAX {
            for action in 0..=2 {
                for inquiry in [0, 4] {
                    assert_locale_actions(&prompt(&[0x99, inquiry, message, action]));
                }
            }
        }
    }

    fn assert_locale_actions(message: &DevicePrompt) {
        let english = PromptPresentation::from_prompt(message);
        let chinese = PromptPresentation::from_prompt_localized(message, "zh-CN");
        let fallback = PromptPresentation::from_prompt_localized(message, "unavailable-locale");
        assert_eq!(
            fallback, english,
            "a missing locale must use English presentation"
        );
        assert_eq!(
            english
                .actions
                .iter()
                .map(|action| action.reply)
                .collect::<Vec<_>>(),
            chinese
                .actions
                .iter()
                .map(|action| action.reply)
                .collect::<Vec<_>>()
        );
        assert_eq!(english.escape_reply(), chinese.escape_reply());
        assert!(!chinese.title.is_ascii());
        assert!(!chinese.body.is_ascii());
        for action in &english.actions {
            assert!(action.reply == PromptReply::Dismiss || message.allows(action.reply));
        }
    }

    #[test]
    fn localization_and_fallback_preserve_flexible_message_and_item_boundaries() {
        for value in 0..=u8::MAX {
            for action in 0..=2 {
                // Vary message identity with a known item, then each item with a known message.
                assert_locale_actions(&prompt(&[0x99, 6, value, 1, 0, action]));
                let item_prompt = prompt(&[0x99, 6, 1, 3, 0, value, 0, action]);
                assert_locale_actions(&item_prompt);
                let view = PromptPresentation::from_prompt(&item_prompt);
                assert_eq!(view.affected_functions.len(), 3);
                assert_eq!(view.affected_functions[0], view.affected_functions[2]);
                if let AlertContents::Flexible { items, .. } = item_prompt.message.contents()
                    && !items[1].supports_reply()
                {
                    assert_eq!(view.actions.len(), 1);
                    assert_eq!(view.actions[0].reply, PromptReply::Dismiss);
                }
            }
        }
    }

    #[test]
    fn chinese_priority_messages_explain_connection_changes_without_protocol_details() {
        for (message, phrase) in [(0x76, "音质优先"), (0x77, "稳定连接优先")] {
            let presentation =
                PromptPresentation::from_prompt_localized(&prompt(&[0x99, 0, message, 1]), "zh-CN");
            assert!(presentation.body.contains(phrase));
            assert!(presentation.body.contains("断开连接"));
            assert_eq!(presentation.actions[0].label, "取消");
            assert_eq!(presentation.actions[1].label, "继续");
            assert!(!presentation.body.contains("0x"));
        }
    }

    #[test]
    fn localized_flexible_messages_preserve_unknown_function_restrictions() {
        let known = PromptPresentation::from_prompt_localized(
            &prompt(&[0x99, 6, 1, 2, 0, 0x14, 1]),
            "zh-CN",
        );
        assert_eq!(known.affected_functions, ["均衡器", "降噪"]);
        assert_eq!(known.actions.len(), 2);

        let unknown = PromptPresentation::from_prompt_localized(
            &prompt(&[0x99, 6, 1, 2, 0, 0xff, 1]),
            "zh-CN",
        );
        assert!(unknown.body.contains("不会确认或取消"));
        assert_eq!(unknown.actions[0].reply, PromptReply::Dismiss);
        assert_eq!(unknown.actions[0].label, "关闭");
        assert!(!unknown.affected_functions[1].contains("0xff"));
    }
}
