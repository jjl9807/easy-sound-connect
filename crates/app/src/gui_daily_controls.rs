//! Device-reported audio and Speak-to-Chat controls.

use crate::{
    Desktop,
    gui_controls::{ControlAction, availability, card, control_button, note},
    gui_operation_feedback::OperationTarget,
};
use ezsc_core::{
    AudioChange, AudioState, ConnectionState, Freshness, SystemChange, SystemState,
    audio::{AudioCapabilityData, AudioKind, AudioParameterValue, DseeMode, PriorityMode},
    system::{DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemKind},
};
use gpui_kit::component::{Disableable, switch::Switch};
use gpui_kit::{Context, Div, div, prelude::*, px};

impl Desktop {
    pub(super) fn render_daily_controls(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(self.render_audio_controls(cx))
            .child(self.render_speak_controls(cx))
    }

    pub(super) fn render_audio_controls(&self, cx: &mut Context<Self>) -> Div {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let (snapshot, blocked) = (
            state.snapshot.clone(),
            state.settings_unavailable() || state.preview,
        );
        let mut cards = div().flex().flex_col().gap(px(16.));
        let Some(snapshot) = snapshot else {
            return cards;
        };
        let ready = snapshot.connection == ConnectionState::Ready;
        let editable = ready && !blocked;
        for (index, entry) in snapshot.audio.iter().enumerate() {
            let title = match entry.kind {
                AudioKind::Dsee | AudioKind::DseeWithReasons => "DSEE",
                AudioKind::ConnectionPriority
                | AudioKind::ConnectionPriorityWithStatus
                | AudioKind::ClassicLePriority
                    if !self.compact =>
                {
                    language.text("Connection quality", "连接质量")
                }
                _ => continue,
            };
            let dsee = matches!(entry.kind, AudioKind::Dsee | AudioKind::DseeWithReasons);
            let mut section = (if dsee
                && matches!(
                    entry
                        .parameters
                        .as_ref()
                        .map(|parameters| parameters.value()),
                    Some(AudioParameterValue::Dsee(_))
                ) {
                crate::gui_style::surface(cx)
            } else {
                card(title, cx)
            })
            .children(
                availability(
                    ready,
                    self.session
                        .read(cx)
                        .operation_feedback_visible(OperationTarget::Audio(entry.kind)),
                    entry.freshness,
                    entry.status.as_ref().is_some_and(|status| status.enabled),
                    language,
                    cx,
                )
                .map(|message| message.when(dsee, |message| message.right(px(76.)))),
            );
            match entry
                .parameters
                .as_ref()
                .map(|parameters| parameters.value())
            {
                Some(AudioParameterValue::Dsee(current)) => {
                    let kind = entry.kind;
                    let change = AudioChange::Dsee(if *current == DseeMode::Auto {
                        DseeMode::Off
                    } else {
                        DseeMode::Auto
                    });
                    let enabled = editable && audio_change_ready(entry, change);
                    let session = self.session.clone();
                    section = section.child(crate::gui_style::setting_heading(
                        "DSEE",
                        Switch::new(("dsee", index))
                            .accessibility_label(language.text("DSEE Auto", "DSEE 自动"))
                            .checked(*current == DseeMode::Auto)
                            .disabled(!enabled)
                            .on_change(move |value, _, cx| {
                                session.update(cx, |session, cx| {
                                    session.submit_control(
                                        ControlAction::Audio(
                                            kind,
                                            AudioChange::Dsee(if *value {
                                                DseeMode::Auto
                                            } else {
                                                DseeMode::Off
                                            }),
                                        ),
                                        "DSEE".into(),
                                    );
                                    cx.notify();
                                })
                            }),
                        cx,
                    ));
                }
                Some(AudioParameterValue::ConnectionPriority { priority, .. }) => {
                    if !priority_choices(entry)
                        .iter()
                        .any(|choice| choice.value == *priority)
                    {
                        section = section.child(note(priority_label(*priority), cx));
                    }
                    let mut choices = div().flex().flex_wrap().gap(px(8.));
                    for (slot, choice) in priority_choices(entry).into_iter().enumerate() {
                        let change = AudioChange::Priority(choice.value);
                        let label = if choice.value == PriorityMode::SOUND_QUALITY {
                            language.text("Sound quality", "音质优先").to_owned()
                        } else if choice.value == PriorityMode::CONNECTION_QUALITY {
                            language
                                .text("Stable connection", "稳定连接优先")
                                .to_owned()
                        } else {
                            choice.label
                        };
                        choices = choices.child(control_button(
                            ("connection-quality", index * 256 + slot),
                            label,
                            editable && choice.selectable && audio_change_ready(entry, change),
                            *priority == choice.value,
                            ControlAction::Audio(entry.kind, change),
                            self.session.clone(),
                            cx,
                        ));
                    }
                    section = section.child(choices).child(note(
                        if is_classic_priority(*priority) {
                            language.text(
                                "Your headset may ask for confirmation or reconnect.",
                                "耳机可能要求确认或重新连接。",
                            )
                        } else {
                            language.text(
                                "Quality controls are available in Classic Audio mode.",
                                "连接质量选项仅在经典音频模式下可用。",
                            )
                        },
                        cx,
                    ));
                }
                _ => {
                    section = section.child(note(
                        language.text("Waiting for headset values…", "正在读取耳机状态…"),
                        cx,
                    ))
                }
            }
            cards = cards.child(section);
        }
        cards
    }

