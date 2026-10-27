//! Read-only headset summary and application actions for native tray menus.

use crate::{
    desktop_integration::{MenuAction, MenuEntry},
    device_status::battery_level,
    preferences::Language,
};
use ezsc_core::{
    ConnectionState, DeviceSnapshot, Freshness,
    audio::{AudioKind, AudioParameterValue, DseeMode},
    common::AudioCodec,
};

pub(super) fn build(snapshot: Option<&DeviceSnapshot>, language: Language) -> Vec<MenuEntry> {
    let model = snapshot.map_or_else(
        || language.text("No headset connected", "未连接耳机"),
        |state| {
            state
                .model_name
                .as_ref()
                .and_then(|name| name.as_utf8().ok())
                .filter(|name| !name.is_empty())
                .unwrap_or(language.text("Your SONY headphones", "你的 SONY 耳机"))
        },
    );
    vec![
        MenuEntry::Info(format!("🎧 {model}")),
        MenuEntry::Info(summary(snapshot, language)),
        MenuEntry::Separator,
        MenuEntry::Action {
            label: language.text("Open main window", "打开主窗口").into(),
            action: MenuAction::OpenMain,
        },
        MenuEntry::Action {
            label: language.text("Quit", "退出").into(),
            action: MenuAction::Quit,
        },
    ]
}

fn summary(snapshot: Option<&DeviceSnapshot>, language: Language) -> String {
    let snapshot = snapshot.filter(|state| {
        !matches!(
            state.connection,
            ConnectionState::Closing | ConnectionState::Disconnected | ConnectionState::Failed
        )
    });
    let level = snapshot.and_then(battery_level);
    let icon = if level.is_some_and(|value| value <= 20) {
        "🪫"
    } else {
        "🔋"
    };
    let battery = level.map_or_else(|| "—".into(), |value| format!("{value}%"));
    let codec = snapshot
        .and_then(|state| state.audio_codec.as_ref())
        .filter(|codec| codec.freshness == Freshness::Current)
        .and_then(|codec| codec.codec)
        .map_or_else(
            || "—".into(),
            |codec| match codec {
                AudioCodec::SBC => "SBC".into(),
                AudioCodec::AAC => "AAC".into(),
                AudioCodec::LDAC => "LDAC".into(),
                AudioCodec::LC3 => "LC3".into(),
                AudioCodec::APT_X => "aptX".into(),
                AudioCodec::APT_X_HD => "aptX HD".into(),
                AudioCodec::UNSETTLED => "—".into(),
                _ => format!(
                    "{} (0x{:02X})",
                    language.text("Unknown", "未知"),
                    codec.as_raw()
                ),
            },
        );
    // This reports the configured Auto mode, not whether DSEE is processing audio.
    let dsee = snapshot.is_some_and(|state| {
        state.audio.iter().any(|entry| {
            matches!(entry.kind, AudioKind::Dsee | AudioKind::DseeWithReasons)
                && entry.freshness == Freshness::Current
                && matches!(
                    entry
                        .parameters
                        .as_ref()
                        .map(|parameters| parameters.value()),
                    Some(AudioParameterValue::Dsee(DseeMode::Auto))
                )
        })
    });
    format!(
        "{icon}{battery} | {codec}{}",
        if dsee { " | DSEE" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_summary_uses_ear_minimum_excludes_case_and_has_inclusive_low_threshold() {
        let mut state = crate::gui_preview::snapshot(true);
        state.batteries[2].reading.as_mut().unwrap().level = 0.try_into().unwrap();
        assert_eq!(
            summary(Some(&state), Language::Chinese),
            "🔋79% | LDAC | DSEE"
        );
        for (level, icon) in [(21, "🔋"), (20, "🪫"), (15, "🪫"), (0, "🪫")] {
            state.batteries[0].reading.as_mut().unwrap().level = level.try_into().unwrap();
            assert_eq!(
                summary(Some(&state), Language::Chinese),
                format!("{icon}{level}% | LDAC | DSEE")
            );
        }
        let state = crate::gui_preview::snapshot(false);
        assert_eq!(
            summary(Some(&state), Language::English),
            "🔋86% | LDAC | DSEE"
        );
    }

    #[test]
    fn missing_and_stale_readings_do_not_become_zero_or_fall_back_to_case() {
        let mut state = crate::gui_preview::snapshot(true);
        state.batteries[0].reading = None;
        assert_eq!(battery_level(&state), Some(79));
        state.batteries[1].freshness = Freshness::Stale;
        state.audio_codec.as_mut().unwrap().freshness = Freshness::Stale;
        state.audio[0].freshness = Freshness::Stale;
        assert_eq!(summary(Some(&state), Language::English), "🔋— | —");
        assert_eq!(summary(None, Language::Chinese), "🔋— | —");
        let mut state = crate::gui_preview::snapshot(true);
        state.connection = ConnectionState::Disconnected;
        assert_eq!(summary(Some(&state), Language::Chinese), "🔋— | —");
    }

    #[test]
    fn dsee_indicator_follows_configured_mode_not_setting_availability() {
        let mut state = crate::gui_preview::snapshot(true);
        state.audio[0].status.as_mut().unwrap().enabled = false;
        assert_eq!(
            summary(Some(&state), Language::English),
            "🔋79% | LDAC | DSEE"
        );
        state.audio[0].parameters = Some(
            ezsc_core::audio::AudioParameters::decode_response(
                libmdr2::types::CommandTable::TABLE_1,
                &[0xe7, 1, 0],
            )
            .unwrap(),
        );
        assert_eq!(summary(Some(&state), Language::English), "🔋79% | LDAC");
    }
}
