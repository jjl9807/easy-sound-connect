//! Device-reported audio and Speak-to-Chat controls.

use ezsc_core::{
    AudioChange, AudioState, ConnectionState, Freshness, SystemChange, SystemState,
    audio::{AudioCapabilityData, AudioKind, AudioParameterValue, DseeMode, PriorityMode},
    system::{DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemKind},
};
use gpui::{Context, Div, ElementId, SharedString, Stateful, div, prelude::*};

use crate::{
    Desktop,
    gui_controls::{ControlAction, action_button, availability, card, note},
};

impl Desktop {
    pub(super) fn render_daily_controls(&self, cx: &mut Context<Self>) -> Div {
        let mut cards = div().flex().flex_col().flex_shrink_0().gap_4().pb_6();
        let Some(snapshot) = &self.snapshot else {
            return cards;
        };
        let ready = snapshot.connection == ConnectionState::Ready;
        let editable = ready && !self.settings_blocked();
        for (index, entry) in snapshot.audio.iter().enumerate() {
            let title = match entry.kind {
                AudioKind::Dsee | AudioKind::DseeWithReasons => "DSEE",
                AudioKind::ConnectionPriority
                | AudioKind::ConnectionPriorityWithStatus
                | AudioKind::ClassicLePriority => "Bluetooth connection quality",
                _ => continue,
            };
            let mut section = card(title).child(availability(
                ready,
                self.busy,
                entry.freshness,
                entry.status.as_ref().is_some_and(|status| status.enabled),
            ));
            let mut options = div().flex().flex_wrap().gap_2();
            match entry
                .parameters
                .as_ref()
                .map(|parameters| parameters.value())
            {
                Some(AudioParameterValue::Dsee(current)) => {
                    section = section.child(note(
                        "Auto allows the headset to use DSEE when applicable; this setting does not report whether processing is active.",
                    ));
                    for (choice_index, (value, label)) in
                        [(DseeMode::Auto, "Auto"), (DseeMode::Off, "Off")]
                            .into_iter()
                            .enumerate()
                    {
                        let change = AudioChange::Dsee(value);
                        options = options.child(choice_button(
                            ("dsee", index * 2 + choice_index),
                            label,
                            format!("DSEE {label}"),
                            ControlAction::Audio(entry.kind, change),
                            editable && audio_change_ready(entry, change),
                            *current == value,
                            cx,
                        ));
                    }
                }
                Some(AudioParameterValue::ConnectionPriority { priority, .. }) => {
                    section = section
                        .child(format!("Current setting: {}", priority_label(*priority)))
                        .child(note(if is_classic_priority(*priority) {
                            "Changing Bluetooth connection quality may interrupt the control connection. Release control and reconnect if it ends."
                        } else {
                            "These controls are available in Classic Audio. Use SONY | Sound Connect to change the headset connection mode."
                        }));
                    for (choice_index, choice) in priority_choices(entry).into_iter().enumerate() {
                        let change = AudioChange::Priority(choice.value);
                        options = options.child(choice_button(
                            ("connection-priority", index * 256 + choice_index),
                            choice.label.clone(),
                            format!("Bluetooth connection quality: {}", choice.label),
                            ControlAction::Audio(entry.kind, change),
                            editable && choice.selectable && audio_change_ready(entry, change),
                            *priority == choice.value,
                            cx,
                        ));
                    }
                }
                _ => {
                    section = section.child(note("Audio-control values have not been read yet."));
                }
            }
            cards = cards.child(section.child(options));
        }
        for (index, entry) in snapshot.system.iter().enumerate() {
            if !matches!(
                entry.kind,
                SystemKind::SpeakToChatType1 | SystemKind::SpeakToChatType2
            ) {
                continue;
            }
            let mut section = card("Speak-to-Chat").child(availability(
                ready,
                self.busy,
                entry.freshness,
                entry.status.is_some_and(|status| status.enabled),
            ));
            if let Some(parameters) = entry
                .parameters
                .filter(|parameters| parameters.kind() == entry.kind)
                && let Some(preview) = parameters.preview()
            {
                let mut switches = div().flex().flex_wrap().gap_2();
                for (value, label) in [(true, "On"), (false, "Off")] {
                    let change = SystemChange::Enabled(value);
                    switches = switches.child(choice_button(
                        ("speak-enabled", index * 2 + usize::from(value)),
                        label,
                        format!("Speak-to-Chat {label}"),
                        ControlAction::System(entry.kind, change),
                        editable && system_change_ready(entry, change),
                        parameters.enabled() == value,
                        cx,
                    ));
                }
                section = section.child(switches);
                let preview_supported = entry
                    .capability
                    .as_ref()
                    .and_then(|capability| capability.speak_to_chat())
                    .is_some_and(|capability| capability.preview_supported);
                if !preview_supported && preview {
                    section = section.child(note("Stop preview before changing Speak-to-Chat."));
                }
                if preview_supported || preview {
                    let change = SystemChange::Preview(!preview);
                    let label = if preview {
                        "Stop preview"
                    } else {
                        "Start preview"
                    };
                    section = section.child(choice_button(
                        ("speak-preview", index),
                        label,
                        format!("Speak-to-Chat: {label}"),
                        ControlAction::System(entry.kind, change),
                        editable && system_change_ready(entry, change),
                        preview,
                        cx,
                    ));
                }
            } else {
                section = section.child(note("Speak-to-Chat switches have not been read yet."));
            }
            // The live talking effect is not the configured on/off switch.
            section = section.child(note(match entry.status.and_then(|status| status.talking) {
                Some(true) if ready && entry.freshness == Freshness::Current => {
                    "Talking effect: active."
                }
                Some(false) if ready && entry.freshness == Freshness::Current => {
                    "Talking effect: inactive."
                }
                Some(true) => "Last reported talking effect: active; state needs to be refreshed.",
                Some(false) => {
                    "Last reported talking effect: inactive; state needs to be refreshed."
                }
                None => "Talking effect has not been read yet.",
            }));
            section = section
                .child(div().child("Detection settings"))
                .child(availability(
                    ready,
                    self.busy,
                    entry.extended_freshness,
                    entry.status.is_some_and(|status| status.enabled),
                ));
            let Some(settings) = entry
                .extended
                .filter(|settings| settings.kind() == entry.kind)
                .and_then(SpeakPresentation::from_settings)
            else {
                cards =
                    cards.child(section.child(note("Detection settings have not been read yet.")));
                continue;
            };
            let mut sensitivity = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child("Sensitivity:");
            for (choice_index, (value, label)) in [
                (DetectionSensitivity::Auto, "Auto"),
                (DetectionSensitivity::High, "High"),
                (DetectionSensitivity::Low, "Low"),
            ]
            .into_iter()
            .enumerate()
            {
                let change = SystemChange::Sensitivity(value);
                sensitivity = sensitivity.child(choice_button(
                    ("speak-sensitivity", index * 3 + choice_index),
                    label,
                    format!("Speak-to-Chat sensitivity: {label}"),
                    ControlAction::System(entry.kind, change),
                    editable && system_change_ready(entry, change),
                    settings.sensitivity == value,
                    cx,
                ));
            }
            section = section.child(sensitivity);
            if let Some(voice_focus) = settings.voice_focus {
                let change = SystemChange::VoiceFocus(!voice_focus);
                let label = if voice_focus {
                    "Voice focus on"
                } else {
                    "Voice focus off"
                };
                section = section.child(choice_button(
                    ("speak-voice-focus", index),
                    label,
                    format!(
                        "Speak-to-Chat voice focus {}",
                        if voice_focus { "off" } else { "on" }
                    ),
                    ControlAction::System(entry.kind, change),
                    editable && system_change_ready(entry, change),
                    voice_focus,
                    cx,
                ));
            }
            let mut mode_out = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child("Automatic end:");
            for (choice_index, (value, label)) in [
                (ModeOutTime::Fast, "Fast"),
                (ModeOutTime::Mid, "Medium"),
                (ModeOutTime::Slow, "Slow"),
                (ModeOutTime::None, "No automatic end"),
            ]
            .into_iter()
            .enumerate()
            {
                let change = SystemChange::ModeOut(value);
                mode_out = mode_out.child(choice_button(
                    ("speak-end", index * 4 + choice_index),
                    label,
                    format!("Speak-to-Chat automatic end: {label}"),
                    ControlAction::System(entry.kind, change),
                    editable && system_change_ready(entry, change),
                    settings.mode_out == value,
                    cx,
                ));
            }
            cards = cards.child(section.child(mode_out));
        }
        cards
    }
}

