//! Shared, current-only battery summary for device cards and native tray menus.

use ezsc_core::{BatteryComponent, ConnectionState, DeviceSnapshot, Freshness};

pub(super) fn battery_level(state: &DeviceSnapshot) -> Option<u8> {
    if matches!(
        state.connection,
        ConnectionState::Closing | ConnectionState::Disconnected | ConnectionState::Failed
    ) {
        return None;
    }
    let split = state.batteries.iter().any(|battery| {
        matches!(
            battery.component,
            BatteryComponent::Left | BatteryComponent::Right
        )
    });
    state
        .batteries
        .iter()
        .filter(|battery| {
            battery.freshness == Freshness::Current
                && if split {
                    matches!(
                        battery.component,
                        BatteryComponent::Left | BatteryComponent::Right
                    )
                } else {
                    battery.component == BatteryComponent::Headset
                }
        })
        .filter_map(|battery| battery.reading.map(|reading| reading.level.as_u8()))
        .min()
}
