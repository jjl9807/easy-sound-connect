use libmdr2::{
    command::CommandError,
    noise::{
        AdaptiveSensitivity, AmbientLevelRange, AmbientMode, ButtonCycle, ButtonKey, ChangeStatus,
        NcValue, NoiseAvailability, NoiseCapability, NoiseKind, NoiseMode, NoiseParameters,
        NoiseSettings, NoiseStatus, NoiseSwitch,
    },
    types::{CommandTable, FunctionId},
};

const TABLE: CommandTable = CommandTable::TABLE_1;

// Short payload data from sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39,
// sonyctl/sim.py INITIAL: WH-1000XM6 firmware 3.1.5. The response opcode and
// inquiry are restored around the stored body; this is not a transport capture.
const XM6_CAPABILITY: &[u8] = &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1];
const XM6_PARAMETERS: &[u8] = &[0x67, 0x19, 1, 1, 1, 1, 20, 0, 0];

#[test]
fn xm6_fixture_decodes_capability_availability_and_complete_parameters() {
    let capability = NoiseCapability::decode_response(TABLE, XM6_CAPABILITY).unwrap();
    assert_eq!(capability.kind(), NoiseKind::AdaptiveNcAndAmbientLevel);
    assert_eq!(capability.ambient_modes().len(), 2);
    assert_eq!(capability.ambient_modes()[0].mode, AmbientMode::Normal);
    assert_eq!(capability.ambient_modes()[1].mode, AmbientMode::Voice);
    let range = capability.ambient_modes()[1].levels.unwrap();
    assert_eq!((range.minimum(), range.maximum(), range.step()), (1, 20, 1));
    assert_eq!(capability.button_key(), None);

    let status = NoiseStatus::decode_response(TABLE, &[0x63, 0x19, 0]).unwrap();
    assert_eq!(status.kind(), NoiseKind::AdaptiveNcAndAmbientLevel);
    assert_eq!(status.availability, NoiseAvailability::Enabled);
    let parameters = NoiseParameters::decode_response(TABLE, XM6_PARAMETERS).unwrap();
    assert_eq!(
        parameters,
        NoiseParameters::Control {
            change: ChangeStatus::Changed,
            effect: NoiseSwitch::On,
            settings: NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Voice,
                level: 20,
                adaptive: NoiseSwitch::Off,
                sensitivity: AdaptiveSensitivity::Standard,
            },
        }
    );
    assert_eq!(
        parameters.encode_set(&capability).unwrap(),
        [0x68, 0x19, 1, 1, 1, 1, 20, 0, 0]
    );
}

#[test]
fn turning_effect_off_preserves_adaptive_settings_and_drag_state() {
    let capability = NoiseCapability::decode_response(TABLE, XM6_CAPABILITY).unwrap();
    let mut parameters =
        NoiseParameters::decode_notification(TABLE, &[0x69, 0x19, 0, 1, 1, 1, 7, 1, 2]).unwrap();
    let NoiseParameters::Control { effect, .. } = &mut parameters else {
        panic!()
    };
    *effect = NoiseSwitch::Off;
    assert_eq!(
        parameters.encode_set(&capability).unwrap(),
        [0x68, 0x19, 0, 0, 1, 1, 7, 1, 2]
    );
}

#[test]
fn button_cycle_fixture_has_no_control_header() {
    let capability = NoiseCapability::decode_response(TABLE, &[0x61, 0x30, 4]).unwrap();
    assert_eq!(capability.button_key(), Some(ButtonKey::NcAmb));
    assert!(capability.ambient_modes().is_empty());
    let parameters = NoiseParameters::decode_response(TABLE, &[0x67, 0x30, 2]).unwrap();
    assert_eq!(
        parameters,
        NoiseParameters::ButtonCycle(ButtonCycle::NcAmbient)
    );
    assert_eq!(parameters.encode_set(&capability).unwrap(), [0x68, 0x30, 2]);
    for cycle in 1..=4 {
        assert_eq!(
            NoiseParameters::ButtonCycle(cycle.try_into().unwrap())
                .encode_set(&capability)
                .unwrap(),
            [0x68, 0x30, cycle],
        );
    }
    for cycle in [0, 5, 80, 255] {
        assert!(NoiseParameters::decode_response(TABLE, &[0x67, 0x30, cycle]).is_err());
    }
    assert!(NoiseParameters::decode_response(TABLE, &[0x67, 0x30, 1, 1, 2]).is_err());
}

