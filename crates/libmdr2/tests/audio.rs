use libmdr2::{
    audio::{
        AudioCapability, AudioCapabilityData, AudioKind, AudioParameterValue, AudioParameters,
        AudioSetting, AudioStatus, AudioStatusDetails, DseeMode, ListeningOption, PriorityControl,
        PriorityMode, RoomSize, SwitchingStream, UpmixItem, UpscalingType,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39, sonyctl/sim.py INITIAL:
// author-reported WH-1000XM6 FW 3.1.5 bodies, prefixed with the corresponding RET
// opcode and inquiry. These are payload fixtures, not complete wire captures.
const XM6_DSEE_CAP: &[u8] = &[0xe1, 1, 2];
const XM6_PRIORITY_CAP: &[u8] = &[0xe1, 5, 3, 0, 1, 2, 1, 0];

fn capability(bytes: &[u8]) -> AudioCapability {
    AudioCapability::decode_response(T1, bytes).unwrap()
}

#[test]
fn published_dsee_and_priority_bodies_have_independent_interpretations() {
    assert_eq!(
        capability(XM6_DSEE_CAP).data(),
        &AudioCapabilityData::Dsee(UpscalingType::DSEE_EXTREME)
    );
    let status = AudioStatus::decode_response(T1, &[0xe3, 1, 0]).unwrap();
    assert!(status.enabled);
    assert_eq!(status.details, AudioStatusDetails::None);
    let parameter = AudioParameters::decode_response(T1, &[0xe7, 1, 1]).unwrap();
    assert_eq!(
        parameter.value(),
        &AudioParameterValue::Dsee(DseeMode::Auto)
    );
    let priority = capability(XM6_PRIORITY_CAP);
    let AudioCapabilityData::Connection {
        priorities,
        ldac_exclusive,
    } = priority.data()
    else {
        panic!()
    };
    assert_eq!(
        priorities
            .iter()
            .map(|value| value.as_raw())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(ldac_exclusive[0].as_raw(), 0);
    let status = AudioStatus::decode_response(T1, &[0xe3, 5, 0, 0]).unwrap();
    assert_eq!(
        status.details,
        AudioStatusDetails::Connection {
            secondary_enabled: true
        }
    );
    let parameter = AudioParameters::decode_response(T1, &[0xe7, 5, 0]).unwrap();
    assert_eq!(
        parameter.value(),
        &AudioParameterValue::ConnectionPriority {
            priority: PriorityMode::SOUND_QUALITY,
            switching_stream: None,
        }
    );
    let bgm = AudioParameters::decode_response(T1, &[0xe7, 9, 1, 2]).unwrap();
    assert_eq!(
        bgm.value(),
        &AudioParameterValue::Bgm {
            enabled: false,
            room_size: RoomSize::Large
        }
    );
    assert_eq!(
        AudioParameters::decode_response(T1, &[0xe7, 4, 1])
            .unwrap()
            .value(),
        &AudioParameterValue::Switch(false)
    );
}

#[test]
fn function_mapping_is_table_aware_and_does_not_use_a_model_name() {
    for (kind, inquiry, function) in [
        (AudioKind::ConnectionPriority, 0, 0xe1),
        (AudioKind::Dsee, 1, 0xe2),
        (AudioKind::ConnectionPriorityWithStatus, 2, 0xe3),
        (AudioKind::Bgm, 3, 0xe4),
        (AudioKind::UpmixCinema, 4, 0xe5),
        (AudioKind::ClassicLePriority, 5, 0xe7),
        (AudioKind::SoundLeakageReduction, 7, 0xe9),
        (AudioKind::ListeningOptions, 8, 0xea),
        (AudioKind::BgmWithReasons, 9, 0xeb),
        (AudioKind::UpmixSeries, 10, 0xec),
        (AudioKind::DseeWithReasons, 11, 0xed),
    ] {
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(AudioKind::try_from(inquiry), Ok(kind));
        assert_eq!(
            kind.function_id(),
            FunctionId {
                table: T1,
                code: function
            }
        );
        assert_eq!(
            AudioKind::from_function(FunctionId {
                table: T1,
                code: function
            }),
            Some(kind)
        );
        assert_eq!(
            AudioKind::from_function(FunctionId {
                table: T2,
                code: function
            }),
            None
        );
        assert_eq!(kind.has_capability_query(), !matches!(inquiry, 0 | 2));
    }
    for inquiry in [6, 12, 13, 255] {
        assert!(AudioKind::try_from(inquiry).is_err());
    }
    for code in [0xe6, 0xe8, 0xef, 0xff] {
        assert_eq!(
            AudioKind::from_function(FunctionId { table: T1, code }),
            None
        );
    }
}

#[test]
fn fixed_connection_layouts_have_no_dynamic_capability_messages() {
    for kind in [
        AudioKind::ConnectionPriority,
        AudioKind::ConnectionPriorityWithStatus,
    ] {
        let cap = AudioCapability::static_connection(kind).unwrap();
        assert_eq!(cap.kind(), kind);
        assert!(!kind.has_capability_query());
        assert!(AudioCapability::decode_response(T1, &[0xe1, kind.inquiry()]).is_err());
        for priority in [
            PriorityMode::SOUND_QUALITY,
            PriorityMode::CONNECTION_QUALITY,
        ] {
            assert_eq!(
                AudioSetting::ConnectionPriority(priority)
                    .encode_set(&cap)
                    .unwrap(),
                [0xe8, kind.inquiry(), priority.as_raw()]
            );
        }
        assert!(
            AudioSetting::ConnectionPriority(PriorityMode::LOW_LATENCY)
                .encode_set(&cap)
                .is_err()
        );
    }
    assert!(AudioCapability::static_connection(AudioKind::ClassicLePriority).is_err());
    assert!(AudioCapability::static_connection(AudioKind::Dsee).is_err());
}

#[test]
fn each_dynamic_capability_layout_preserves_its_field_order() {
    let vectors: &[&[u8]] = &[
        &[0xe1, 1, 3],
        &[0xe1, 3, 2, 0, 3],
        &[0xe1, 4, 0],
        &[0xe1, 5, 2, 0, 1, 1, 0],
        &[0xe1, 7, 1, 1],
        &[0xe1, 8, 1, 2, 0, 0, 4, 1],
        &[0xe1, 9, 1, 0xfe],
        &[0xe1, 10, 2, 0, 2, 1, 1],
        &[0xe1, 11, 0],
    ];
    for bytes in vectors {
        assert_eq!(capability(bytes).kind().inquiry(), bytes[1]);
    }
    let listening = capability(vectors[5]);
    let AudioCapabilityData::ListeningOptions { flexible, options } = listening.data() else {
        panic!()
    };
    assert!(*flexible);
    assert!(options[0].mandatory);
    assert!(!options[1].mandatory);
    assert_eq!(options[0].option, ListeningOption::STANDARD);
    assert_eq!(options[1].option, ListeningOption::MUSIC);
    let upmix = capability(vectors[7]);
    let AudioCapabilityData::Upmix { items, exclusive } = upmix.data() else {
        panic!()
    };
    assert_eq!(items, &[UpmixItem::NONE, UpmixItem::GAME]);
    assert_eq!(exclusive[0].as_raw(), 1);
}

#[test]
fn status_distinguishes_availability_secondary_flag_assignment_and_reasons() {
    for inquiry in [0, 1, 3, 4, 7] {
        let status = AudioStatus::decode_response(T1, &[0xe3, inquiry, 1]).unwrap();
        assert!(!status.enabled);
        assert_eq!(status.details, AudioStatusDetails::None);
    }
    for inquiry in [2, 5] {
        let status = AudioStatus::decode_notification(T1, &[0xe5, inquiry, 0, 1]).unwrap();
        assert!(status.enabled);
        assert_eq!(
            status.details,
            AudioStatusDetails::Connection {
                secondary_enabled: false
            }
        );
    }
    for inquiry in [9, 11] {
        let status = AudioStatus::decode_response(T1, &[0xe3, inquiry, 1, 3, 0, 11, 0x82]).unwrap();
        let AudioStatusDetails::Reasons(reasons) = status.details else {
            panic!()
        };
        assert_eq!(
            reasons.iter().map(|r| r.as_raw()).collect::<Vec<_>>(),
            [0, 11, 0x82]
        );
    }
    assert_eq!(
        AudioStatus::decode_response(T1, &[0xe3, 8, 1, 0])
            .unwrap()
            .details,
        AudioStatusDetails::ListeningOptions { assigned: true }
    );
    let status = AudioStatus::decode_response(T1, &[0xe3, 10, 0, 0xab]).unwrap();
    let AudioStatusDetails::Upmix(effect) = status.details else {
        panic!()
    };
    assert_eq!(effect.as_raw(), 0xab);
}

#[test]
fn every_parameter_family_decodes_response_and_notification_without_conflating_headers() {
    let vectors: &[&[u8]] = &[
        &[0xe7, 0, 1],
        &[0xe7, 1, 0],
        &[0xe7, 2, 0],
        &[0xe7, 3, 0, 2],
        &[0xe7, 4, 0],
        &[0xe7, 5, 2],
        &[0xe7, 7, 1],
        &[0xe7, 8, 2, 0, 4],
        &[0xe7, 9, 1, 0],
        &[0xe7, 10, 3],
        &[0xe7, 11, 1],
    ];
    for bytes in vectors {
        let value = AudioParameters::decode_response(T1, bytes).unwrap();
        assert_eq!(value.kind().inquiry(), bytes[1]);
        assert!(AudioParameters::decode_notification(T1, bytes).is_err());
        let mut update = bytes.to_vec();
        update[0] = 0xe9;
        if bytes[1] == 5 {
            update.push(0);
        }
        let value = AudioParameters::decode_notification(T1, &update).unwrap();
        assert_eq!(value.kind().inquiry(), bytes[1]);
        assert!(AudioParameters::decode_response(T1, &update).is_err());
    }
}

#[test]
fn classic_le_response_notification_and_set_have_different_layouts() {
    let cap = capability(XM6_PRIORITY_CAP);
    assert_eq!(
        AudioSetting::ConnectionPriority(PriorityMode::LOW_LATENCY)
            .encode_set(&cap)
            .unwrap(),
        [0xe8, 5, 2, 0]
    );
    let value = AudioParameters::decode_notification(T1, &[0xe9, 5, 2, 1]).unwrap();
    assert_eq!(
        value.value(),
        &AudioParameterValue::ConnectionPriority {
            priority: PriorityMode::LOW_LATENCY,
            switching_stream: Some(SwitchingStream::LeAudio),
        }
    );
    assert!(AudioParameters::decode_response(T1, &[0xe7, 5, 2, 1]).is_err());
    assert!(AudioParameters::decode_notification(T1, &[0xe9, 5, 2]).is_err());
    assert!(AudioParameters::decode_notification(T1, &[0xe9, 5, 2, 3]).is_err());
}

#[test]
fn classic_le_control_supports_both_protocol_values_without_changing_priority() {
    let cap = capability(XM6_PRIORITY_CAP);
    for priority in [
        PriorityMode::SOUND_QUALITY,
        PriorityMode::CONNECTION_QUALITY,
        PriorityMode::LOW_LATENCY,
    ] {
        for (control, byte) in [
            (PriorityControl::Enabled, 0),
            (PriorityControl::Disabled, 1),
        ] {
            assert_eq!(
                AudioSetting::ConnectionPriorityWithControl { priority, control }
                    .encode_set(&cap)
                    .unwrap(),
                [0xe8, 5, priority.as_raw(), byte]
            );
        }
        assert_eq!(
            AudioSetting::ConnectionPriority(priority)
                .encode_set(&cap)
                .unwrap(),
            [0xe8, 5, priority.as_raw(), 0]
        );
    }
    assert_eq!(PriorityControl::try_from(0), Ok(PriorityControl::Enabled));
    assert_eq!(PriorityControl::try_from(1), Ok(PriorityControl::Disabled));
    assert!(PriorityControl::try_from(2).is_err());
}

#[test]
fn explicit_priority_control_requires_classic_le_layout_and_advertised_known_priority() {
    let control = PriorityControl::Disabled;
    for kind in [
        AudioKind::ConnectionPriority,
        AudioKind::ConnectionPriorityWithStatus,
    ] {
        assert!(
            AudioSetting::ConnectionPriorityWithControl {
                priority: PriorityMode::SOUND_QUALITY,
                control,
            }
            .encode_set(&AudioCapability::static_connection(kind).unwrap())
            .is_err()
        );
    }
    assert!(
        AudioSetting::ConnectionPriorityWithControl {
            priority: PriorityMode::SOUND_QUALITY,
            control,
        }
        .encode_set(&capability(XM6_DSEE_CAP))
        .is_err()
    );
    for (cap, priority) in [
        (
            capability(&[0xe1, 5, 2, 0, 1, 0]),
            PriorityMode::LOW_LATENCY,
        ),
        (
            capability(&[0xe1, 5, 2, 0, 0xaa, 0]),
            PriorityMode::from_raw(0xaa),
        ),
    ] {
        for control in [PriorityControl::Enabled, PriorityControl::Disabled] {
            assert!(
                AudioSetting::ConnectionPriorityWithControl { priority, control }
                    .encode_set(&cap)
                    .is_err()
            );
        }
    }
}

#[test]
fn dsee_auto_is_one_but_other_audio_switches_use_zero_for_on() {
    for inquiry in [1, 11] {
        let cap = capability(&[0xe1, inquiry, 2]);
        assert_eq!(
            AudioSetting::Dsee(DseeMode::Auto).encode_set(&cap).unwrap(),
            [0xe8, inquiry, 1]
        );
        assert_eq!(
            AudioSetting::Dsee(DseeMode::Off).encode_set(&cap).unwrap(),
            [0xe8, inquiry, 0]
        );
    }
    assert_eq!(
        AudioSetting::UpmixCinema(true)
            .encode_set(&capability(&[0xe1, 4, 0]))
            .unwrap(),
        [0xe8, 4, 0]
    );
    assert_eq!(
        AudioSetting::SoundLeakageReduction(false)
            .encode_set(&capability(&[0xe1, 7, 0]))
            .unwrap(),
        [0xe8, 7, 1]
    );
}

#[test]
fn background_writes_preserve_room_size_even_while_disabled() {
    for inquiry in [3, 9] {
        let cap = capability(&[0xe1, inquiry, 0]);
        assert_eq!(
            AudioSetting::Bgm {
                enabled: false,
                room_size: RoomSize::Large
            }
            .encode_set(&cap)
            .unwrap(),
            [0xe8, inquiry, 1, 2]
        );
        assert_eq!(
            AudioSetting::Bgm {
                enabled: true,
                room_size: RoomSize::Small
            }
            .encode_set(&cap)
            .unwrap(),
            [0xe8, inquiry, 0, 0]
        );
    }
}

#[test]
fn choice_writes_require_advertised_known_values_and_matching_families() {
    let priority = capability(&[0xe1, 5, 2, 0, 1, 0]);
    assert!(
        AudioSetting::ConnectionPriority(PriorityMode::LOW_LATENCY)
            .encode_set(&priority)
            .is_err()
    );
    let priority_unknown = capability(&[0xe1, 5, 2, 0, 0xaa, 0]);
    assert!(
        AudioSetting::ConnectionPriority(PriorityMode::from_raw(0xaa))
            .encode_set(&priority_unknown)
            .is_err()
    );
    let upmix = capability(&[0xe1, 10, 2, 0, 3, 0]);
    assert_eq!(
        AudioSetting::Upmix(UpmixItem::MUSIC)
            .encode_set(&upmix)
            .unwrap(),
        [0xe8, 10, 3]
    );
    assert!(
        AudioSetting::Upmix(UpmixItem::CINEMA)
            .encode_set(&upmix)
            .is_err()
    );
    assert!(
        AudioSetting::Dsee(DseeMode::Auto)
            .encode_set(&priority)
            .is_err()
    );
    assert!(
        AudioSetting::ConnectionPriority(PriorityMode::SOUND_QUALITY)
            .encode_set(&upmix)
            .is_err()
    );
    assert!(
        AudioSetting::UpmixCinema(true)
            .encode_set(&capability(&[0xe1, 7, 0]))
            .is_err()
    );
}

#[test]
fn listening_assignment_checks_membership_duplicates_and_mandatory_entries() {
    let cap = capability(&[0xe1, 8, 1, 3, 0, 0, 1, 1, 4, 1]);
    let set = |items| AudioSetting::ListeningOptions(items).encode_set(&cap);
    assert_eq!(
        set(vec![ListeningOption::STANDARD, ListeningOption::MUSIC]).unwrap(),
        [0xe8, 8, 2, 0, 4]
    );
    assert!(set(vec![ListeningOption::MUSIC]).is_err());
    assert!(set(vec![ListeningOption::STANDARD, ListeningOption::STANDARD]).is_err());
    assert!(set(vec![ListeningOption::STANDARD, ListeningOption::GAME]).is_err());
    assert!(set(vec![ListeningOption::STANDARD; 256]).is_err());
    let ambiguous = capability(&[0xe1, 8, 1, 2, 0, 0, 0, 1]);
    assert!(
        AudioSetting::ListeningOptions(vec![ListeningOption::STANDARD])
            .encode_set(&ambiguous)
            .is_err()
    );
    let unknown = capability(&[0xe1, 8, 1, 1, 0xaa, 0]);
    assert!(
        AudioSetting::ListeningOptions(vec![ListeningOption::from_raw(0xaa)])
            .encode_set(&unknown)
            .is_err()
    );
}

#[test]
fn unknown_identity_bytes_remain_available_without_authorizing_unknown_operations() {
    assert_eq!(
        capability(&[0xe1, 1, 0xee]).data(),
        &AudioCapabilityData::Dsee(UpscalingType::from_raw(0xee))
    );
    let cap = capability(&[0xe1, 3, 3, 0, 0xfe, 0xfe]);
    let AudioCapabilityData::ExclusiveFunctions(items) = cap.data() else {
        panic!()
    };
    assert_eq!(
        items.iter().map(|x| x.as_raw()).collect::<Vec<_>>(),
        [0, 0xfe, 0xfe]
    );
    let priority = AudioParameters::decode_response(T1, &[0xe7, 5, 0xfe]).unwrap();
    assert_eq!(
        priority.value(),
        &AudioParameterValue::ConnectionPriority {
            priority: PriorityMode::from_raw(0xfe),
            switching_stream: None
        }
    );
    assert_eq!(
        AudioParameters::decode_response(T1, &[0xe7, 10, 0xfe])
            .unwrap()
            .value(),
        &AudioParameterValue::Upmix(UpmixItem::from_raw(0xfe))
    );
    assert_eq!(
        AudioParameters::decode_response(T1, &[0xe7, 8, 2, 0xfe, 0xfe])
            .unwrap()
            .value(),
        &AudioParameterValue::ListeningOptions(vec![ListeningOption::from_raw(0xfe); 2])
    );
}

#[test]
fn byte_counts_accept_255_without_signed_conversion_or_truncation() {
    let mut exclusive = vec![0xe1, 4, 255];
    exclusive.extend(0..255);
    let cap = capability(&exclusive);
    let AudioCapabilityData::ExclusiveFunctions(items) = cap.data() else {
        panic!()
    };
    assert_eq!(items.len(), 255);
    assert_eq!(items.last().unwrap().as_raw(), 254);
    let mut status = vec![0xe3, 11, 1, 255];
    status.extend(0..255);
    let AudioStatusDetails::Reasons(reasons) =
        AudioStatus::decode_response(T1, &status).unwrap().details
    else {
        panic!()
    };
    assert_eq!(reasons.len(), 255);
    let mut assigned = vec![0xe7, 8, 255];
    assigned.extend(0..255);
    let params = AudioParameters::decode_response(T1, &assigned).unwrap();
    let AudioParameterValue::ListeningOptions(items) = params.value() else {
        panic!()
    };
    assert_eq!(items.len(), 255);
}

#[test]
fn exact_lengths_headers_and_all_truncation_points_are_checked() {
    let capabilities: &[&[u8]] = &[
        XM6_DSEE_CAP,
        XM6_PRIORITY_CAP,
        &[0xe1, 3, 2, 0, 1],
        &[0xe1, 8, 1, 2, 0, 0, 1, 1],
        &[0xe1, 10, 2, 0, 1, 1, 0],
    ];
    for bytes in capabilities {
        assert!(AudioCapability::decode_response(T1, bytes).is_ok());
        for length in 0..bytes.len() {
            assert!(AudioCapability::decode_response(T1, &bytes[..length]).is_err());
        }
        let mut extra = bytes.to_vec();
        extra.push(0);
        assert!(AudioCapability::decode_response(T1, &extra).is_err());
        assert!(AudioCapability::decode_response(T2, bytes).is_err());
        let mut wrong = bytes.to_vec();
        wrong[0] = 0xe3;
        assert!(AudioCapability::decode_response(T1, &wrong).is_err());
    }
    for bytes in [
        vec![0xe3, 0, 0],
        vec![0xe3, 2, 0, 1],
        vec![0xe3, 9, 1, 2, 0, 3],
        vec![0xe3, 8, 1, 0],
        vec![0xe3, 10, 0, 1],
    ] {
        assert!(AudioStatus::decode_response(T1, &bytes).is_ok());
        for length in 0..bytes.len() {
            assert!(AudioStatus::decode_response(T1, &bytes[..length]).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(AudioStatus::decode_response(T1, &extra).is_err());
        assert!(AudioStatus::decode_response(T2, &bytes).is_err());
        assert!(AudioStatus::decode_notification(T1, &bytes).is_err());
    }
    for bytes in [
        vec![0xe7, 0, 1],
        vec![0xe7, 1, 1],
        vec![0xe7, 3, 0, 1],
        vec![0xe7, 4, 0],
        vec![0xe7, 8, 2, 0, 4],
        vec![0xe7, 10, 3],
    ] {
        assert!(AudioParameters::decode_response(T1, &bytes).is_ok());
        for length in 0..bytes.len() {
            assert!(AudioParameters::decode_response(T1, &bytes[..length]).is_err());
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(AudioParameters::decode_response(T1, &extra).is_err());
        assert!(AudioParameters::decode_response(T2, &bytes).is_err());
    }
}

#[test]
fn malformed_closed_fields_and_capability_relationships_are_rejected() {
    for value in [2, 0x7f, 0xff] {
        assert!(AudioStatus::decode_response(T1, &[0xe3, 1, value]).is_err());
        assert!(AudioStatus::decode_response(T1, &[0xe3, 5, 0, value]).is_err());
        assert!(AudioStatus::decode_response(T1, &[0xe3, 8, 0, value]).is_err());
        assert!(AudioParameters::decode_response(T1, &[0xe7, 1, value]).is_err());
        assert!(AudioParameters::decode_response(T1, &[0xe7, 4, value]).is_err());
        assert!(AudioParameters::decode_response(T1, &[0xe7, 3, value, 0]).is_err());
        assert!(AudioCapability::decode_response(T1, &[0xe1, 8, value, 0]).is_err());
        assert!(AudioCapability::decode_response(T1, &[0xe1, 8, 1, 1, 0, value]).is_err());
    }
    assert!(AudioParameters::decode_response(T1, &[0xe7, 3, 0, 3]).is_err());
    assert!(AudioCapability::decode_response(T1, &[0xe1, 5, 1, 0, 0]).is_err());
    assert!(AudioCapability::decode_response(T1, &[0xe1, 5, 2, 0, 1, 1]).is_err());
    assert!(AudioStatus::decode_response(T1, &[0xe3, 11, 1, 1]).is_err());
    assert!(AudioParameters::decode_response(T1, &[0xe7, 6, 0]).is_err());
    assert!(AudioParameters::decode_response(T1, &[0xe7, 12, 0]).is_err());
}
