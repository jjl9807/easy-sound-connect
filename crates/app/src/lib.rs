//! Presentation helpers shared by the desktop window and connection probe.

use ezsc_core::{
    AudioCodecState, BatteryComponent, ConnectionState, DeviceSnapshot, Freshness,
    common::AudioCodec,
};

pub mod device_prompts;

pub fn connection_label(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connected => "Connected",
        ConnectionState::Negotiating => "Negotiating capabilities",
        ConnectionState::Synchronizing => "Synchronizing state",
        ConnectionState::Ready => "Ready",
        ConnectionState::Closing => "Releasing control",
        ConnectionState::Disconnected => "Disconnected",
        ConnectionState::Failed => "Connection failed",
    }
}

/// The headset's reported codec, independent of its connection-quality setting.
pub fn audio_codec_line(state: &AudioCodecState) -> String {
    let value = match state.codec {
        None => "—".to_owned(),
        Some(codec) => match codec {
            AudioCodec::UNSETTLED => "Not determined".to_owned(),
            AudioCodec::SBC => "SBC".to_owned(),
            AudioCodec::AAC => "AAC".to_owned(),
            AudioCodec::LDAC => "LDAC".to_owned(),
            AudioCodec::APT_X => "aptX".to_owned(),
            AudioCodec::APT_X_HD => "aptX HD".to_owned(),
            AudioCodec::LC3 => "LC3".to_owned(),
            AudioCodec::OTHER => "Other".to_owned(),
            _ => format!("Unknown (0x{:02X})", codec.as_raw()),
        },
    };
    let freshness = match state.freshness {
        Freshness::Unknown => " · not read",
        Freshness::Current => "",
        Freshness::Stale => " · stale",
    };
    format!("Audio codec: {value}{freshness}")
}

pub fn snapshot_lines(snapshot: &DeviceSnapshot) -> Vec<String> {
    let mut lines = vec![
        format!("Status: {}", connection_label(snapshot.connection)),
        format!(
            "Model: {}",
            snapshot
                .model_name
                .as_ref()
                .and_then(|text| text.as_utf8().ok())
                .unwrap_or("—")
        ),
        format!(
            "Firmware: {}",
            snapshot
                .firmware_version
                .as_ref()
                .and_then(|text| text.as_utf8().ok())
                .unwrap_or("—")
        ),
        format!("Transport: {:?}", snapshot.transport),
        format!("Advertised functions: {}", snapshot.functions.len()),
    ];
    if let Some(codec) = &snapshot.audio_codec {
        lines.push(audio_codec_line(codec));
    }
    for endpoint in &snapshot.batteries {
        let name = match endpoint.component {
            BatteryComponent::Headset => "Headset",
            BatteryComponent::Left => "Left",
            BatteryComponent::Right => "Right",
            BatteryComponent::Case => "Case",
        };
        let value = match endpoint.reading {
            Some(reading) => format!("{}% · {:?}", reading.level.as_u8(), reading.charging),
            None => "—".to_owned(),
        };
        let freshness = match endpoint.freshness {
            Freshness::Unknown => " · not read",
            Freshness::Current => "",
            Freshness::Stale => " · stale",
        };
        lines.push(format!("{name}: {value}{freshness}"));
    }
    if let Some(error) = &snapshot.last_error {
        lines.push(format!("Error: {error}"));
    }
    lines
}

/// Compact current control values for notification watching in the CLI probe.
pub fn control_lines(snapshot: &DeviceSnapshot) -> Vec<String> {
    let mut lines = vec![format!(
        "Generation: {} · messages: {:?}",
        snapshot.generation, snapshot.message_counters
    )];
    if let Some(prompt) = &snapshot.pending_prompt {
        lines.push(format!(
            "Pending device prompt: {:?} · {:?}",
            prompt.id, prompt.message
        ));
    }
    for state in &snapshot.noise {
        lines.push(format!(
            "Noise {:?}: {:?} · {:?} · {:?}",
            state.kind, state.parameters, state.status, state.freshness
        ));
    }
    for state in &snapshot.equalizers {
        lines.push(format!(
            "EQ {:?}: {:?} · {:?} · {:?}",
            state.kind, state.parameters, state.status, state.freshness
        ));
    }
    for state in &snapshot.audio {
        lines.push(format!(
            "Audio {:?}: {:?} · {:?} · {:?}",
            state.kind, state.parameters, state.status, state.freshness
        ));
    }
    for state in &snapshot.system {
        lines.push(format!(
            "System {:?}: {:?} · {:?} · {:?}",
            state.kind, state.parameters, state.status, state.freshness
        ));
        lines.push(format!(
            "System extended {:?}: {:?} · {:?}",
            state.kind, state.extended, state.extended_freshness
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_display_distinguishes_missing_unsettled_unknown_and_stale_reports() {
        let mut state = AudioCodecState {
            codec: None,
            freshness: Freshness::Unknown,
        };
        assert_eq!(audio_codec_line(&state), "Audio codec: — · not read");
        state.freshness = Freshness::Current;
        for (codec, text) in [
            (AudioCodec::UNSETTLED, "Not determined"),
            (AudioCodec::SBC, "SBC"),
            (AudioCodec::AAC, "AAC"),
            (AudioCodec::LDAC, "LDAC"),
            (AudioCodec::APT_X, "aptX"),
            (AudioCodec::APT_X_HD, "aptX HD"),
            (AudioCodec::LC3, "LC3"),
            (AudioCodec::OTHER, "Other"),
            (AudioCodec::from_raw(0x7e), "Unknown (0x7E)"),
        ] {
            state.codec = Some(codec);
            assert_eq!(audio_codec_line(&state), format!("Audio codec: {text}"));
        }
        state.codec = Some(AudioCodec::LDAC);
        state.freshness = Freshness::Stale;
        assert_eq!(audio_codec_line(&state), "Audio codec: LDAC · stale");
    }
}