fn choice_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    operation_label: String,
    action: ControlAction,
    enabled: bool,
    selected: bool,
    cx: &mut Context<Desktop>,
) -> Stateful<Div> {
    action_button(id, label, enabled, selected).on_click(cx.listener(move |view, _, _, cx| {
        if enabled {
            view.submit_control(action, operation_label.clone());
            cx.notify();
        }
    }))
}

fn priority_label(value: PriorityMode) -> String {
    match value {
        PriorityMode::SOUND_QUALITY => "Sound quality".into(),
        PriorityMode::CONNECTION_QUALITY => "Stable connection".into(),
        PriorityMode::LOW_LATENCY => "LE Audio".into(),
        _ => format!("Unsupported priority 0x{:02X}", value.as_raw()),
    }
}

fn is_classic_priority(value: PriorityMode) -> bool {
    matches!(
        value,
        PriorityMode::SOUND_QUALITY | PriorityMode::CONNECTION_QUALITY
    )
}

struct PriorityChoice {
    value: PriorityMode,
    label: String,
    selectable: bool,
}

fn priority_choices(entry: &AudioState) -> Vec<PriorityChoice> {
    let Some(AudioCapabilityData::Connection { priorities, .. }) = entry
        .capability
        .as_ref()
        .filter(|capability| capability.kind() == entry.kind)
        .map(|capability| capability.data())
    else {
        return Vec::new();
    };
    priorities
        .iter()
        .filter(|value| is_classic_priority(**value))
        .map(|value| PriorityChoice {
            value: *value,
            label: priority_label(*value),
            selectable: priorities
                .iter()
                .filter(|candidate| *candidate == value)
                .count()
                == 1,
        })
        .collect()
}

