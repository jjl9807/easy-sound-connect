//! Device status artwork selected from reported state, never from model names.

use crate::preferences::Language;
use ezsc_core::{BatteryComponent, DeviceSnapshot, Freshness};
use gpui_kit::component::{ActiveTheme, tooltip::Tooltip};
use gpui_kit::{assets::IconName, prelude::*, *};
use libmdr2::{
    audio::{AudioCapabilityData, AudioKind, AudioParameterValue, DseeMode, UpscalingType},
    battery::{BatteryReading, ChargingStatus},
    common::AudioCodec,
};

fn themed_asset(stem: &str, extension: &str, cx: &App) -> String {
    let theme = if cx.theme().is_dark() {
        "dark"
    } else {
        "light"
    };
    format!("{stem}_{theme}.{extension}")
}

fn battery_icon(reading: Option<BatteryReading>) -> IconName {
    match reading {
        Some(reading) if reading.charging == ChargingStatus::Charging => IconName::BatteryCharging,
        Some(reading) if reading.level.as_u8() >= 80 => IconName::BatteryFull,
        Some(reading) if reading.level.as_u8() <= 20 => IconName::BatteryLow,
        Some(_) => IconName::BatteryMedium,
        None => IconName::Battery,
    }
}

fn freshness_suffix(freshness: Freshness, ready: bool, language: Language) -> &'static str {
    match freshness {
        Freshness::Unknown => language.text(" · not read", " · 未读取"),
        Freshness::Current if ready => "",
        _ => language.text(" · last known", " · 上次读取"),
    }
}

pub(super) fn batteries(
    snapshot: &DeviceSnapshot,
    ready: bool,
    language: Language,
    cx: &App,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap(px(24.))
        .children(
            snapshot
                .batteries
                .iter()
                .enumerate()
                .map(|(index, endpoint)| {
                    let label = match endpoint.component {
                        BatteryComponent::Headset => language.text("Battery", "电量"),
                        BatteryComponent::Left => language.text("Left", "左耳"),
                        BatteryComponent::Right => language.text("Right", "右耳"),
                        BatteryComponent::Case => language.text("Case", "充电盒"),
                    };
                    let current = ready && endpoint.freshness == Freshness::Current;
                    let level = endpoint.reading.map(|reading| reading.level.as_u8());
                    let tint = if !current {
                        cx.theme().muted_foreground
                    } else if level.is_some_and(|level| level <= 20) {
                        cx.theme().danger
                    } else {
                        cx.theme().foreground
                    };
                    let value = level.map_or_else(|| "—".to_owned(), |value| format!("{value}%"));
                    let charging = endpoint
                        .reading
                        .map_or("", |reading| match reading.charging {
                            ChargingStatus::Charging => language.text(" · charging", " · 充电中"),
                            ChargingStatus::Charged => {
                                language.text(" · fully charged", " · 已充满")
                            }
                            ChargingStatus::NotCharging => "",
                            _ => language.text(" · charging status unknown", " · 充电状态未知"),
                        });
                    let age = freshness_suffix(endpoint.freshness, ready, language);
                    let description = format!("{label}: {value}{charging}{age}");
                    let icon = match endpoint.component {
                        BatteryComponent::Left | BatteryComponent::Right => img(themed_asset(
                            if endpoint.component == BatteryComponent::Left {
                                "label_left"
                            } else {
                                "label_right"
                            },
                            "svg",
                            cx,
                        ))
                        .size(px(19.))
                        .object_fit(ObjectFit::Contain)
                        .opacity(if current { 1. } else { 0.45 })
                        .into_any_element(),
                        _ => svg()
                            .path(battery_icon(endpoint.reading).path())
                            .size(px(21.))
                            .text_color(tint)
                            .into_any_element(),
                    };
                    div()
                        .id(("hero-battery", index))
                        .role(accesskit::Role::Group)
                        .aria_label(description.clone())
                        .tooltip(move |window, cx| {
                            Tooltip::new(description.clone()).build(window, cx)
                        })
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(icon)
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tint)
                                .child(value),
                        )
                }),
        )
}

struct Mark {
    stem: &'static str,
    name: &'static str,
    width: f32,
}

const MARK_HEIGHT: f32 = 28.;

fn codec_mark(codec: AudioCodec) -> Option<Mark> {
    let (stem, name, width) = match codec {
        AudioCodec::SBC => ("codec_sbc", "SBC", 62.),
        AudioCodec::AAC => ("codec_aac", "AAC", 62.),
        AudioCodec::LDAC => ("codec_ldac", "LDAC", 80.),
        AudioCodec::APT_X => ("codec_aptx", "aptX", 66.),
        AudioCodec::APT_X_HD => ("codec_aptxhd", "aptX HD", 106.),
        AudioCodec::LC3 => ("codec_lc3", "LC3", 54.),
        _ => return None,
    };
    Some(Mark { stem, name, width })
}

