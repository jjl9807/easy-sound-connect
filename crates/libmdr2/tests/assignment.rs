use libmdr2::{
    assignment::{
        AssignmentAction as Action, AssignmentCapability as Capability,
        AssignmentCustomParameters as Custom, AssignmentFunction as Function, AssignmentKey as Key,
        AssignmentKind as Kind, AssignmentParameters as Parameters, AssignmentPreset as Preset,
        AssignmentStatus as Status,
    },
    command::{Notification, Request, Response},
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// Synthetic protocol example, including a service preset and an unknown key.
// No device/model assumptions or reference implementation code are used.
fn capability_bytes(kind: Kind) -> Vec<u8> {
    let mut bytes = vec![0xf1, kind.inquiry()];
    if kind == Kind::WithLimitations {
        bytes.push(1);
    }
    bytes.extend([
        3, 0, 1, 0, 3, 0, 1, 0, 0, 2, 0x20, 1, 0, 0, 0x20, 0x71, 0, 2, 1, 0x21, 2, 0x21, 0x43,
        0x10, 0x23, 2, 0x23, 0x30, 1, 0, 0x31, 1, 0x31, 1, 0, 0, 0x32, 0xee, 0xee, 0xee, 1, 0xee,
        1, 0, 0xee, 0xee,
    ]);
    bytes
}
fn capability(kind: Kind) -> Capability {
    Capability::decode_response(T1, &capability_bytes(kind)).unwrap()
}

#[test]
fn shared_system_routes_select_assignment_from_inquiry() {
    for kind in [Kind::Standard, Kind::WithLimitations] {
        let request = Request::AssignmentCapability(kind).encode().unwrap();
        assert_eq!(request.table, T1);
        assert_eq!(request.payload, [0xf0, kind.inquiry()]);
        assert!(matches!(
            request.expected_response.decode(T1, &capability_bytes(kind)),
            Ok(Response::AssignmentCapability(cap)) if cap.kind() == kind
        ));
        assert!(
            request
                .expected_response
                .decode(T2, &capability_bytes(kind))
                .is_err()
        );
        let inquiries = [
            (Request::AssignmentStatus(kind), 0xf2),
            (Request::AssignmentParameters(kind), 0xf6),
            (Request::AssignmentCustomParameters(kind), 0xfa),
        ];
        for (request, opcode) in inquiries {
            assert_eq!(request.encode().unwrap().payload, [opcode, kind.inquiry()]);
        }
        assert!(matches!(
            Notification::decode(T1, &[0xf5, kind.inquiry(), 1, 0]),
            Ok(Notification::AssignmentStatus(_))
        ));
        assert!(matches!(
            Notification::decode(T1, &[0xf9, kind.inquiry(), 1, 0]),
            Ok(Notification::AssignmentParameters(_))
        ));
        let mut custom = custom_bytes(kind);
        custom[0] = 0xfd;
        assert!(matches!(
            Notification::decode(T1, &custom),
            Ok(Notification::AssignmentCustomParameters(_))
        ));
    }
}
fn current(kind: Kind, left: u8) -> Parameters {
    Parameters::decode_response(T1, &[0xf7, kind.inquiry(), 3, left, 0x31, 0xee]).unwrap()
}
fn custom_bytes(kind: Kind) -> Vec<u8> {
    vec![
        0xfb,
        kind.inquiry(),
        3,
        0x31,
        1,
        0,
        0x32,
        0x71,
        3,
        1,
        0x43,
        0x10,
        0x30,
        0xee,
        0xcc,
        0xee,
        1,
        0xee,
        0xee,
    ]
}

#[test]
fn nested_capabilities_preserve_order_defaults_and_unknown_identities() {
    for kind in [Kind::Standard, Kind::WithLimitations] {
        let cap = capability(kind);
        assert_eq!(cap.kind(), kind);
        assert_eq!(
            cap.limitation().map(|value| value.as_raw()),
            if kind == Kind::Standard {
                None
            } else {
                Some(1)
            }
        );
        assert_eq!(cap.keys().len(), 3);
        let first = &cap.keys()[0];
        assert_eq!(first.key, Key::LEFT);
        assert_eq!(first.default_preset, Preset::AMBIENT_SOUND);
        assert_eq!(first.presets[0].fixed[0].function, Function::NC_AMBIENT);
        assert_eq!(
            first.presets[2].customizable[0]
                .functions
                .iter()
                .map(|value| value.as_raw())
                .collect::<Vec<_>>(),
            [0x21, 0x43]
        );
        assert_eq!(cap.keys()[1].presets[0].preset.as_raw(), 0x31);
        assert_eq!(cap.keys()[2].key.as_raw(), 0xee);
        assert_eq!(cap.keys()[2].key_type.as_raw(), 0xee);
        assert_eq!(cap.keys()[2].presets[0].fixed[0].action.as_raw(), 0xee);
    }
}

#[test]
fn function_identity_selects_layout_and_command_table() {
    for (kind, code, inquiry) in [
        (Kind::Standard, 0xf3, 3),
        (Kind::WithLimitations, 0xfe, 0x0e),
    ] {
        assert_eq!(kind.function_id(), FunctionId { table: T1, code });
        assert_eq!(
            Kind::from_function(FunctionId { table: T1, code }),
            Some(kind)
        );
        assert_eq!(Kind::from_function(FunctionId { table: T2, code }), None);
        assert_eq!(Kind::try_from(inquiry).unwrap(), kind);
    }
    for inquiry in [0, 2, 4, 0x0d, 0xff] {
        assert!(Kind::try_from(inquiry).is_err());
    }
}

#[test]
fn preset_change_keeps_service_and_unknown_keys_unchanged() {
    for kind in [Kind::Standard, Kind::WithLimitations] {
        let cap = capability(kind);
        let snapshot = current(kind, 0);
        assert_eq!(
            snapshot
                .encode_preset_change(&cap, Key::LEFT, Preset::PLAYBACK, None)
                .unwrap(),
            [0xf8, kind.inquiry(), 3, 0x20, 0x31, 0xee]
        );
        assert_eq!(
            snapshot
                .presets()
                .iter()
                .map(|value| value.as_raw())
                .collect::<Vec<_>>(),
            [0, 0x31, 0xee]
        );
        for forbidden in [0x31, 0x36, 0xee] {
            assert!(
                snapshot
                    .encode_preset_change(&cap, Key::LEFT, Preset::from_raw(forbidden), None)
                    .is_err()
            );
        }
        assert!(
            snapshot
                .encode_preset_change(&cap, Key::from_raw(0xee), Preset::NONE, None)
                .is_err()
        );
    }
}

#[test]
fn a_local_preset_identity_does_not_authorize_service_actions() {
    // A playback-named preset contains an assistant action; selecting it is rejected.
    let cap =
        Capability::decode_response(T1, &[0xf1, 3, 1, 0, 0, 0, 1, 0x20, 1, 0, 0, 0x30]).unwrap();
    let snapshot = Parameters::decode_response(T1, &[0xf7, 3, 1, 0]).unwrap();
    assert!(
        snapshot
            .encode_preset_change(&cap, Key::LEFT, Preset::PLAYBACK, None)
            .is_err()
    );
    // A duplicated fixed action has no unambiguous activation map.
    let cap =
        Capability::decode_response(T1, &[0xf1, 3, 1, 0, 0, 0, 1, 0x20, 2, 0, 0, 0x20, 0, 0x21])
            .unwrap();
    assert!(
        snapshot
            .encode_preset_change(&cap, Key::LEFT, Preset::PLAYBACK, None)
            .is_err()
    );
}

#[test]
fn selecting_a_custom_preset_requires_a_complete_known_local_action_map() {
    let cap = capability(Kind::Standard);
    let snapshot = current(Kind::Standard, 0);
    assert!(
        snapshot
            .encode_preset_change(&cap, Key::LEFT, Preset::CUSTOM_1, None)
            .is_err()
    );
    let map = Custom::decode_response(T1, &[0xfb, 3, 1, 0x71, 2, 1, 0x21, 0x10, 0x23]).unwrap();
    assert_eq!(
        snapshot
            .encode_preset_change(&cap, Key::LEFT, Preset::CUSTOM_1, Some(&map))
            .unwrap(),
        [0xf8, 3, 3, 0x71, 0x31, 0xee]
    );
    for bytes in [
        vec![0xfb, 3, 1, 0x71, 1, 1, 0x21],
        vec![0xfb, 3, 1, 0x71, 2, 1, 0x43, 0x10, 0x23],
        vec![0xfb, 3, 1, 0x71, 2, 1, 0x21, 0x10, 0x24],
        vec![0xfb, 3, 1, 0x71, 2, 1, 0x21, 1, 0x21],
        vec![0xfb, 3, 1, 0x71, 2, 1, 0x21, 0xee, 0x23],
    ] {
        let map = Custom::decode_response(T1, &bytes).unwrap();
        assert!(
            snapshot
                .encode_preset_change(&cap, Key::LEFT, Preset::CUSTOM_1, Some(&map))
                .is_err()
        );
    }
}

#[test]
fn editing_a_custom_function_preserves_every_unrelated_raw_field() {
    for kind in [Kind::Standard, Kind::WithLimitations] {
        let cap = capability(kind);
        let snapshot = current(kind, 0x71);
        let bytes = custom_bytes(kind);
        let map = Custom::decode_response(T1, &bytes).unwrap();
        assert_eq!(map.records()[0].preset.as_raw(), 0x31);
        let payload = map
            .encode_function_change(
                &cap,
                &snapshot,
                Key::LEFT,
                Action::DOUBLE_TAP,
                Function::NEXT_TRACK,
            )
            .unwrap();
        assert_eq!(
            payload,
            [
                0xfc,
                kind.inquiry(),
                3,
                0x31,
                1,
                0,
                0x32,
                0x71,
                3,
                1,
                0x21,
                0x10,
                0x30,
                0xee,
                0xcc,
                0xee,
                1,
                0xee,
                0xee,
            ]
        );
        assert_eq!(map.records()[1].bindings[0].function.as_raw(), 0x43);
        for (action, function) in [
            (Action::DOUBLE_TAP, Function::from_raw(0x43)),
            (Action::DOUBLE_TAP, Function::VOLUME_UP),
            (Action::from_raw(0xee), Function::NONE),
        ] {
            assert!(
                map.encode_function_change(&cap, &snapshot, Key::LEFT, action, function)
                    .is_err()
            );
        }
    }
}

#[test]
fn custom_records_are_preset_keyed_and_ambiguous_targets_are_rejected() {
    let cap = capability(Kind::Standard);
    let current = current(Kind::Standard, 0x71);
    for bytes in [
        vec![0xfb, 3, 1, 0x72, 1, 1, 0x21],
        vec![0xfb, 3, 2, 0x71, 1, 1, 0x21, 0x71, 1, 1, 0x21],
        vec![0xfb, 3, 1, 0x71, 2, 1, 0x21, 1, 0x21],
    ] {
        let map = Custom::decode_response(T1, &bytes).unwrap();
        assert!(
            map.encode_function_change(
                &cap,
                &current,
                Key::LEFT,
                Action::DOUBLE_TAP,
                Function::NEXT_TRACK
            )
            .is_err()
        );
    }
    let ambiguous = Parameters::decode_response(T1, &[0xf7, 3, 3, 0x71, 0x71, 0xee]).unwrap();
    let map = Custom::decode_response(T1, &custom_bytes(Kind::Standard)).unwrap();
    assert!(
        map.encode_function_change(
            &cap,
            &ambiguous,
            Key::LEFT,
            Action::DOUBLE_TAP,
            Function::NEXT_TRACK
        )
        .is_err()
    );
}

#[test]
fn status_and_parameter_lists_validate_key_counts_without_losing_unknowns() {
    let cap = capability(Kind::Standard);
    let status = Status::decode_response(T1, &[0xf3, 3, 3, 0, 1, 0]).unwrap();
    assert_eq!(status.enabled(), [true, false, true]);
    status.validate_capability(&cap).unwrap();
    let params = current(Kind::Standard, 0xee);
    params.validate_capability(&cap).unwrap();
    assert!(
        Status::decode_response(T1, &[0xf3, 3, 1, 0])
            .unwrap()
            .validate_capability(&cap)
            .is_err()
    );
    assert!(
        Parameters::decode_response(T1, &[0xf7, 3, 1, 0])
            .unwrap()
            .validate_capability(&cap)
            .is_err()
    );
    for invalid in 2..=u8::MAX {
        assert!(Status::decode_response(T1, &[0xf3, 3, 1, invalid]).is_err());
    }
}

#[test]
fn unknown_limitations_and_mismatched_capabilities_cannot_authorize_writes() {
    let standard = current(Kind::Standard, 0);
    assert!(
        standard
            .encode_preset_change(
                &capability(Kind::WithLimitations),
                Key::LEFT,
                Preset::PLAYBACK,
                None
            )
            .is_err()
    );
    let mut bytes = capability_bytes(Kind::WithLimitations);
    bytes[2] = 0xee;
    let cap = Capability::decode_response(T1, &bytes).unwrap();
    assert_eq!(cap.limitation().unwrap().as_raw(), 0xee);
    let current = current(Kind::WithLimitations, 0);
    assert!(
        current
            .encode_preset_change(&cap, Key::LEFT, Preset::PLAYBACK, None)
            .is_err()
    );
}

#[test]
fn malformed_nested_counts_fail_without_partial_capabilities() {
    for bytes in [
        vec![0xf1, 3, 0],
        vec![0xf1, 3, 1, 0, 0, 0, 0],
        vec![0xf1, 3, 1, 0, 0, 0, 1, 0, 0, 0],
        vec![0xf1, 3, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0],
        vec![0xf1, 3, 1, 0, 0, 0, 1, 0, 255, 0],
    ] {
        assert!(Capability::decode_response(T1, &bytes).is_err());
    }
    for bytes in [
        vec![0xfb, 3, 0],
        vec![0xfb, 3, 1, 0, 0],
        vec![0xfb, 3, 1, 0, 255],
    ] {
        assert!(Custom::decode_response(T1, &bytes).is_err());
    }
}

#[test]
fn all_lengths_opcodes_tables_and_variants_are_checked() {
    for kind in [Kind::Standard, Kind::WithLimitations] {
        let cap = capability_bytes(kind);
        for length in 0..cap.len() {
            assert!(Capability::decode_response(T1, &cap[..length]).is_err());
        }
        let mut extra = cap.clone();
        extra.push(0);
        assert!(Capability::decode_response(T1, &extra).is_err());
        assert!(Capability::decode_response(T2, &cap).is_err());
        let map = custom_bytes(kind);
        for length in 0..map.len() {
            assert!(Custom::decode_response(T1, &map[..length]).is_err());
        }
        extra = map.clone();
        extra.push(0);
        assert!(Custom::decode_response(T1, &extra).is_err());
        assert!(Custom::decode_notification(T1, &map).is_err());
        extra = map;
        extra[0] = 0xfd;
        assert!(Custom::decode_notification(T1, &extra).is_ok());
        for (response, notification) in [(0xf3, 0xf5), (0xf7, 0xf9)] {
            let bytes = [response, kind.inquiry(), 2, 0, 1];
            for length in 0..bytes.len() {
                if response == 0xf3 {
                    assert!(Status::decode_response(T1, &bytes[..length]).is_err());
                } else {
                    assert!(Parameters::decode_response(T1, &bytes[..length]).is_err());
                }
            }
            let update = [notification, kind.inquiry(), 2, 0, 1];
            if response == 0xf3 {
                assert!(Status::decode_notification(T1, &update).is_ok());
                assert!(Status::decode_response(T1, &update).is_err());
                assert!(Status::decode_response(T2, &bytes).is_err());
            } else {
                assert!(Parameters::decode_notification(T1, &update).is_ok());
                assert!(Parameters::decode_response(T1, &update).is_err());
                assert!(Parameters::decode_response(T2, &bytes).is_err());
            }
        }
    }
}

#[test]
fn unsigned_counts_accept_255_entries_and_preserve_wire_order() {
    let mut cap = vec![0xf1, 3, 255];
    for key in 0..255 {
        cap.extend([key, 0, 0, 1, 0, 1, 0, 0, 0]);
    }
    let cap = Capability::decode_response(T1, &cap).unwrap();
    assert_eq!(cap.keys().len(), 255);
    assert_eq!(cap.keys()[254].key.as_raw(), 254);
    let mut status = vec![0xf3, 3, 255];
    status.extend([0; 255]);
    Status::decode_response(T1, &status)
        .unwrap()
        .validate_capability(&cap)
        .unwrap();
    let mut parameters = vec![0xf7, 3, 255];
    parameters.extend(0..255);
    let params = Parameters::decode_response(T1, &parameters).unwrap();
    params.validate_capability(&cap).unwrap();
    assert_eq!(params.presets()[254].as_raw(), 254);
    let mut custom = vec![0xfb, 3, 1, 0x71, 255];
    for action in 0..255 {
        custom.extend([action, 0]);
    }
    let map = Custom::decode_response(T1, &custom).unwrap();
    assert_eq!(map.records()[0].bindings.len(), 255);
    assert_eq!(map.records()[0].bindings[254].action.as_raw(), 254);
}