pub(super) fn audio_change_ready(entry: &AudioState, change: AudioChange) -> bool {
    if entry.freshness != Freshness::Current
        || !entry
            .status
            .as_ref()
            .is_some_and(|status| status.kind() == entry.kind && status.enabled)
        || !entry
            .capability
            .as_ref()
            .is_some_and(|capability| capability.kind() == entry.kind)
        || !entry
            .parameters
            .as_ref()
            .is_some_and(|parameters| parameters.kind() == entry.kind)
    {
        return false;
    }
    match change {
        AudioChange::Dsee(_) => {
            matches!(entry.kind, AudioKind::Dsee | AudioKind::DseeWithReasons)
                && matches!(
                    entry
                        .capability
                        .as_ref()
                        .map(|capability| capability.data()),
                    Some(AudioCapabilityData::Dsee(_))
                )
                && matches!(
                    entry
                        .parameters
                        .as_ref()
                        .map(|parameters| parameters.value()),
                    Some(AudioParameterValue::Dsee(_))
                )
        }
        AudioChange::Priority(value) => {
            matches!(
                entry.kind,
                AudioKind::ConnectionPriority
                    | AudioKind::ConnectionPriorityWithStatus
                    | AudioKind::ClassicLePriority
            ) && matches!(
                entry
                    .parameters
                    .as_ref()
                    .map(|parameters| parameters.value()),
                Some(AudioParameterValue::ConnectionPriority { priority, .. })
                    if is_classic_priority(*priority)
            ) && priority_choices(entry)
                .iter()
                .any(|choice| choice.value == value && choice.selectable)
        }
    }
}

