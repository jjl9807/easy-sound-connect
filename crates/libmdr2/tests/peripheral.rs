use libmdr2::{
    peripheral::{
        ConnectivityAction, DeviceList, OperationOutcome, PeripheralCapability, PeripheralKind,
        PeripheralMode, PeripheralOperation, PeripheralParameters, PeripheralResultData,
        PeripheralResultReport, PeripheralSetting, PeripheralStatus, SourceSwitchResult,
    },
    types::{BluetoothAddress, CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;
const ADDRESS: &[u8; 17] = b"02:00:00:00:AB:01";
const OTHER_ADDRESS: &[u8; 17] = b"02:00:00:00:AB:02";

// Only the short 08 02 00 capability body is taken from sonymdr_open
// 44adbf92df1525571561bbebc0485ea9c436ae39 sonyctl/sim.py INITIAL.
// All device records, names, addresses, mutations and result cases are synthetic.
const CAPABILITY: &[u8] = &[0x31, 0x02, 0x08, 0x02, 0x00];

fn cap(kind: PeripheralKind) -> PeripheralCapability {
    if kind.has_capability_query() {
        PeripheralCapability::decode_response(T2, &[0x31, kind.inquiry(), 8, 2, 0]).unwrap()
    } else {
        PeripheralCapability::fixed(kind).unwrap()
    }
}

fn address() -> BluetoothAddress {
    BluetoothAddress::parse_ascii(ADDRESS).unwrap()
}

fn list_payload(inquiry: u8, records: &[(&[u8; 17], u8, &[u8])], playback: u8) -> Vec<u8> {
    let mut payload = vec![0x37, inquiry, u8::try_from(records.len()).unwrap()];
    for (address, slot, name) in records {
        payload.extend_from_slice(*address);
        payload.push(*slot);
        if inquiry == 2 {
            payload.extend_from_slice(&[0x23, 0x45, 0x67]);
        }
        payload.push(u8::try_from(name.len()).unwrap());
        payload.extend_from_slice(name);
    }
    payload.push(playback);
    payload
}

fn list(inquiry: u8, records: &[(&[u8; 17], u8, &[u8])]) -> DeviceList {
    let PeripheralParameters::Devices(list) =
        PeripheralParameters::decode_response(T2, &list_payload(inquiry, records, 0x7f)).unwrap()
    else {
        panic!("expected device records")
    };
    list
}

fn result(inquiry: u8, action: u8, code: u8, address: &[u8; 17]) -> Vec<u8> {
    let mut bytes = vec![0x3d, inquiry];
    if inquiry != 1 {
        bytes.push(action);
    }
    bytes.push(code);
    bytes.extend_from_slice(address);
    bytes
}

#[test]
fn discovery_selects_layout_without_assuming_transport_or_device_model() {
    for (code, kind) in [
        (0x30, PeripheralKind::Devices),
        (0x31, PeripheralKind::SourceSwitch),
        (0x32, PeripheralKind::DevicesWithClass),
        (0x33, PeripheralKind::DevicesWithClass),
        (0x34, PeripheralKind::MusicHandover),
    ] {
        let function = FunctionId { table: T2, code };
        assert_eq!(PeripheralKind::from_function(function), Some(kind));
        assert!(kind.function_ids().contains(&function));
        assert_eq!(
            PeripheralKind::from_function(FunctionId { table: T1, code }),
            None
        );
        assert_eq!(PeripheralKind::try_from(kind.inquiry()).unwrap(), kind);
    }
    assert_eq!(
        PeripheralKind::from_function(FunctionId {
            table: T2,
            code: 0x35
        }),
        None
    );
    assert!(PeripheralKind::try_from(4).is_err());
}

#[test]
fn published_capacity_body_preserves_unsigned_counts_and_fixed_descriptor_boundaries() {
    let capability = PeripheralCapability::decode_response(T2, CAPABILITY).unwrap();
    let limits = capability.limits().unwrap();
    assert_eq!(limits.max_paired, 8);
    assert_eq!(limits.max_connected, 2);
    assert!(limits.file_transfer_possible);
    for inquiry in [0, 2] {
        let capability =
            PeripheralCapability::decode_response(T2, &[0x31, inquiry, 0, 255, 1]).unwrap();
        assert_eq!(capability.limits().unwrap().max_paired, 0);
        assert_eq!(capability.limits().unwrap().max_connected, 255);
        assert!(!capability.limits().unwrap().file_transfer_possible);
        assert!(PeripheralCapability::fixed(capability.kind()).is_err());
    }
    for kind in [PeripheralKind::SourceSwitch, PeripheralKind::MusicHandover] {
        assert!(!kind.has_capability_query());
        assert!(!kind.has_status_query());
        assert_eq!(PeripheralCapability::fixed(kind).unwrap().limits(), None);
        assert!(
            PeripheralCapability::decode_response(T2, &[0x31, kind.inquiry(), 8, 2, 0]).is_err()
        );
    }
    assert!(PeripheralCapability::decode_response(T2, &[0x31, 0, 8, 2, 2]).is_err());
    assert!(PeripheralCapability::decode_response(T1, CAPABILITY).is_err());
    for length in 0..CAPABILITY.len() {
        assert!(PeripheralCapability::decode_response(T2, &CAPABILITY[..length]).is_err());
    }
    assert!(PeripheralCapability::decode_response(T2, &[0x31, 2, 8, 2, 0, 0]).is_err());
}

#[test]
fn device_availability_and_pairing_mode_are_reports_with_strict_flags() {
    for inquiry in [0, 2] {
        let status = PeripheralStatus::decode_response(T2, &[0x33, inquiry, 1, 0]).unwrap();
        assert_eq!(status.mode, PeripheralMode::Pairing);
        assert!(status.enabled);
        assert_eq!(status.kind().inquiry(), inquiry);
        let status = PeripheralStatus::decode_notification(T2, &[0x35, inquiry, 0, 1]).unwrap();
        assert_eq!(status.mode, PeripheralMode::Normal);
        assert!(!status.enabled);
        for bad in [
            vec![0x33, inquiry, 2, 0],
            vec![0x33, inquiry, 0, 2],
            vec![0x33, inquiry, 0],
            vec![0x33, inquiry, 0, 0, 0],
        ] {
            assert!(PeripheralStatus::decode_response(T2, &bad).is_err());
        }
    }
    for inquiry in [1, 3] {
        assert!(PeripheralStatus::decode_response(T2, &[0x33, inquiry, 0, 0]).is_err());
    }
    assert!(PeripheralStatus::decode_notification(T2, &[0x33, 0, 0, 0]).is_err());
    assert!(PeripheralStatus::decode_response(T1, &[0x33, 0, 0, 0]).is_err());
}

#[test]
fn both_record_layouts_preserve_names_order_slots_and_playback_metadata() {
    for inquiry in [0, 2] {
        let records = list(
            inquiry,
            &[
                (OTHER_ADDRESS, 0, b"Offline"),
                (ADDRESS, 200, &[0xff, b'X']),
            ],
        );
        assert_eq!(records.kind().inquiry(), inquiry);
        assert_eq!(records.playback_right_raw(), 0x7f);
        assert_eq!(records.devices().len(), 2);
        assert_eq!(records.devices()[0].address().to_ascii(), *OTHER_ADDRESS);
        assert_eq!(records.devices()[0].name_utf8().unwrap(), "Offline");
        assert!(!records.devices()[0].is_connected());
        assert_eq!(records.devices()[1].address(), address());
        assert_eq!(records.devices()[1].connection_slot(), 200);
        assert!(records.devices()[1].is_connected());
        assert_eq!(records.devices()[1].name_bytes(), &[0xff, b'X']);
        assert!(records.devices()[1].name_utf8().is_err());
        assert_eq!(
            records.devices()[1].bluetooth_class(),
            (inquiry == 2).then_some(0x234567)
        );

        let mut payload = list_payload(inquiry, &[(ADDRESS, 1, "电脑".as_bytes())], 0);
        let response = PeripheralParameters::decode_response(T2, &payload).unwrap();
        payload[0] = 0x39;
        assert_eq!(
            PeripheralParameters::decode_notification(T2, &payload).unwrap(),
            response
        );
    }
}

#[test]
fn empty_and_maximum_unsigned_lists_are_valid_without_invented_count_caps() {
    for inquiry in [0, 2] {
        let empty = list(inquiry, &[]);
        assert!(empty.devices().is_empty());
        assert_eq!(empty.playback_right_raw(), 0x7f);
        let records = vec![(ADDRESS, 255, b"n".as_slice()); 255];
        let repeated = list(inquiry, &records);
        assert_eq!(repeated.devices().len(), 255);
        assert!(
            PeripheralOperation::Disconnect(address())
                .encode(&cap(repeated.kind()), &repeated)
                .is_err()
        );
    }
}

#[test]
fn friendly_name_lengths_and_record_count_are_checked_at_the_wire_boundary() {
    for inquiry in [0, 2] {
        for length in [1, 127, 128] {
            let name = vec![b'n'; length];
            assert_eq!(
                list(inquiry, &[(ADDRESS, 1, &name)]).devices()[0].name_bytes(),
                name
            );
        }
        for length in [0, 129, 255] {
            let name = vec![b'n'; length];
            assert!(
                PeripheralParameters::decode_response(
                    T2,
                    &list_payload(inquiry, &[(ADDRESS, 1, &name)], 0)
                )
                .is_err()
            );
        }
        let payload = list_payload(
            inquiry,
            &[(ADDRESS, 1, b"One"), (OTHER_ADDRESS, 0, b"Two")],
            0,
        );
        for length in 0..payload.len() {
            assert!(
                PeripheralParameters::decode_response(T2, &payload[..length]).is_err(),
                "inquiry {inquiry}, truncation {length}"
            );
        }
        let mut trailing = payload.clone();
        trailing.push(0);
        assert!(PeripheralParameters::decode_response(T2, &trailing).is_err());
        let mut count = payload.clone();
        count[2] = 1;
        assert!(PeripheralParameters::decode_response(T2, &count).is_err());
        count[2] = 3;
        assert!(PeripheralParameters::decode_response(T2, &count).is_err());
        let mut malformed_address = payload;
        malformed_address[5] = b'-';
        assert!(PeripheralParameters::decode_response(T2, &malformed_address).is_err());
    }
}

#[test]
fn keep_source_response_and_notification_have_distinct_shapes() {
    assert_eq!(
        PeripheralParameters::decode_response(T2, &[0x37, 1, 0]).unwrap(),
        PeripheralParameters::SourceKeep {
            enabled: true,
            result: None
        }
    );
    assert_eq!(
        PeripheralParameters::decode_notification(T2, &[0x39, 1, 1, 3]).unwrap(),
        PeripheralParameters::SourceKeep {
            enabled: false,
            result: Some(SourceSwitchResult::A2DP_NOT_CONNECTED)
        }
    );
    assert_eq!(
        PeripheralParameters::decode_notification(T2, &[0x39, 1, 0, 0xab]).unwrap(),
        PeripheralParameters::SourceKeep {
            enabled: true,
            result: Some(SourceSwitchResult::from_raw(0xab))
        }
    );
    for payload in [
        &[0x37, 1, 0, 0][..],
        &[0x37, 1, 2][..],
        &[0x39, 1, 0][..],
        &[0x39, 1, 2, 0][..],
        &[0x39, 1, 0, 0, 0][..],
    ] {
        let decoded = if payload[0] == 0x37 {
            PeripheralParameters::decode_response(T2, payload)
        } else {
            PeripheralParameters::decode_notification(T2, payload)
        };
        assert!(decoded.is_err());
    }
    assert!(PeripheralParameters::decode_response(T1, &[0x37, 1, 0]).is_err());
}

#[test]
fn source_preferences_use_wire_on_off_and_matching_fixed_capabilities() {
    let source = cap(PeripheralKind::SourceSwitch);
    let handover = cap(PeripheralKind::MusicHandover);
    for (enabled, byte) in [(true, 0), (false, 1)] {
        assert_eq!(
            PeripheralSetting::SourceKeep(enabled)
                .encode_set(&source)
                .unwrap(),
            [0x38, 1, byte]
        );
        assert_eq!(
            PeripheralSetting::MusicHandover(enabled)
                .encode_set(&handover)
                .unwrap(),
            [0x38, 3, byte]
        );
        assert_eq!(
            PeripheralParameters::decode_response(T2, &[0x37, 3, byte]).unwrap(),
            PeripheralParameters::MusicHandover(enabled)
        );
        assert_eq!(
            PeripheralParameters::decode_notification(T2, &[0x39, 3, byte]).unwrap(),
            PeripheralParameters::MusicHandover(enabled)
        );
    }
    assert!(
        PeripheralSetting::SourceKeep(true)
            .encode_set(&handover)
            .is_err()
    );
    assert!(
        PeripheralSetting::MusicHandover(false)
            .encode_set(&source)
            .is_err()
    );
    assert!(
        PeripheralSetting::SourceKeep(true)
            .encode_set(&cap(PeripheralKind::Devices))
            .is_err()
    );
    assert!(PeripheralParameters::decode_response(T2, &[0x37, 3, 2]).is_err());
    assert!(PeripheralParameters::decode_notification(T2, &[0x39, 3, 0, 0]).is_err());
}

#[test]
fn targeted_operations_encode_exact_actions_and_canonical_addresses() {
    let target = BluetoothAddress::parse_ascii(b"02:00:00:00:ab:01").unwrap();
    for inquiry in [0, 2] {
        let records = list(inquiry, &[(b"02:00:00:00:ab:01", 1, b"Laptop")]);
        let capability = cap(records.kind());
        for (operation, action) in [
            (PeripheralOperation::Connect(target), 1),
            (PeripheralOperation::Disconnect(target), 0),
        ] {
            let encoded = operation.encode(&capability, &records).unwrap();
            let mut expected = vec![0x3c, inquiry, action];
            expected.extend_from_slice(ADDRESS);
            assert_eq!(encoded.payload, expected);
            assert_eq!(encoded.expected_result.kind(), records.kind());
            assert_eq!(encoded.expected_result.address(), target);
        }
        let switched = PeripheralOperation::SwitchSource(target)
            .encode(&cap(PeripheralKind::SourceSwitch), &records)
            .unwrap();
        assert_eq!(switched.payload, b"\x3c\x0102:00:00:00:AB:01");
    }
}

#[test]
fn operations_reject_missing_ambiguous_or_mismatched_targets() {
    let kind = PeripheralKind::Devices;
    let missing = list(0, &[(OTHER_ADDRESS, 1, b"Other")]);
    let duplicate = list(
        0,
        &[(ADDRESS, 1, b"One"), (b"02:00:00:00:ab:01", 2, b"Two")],
    );
    let disconnected = list(0, &[(ADDRESS, 0, b"Offline")]);
    for records in [&missing, &duplicate] {
        for operation in [
            PeripheralOperation::Connect(address()),
            PeripheralOperation::Disconnect(address()),
            PeripheralOperation::SwitchSource(address()),
        ] {
            assert!(operation.encode(&cap(kind), records).is_err());
            assert!(
                operation
                    .encode(&cap(PeripheralKind::SourceSwitch), records)
                    .is_err()
            );
        }
    }
    assert!(
        PeripheralOperation::SwitchSource(address())
            .encode(&cap(PeripheralKind::SourceSwitch), &disconnected)
            .is_err()
    );
    assert!(
        PeripheralOperation::Connect(address())
            .encode(&cap(PeripheralKind::DevicesWithClass), &disconnected)
            .is_err()
    );
    assert!(
        PeripheralOperation::Disconnect(address())
            .encode(&cap(PeripheralKind::SourceSwitch), &disconnected)
            .is_err()
    );
    assert!(
        PeripheralOperation::SwitchSource(address())
            .encode(&cap(kind), &disconnected)
            .is_err()
    );
    // Occupancy is dynamic metadata, not permission inferred from max capacities.
    let zero_capacity = PeripheralCapability::decode_response(T2, &[0x31, 0, 0, 0, 1]).unwrap();
    assert!(
        PeripheralOperation::Connect(address())
            .encode(&zero_capacity, &disconnected)
            .is_ok()
    );
}

#[test]
fn connectivity_results_distinguish_progress_busy_failure_and_unknown() {
    let outcomes = [
        OperationOutcome::Succeeded,
        OperationOutcome::Failed,
        OperationOutcome::InProgress,
        OperationOutcome::Busy,
    ];
    for inquiry in [0, 2] {
        for action in 0..=2 {
            for (suffix, expected) in outcomes.into_iter().enumerate() {
                let code = action * 16 + u8::try_from(suffix).unwrap();
                let report = PeripheralResultReport::decode_notification(
                    T2,
                    &result(inquiry, action, code, ADDRESS),
                )
                .unwrap();
                assert_eq!(report.outcome(), expected);
                let PeripheralResultData::Connectivity {
                    action: parsed_action,
                    result: parsed_result,
                    address: parsed_address,
                } = report.data
                else {
                    panic!("expected connection result")
                };
                assert_eq!(parsed_action.as_raw(), action);
                assert_eq!(parsed_result.as_raw(), code);
                assert_eq!(parsed_address, address());
            }
        }
    }
    // Unknown actions, unknown results and mismatched result families remain readable.
    for (action, code) in [
        (0, 0x10),
        (1, 0),
        (2, 0x10),
        (3, 0x30),
        (0xff, 0xff),
        (1, 0x14),
        (1, 0x30),
    ] {
        let report =
            PeripheralResultReport::decode_notification(T2, &result(0, action, code, ADDRESS))
                .unwrap();
        assert_eq!(report.outcome(), OperationOutcome::Unknown);
        let PeripheralResultData::Connectivity {
            action: a,
            result: r,
            ..
        } = report.data
        else {
            panic!("expected raw identities")
        };
        assert_eq!((a.as_raw(), r.as_raw()), (action, code));
    }
    assert_eq!(ConnectivityAction::CONNECT.as_raw(), 1);
    assert_eq!(ConnectivityAction::DISCONNECT.as_raw(), 0);
    assert_eq!(ConnectivityAction::UNPAIR.as_raw(), 2);
}

#[test]
fn source_switch_results_preserve_failures_and_unknown_values() {
    for (code, expected) in [
        (0, OperationOutcome::Succeeded),
        (1, OperationOutcome::Failed),
        (2, OperationOutcome::Failed),
        (3, OperationOutcome::Failed),
        (4, OperationOutcome::Failed),
        (5, OperationOutcome::Unknown),
        (255, OperationOutcome::Unknown),
    ] {
        let report =
            PeripheralResultReport::decode_notification(T2, &result(1, 0, code, ADDRESS)).unwrap();
        assert_eq!(report.outcome(), expected);
        let PeripheralResultData::SourceSwitch {
            result,
            address: parsed,
        } = report.data
        else {
            panic!("expected source switch result")
        };
        assert_eq!(result.as_raw(), code);
        assert_eq!(parsed, address());
    }
}

#[test]
fn operation_matching_requires_kind_action_and_address_but_does_not_confirm_success() {
    let records = list(0, &[(ADDRESS, 1, b"Laptop")]);
    let command = PeripheralOperation::Connect(address())
        .encode(&cap(records.kind()), &records)
        .unwrap();
    for code in [0x10, 0x11, 0x12, 0x13, 0xff] {
        let report =
            PeripheralResultReport::decode_notification(T2, &result(0, 1, code, ADDRESS)).unwrap();
        assert!(command.expected_result.matches(&report));
        assert_eq!(
            report.outcome() == OperationOutcome::Succeeded,
            code == 0x10
        );
    }
    for bytes in [
        result(2, 1, 0x10, ADDRESS),
        result(0, 0, 0, ADDRESS),
        result(0, 1, 0x10, OTHER_ADDRESS),
        result(1, 0, 0, ADDRESS),
    ] {
        assert!(
            !command
                .expected_result
                .matches(&PeripheralResultReport::decode_notification(T2, &bytes).unwrap())
        );
    }
    let switch = PeripheralOperation::SwitchSource(address())
        .encode(&cap(PeripheralKind::SourceSwitch), &records)
        .unwrap();
    assert!(switch.expected_result.matches(
        &PeripheralResultReport::decode_notification(T2, &result(1, 0, 0, ADDRESS)).unwrap()
    ));
    assert!(!switch.expected_result.matches(
        &PeripheralResultReport::decode_notification(T2, &result(1, 0, 0, OTHER_ADDRESS)).unwrap()
    ));
}

#[test]
fn targeted_result_decoders_reject_wrong_table_opcode_length_and_address() {
    for inquiry in [0, 1, 2] {
        let payload = result(inquiry, 1, 0x10, ADDRESS);
        for length in 0..payload.len() {
            assert!(PeripheralResultReport::decode_notification(T2, &payload[..length]).is_err());
        }
        let mut trailing = payload.clone();
        trailing.push(0);
        assert!(PeripheralResultReport::decode_notification(T2, &trailing).is_err());
        assert!(PeripheralResultReport::decode_notification(T1, &payload).is_err());
        let mut wrong_opcode = payload.clone();
        wrong_opcode[0] = 0x39;
        assert!(PeripheralResultReport::decode_notification(T2, &wrong_opcode).is_err());
        let mut bad_address = payload;
        *bad_address.last_mut().unwrap() = b'G';
        assert!(PeripheralResultReport::decode_notification(T2, &bad_address).is_err());
    }
    assert!(PeripheralResultReport::decode_notification(T2, &result(3, 1, 0, ADDRESS)).is_err());
}
