//! Current-only battery summary for device cards.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_battery_uses_ear_minimum_and_excludes_case() {
        let mut state = crate::gui_preview::snapshot(true);
        state.batteries[2].reading.as_mut().unwrap().level = 0.try_into().unwrap();
        assert_eq!(battery_level(&state), Some(79));
        for level in [21, 20, 15, 0] {
            state.batteries[0].reading.as_mut().unwrap().level = level.try_into().unwrap();
            assert_eq!(battery_level(&state), Some(level));
        }
        let state = crate::gui_preview::snapshot(false);
        assert_eq!(battery_level(&state), Some(86));
    }

    #[test]
    fn missing_and_stale_readings_do_not_become_zero_or_fall_back_to_case() {
        let mut state = crate::gui_preview::snapshot(true);
        state.batteries[0].reading = None;
        assert_eq!(battery_level(&state), Some(79));
        state.batteries[1].freshness = Freshness::Stale;
        assert_eq!(battery_level(&state), None);
        let mut state = crate::gui_preview::snapshot(true);
        state.connection = ConnectionState::Disconnected;
        assert_eq!(battery_level(&state), None);
    }
}