#[test]
fn nc_only_uses_static_capability_and_keeps_its_two_switches() {
    let capability = NoiseCapability::nc_only();
    assert_eq!(
        capability,
        NoiseCapability::decode_response(TABLE, &[0x61, 1]).unwrap()
    );
    assert!(!capability.kind().has_capability_query());
    let parameters = NoiseParameters::decode_response(TABLE, &[0x67, 1, 1, 0, 1]).unwrap();
    assert_eq!(
        parameters,
        NoiseParameters::Control {
            change: ChangeStatus::Changed,
            effect: NoiseSwitch::Off,
            settings: NoiseSettings::NcOnly {
                nc: NoiseSwitch::On
            },
        }
    );
    assert_eq!(
        parameters.encode_set(&capability).unwrap(),
        [0x68, 1, 1, 0, 1]
    );
}

#[test]
fn functions_select_layouts_without_a_model_allowlist() {
    let mappings = [
        (0x61, 0x01, NoiseKind::NcOnly),
        (0x63, 0x12, NoiseKind::NcModesAndAmbientSwitch),
        (0x64, 0x13, NoiseKind::NcAndAmbientLevel),
        (0x65, 0x14, NoiseKind::NcModesAndAmbientLevel),
        (0x68, 0x15, NoiseKind::AutoNcAndAmbientLevel),
        (0x6a, 0x16, NoiseKind::DualSingleNcAndAmbientLevel),
        (0x6b, 0x17, NoiseKind::DualNcAndAmbientLevel),
        (0x6d, 0x19, NoiseKind::AdaptiveNcAndAmbientLevel),
        (0x67, 0x22, NoiseKind::AmbientLevel),
        (0x69, 0x30, NoiseKind::ButtonCycle),
    ];
    for (code, inquiry, kind) in mappings {
        let function = FunctionId { table: TABLE, code };
        assert_eq!(NoiseKind::from_function(function), Some(kind));
        assert_eq!(kind.function_id(), function);
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(NoiseKind::try_from(inquiry), Ok(kind));
        assert_eq!(kind.has_capability_query(), inquiry != 1);
        assert_eq!(
            NoiseKind::from_function(FunctionId {
                table: CommandTable::TABLE_2,
                code
            }),
            None
        );
    }
    for unsupported in [0x62, 0x66, 0x6c, 0x70, 0xff] {
        assert_eq!(
            NoiseKind::from_function(FunctionId {
                table: TABLE,
                code: unsupported
            }),
            None
        );
    }
    for unsupported in [0x11, 0x18, 0x21, 0x40, 0xff] {
        assert!(NoiseKind::try_from(unsupported).is_err());
    }
}

