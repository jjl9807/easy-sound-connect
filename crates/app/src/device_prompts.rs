//! User-facing descriptions and explicit actions for device messages.

use ezsc_core::{
    DevicePrompt, PromptReply,
    alert::{AlertActionType, AlertContents},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptAction {
    pub label: &'static str,
    pub reply: PromptReply,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptPresentation {
    pub title: &'static str,
    pub body: &'static str,
    pub affected_functions: Vec<String>,
    pub actions: Vec<PromptAction>,
}

impl PromptPresentation {
    pub fn from_prompt(prompt: &DevicePrompt) -> Self {
        let (description, action, affected_functions, known_scope) = match prompt.message.contents()
        {
            AlertContents::Fixed { message, action } => (
                fixed_description(message.as_raw()),
                Some(*action),
                Vec::new(),
                message.supports_reply(),
            ),
            AlertContents::Flexible {
                message,
                items,
                action,
            } => (
                flexible_description(message.as_raw()),
                Some(*action),
                items.iter().map(|item| item_label(item.as_raw())).collect(),
                message.supports_reply() && items.iter().all(|item| item.supports_reply()),
            ),
            _ => (None, None, Vec::new(), false),
        };
        let supported = known_scope
            && description.is_some()
            && (prompt.allows(PromptReply::Accept)
                || prompt.allows(PromptReply::Reject)
                || action == Some(AlertActionType::ConfirmationOnly));
        let body = if supported {
            description.unwrap_or_default()
        } else {
            "This device request cannot be completed here. Use SONY | Sound Connect to review it. Closing this message does not accept or cancel the request on the headset."
        };
        let actions = if supported && action != Some(AlertActionType::ConfirmationOnly) {
            let mut actions = Vec::new();
            if prompt.allows(PromptReply::Reject) {
                actions.push(PromptAction {
                    label: "Cancel",
                    reply: PromptReply::Reject,
                });
            }
            if prompt.allows(PromptReply::Accept) {
                actions.push(PromptAction {
                    label: if action == Some(AlertActionType::ConfirmWithReply) {
                        "OK"
                    } else {
                        "Continue"
                    },
                    reply: PromptReply::Accept,
                });
            }
            actions
        } else {
            vec![PromptAction {
                label: "Close",
                reply: PromptReply::Dismiss,
            }]
        };
        Self {
            title: if supported && action != Some(AlertActionType::ConfirmationOnly) {
                "Headset confirmation"
            } else {
                "Headset message"
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

fn fixed_description(message: u8) -> Option<&'static str> {
    Some(match message {
        0x00 => {
            "Changing Bluetooth connection quality will disconnect the headset. Reconnect afterward if needed."
        }
        0x01 => {
            "Changing the button assignment will disconnect the headset. Reconnect afterward if needed."
        }
        0x06 => {
            "Changing the two-device connection setting will disable LDAC and disconnect the headset."
        }
        0x07 => "Changing the two-device connection setting will disconnect the headset.",
        0x08 => "Using the equalizer and DSEE together increases battery consumption.",
        0x09 => {
            "Playback control is already assigned to another button or touch sensor. This change will duplicate that assignment."
        }
        0x0a => "The touch-sensor control panel will be disabled.",
        0x0b => "The touch-sensor control panel will be disabled and the headset will reconnect.",
        0x0c => "The touch-sensor control panel will be enabled.",
        0x0d => "The touch-sensor control panel will be enabled and the headset will reconnect.",
        0x1c => "The headset requests confirmation to select sound-quality priority.",
        0x2a => "LDAC is unavailable in the current sound-quality-priority configuration.",
        0x2b => "Using these sound functions together increases battery consumption.",
        0x30 => "Changing to Classic Bluetooth audio will disconnect the headset.",
        0x53 => {
            "The headset requests confirmation for the LDAC 990 kbps setting. Audio may be interrupted when the connection is unstable."
        }
        0x59 => "Automatic volume adjustment will be enabled.",
        0x5a => "Automatic volume adjustment will be enabled while safe-volume control is on.",
        0x5b => {
            "Automatic volume adjustment will be enabled while the safe-volume limit is active."
        }
        0x5c => {
            "The equalizer and BGM mode cannot be used together. Changing the equalizer will affect BGM mode."
        }
        0x5e => "Playback control will be assigned to the left side of the headset.",
        0x5f => "Playback control will be assigned to the right side of the headset.",
        0x65 => "This button assignment will enable repeated-tap controls.",
        0x66 => "This button assignment will disable repeated-tap controls.",
        0x70 => "The headset requests confirmation to use LDAC with two devices connected.",
        0x71 => {
            "The headset requests confirmation to select sound-quality priority with two devices connected."
        }
        0x72 => "Two devices are connected while LDAC is in use.",
        0x73 => {
            "The headset requests confirmation to use LDAC 990 kbps with two devices connected. Audio may be interrupted when the connection is unstable."
        }
        0x74 => {
            "Switching from LE Audio to Classic sound-quality priority will disconnect the headset. Reconnect afterward if needed."
        }
        0x75 => {
            "Switching from LE Audio to Classic stable-connection priority will disconnect the headset. Reconnect afterward if needed."
        }
        0x76 => {
            "Switching to sound-quality priority will disconnect the headset. Reconnect afterward if needed."
        }
        0x77 => {
            "Switching to stable-connection priority will disconnect the headset. Reconnect afterward if needed."
        }
        0x86 => {
            "A sound effect will be enabled while stamina mode is on. This may increase battery consumption."
        }
        0x87 => {
            "A sound effect will be enabled while stamina mode is on and lighting is off. This may increase battery consumption."
        }
        0x90 => "BGM mode will be disabled.",
        _ => return None,
    })
}

fn flexible_description(message: u8) -> Option<&'static str> {
    Some(match message {
        0x00 | 0x07 | 0x0b => {
            "Using the following functions together increases battery consumption."
        }
        0x01 => {
            "The following functions cannot be used with a two-device connection. Continuing will change their availability."
        }
        0x08 => {
            "The following functions cannot be used with BGM mode. Continuing will change their availability."
        }
        0x09 => {
            "The following functions cannot be used with the equalizer. Continuing will change their availability."
        }
        0x0d => {
            "Changing the connection affects the following functions. They may become unavailable with LE Audio."
        }
        0x11 => {
            "Changing to LE Audio will restrict the following functions and may disconnect the headset."
        }
        _ => return None,
    })
}

fn item_label(item: u8) -> String {
    match item {
        0x00 => "Equalizer",
        0x01 => "DSEE",
        0x02 => "Speak-to-Chat",
        0x03 => "Automatic volume adjustment",
        0x06 => "LDAC",
        0x07 => "Sound-quality priority",
        0x0b => "Two-device connection",
        0x0d => "BGM mode",
        0x10 => "LE Audio",
        0x14 => "Noise cancelling",
        0x18 => "Connection mode",
        _ => return "Unsupported function — review in SONY | Sound Connect".into(),
    }
    .into()
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
                label: "Close",
                reply: PromptReply::Dismiss
            }]
        );
        let positive = PromptPresentation::from_prompt(&prompt(&[0x99, 0, 0x08, 2]));
        assert_eq!(
            positive.actions,
            [PromptAction {
                label: "OK",
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
}