    pub(super) fn render_speak_controls(&self, cx: &mut Context<Self>) -> Div {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let (snapshot, blocked) = (
            state.snapshot.clone(),
            state.settings_unavailable() || state.preview,
        );
        let mut cards = div().flex().flex_col().gap(px(16.));
        let Some(snapshot) = snapshot else {
            return cards;
        };
        let ready = snapshot.connection == ConnectionState::Ready;
        let editable = ready && !blocked;
        for (index, entry) in snapshot.system.iter().enumerate() {
            if !matches!(
                entry.kind,
                SystemKind::SpeakToChatType1 | SystemKind::SpeakToChatType2
            ) {
                continue;
            }
            let kind = entry.kind;
            let waiting = self
                .session
                .read(cx)
                .operation_feedback_visible(OperationTarget::System(kind));
            let parameters = entry
                .parameters
                .filter(|parameters| parameters.kind() == kind)
                .and_then(|parameters| parameters.preview().map(|preview| (parameters, preview)));
            let mut section = (if parameters.is_some() {
                crate::gui_style::surface(cx)
            } else {
                card(language.text("Speak-to-Chat", "智能免摘对话"), cx)
            })
            .children(
                availability(
                    ready,
                    waiting,
                    entry.freshness,
                    entry.status.is_some_and(|status| status.enabled),
                    language,
                    cx,
                )
                .map(|message| message.when(waiting, |message| message.right(px(76.)))),
            );
            if let Some((parameters, preview)) = parameters {
                let change = SystemChange::Enabled(!parameters.enabled());
                let enabled = editable && system_change_ready(entry, change);
                let session = self.session.clone();
                let label = language.text("Speak-to-Chat", "智能免摘对话");
                section = section.child(crate::gui_style::setting_heading(
                    label,
                    Switch::new(("speak-enabled", index))
                        .accessibility_label(label)
                        .checked(parameters.enabled())
                        .disabled(!enabled)
                        .on_change(move |value, _, cx| {
                            session.update(cx, |session, cx| {
                                session.submit_control(
                                    ControlAction::System(kind, SystemChange::Enabled(*value)),
                                    label.into(),
                                );
                                cx.notify();
                            })
                        }),
                    cx,
                ));
                if self.compact {
                    cards = cards.child(section);
                    continue;
                }
                let preview_supported = entry
                    .capability
                    .as_ref()
                    .and_then(|capability| capability.speak_to_chat())
                    .is_some_and(|capability| capability.preview_supported);
                if !preview_supported && preview {
                    section = section.child(note(
                        language.text(
                            "Stop preview before changing this setting.",
                            "请先停止试听，再更改此设置。",
                        ),
                        cx,
                    ));
                }
                if preview_supported || preview {
                    let change = SystemChange::Preview(!preview);
                    section = section.child(
                        control_button(
                            ("speak-preview", index),
                            if preview {
                                language.text("Stop preview", "停止试听")
                            } else {
                                language.text("Preview", "试听")
                            },
                            editable && system_change_ready(entry, change),
                            preview,
                            ControlAction::System(kind, change),
                            self.session.clone(),
                            cx,
                        )
                        .self_start(),
                    );
                }
            } else {
                section = section.child(note(
                    language.text("Waiting for headset values…", "正在读取耳机状态…"),
                    cx,
                ));
            }
            if self.compact {
                cards = cards.child(section);
                continue;
            }
            if ready
                && entry.freshness == Freshness::Current
                && entry.status.and_then(|status| status.talking) == Some(true)
            {
                section =
                    section.child(note(language.text("Conversation detected", "正在对话"), cx));
            }
            let Some(settings) = entry
                .extended
                .filter(|settings| settings.kind() == kind)
                .and_then(SpeakPresentation::from_settings)
            else {
                cards = cards.child(section.child(note(
                    language.text(
                        "Detection settings have not been read yet.",
                        "尚未读取检测设置。",
                    ),
                    cx,
                )));
                continue;
            };
            if entry.extended_freshness != entry.freshness {
                section = section.children(availability(
                    ready,
                    false,
                    entry.extended_freshness,
                    entry.status.is_some_and(|status| status.enabled),
                    language,
                    cx,
                ));
            }
            section = section.child(crate::gui_style::divider(cx));
            let mut sensitivity = div().flex().flex_wrap().items_center().gap(px(8.));
            for (slot, value, label) in [
                (0, DetectionSensitivity::Auto, language.text("Auto", "自动")),
                (1, DetectionSensitivity::High, language.text("High", "高")),
                (2, DetectionSensitivity::Low, language.text("Low", "低")),
            ] {
                let change = SystemChange::Sensitivity(value);
                sensitivity = sensitivity.child(control_button(
                    ("speak-sensitivity", index * 3 + slot),
                    label,
                    editable && system_change_ready(entry, change),
                    settings.sensitivity == value,
                    ControlAction::System(kind, change),
                    self.session.clone(),
                    cx,
                ));
            }
            section = section.child(crate::gui_style::setting_row(
                language.text("Sensitivity", "灵敏度"),
                sensitivity,
                cx,
            ));
            if let Some(voice_focus) = settings.voice_focus {
                let enabled =
                    editable && system_change_ready(entry, SystemChange::VoiceFocus(!voice_focus));
                let session = self.session.clone();
                let label = language.text("Voice focus", "语音聚焦");
                section = section.child(crate::gui_style::setting_row(
                    label,
                    Switch::new(("speak-voice", index))
                        .accessibility_label(label)
                        .checked(voice_focus)
                        .disabled(!enabled)
                        .on_change(move |value, _, cx| {
                            session.update(cx, |session, cx| {
                                session.submit_control(
                                    ControlAction::System(kind, SystemChange::VoiceFocus(*value)),
                                    label.into(),
                                );
                                cx.notify();
                            })
                        }),
                    cx,
                ));
            }
            let mut end = div().flex().flex_wrap().items_center().gap(px(8.));
            for (slot, value, label) in [
                (0, ModeOutTime::Fast, language.text("Fast", "短")),
                (1, ModeOutTime::Mid, language.text("Medium", "中")),
                (2, ModeOutTime::Slow, language.text("Slow", "长")),
                (3, ModeOutTime::None, language.text("Never", "不自动结束")),
            ] {
                let change = SystemChange::ModeOut(value);
                end = end.child(control_button(
                    ("speak-end", index * 4 + slot),
                    label,
                    editable && system_change_ready(entry, change),
                    settings.mode_out == value,
                    ControlAction::System(kind, change),
                    self.session.clone(),
                    cx,
                ));
            }
            cards = cards.child(
                section.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(note(language.text("Automatic end", "自动结束"), cx))
                        .child(end),
                ),
            );
        }
        cards
    }
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
