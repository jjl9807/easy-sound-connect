//! Synthetic layouts for offline visual review; these are not device profiles.

use bluetooth::{DeviceId, TransportKind};
use ezsc_core::{
    AudioCodecState, AudioState, BatteryComponent, BatteryEndpoint, ConnectionState,
    DeviceSnapshot, EqualizerState, Freshness, FunctionSupport, MessageCounters, NoiseState,
    SystemState,
};
use libmdr2::{
    audio::{AudioCapability, AudioKind, AudioParameters, AudioStatus},
    battery::{BatteryKind, BatteryReading, ChargingStatus},
    command::SupportedFunction,
    common::{AudioCodec, CommonKind},
    device::DeviceInfo,
    equalizer::{
        EqualizerBandInfo, EqualizerCapability, EqualizerKind, EqualizerParameters, EqualizerStatus,
    },
    noise::{
        AdaptiveSensitivity, AmbientMode, ChangeStatus, NoiseCapability, NoiseKind, NoiseMode,
        NoiseParameters, NoiseSettings, NoiseStatus, NoiseSwitch,
    },
    system::{
        DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemCapability, SystemKind,
        SystemParameters, SystemStatus,
    },
    types::{CommandTable, DisplayLanguage},
};

const TABLE: CommandTable = CommandTable::TABLE_1;

/// Creates a complete visual fixture without opening a transport or a session.
///
/// The two shapes exercise single and split batteries and different EQ widths.
/// They deliberately use the same generic product name, independent of any real
/// model's advertised capability list or validation status.
pub(super) fn snapshot(tws: bool) -> DeviceSnapshot {
    let DeviceInfo::ModelName(name) = DeviceInfo::decode(TABLE, b"\x05\x01\x0fPreview headset")
        .expect("valid synthetic product name")
    else {
        unreachable!("the fixed inquiry requests a model name")
    };
    let batteries = if tws {
        vec![
            battery(BatteryComponent::Left, BatteryKind::LeftRight, 82),
            battery(BatteryComponent::Right, BatteryKind::LeftRight, 79),
            battery(BatteryComponent::Case, BatteryKind::Cradle, 64),
        ]
    } else {
        vec![battery(BatteryComponent::Headset, BatteryKind::Single, 86)]
    };
    let noise = noise();
    let equalizer = equalizer(tws);
    let audio = vec![
        audio(AudioKind::Dsee, &[0xe1, 1, 2], &[0xe3, 1, 0], &[0xe7, 1, 1]),
        audio(
            AudioKind::ClassicLePriority,
            &[0xe1, 5, 2, 0, 1, 0],
            &[0xe3, 5, 0, 0],
            &[0xe7, 5, 0],
        ),
    ];
    let system = speak_to_chat();
    let mut functions = vec![
        noise.kind.function_id(),
        equalizer.kind.function_id(),
        system.kind.function_id(),
    ]
    .into_iter()
    .chain(audio.iter().map(|state| state.kind.function_id()))
    .map(|id| FunctionSupport {
        advertised: SupportedFunction { id, priority: 0 },
        readable: true,
        writable: true,
    })
    .collect::<Vec<_>>();
    let battery_kinds = if tws {
        vec![BatteryKind::LeftRight, BatteryKind::Cradle]
    } else {
        vec![BatteryKind::Single]
    };
    functions.extend(
        battery_kinds
            .into_iter()
            .map(|kind| kind.function_id())
            .chain([CommonKind::AudioCodec.function_id()])
            .map(|id| FunctionSupport {
                advertised: SupportedFunction { id, priority: 0 },
                readable: true,
                writable: false,
            }),
    );
    DeviceSnapshot {
        device: DeviceId(
            if tws {
                "preview-tws"
            } else {
                "preview-headband"
            }
            .into(),
        ),
        generation: 0,
        transport: TransportKind::Rfcomm,
        connection: ConnectionState::Ready,
        language: DisplayLanguage::ENGLISH,
        protocol: None,
        capability_info: None,
        functions,
        model_name: Some(name),
        firmware_version: None,
        appearance: None,
        batteries,
        noise: vec![noise],
        equalizers: vec![equalizer],
        audio,
        audio_codec: Some(AudioCodecState {
            codec: Some(AudioCodec::LDAC),
            freshness: Freshness::Current,
        }),
        system: vec![system],
        pending_prompt: None,
        last_error: None,
        message_counters: MessageCounters::default(),
        unhandled_messages: 0,
    }
}

fn battery(component: BatteryComponent, source: BatteryKind, level: u8) -> BatteryEndpoint {
    BatteryEndpoint {
        component,
        reading: Some(BatteryReading {
            level: level.try_into().expect("valid synthetic percentage"),
            charging: ChargingStatus::NotCharging,
            threshold: None,
        }),
        source,
        freshness: Freshness::Current,
    }
}