pub(super) fn system_change_ready(entry: &SystemState, change: SystemChange) -> bool {
    if !matches!(
        entry.kind,
        SystemKind::SpeakToChatType1 | SystemKind::SpeakToChatType2
    ) || !entry
        .status
        .is_some_and(|status| status.kind() == entry.kind && status.enabled)
    {
        return false;
    }
    let Some(capability) = entry
        .capability
        .as_ref()
        .filter(|capability| capability.kind() == entry.kind)
        .and_then(|capability| capability.speak_to_chat())
    else {
        return false;
    };
    let regular_ready = entry.freshness == Freshness::Current
        && entry.parameters.is_some_and(|parameters| {
            parameters.kind() == entry.kind && parameters.preview().is_some()
        });
    match change {
        SystemChange::Enabled(_) => {
            regular_ready
                && (capability.preview_supported
                    || entry.parameters.and_then(|parameters| parameters.preview()) == Some(false))
        }
        SystemChange::Preview(value) => regular_ready && (!value || capability.preview_supported),
        SystemChange::Sensitivity(_) | SystemChange::ModeOut(_) | SystemChange::VoiceFocus(_) => {
            entry.extended_freshness == Freshness::Current
                && entry
                    .extended
                    .is_some_and(|settings| settings.kind() == entry.kind)
                && (!matches!(change, SystemChange::VoiceFocus(_))
                    || entry.kind == SystemKind::SpeakToChatType1)
        }
    }
}

struct SpeakPresentation {
    sensitivity: DetectionSensitivity,
    voice_focus: Option<bool>,
    mode_out: ModeOutTime,
}

