use libmdr2::{
    command::CommandError,
    power::{
        PowerCapability, PowerKind, PowerOff, PowerOffOption, PowerParameters, PowerSettings,
        PowerStatus,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// Short initial-state bodies from sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39,
// sonyctl/sim.py INITIAL, labelled WH-1000XM6 firmware 3.1.5. RET headers are
// reconstructed; these are payload fixtures, not complete transport captures.
const XM6_CAPABILITY: &[u8] = &[0x21, 0x05, 2, 0x10, 0x11];
const XM6_PARAMETERS: &[u8] = &[0x27, 0x05, 0x10, 0x00];

#[test]
fn published_auto_off_state_keeps_a_time_not_offered_by_the_capability() {
    let cap = PowerCapability::decode_response(T1, XM6_CAPABILITY).unwrap();
    assert_eq!(cap.kind(), PowerKind::AutoPowerOffWithWearingDetection);
    assert_eq!(
        cap.options(),
        [PowerOffOption::WHEN_REMOVED, PowerOffOption::DISABLED]
    );
    let mut params = PowerParameters::decode_response(T1, XM6_PARAMETERS).unwrap();
    assert_eq!(
        params.settings,
        PowerSettings::AutoPowerOff {
            active: PowerOffOption::WHEN_REMOVED,
            selected_time: PowerOffOption::FIVE_MINUTES,
        }
    );
    assert_eq!(params.encode_set(&cap).unwrap(), [0x28, 5, 0x10, 0]);
    let PowerSettings::AutoPowerOff { active, .. } = &mut params.settings else {
        panic!()
    };
    *active = PowerOffOption::DISABLED;
    assert_eq!(params.encode_set(&cap).unwrap(), [0x28, 5, 0x11, 0]);
}

#[test]
fn function_and_inquiry_mapping_preserves_command_table() {
    for (kind, table, function, inquiry) in [
        (PowerKind::AutoPowerOff, T1, 0x24, 4),
        (PowerKind::AutoPowerOffWithWearingDetection, T1, 0x25, 5),
        (PowerKind::PowerSaving, T1, 0x26, 6),
        (PowerKind::CaringCharge, T1, 0x2c, 0x0c),
        (PowerKind::AutoStandby, T2, 0x20, 0),
        (PowerKind::CaringChargeWithThreshold, T2, 0x22, 1),
    ] {
        let id = FunctionId {
            table,
            code: function,
        };
        assert_eq!(PowerKind::from_function(id), Some(kind));
        assert_eq!(kind.function_id(), id);
        assert_eq!(kind.table(), table);
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(PowerKind::from_wire(table, inquiry).unwrap(), kind);
    }
    assert_eq!(
        PowerKind::from_function(FunctionId {
            table: T1,
            code: 0x20
        }),
        None
    );
    assert_eq!(
        PowerKind::from_function(FunctionId {
            table: T2,
            code: 0x24
        }),
        None
    );
    assert!(PowerKind::from_wire(T1, 0).is_err());
    assert!(PowerKind::from_wire(T2, 5).is_err());
    assert!(PowerKind::from_wire(CommandTable::from_raw(9), 4).is_err());
}

#[test]
fn fixed_descriptors_are_only_for_layouts_without_capability_queries() {
    for kind in [PowerKind::PowerSaving, PowerKind::CaringCharge] {
        assert!(!kind.has_capability_query());
        let cap = PowerCapability::fixed(kind).unwrap();
        assert_eq!(cap.kind(), kind);
        assert!(cap.options().is_empty());
        assert_eq!(cap.charge_threshold(), None);
        assert_eq!(cap.standby_candidate(), None);
        assert!(PowerCapability::decode_response(T1, &[0x21, kind.inquiry()]).is_err());
    }
    for kind in [
        PowerKind::AutoPowerOff,
        PowerKind::AutoPowerOffWithWearingDetection,
        PowerKind::AutoStandby,
        PowerKind::CaringChargeWithThreshold,
    ] {
        assert!(kind.has_capability_query());
        assert!(PowerCapability::fixed(kind).is_err());
    }
}

#[test]
fn option_identity_is_not_a_duration_or_model_rule() {
    for (raw, minutes) in [(0, 5), (1, 30), (2, 60), (3, 180), (4, 15)] {
        assert_eq!(PowerOffOption::from_raw(raw).minutes(), Some(minutes));
    }
    for raw in [0x10, 0x11, 0xfe, 0xff] {
        assert_eq!(PowerOffOption::from_raw(raw).minutes(), None);
        assert_eq!(PowerOffOption::from_raw(raw).as_raw(), raw);
    }
}

#[test]
fn capability_options_preserve_empty_unknown_duplicate_and_full_unsigned_counts() {
    let cap = PowerCapability::decode_response(T1, &[0x21, 4, 4, 1, 0xe0, 1, 0x11]).unwrap();
    assert_eq!(
        cap.options().iter().map(|x| x.as_raw()).collect::<Vec<_>>(),
        [1, 0xe0, 1, 0x11]
    );
    assert!(
        PowerCapability::decode_response(T1, &[0x21, 5, 0])
            .unwrap()
            .options()
            .is_empty()
    );
    let mut payload = vec![0x21, 4, 255];
    payload.extend(0..255);
    let cap = PowerCapability::decode_response(T1, &payload).unwrap();
    assert_eq!(cap.options().len(), 255);
    assert_eq!(cap.options()[254].as_raw(), 254);
    payload.pop();
    assert!(PowerCapability::decode_response(T1, &payload).is_err());
}

#[test]
fn auto_off_writes_require_advertised_active_and_selected_timer_values() {
    let cap = PowerCapability::decode_response(T1, &[0x21, 4, 3, 0, 1, 0x11]).unwrap();
    let make = |active, selected_time| {
        PowerParameters::new(
            PowerKind::AutoPowerOff,
            PowerSettings::AutoPowerOff {
                active,
                selected_time,
            },
        )
        .unwrap()
    };
    assert_eq!(
        make(PowerOffOption::FIVE_MINUTES, PowerOffOption::THIRTY_MINUTES)
            .encode_set(&cap)
            .unwrap(),
        [0x28, 4, 0, 1]
    );
    assert_eq!(
        make(PowerOffOption::DISABLED, PowerOffOption::THIRTY_MINUTES)
            .encode_set(&cap)
            .unwrap(),
        [0x28, 4, 0x11, 1]
    );
    for (active, selected) in [(0x10, 1), (2, 1), (1, 2), (1, 0x11), (1, 0xff), (0xff, 1)] {
        assert!(
            make(
                PowerOffOption::from_raw(active),
                PowerOffOption::from_raw(selected)
            )
            .encode_set(&cap)
            .is_err()
        );
    }
    let empty = PowerCapability::decode_response(T1, &[0x21, 4, 0]).unwrap();
    assert!(
        make(PowerOffOption::DISABLED, PowerOffOption::FIVE_MINUTES)
            .encode_set(&empty)
            .is_err()
    );
}

#[test]
fn unknown_or_wrong_layout_options_remain_readable_but_not_writable() {
    for (inquiry, active, time) in [(4, 0x10, 0), (4, 0xff, 0), (5, 0xfe, 0)] {
        let cap = PowerCapability::decode_response(T1, &[0x21, inquiry, 1, active]).unwrap();
        let params = PowerParameters::decode_response(T1, &[0x27, inquiry, active, time]).unwrap();
        assert!(params.encode_set(&cap).is_err());
    }
    let cap = PowerCapability::decode_response(T1, XM6_CAPABILITY).unwrap();
    let unknown_time = PowerParameters::decode_response(T1, &[0x27, 5, 0x11, 0xe0]).unwrap();
    assert!(unknown_time.encode_set(&cap).is_err());
}

#[test]
fn standby_candidate_and_caring_charge_threshold_are_not_writable_parameters() {
    let standby = PowerCapability::decode_response(T2, &[0x21, 0, 0]).unwrap();
    assert_eq!(standby.standby_candidate(), Some(0));
    assert_eq!(standby.standby_minutes(), Some(15));
    let unknown = PowerCapability::decode_response(T2, &[0x21, 0, 9]).unwrap();
    assert_eq!(unknown.standby_candidate(), Some(9));
    assert_eq!(unknown.standby_minutes(), None);
    let params = PowerParameters::new(
        PowerKind::AutoStandby,
        PowerSettings::Switch { enabled: true },
    )
    .unwrap();
    assert_eq!(params.encode_set(&standby).unwrap(), [0x28, 0, 0]);
    assert!(params.encode_set(&unknown).is_err());
    for threshold in [1, 80, 100] {
        let cap = PowerCapability::decode_response(T2, &[0x21, 1, threshold]).unwrap();
        assert_eq!(cap.charge_threshold().unwrap().as_u8(), threshold);
        let params = PowerParameters::new(
            PowerKind::CaringChargeWithThreshold,
            PowerSettings::Switch { enabled: false },
        )
        .unwrap();
        assert_eq!(params.encode_set(&cap).unwrap(), [0x28, 1, 1]);
    }
    for threshold in [0, 101, 255] {
        assert!(PowerCapability::decode_response(T2, &[0x21, 1, threshold]).is_err());
    }
}

#[test]
fn availability_notice_and_setting_have_independent_meanings() {
    let status = PowerStatus::decode_response(T2, &[0x23, 1, 0, 1]).unwrap();
    assert_eq!(status.kind(), PowerKind::CaringChargeWithThreshold);
    assert!(status.enabled);
    assert_eq!(status.battery_notice_required, Some(false));
    let params = PowerParameters::decode_response(T2, &[0x27, 1, 1]).unwrap();
    assert_eq!(params.settings, PowerSettings::Switch { enabled: false });
    let status = PowerStatus::decode_notification(T2, &[0x25, 1, 1, 0]).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.battery_notice_required, Some(true));
    for (table, inquiry) in [(T1, 4), (T1, 5), (T1, 6), (T1, 12), (T2, 0)] {
        let status = PowerStatus::decode_response(table, &[0x23, inquiry, 0]).unwrap();
        assert!(status.enabled);
        assert_eq!(status.battery_notice_required, None);
    }
}

#[test]
fn every_switch_layout_encodes_the_inverted_on_off_byte() {
    let capabilities = [
        PowerCapability::fixed(PowerKind::PowerSaving).unwrap(),
        PowerCapability::fixed(PowerKind::CaringCharge).unwrap(),
        PowerCapability::decode_response(T2, &[0x21, 0, 0]).unwrap(),
        PowerCapability::decode_response(T2, &[0x21, 1, 80]).unwrap(),
    ];
    for cap in capabilities {
        for (enabled, byte) in [(true, 0), (false, 1)] {
            let params =
                PowerParameters::new(cap.kind(), PowerSettings::Switch { enabled }).unwrap();
            assert_eq!(
                params.encode_set(&cap).unwrap(),
                [0x28, cap.kind().inquiry(), byte]
            );
            assert_eq!(
                PowerParameters::decode_response(
                    cap.kind().table(),
                    &[0x27, cap.kind().inquiry(), byte]
                )
                .unwrap(),
                params
            );
        }
    }
}

#[test]
fn capability_association_rejects_both_table_and_layout_mismatch() {
    let params = PowerParameters::decode_response(T1, XM6_PARAMETERS).unwrap();
    let cap = PowerCapability::decode_response(T1, &[0x21, 4, 2, 0, 0x11]).unwrap();
    assert!(matches!(
        params.encode_set(&cap),
        Err(CommandError::CapabilityMismatch { .. })
    ));
    let cap = PowerCapability::decode_response(T2, &[0x21, 0, 0]).unwrap();
    assert!(matches!(
        params.encode_set(&cap),
        Err(CommandError::WrongTable { .. })
    ));
}

#[test]
fn writable_fields_cannot_change_the_layout_shape() {
    assert!(
        PowerParameters::new(
            PowerKind::AutoPowerOff,
            PowerSettings::Switch { enabled: true }
        )
        .is_err()
    );
    let cap = PowerCapability::fixed(PowerKind::CaringCharge).unwrap();
    let mut params = PowerParameters::new(
        PowerKind::CaringCharge,
        PowerSettings::Switch { enabled: false },
    )
    .unwrap();
    params.settings = PowerSettings::AutoPowerOff {
        active: PowerOffOption::DISABLED,
        selected_time: PowerOffOption::FIVE_MINUTES,
    };
    assert!(params.encode_set(&cap).is_err());
}

#[test]
fn closed_flags_reject_every_value_other_than_zero_and_one() {
    for value in 2..=255 {
        assert!(PowerStatus::decode_response(T1, &[0x23, 4, value]).is_err());
        assert!(PowerStatus::decode_response(T2, &[0x23, 1, 0, value]).is_err());
        for (table, inquiry) in [(T1, 6), (T1, 12), (T2, 0), (T2, 1)] {
            assert!(PowerParameters::decode_response(table, &[0x27, inquiry, value]).is_err());
        }
    }
}

#[test]
fn responses_notifications_and_battery_messages_do_not_substitute_for_each_other() {
    assert!(PowerStatus::decode_response(T1, &[0x25, 4, 0]).is_err());
    assert!(PowerStatus::decode_notification(T1, &[0x23, 4, 0]).is_err());
    assert!(PowerParameters::decode_response(T1, &[0x29, 5, 0x10, 0]).is_err());
    assert!(PowerParameters::decode_notification(T1, XM6_PARAMETERS).is_err());
    assert!(PowerStatus::decode_response(T1, &[0x23, 0, 81, 0]).is_err());
    assert!(PowerParameters::decode_response(T1, &[0x27, 0, 0]).is_err());
    assert!(PowerStatus::decode_response(T2, &[0x23, 4, 0]).is_err());
    assert!(PowerParameters::decode_response(T2, &[0x27, 5, 0, 0]).is_err());
    assert!(PowerStatus::decode_response(CommandTable::from_raw(3), &[0x23, 4, 0]).is_err());
}

#[test]
fn all_decoders_require_exact_lengths_without_panicking_on_truncation() {
    type Decode = fn(CommandTable, &[u8]) -> Result<(), CommandError>;
    let cases: &[(CommandTable, &[u8], Decode)] = &[
        (T1, XM6_CAPABILITY, |t, p| {
            PowerCapability::decode_response(t, p).map(|_| ())
        }),
        (T1, &[0x21, 4, 2, 0, 0x11], |t, p| {
            PowerCapability::decode_response(t, p).map(|_| ())
        }),
        (T2, &[0x21, 0, 0], |t, p| {
            PowerCapability::decode_response(t, p).map(|_| ())
        }),
        (T2, &[0x21, 1, 80], |t, p| {
            PowerCapability::decode_response(t, p).map(|_| ())
        }),
        (T1, XM6_PARAMETERS, |t, p| {
            PowerParameters::decode_response(t, p).map(|_| ())
        }),
        (T2, &[0x27, 1, 1], |t, p| {
            PowerParameters::decode_response(t, p).map(|_| ())
        }),
        (T1, &[0x23, 4, 0], |t, p| {
            PowerStatus::decode_response(t, p).map(|_| ())
        }),
        (T2, &[0x23, 1, 0, 1], |t, p| {
            PowerStatus::decode_response(t, p).map(|_| ())
        }),
        (T1, &[0x29, 5, 0x10, 0], |t, p| {
            PowerParameters::decode_notification(t, p).map(|_| ())
        }),
        (T2, &[0x25, 1, 0, 1], |t, p| {
            PowerStatus::decode_notification(t, p).map(|_| ())
        }),
    ];
    for &(table, payload, decode) in cases {
        assert!(decode(table, payload).is_ok());
        for end in 0..payload.len() {
            assert!(decode(table, &payload[..end]).is_err());
        }
        let mut trailing = payload.to_vec();
        trailing.push(0);
        assert!(decode(table, &trailing).is_err());
        let mut bad = payload.to_vec();
        bad[0] = 0xff;
        assert!(decode(table, &bad).is_err());
    }
}

#[test]
fn power_off_exposes_only_the_fixed_user_command() {
    assert_eq!(
        PowerOff::function_id(),
        FunctionId {
            table: T1,
            code: 0x23
        }
    );
    assert_eq!(PowerOff::encode(), [0x24, 3, 1]);
    assert!(PowerKind::from_wire(T1, 3).is_err());
    assert!(PowerStatus::decode_response(T1, &[0x23, 3, 0]).is_err());
}
