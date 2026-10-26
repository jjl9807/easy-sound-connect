//! Presentation helpers shared by the desktop window and connection probe.

use ezsc_core::{BatteryComponent, ConnectionState, DeviceSnapshot, Freshness};

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
    lines
}