fn noise() -> NoiseState {
    NoiseState {
        kind: NoiseKind::AdaptiveNcAndAmbientLevel,
        capability: Some(
            NoiseCapability::decode_response(TABLE, &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1])
                .expect("valid synthetic ambient ranges"),
        ),
        status: Some(
            NoiseStatus::decode_response(TABLE, &[0x63, 0x19, 0])
                .expect("valid synthetic noise status"),
        ),
        parameters: Some(NoiseParameters::Control {
            change: ChangeStatus::Changed,
            effect: NoiseSwitch::On,
            settings: NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Voice,
                level: 12,
                adaptive: NoiseSwitch::On,
                sensitivity: AdaptiveSensitivity::Standard,
            },
        }),
        freshness: Freshness::Current,
    }
}

fn equalizer(tws: bool) -> EqualizerState {
    let (frequencies, levels): (&[u16], &[u8]) = if tws {
        (
            &[31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000],
            &[8, 7, 6, 5, 6, 7, 8, 7, 6, 5],
        )
    } else {
        (&[400, 1000, 2500, 6300, 16000], &[7, 6, 5, 7, 8])
    };
    let count = levels.len() as u8;
    let capability = EqualizerCapability::decode_response(
        TABLE,
        &[0x51, 0, count, 13, 3, 0, 0, 0xa1, 0, 0xa2, 0],
    )
    .expect("valid synthetic EQ capability");
    let mut parameter_bytes = vec![0x57, 0, 0xa1, count];
    parameter_bytes.extend_from_slice(levels);
    let parameters = EqualizerParameters::decode_response(TABLE, &parameter_bytes)
        .expect("valid synthetic EQ curve");
    let mut band_bytes = vec![0x5b, 0, count];
    for frequency in frequencies {
        band_bytes.push(1);
        band_bytes.extend_from_slice(&frequency.to_be_bytes());
    }
    let band_info = EqualizerBandInfo::decode_response(TABLE, &band_bytes)
        .expect("valid synthetic EQ frequencies");
    EqualizerState {
        kind: EqualizerKind::Preset,
        capability: Some(capability),
        status: Some(
            EqualizerStatus::decode_response(TABLE, &[0x53, 0, 0])
                .expect("valid synthetic EQ status"),
        ),
        parameters: Some(parameters),
        band_info: Some(band_info),
        freshness: Freshness::Current,
    }
}

fn audio(kind: AudioKind, capability: &[u8], status: &[u8], parameters: &[u8]) -> AudioState {
    AudioState {
        kind,
        capability: Some(
            AudioCapability::decode_response(TABLE, capability)
                .expect("valid synthetic audio capability"),
        ),
        status: Some(
            AudioStatus::decode_response(TABLE, status).expect("valid synthetic audio status"),
        ),
        parameters: Some(
            AudioParameters::decode_response(TABLE, parameters)
                .expect("valid synthetic audio setting"),
        ),
        freshness: Freshness::Current,
    }
}

fn speak_to_chat() -> SystemState {
    SystemState {
        kind: SystemKind::SpeakToChatType2,
        capability: Some(
            SystemCapability::decode_response(TABLE, &[0xf1, 12, 1, 5, 15, 30])
                .expect("valid synthetic speech capability"),
        ),
        status: Some(
            SystemStatus::decode_response(TABLE, &[0xf3, 12, 0, 0])
                .expect("valid synthetic speech status"),
        ),
        parameters: Some(SystemParameters::SpeakToChatType2 {
            enabled: true,
            preview: false,
        }),
        freshness: Freshness::Current,
        extended: Some(SpeakToChatSettings::Type2 {
            sensitivity: DetectionSensitivity::Auto,
            mode_out: ModeOutTime::Mid,
        }),
        extended_freshness: Freshness::Current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libmdr2::audio::{AudioParameterValue, AudioSetting};

    #[test]
    fn preview_layouts_have_matching_capabilities_and_valid_parameters() {
        for tws in [false, true] {
            let snapshot = snapshot(tws);
            assert_eq!(
                snapshot.model_name.as_ref().unwrap().as_utf8().unwrap(),
                "Preview headset"
            );
            for state in snapshot.noise {
                state
                    .parameters
                    .unwrap()
                    .encode_set(state.capability.as_ref().unwrap())
                    .unwrap();
            }
            for state in snapshot.equalizers {
                let capability = state.capability.as_ref().unwrap();
                state
                    .parameters
                    .unwrap()
                    .validate_capability(capability)
                    .unwrap();
                state
                    .band_info
                    .unwrap()
                    .validate_capability(capability)
                    .unwrap();
            }
            for state in snapshot.audio {
                let parameters = state.parameters.unwrap();
                let setting = match parameters.value() {
                    AudioParameterValue::Dsee(mode) => AudioSetting::Dsee(*mode),
                    AudioParameterValue::ConnectionPriority { priority, .. } => {
                        AudioSetting::ConnectionPriority(*priority)
                    }
                    _ => panic!("unexpected preview audio layout"),
                };
                setting
                    .encode_set(state.capability.as_ref().unwrap())
                    .unwrap();
            }
            for state in snapshot.system {
                let capability = state.capability.as_ref().unwrap();
                state.parameters.unwrap().encode_set(capability).unwrap();
                state.extended.unwrap().encode_set(capability).unwrap();
            }
        }
    }
}