#[test]
fn each_control_layout_preserves_its_distinct_wire_fields() {
    // Independently constructed examples exercise each layout's distinct field
    // order and length; these are synthetic protocol examples, not captures.
    let examples: &[(&[u8], NoiseSettings)] = &[
        (
            &[0x67, 0x12, 0, 1, 2, 1, 0],
            NoiseSettings::NcModesAndAmbientSwitch {
                nc: NcValue::Dual,
                ambient_mode: AmbientMode::Voice,
                ambient: NoiseSwitch::Off,
            },
        ),
        (
            &[0x67, 0x13, 0, 1, 1, 1, 7],
            NoiseSettings::NcAndAmbientLevel {
                nc: NoiseSwitch::On,
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
        (
            &[0x67, 0x14, 0, 1, 2, 1, 7],
            NoiseSettings::NcModesAndAmbientLevel {
                nc: NcValue::Dual,
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
        (
            &[0x67, 0x15, 0, 1, 0, 5, 1, 7],
            NoiseSettings::AutoNcAndAmbientLevel {
                mode: NoiseMode::NoiseCancelling,
                nc: NcValue::AutoDual,
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
        (
            &[0x67, 0x16, 0, 1, 1, 1, 1, 7],
            NoiseSettings::DualSingleNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                nc: NcValue::Single,
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
        (
            &[0x67, 0x17, 0, 1, 1, 1, 7],
            NoiseSettings::DualNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
        (
            &[0x67, 0x22, 0, 1, 1, 7],
            NoiseSettings::AmbientLevel {
                ambient_mode: AmbientMode::Voice,
                level: 7,
            },
        ),
    ];
    for &(bytes, settings) in examples {
        let parameters = NoiseParameters::decode_response(TABLE, bytes).unwrap();
        assert_eq!(
            parameters,
            NoiseParameters::Control {
                change: ChangeStatus::Changing,
                effect: NoiseSwitch::On,
                settings
            }
        );
        let capability_bytes = if bytes[1] == 0x12 {
            vec![0x61, 0x12, 2, 0, 1]
        } else {
            vec![0x61, bytes[1], 1, 1, 1, 21, 3]
        };
        let capability = NoiseCapability::decode_response(TABLE, &capability_bytes).unwrap();
        let mut expected_write = bytes.to_vec();
        expected_write[0] = 0x68;
        assert_eq!(parameters.encode_set(&capability).unwrap(), expected_write);
        let mut notification = bytes.to_vec();
        notification[0] = 0x69;
        assert_eq!(
            NoiseParameters::decode_notification(TABLE, &notification).unwrap(),
            parameters
        );
    }
}

#[test]
fn range_steps_include_the_clamped_terminal_and_cannot_overflow() {
    let range = AmbientLevelRange::new(1, 6, 2).unwrap();
    for value in 0..=255 {
        assert_eq!(range.contains(value), [1, 3, 5, 6].contains(&value));
    }
    let near_limit = AmbientLevelRange::new(254, 255, 255).unwrap();
    for value in 0..=255 {
        assert_eq!(near_limit.contains(value), value >= 254);
    }
    let single = AmbientLevelRange::new(5, 5, 20).unwrap();
    assert!(single.contains(5));
    assert!(!single.contains(4));
    for (minimum, maximum, step) in [(255, 255, 1), (0, 0, 1), (8, 7, 1), (0, 20, 0)] {
        assert!(AmbientLevelRange::new(minimum, maximum, step).is_err());
    }
}

#[test]
fn writes_enforce_mode_specific_ranges_including_while_effect_is_off() {
    let capability =
        NoiseCapability::decode_response(TABLE, &[0x61, 0x22, 2, 0, 1, 6, 2, 1, 10, 20, 5])
            .unwrap();
    for mode in [0, 1] {
        for level in 0..=255 {
            let parameters =
                NoiseParameters::decode_response(TABLE, &[0x67, 0x22, 1, 0, mode, level]).unwrap();
            let permitted = if mode == 0 {
                [1, 3, 5, 6].contains(&level)
            } else {
                [10, 15, 20].contains(&level)
            };
            assert_eq!(
                parameters.encode_set(&capability).is_ok(),
                permitted,
                "mode={mode}, level={level}"
            );
        }
    }
}

#[test]
fn nc_only_states_of_seamless_layouts_allow_inactive_zero_outside_ambient_range() {
    for (inquiry, active_nc) in [(0x13, 1), (0x14, 1), (0x14, 2)] {
        let capability =
            NoiseCapability::decode_response(TABLE, &[0x61, inquiry, 1, 0, 1, 20, 1]).unwrap();
        for effect in [0, 1] {
            let parameters = NoiseParameters::decode_response(
                TABLE,
                &[0x67, inquiry, 1, effect, active_nc, 0, 0],
            )
            .unwrap();
            assert_eq!(
                parameters.encode_set(&capability).unwrap(),
                [0x68, inquiry, 1, effect, active_nc, 0, 0]
            );
        }
        for (nc, mode, level) in [(0, 0, 0), (active_nc, 1, 0), (active_nc, 0, 21)] {
            let parameters =
                NoiseParameters::decode_response(TABLE, &[0x67, inquiry, 1, 1, nc, mode, level])
                    .unwrap();
            assert!(parameters.encode_set(&capability).is_err());
        }
    }
    // A dedicated NC/ambient selector retains an ordinary stored ambient
    // level; selecting NC is not a general exemption from its level range.
    for inquiry in [0x15, 0x16, 0x17, 0x19, 0x22] {
        let capability =
            NoiseCapability::decode_response(TABLE, &[0x61, inquiry, 1, 0, 1, 20, 1]).unwrap();
        let bytes = match inquiry {
            0x15 | 0x16 => vec![0x67, inquiry, 1, 0, 0, 2, 0, 0],
            0x17 => vec![0x67, inquiry, 1, 0, 0, 0, 0],
            0x19 => vec![0x67, inquiry, 1, 0, 0, 0, 0, 0, 0],
            0x22 => vec![0x67, inquiry, 1, 0, 0, 0],
            _ => unreachable!(),
        };
        let parameters = NoiseParameters::decode_response(TABLE, &bytes).unwrap();
        assert!(parameters.encode_set(&capability).is_err());
    }
}

#[test]
fn missing_and_ambiguous_modes_do_not_authorize_writes() {
    let parameters = NoiseParameters::decode_response(TABLE, &[0x67, 0x22, 1, 1, 1, 10]).unwrap();
    for bytes in [
        vec![0x61, 0x22, 0],
        vec![0x61, 0x22, 1, 0, 1, 20, 1],
        vec![0x61, 0x22, 2, 1, 1, 20, 1, 1, 1, 20, 1],
    ] {
        let capability = NoiseCapability::decode_response(TABLE, &bytes).unwrap();
        assert!(parameters.encode_set(&capability).is_err());
    }
    let switch = NoiseParameters::decode_response(TABLE, &[0x67, 0x12, 1, 1, 2, 1, 1]).unwrap();
    let capability = NoiseCapability::decode_response(TABLE, &[0x61, 0x12, 1, 0]).unwrap();
    assert!(switch.encode_set(&capability).is_err());
}

#[test]
fn writes_reject_capabilities_of_similar_but_different_layouts() {
    let parameters =
        NoiseParameters::decode_response(TABLE, &[0x67, 0x17, 1, 1, 1, 0, 20]).unwrap();
    let capability = NoiseCapability::decode_response(TABLE, XM6_CAPABILITY).unwrap();
    assert_eq!(
        parameters.encode_set(&capability),
        Err(CommandError::CapabilityMismatch {
            expected: 0x17,
            actual: 0x19
        })
    );
    let other_capability = NoiseCapability::decode_response(TABLE, &[0x61, 0x30, 4]).unwrap();
    assert!(parameters.encode_set(&other_capability).is_err());
}

#[test]
fn availability_is_not_an_on_off_parameter_or_a_query_response() {
    for inquiry in [1, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x19, 0x22, 0x30] {
        let status = NoiseStatus::decode_notification(TABLE, &[0x65, inquiry, 1]).unwrap();
        assert_eq!(status.availability, NoiseAvailability::Disabled);
        assert_eq!(status.kind().inquiry(), inquiry);
        assert!(NoiseStatus::decode_response(TABLE, &[0x65, inquiry, 1]).is_err());
        assert!(NoiseStatus::decode_notification(TABLE, &[0x63, inquiry, 0]).is_err());
        assert!(NoiseStatus::decode_response(TABLE, &[0x63, inquiry, 2]).is_err());
    }
    assert!(NoiseParameters::decode_notification(TABLE, XM6_PARAMETERS).is_err());
    assert!(NoiseParameters::decode_response(TABLE, &[0x69, 0x30, 2]).is_err());
    assert!(NoiseCapability::decode_response(TABLE, &[0x63, 0x30, 0]).is_err());
}

#[test]
fn malformed_parameter_fields_and_lengths_are_rejected() {
    let samples: &[&[u8]] = &[
        &[0x67, 1, 1, 1, 0],
        &[0x67, 0x12, 1, 1, 2, 0, 0],
        &[0x67, 0x13, 1, 1, 0, 0, 10],
        &[0x67, 0x14, 1, 1, 2, 0, 10],
        &[0x67, 0x15, 1, 1, 0, 3, 0, 10],
        &[0x67, 0x16, 1, 1, 0, 2, 0, 10],
        &[0x67, 0x17, 1, 1, 0, 0, 10],
        XM6_PARAMETERS,
        &[0x67, 0x22, 1, 1, 0, 10],
        &[0x67, 0x30, 1],
    ];
    for &sample in samples {
        for length in 0..sample.len() {
            assert!(NoiseParameters::decode_response(TABLE, &sample[..length]).is_err());
        }
        let mut trailing = sample.to_vec();
        trailing.push(0);
        assert!(NoiseParameters::decode_response(TABLE, &trailing).is_err());
        assert!(NoiseParameters::decode_response(CommandTable::TABLE_2, sample).is_err());
    }
    for (offset, bad) in [(2, 2), (3, 2), (4, 2), (5, 2), (7, 2), (8, 3)] {
        let mut bytes = XM6_PARAMETERS.to_vec();
        bytes[offset] = bad;
        assert!(
            NoiseParameters::decode_response(TABLE, &bytes).is_err(),
            "offset={offset}"
        );
    }
    for bytes in [
        vec![0x67, 1, 1, 1, 2],
        vec![0x67, 0x12, 1, 1, 6, 0, 0],
        vec![0x67, 0x12, 1, 1, 2, 0, 2],
        vec![0x67, 0x13, 1, 1, 2, 0, 10],
        vec![0x67, 0x14, 1, 1, 6, 0, 10],
        vec![0x67, 0x15, 1, 1, 2, 3, 0, 10],
        vec![0x67, 0x16, 1, 1, 0, 6, 0, 10],
        vec![0x67, 0x22, 1, 1, 2, 10],
    ] {
        assert!(NoiseParameters::decode_response(TABLE, &bytes).is_err());
    }
}

#[test]
fn malformed_capabilities_and_statuses_are_rejected_without_partial_results() {
    for length in 0..XM6_CAPABILITY.len() {
        assert!(NoiseCapability::decode_response(TABLE, &XM6_CAPABILITY[..length]).is_err());
    }
    for bytes in [
        vec![0x61, 0x12, 2, 0],
        vec![0x61, 0x12, 1, 2],
        vec![0x61, 0x19, 1, 2, 1, 20, 1],
        vec![0x61, 0x19, 1, 0, 255, 255, 1],
        vec![0x61, 0x19, 1, 0, 0, 0, 1],
        vec![0x61, 0x19, 1, 0, 20, 1, 1],
        vec![0x61, 0x19, 1, 0, 1, 20, 0],
        vec![0x61, 0x30, 6],
        vec![0x61, 1, 0],
    ] {
        assert!(NoiseCapability::decode_response(TABLE, &bytes).is_err());
    }
    assert!(NoiseCapability::decode_response(CommandTable::TABLE_2, XM6_CAPABILITY).is_err());
    for bytes in [vec![], vec![0x63], vec![0x63, 0x19], vec![0x63, 0x19, 0, 0]] {
        assert!(NoiseStatus::decode_response(TABLE, &bytes).is_err());
    }
    assert!(NoiseStatus::decode_response(CommandTable::TABLE_2, &[0x63, 0x19, 0]).is_err());
}

#[test]
fn capability_count_is_an_unsigned_byte_with_a_bounded_exact_length() {
    let mut bytes = vec![0x61, 0x19, 255];
    for _ in 0..255 {
        bytes.extend([0, 1, 20, 1]);
    }
    let capability = NoiseCapability::decode_response(TABLE, &bytes).unwrap();
    assert_eq!(capability.ambient_modes().len(), 255);
    bytes.push(0);
    assert!(NoiseCapability::decode_response(TABLE, &bytes).is_err());
}
