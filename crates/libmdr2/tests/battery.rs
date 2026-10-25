use libmdr2::{
    battery::{BatteryKind, BatteryReadings, BatteryStatus, ChargingStatus, Percentage},
    command::CommandError,
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn published_single_battery_capture_preserves_percentage_and_charge_state() {
    // sonymdr_open 44adbf92, sonyctl/sim.py INITIAL: GET 22/00 -> body 51 00.
    // Upstream labels it a WH-1000XM6 FW 3.1.5 capture. The RET header is rebuilt
    // from the POWER_RET_STATUS definition; provenance is in fixtures/README.md.
    let status = BatteryStatus::decode_response(T1, &[0x23, 0, 0x51, 0]).unwrap();
    assert_eq!(status.kind, BatteryKind::Single);
    let BatteryReadings::Single(reading) = status.batteries else {
        panic!("expected a single battery");
    };
    assert_eq!(reading.level.as_u8(), 81);
    assert_eq!(reading.charging, ChargingStatus::NotCharging);
    assert_eq!(reading.threshold, None);
}

#[test]
fn function_mapping_keeps_the_command_table_and_exact_variant() {
    let cases = [
        (BatteryKind::Single, 0x00, 0x20),
        (BatteryKind::LeftRight, 0x01, 0x21),
        (BatteryKind::Cradle, 0x02, 0x22),
        (BatteryKind::SingleWithThreshold, 0x08, 0x28),
        (BatteryKind::LeftRightWithThreshold, 0x09, 0x29),
        (BatteryKind::CradleWithThreshold, 0x0a, 0x2a),
    ];
    for (kind, inquiry, code) in cases {
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(BatteryKind::try_from(inquiry).unwrap(), kind);
        assert_eq!(kind.function_id(), FunctionId { table: T1, code });
        assert_eq!(BatteryKind::from_function(kind.function_id()), Some(kind));
        assert_eq!(
            BatteryKind::from_function(FunctionId { table: T2, code }),
            None
        );
        assert_eq!(kind.has_threshold(), inquiry >= 8);
    }
    assert_eq!(
        BatteryKind::from_function(FunctionId {
            table: T1,
            code: 0x23
        }),
        None
    );
    assert!(BatteryKind::try_from(0x03).is_err());
    assert!(BatteryKind::try_from(0xff).is_err());
}

#[test]
fn single_and_cradle_variants_preserve_optional_thresholds() {
    for (inquiry, expected_kind, cradle, threshold) in [
        (0, BatteryKind::Single, false, None),
        (2, BatteryKind::Cradle, true, None),
        (8, BatteryKind::SingleWithThreshold, false, Some(20)),
        (10, BatteryKind::CradleWithThreshold, true, Some(20)),
    ] {
        let mut payload = vec![0x23, inquiry, 63, 1];
        payload.extend(threshold);
        let status = BatteryStatus::decode_response(T1, &payload).unwrap();
        assert_eq!(status.kind, expected_kind);
        let reading = match status.batteries {
            BatteryReadings::Single(reading) if !cradle => reading,
            BatteryReadings::Cradle(reading) if cradle => reading,
            other => panic!("wrong endpoint: {other:?}"),
        };
        assert_eq!(reading.level.as_u8(), 63);
        assert_eq!(reading.charging, ChargingStatus::Charging);
        assert_eq!(reading.threshold.map(Percentage::as_u8), threshold);
    }
}

#[test]
fn left_right_thresholds_follow_both_level_status_pairs() {
    // Synthetic asymmetric values make a mistaken interleaved threshold layout visible.
    let status = BatteryStatus::decode_response(T1, &[0x23, 9, 47, 1, 82, 3, 10, 20]).unwrap();
    assert_eq!(status.kind, BatteryKind::LeftRightWithThreshold);
    let BatteryReadings::LeftRight { left, right } = status.batteries else {
        panic!("expected left/right endpoints");
    };
    assert_eq!(left.level.as_u8(), 47);
    assert_eq!(left.charging, ChargingStatus::Charging);
    assert_eq!(left.threshold.unwrap().as_u8(), 10);
    assert_eq!(right.level.as_u8(), 82);
    assert_eq!(right.charging, ChargingStatus::Charged);
    assert_eq!(right.threshold.unwrap().as_u8(), 20);

    let ordinary = BatteryStatus::decode_response(T1, &[0x23, 1, 47, 1, 82, 3]).unwrap();
    let BatteryReadings::LeftRight { left, right } = ordinary.batteries else {
        panic!("expected left/right endpoints");
    };
    assert_eq!(left.threshold, None);
    assert_eq!(right.threshold, None);
}

#[test]
fn zero_level_unknown_charging_and_charged_are_independent() {
    for (code, expected) in [
        (0, ChargingStatus::NotCharging),
        (1, ChargingStatus::Charging),
        (2, ChargingStatus::Unknown),
        (3, ChargingStatus::Charged),
    ] {
        let status = BatteryStatus::decode_response(T1, &[0x23, 0, 0, code]).unwrap();
        let BatteryReadings::Single(reading) = status.batteries else {
            panic!("expected a reading even for 0%");
        };
        assert_eq!(reading.level.as_u8(), 0);
        assert_eq!(reading.charging, expected);
        assert_eq!(expected.as_u8(), code);
    }
    assert_eq!(Percentage::try_from(100).unwrap().as_u8(), 100);
    assert!(Percentage::try_from(101).is_err());
    assert!(Percentage::try_from(0xff).is_err());
}

#[test]
fn notifications_share_bodies_but_cannot_satisfy_response_decoding() {
    for payload in [
        vec![0x23, 0, 50, 0],
        vec![0x23, 1, 10, 2, 90, 3],
        vec![0x23, 2, 50, 0],
        vec![0x23, 8, 50, 0, 10],
        vec![0x23, 9, 10, 2, 90, 3, 10, 20],
        vec![0x23, 10, 50, 0, 10],
    ] {
        let response = BatteryStatus::decode_response(T1, &payload).unwrap();
        let mut notification = payload.clone();
        notification[0] = 0x25;
        assert_eq!(
            BatteryStatus::decode_notification(T1, &notification).unwrap(),
            response
        );
        assert!(matches!(
            BatteryStatus::decode_response(T1, &notification),
            Err(CommandError::UnexpectedOpcode { .. })
        ));
        assert!(matches!(
            BatteryStatus::decode_notification(T1, &payload),
            Err(CommandError::UnexpectedOpcode { .. })
        ));
    }
}

#[test]
fn exact_lengths_reject_all_truncated_or_extended_layouts() {
    for payload in [
        vec![0x23, 0, 50, 0],
        vec![0x23, 1, 10, 2, 90, 3],
        vec![0x23, 2, 50, 0],
        vec![0x23, 8, 50, 0, 10],
        vec![0x23, 9, 10, 2, 90, 3, 10, 20],
        vec![0x23, 10, 50, 0, 10],
    ] {
        for length in 0..payload.len() {
            assert!(
                BatteryStatus::decode_response(T1, &payload[..length]).is_err(),
                "accepted prefix {length}: {payload:?}"
            );
        }
        let mut extended = payload;
        extended.push(0);
        assert!(BatteryStatus::decode_response(T1, &extended).is_err());
    }
    // Inquiries 09/0a require threshold bytes even when a reported level is zero.
    assert!(BatteryStatus::decode_response(T1, &[0x23, 9, 0, 2, 80, 0]).is_err());
    assert!(BatteryStatus::decode_response(T1, &[0x23, 10, 80, 0]).is_err());
}

#[test]
fn percentages_and_charging_codes_are_validated_at_every_endpoint() {
    let original = [0x23, 9, 0, 0, 100, 3, 0, 100];
    assert!(BatteryStatus::decode_response(T1, &original).is_ok());
    for index in [2, 4, 6, 7] {
        for invalid in [101, 128, 255] {
            let mut payload = original;
            payload[index] = invalid;
            assert!(matches!(
                BatteryStatus::decode_response(T1, &payload),
                Err(CommandError::InvalidValue { .. })
            ));
        }
    }
    for index in [3, 5] {
        for invalid in [4, 128, 255] {
            let mut payload = original;
            payload[index] = invalid;
            assert!(matches!(
                BatteryStatus::decode_response(T1, &payload),
                Err(CommandError::InvalidValue { .. })
            ));
        }
    }
}

#[test]
fn other_tables_inquiries_and_command_families_are_rejected() {
    assert!(matches!(
        BatteryStatus::decode_response(T2, &[0x23, 0, 50, 0]),
        Err(CommandError::WrongTable { .. })
    ));
    for inquiry in [3, 4, 5, 6, 7, 11, 12, 255] {
        assert!(matches!(
            BatteryStatus::decode_response(T1, &[0x23, inquiry, 50, 0]),
            Err(CommandError::InvalidValue {
                field: "battery inquiry",
                ..
            })
        ));
    }
    for opcode in [0x21, 0x22, 0x24, 0x27, 0x29] {
        assert!(matches!(
            BatteryStatus::decode_response(T1, &[opcode, 0, 50, 0]),
            Err(CommandError::UnexpectedOpcode { .. })
        ));
    }
}