fn dsee_mark(technology: UpscalingType) -> Option<Mark> {
    let (stem, name, width) = match technology {
        UpscalingType::DSEE => ("dsee", "DSEE", 80.),
        UpscalingType::DSEE_HX => ("dseehx", "DSEE HX", 130.),
        UpscalingType::DSEE_EXTREME => ("dseehx_ai", "DSEE Extreme", 192.),
        UpscalingType::DSEE_ULTIMATE => ("dsee_ult", "DSEE Ultimate", 192.),
        _ => return None,
    };
    Some(Mark { stem, name, width })
}

fn status_mark(
    id: impl Into<ElementId>,
    mark: Option<Mark>,
    fallback: IconName,
    description: String,
    emphasized: bool,
    cx: &App,
) -> Stateful<Div> {
    let image = match mark {
        Some(mark) => img(themed_asset(mark.stem, "png", cx))
            .w(px(mark.width * MARK_HEIGHT / 36.))
            .h(px(MARK_HEIGHT))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => svg()
            .path(fallback.path())
            .size(px(20.))
            .text_color(cx.theme().muted_foreground)
            .into_any_element(),
    };
    div()
        .id(id)
        .role(accesskit::Role::Image)
        .aria_label(description.clone())
        .tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
        .flex()
        .items_center()
        .h(px(MARK_HEIGHT))
        .opacity(if emphasized { 1. } else { 0.45 })
        .child(image)
}

pub(super) fn audio_marks(
    snapshot: &DeviceSnapshot,
    ready: bool,
    language: Language,
    cx: &App,
) -> Div {
    let mut marks = div()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap(px(16.));
    if let Some(codec) = &snapshot.audio_codec {
        let mark = codec.codec.and_then(codec_mark);
        let name = mark.as_ref().map_or_else(
            || match codec.codec {
                None => language.text("Audio codec not read", "编码尚未读取").into(),
                Some(AudioCodec::UNSETTLED) => language
                    .text("Audio codec not determined", "编码尚未确定")
                    .into(),
                Some(value) => format!(
                    "{} (0x{:02X})",
                    language.text("Unknown audio codec", "未知编码"),
                    value.as_raw()
                ),
            },
            |mark| mark.name.to_owned(),
        );
        let description = format!(
            "{name}{}",
            freshness_suffix(codec.freshness, ready, language)
        );
        marks = marks.child(status_mark(
            "hero-codec",
            mark,
            IconName::AudioLines,
            description,
            ready && codec.freshness == Freshness::Current,
            cx,
        ));
    }
    for (index, entry) in snapshot.audio.iter().enumerate() {
        if !matches!(entry.kind, AudioKind::Dsee | AudioKind::DseeWithReasons) {
            continue;
        }
        let Some(AudioCapabilityData::Dsee(technology)) = entry
            .capability
            .as_ref()
            .map(|capability| capability.data())
        else {
            continue;
        };
        let mark = dsee_mark(*technology);
        let name = mark.as_ref().map_or_else(
            || format!("DSEE (0x{:02X})", technology.as_raw()),
            |mark| mark.name.to_owned(),
        );
        let mode = entry
            .parameters
            .as_ref()
            .and_then(|parameters| match parameters.value() {
                AudioParameterValue::Dsee(mode) => Some(*mode),
                _ => None,
            });
        // Auto is a configuration, not proof that upscaling is processing audio.
        let mode_label = match mode {
            Some(DseeMode::Auto) => language.text("Auto", "自动"),
            Some(DseeMode::Off) => language.text("Off", "关闭"),
            None => language.text("Not read", "未读取"),
        };
        let description = format!(
            "{name} · {mode_label}{}",
            freshness_suffix(entry.freshness, ready, language)
        );
        marks = marks.child(status_mark(
            ("hero-dsee", index),
            mark,
            IconName::Sparkles,
            description,
            ready && entry.freshness == Freshness::Current && mode == Some(DseeMode::Auto),
            cx,
        ));
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn battery_boundaries_and_charging_override() {
        for (level, expected) in [
            (0, IconName::BatteryLow),
            (20, IconName::BatteryLow),
            (21, IconName::BatteryMedium),
            (79, IconName::BatteryMedium),
            (80, IconName::BatteryFull),
            (100, IconName::BatteryFull),
        ] {
            let reading = BatteryReading {
                level: level.try_into().unwrap(),
                charging: ChargingStatus::NotCharging,
                threshold: None,
            };
            assert_eq!(battery_icon(Some(reading)), expected);
            assert_eq!(
                battery_icon(Some(BatteryReading {
                    charging: ChargingStatus::Charging,
                    ..reading
                })),
                IconName::BatteryCharging
            );
        }
        assert_eq!(battery_icon(None), IconName::Battery);
    }

    #[test]
    fn unknown_technologies_never_get_a_known_logo() {
        for codec in [
            AudioCodec::UNSETTLED,
            AudioCodec::OTHER,
            AudioCodec::from_raw(0x7e),
        ] {
            assert!(codec_mark(codec).is_none());
        }
        assert!(dsee_mark(UpscalingType::from_raw(0x7e)).is_none());
    }
}