impl SpeakPresentation {
    fn from_settings(settings: SpeakToChatSettings) -> Option<Self> {
        match settings {
            SpeakToChatSettings::Type1 {
                sensitivity,
                voice_focus,
                mode_out,
            } => Some(Self {
                sensitivity,
                voice_focus: Some(voice_focus),
                mode_out,
            }),
            SpeakToChatSettings::Type2 {
                sensitivity,
                mode_out,
            } => Some(Self {
                sensitivity,
                voice_focus: None,
                mode_out,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezsc_core::{
        audio::{AudioCapability, AudioParameters, AudioStatus},
        system::{SystemCapability, SystemParameters, SystemStatus},
    };

    #[test]
    fn bluetooth_quality_offers_only_advertised_classic_choices() {
        let kind = AudioKind::ClassicLePriority;
        let table = kind.function_id().table;
        let mut entry = AudioState {
            kind,
            capability: Some(
                AudioCapability::decode_response(table, &[0xe1, 5, 3, 2, 0x77, 0, 0]).unwrap(),
            ),
            status: Some(AudioStatus::decode_response(table, &[0xe3, 5, 0, 1]).unwrap()),
            parameters: Some(AudioParameters::decode_response(table, &[0xe7, 5, 0]).unwrap()),
            freshness: Freshness::Current,
        };
        let choices = priority_choices(&entry);
        assert_eq!(
            choices
                .iter()
                .map(|choice| choice.value.as_raw())
                .collect::<Vec<_>>(),
            [0]
        );
        assert!(choices[0].selectable);
        assert_eq!(choices[0].label, "Sound quality");
        assert!(audio_change_ready(
            &entry,
            AudioChange::Priority(PriorityMode::SOUND_QUALITY)
        ));
        assert!(!audio_change_ready(
            &entry,
            AudioChange::Priority(PriorityMode::LOW_LATENCY)
        ));
        assert!(!audio_change_ready(
            &entry,
            AudioChange::Priority(PriorityMode::CONNECTION_QUALITY)
        ));
        assert!(!audio_change_ready(
            &entry,
            AudioChange::Priority(PriorityMode::from_raw(0x77))
        ));
        entry.capability =
            Some(AudioCapability::decode_response(table, &[0xe1, 5, 4, 2, 1, 0x77, 0, 0]).unwrap());
        assert_eq!(
            priority_choices(&entry)
                .iter()
                .map(|choice| choice.value.as_raw())
                .collect::<Vec<_>>(),
            [1, 0]
        );
        for current in [PriorityMode::LOW_LATENCY, PriorityMode::from_raw(0x77)] {
            entry.parameters = Some(
                AudioParameters::decode_response(table, &[0xe7, 5, current.as_raw()]).unwrap(),
            );
            for target in [
                PriorityMode::SOUND_QUALITY,
                PriorityMode::CONNECTION_QUALITY,
            ] {
                assert!(!audio_change_ready(&entry, AudioChange::Priority(target)));
            }
            assert!(matches!(entry.parameters.as_ref().unwrap().value(),
                AudioParameterValue::ConnectionPriority { priority, .. } if *priority == current
            ));
        }
        entry.parameters = Some(AudioParameters::decode_response(table, &[0xe7, 5, 0]).unwrap());
        entry.freshness = Freshness::Stale;
        assert!(!audio_change_ready(
            &entry,
            AudioChange::Priority(PriorityMode::SOUND_QUALITY)
        ));
    }

    #[test]
    fn speak_banks_have_independent_freshness_and_unsupported_preview_can_be_stopped() {
        let kind = SystemKind::SpeakToChatType1;
        let table = kind.function_id().table;
        let mut entry = SystemState {
            kind,
            capability: Some(
                SystemCapability::decode_response(table, &[0xf1, 2, 0, 90, 0, 8]).unwrap(),
            ),
            status: Some(SystemStatus::decode_response(table, &[0xf3, 2, 0, 0]).unwrap()),
            parameters: Some(SystemParameters::SpeakToChatType1 {
                enabled: false,
                preview: true,
            }),
            freshness: Freshness::Current,
            extended: Some(SpeakToChatSettings::Type1 {
                sensitivity: DetectionSensitivity::Low,
                voice_focus: true,
                mode_out: ModeOutTime::Slow,
            }),
            extended_freshness: Freshness::Stale,
        };
        assert!(!system_change_ready(&entry, SystemChange::Enabled(true)));
        assert!(!system_change_ready(&entry, SystemChange::Enabled(false)));
        assert!(system_change_ready(&entry, SystemChange::Preview(false)));
        assert!(!system_change_ready(&entry, SystemChange::Preview(true)));
        assert!(!system_change_ready(
            &entry,
            SystemChange::Sensitivity(DetectionSensitivity::Auto)
        ));
        entry.parameters = Some(SystemParameters::SpeakToChatType1 {
            enabled: false,
            preview: false,
        });
        assert!(system_change_ready(&entry, SystemChange::Enabled(true)));
        entry.freshness = Freshness::Stale;
        entry.extended_freshness = Freshness::Current;
        assert!(!system_change_ready(&entry, SystemChange::Enabled(true)));
        assert!(system_change_ready(
            &entry,
            SystemChange::Sensitivity(DetectionSensitivity::Auto)
        ));
        let presentation = SpeakPresentation::from_settings(entry.extended.unwrap()).unwrap();
        assert_eq!(presentation.voice_focus, Some(true));
        assert_eq!(presentation.mode_out, ModeOutTime::Slow);
        assert!(system_change_ready(
            &entry,
            SystemChange::ModeOut(ModeOutTime::None)
        ));
    }

    #[test]
    fn speak_type_two_does_not_offer_type_one_voice_focus() {
        let kind = SystemKind::SpeakToChatType2;
        let table = kind.function_id().table;
        let entry = SystemState {
            kind,
            capability: Some(
                SystemCapability::decode_response(table, &[0xf1, 12, 1, 0, 0, 0]).unwrap(),
            ),
            status: Some(SystemStatus::decode_response(table, &[0xf3, 12, 0, 1]).unwrap()),
            parameters: Some(SystemParameters::SpeakToChatType2 {
                enabled: true,
                preview: false,
            }),
            freshness: Freshness::Current,
            extended: Some(SpeakToChatSettings::Type2 {
                sensitivity: DetectionSensitivity::Auto,
                mode_out: ModeOutTime::None,
            }),
            extended_freshness: Freshness::Current,
        };
        let presentation = SpeakPresentation::from_settings(entry.extended.unwrap()).unwrap();
        assert_eq!(presentation.voice_focus, None);
        assert!(!system_change_ready(&entry, SystemChange::VoiceFocus(true)));
        assert!(system_change_ready(
            &entry,
            SystemChange::ModeOut(ModeOutTime::Fast)
        ));
    }
}
