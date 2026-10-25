use libmdr2::{
    command::CommandError,
    system::{
        DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemCapability, SystemKind,
        SystemParameters, SystemStatus,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;

// Short initial-state bodies from sonymdr_open, revision
// 44adbf92df1525571561bbebc0485ea9c436ae39, sonyctl/sim.py INITIAL.
// Upstream reports WH-1000XM6 FW 3.1.5. RET headers are reconstructed.
const XM6_CAPABILITY: &[u8] = &[0xf1, 0x0c, 1, 5, 15, 30];
const XM6_STATUS: &[u8] = &[0xf3, 0x0c, 0, 0];
const XM6_PARAMETERS: &[u8] = &[0xf7, 0x0c, 1, 1];
const XM6_EXTENDED: &[u8] = &[0xfb, 0x0c, 0, 0];

fn capability(kind: SystemKind, preview: bool) -> SystemCapability {
    SystemCapability::decode_response(T1, &[0xf1, kind.inquiry(), u8::from(preview), 5, 15, 30])
        .unwrap()
}

#[test]
fn published_speak_to_chat_bodies_preserve_separate_states() {
    let cap = SystemCapability::decode_response(T1, XM6_CAPABILITY).unwrap();
    assert_eq!(cap.kind(), SystemKind::SpeakToChatType2);
    let fields = cap.speak_to_chat().unwrap();
    assert!(fields.preview_supported);
    assert_eq!(fields.mode_out_values, [5, 15, 30]);
    assert_eq!(fields.mode_out_value(ModeOutTime::Fast), Some(5));
    assert_eq!(fields.mode_out_value(ModeOutTime::Mid), Some(15));
    assert_eq!(fields.mode_out_value(ModeOutTime::Slow), Some(30));
    assert_eq!(fields.mode_out_value(ModeOutTime::None), None);

    let status = SystemStatus::decode_response(T1, XM6_STATUS).unwrap();
    assert!(status.enabled);
    assert_eq!(status.talking, Some(false));
    let parameters = SystemParameters::decode_response(T1, XM6_PARAMETERS).unwrap();
    assert_eq!(
        parameters,
        SystemParameters::SpeakToChatType2 {
            enabled: false,
            preview: false
        }
    );
    let extended = SpeakToChatSettings::decode_response(T1, XM6_EXTENDED).unwrap();
    assert_eq!(
        extended,
        SpeakToChatSettings::Type2 {
            sensitivity: DetectionSensitivity::Auto,
            mode_out: ModeOutTime::Fast
        }
    );
    assert_eq!(parameters.encode_set(&cap).unwrap(), [0xf8, 0x0c, 1, 1]);
    assert_eq!(extended.encode_set(&cap).unwrap(), [0xfc, 0x0c, 0, 0]);
}

#[test]
fn published_wearing_and_head_gesture_switches_are_local_toggles() {
    for (bytes, kind, on) in [
        ([0xf7, 0x01, 0], SystemKind::PlaybackControlByWearing, true),
        ([0xf7, 0x0f, 1], SystemKind::HeadGesture, false),
    ] {
        let parameters = SystemParameters::decode_response(T1, &bytes).unwrap();
        assert_eq!(parameters.kind(), kind);
        assert_eq!(parameters.enabled(), on);
        assert_eq!(parameters.preview(), None);
        let cap = SystemCapability::fixed(kind).unwrap();
        assert!(cap.speak_to_chat().is_none());
        assert_eq!(
            parameters.encode_set(&cap).unwrap(),
            [0xf8, bytes[1], bytes[2]]
        );
        let status = SystemStatus::decode_response(T1, &[0xf3, bytes[1], 0]).unwrap();
        assert!(status.enabled);
        assert_eq!(status.talking, None);
    }
}

#[test]
fn functions_select_only_implemented_layouts_on_table_one() {
    for (code, inquiry, kind) in [
        (0xf0, 0x00, SystemKind::Vibrator),
        (0xf1, 0x01, SystemKind::PlaybackControlByWearing),
        (0xf2, 0x02, SystemKind::SpeakToChatType1),
        (0xfa, 0x0a, SystemKind::AutoVolume),
        (0xfc, 0x0c, SystemKind::SpeakToChatType2),
        (0xff, 0x0f, SystemKind::HeadGesture),
    ] {
        let function = FunctionId { table: T1, code };
        assert_eq!(kind.function_id(), function);
        assert_eq!(SystemKind::from_function(function), Some(kind));
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(SystemKind::try_from(inquiry), Ok(kind));
        assert_eq!(
            SystemKind::from_function(FunctionId {
                table: CommandTable::TABLE_2,
                code
            }),
            None
        );
    }
    for excluded in [0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfb, 0xfd, 0xfe] {
        assert_eq!(
            SystemKind::from_function(FunctionId {
                table: T1,
                code: excluded
            }),
            None
        );
    }
    for excluded in [3, 4, 5, 6, 7, 8, 9, 11, 13, 14, 16, 255] {
        assert!(SystemKind::try_from(excluded).is_err());
    }
}

#[test]
fn fixed_descriptors_do_not_fabricate_speak_to_chat_capabilities() {
    for kind in [
        SystemKind::Vibrator,
        SystemKind::PlaybackControlByWearing,
        SystemKind::AutoVolume,
        SystemKind::HeadGesture,
    ] {
        assert!(!kind.has_capability_query());
        assert!(!kind.has_extended_parameters());
        assert_eq!(SystemCapability::fixed(kind).unwrap().kind(), kind);
        assert!(SystemCapability::decode_response(T1, &[0xf1, kind.inquiry()]).is_err());
        assert!(SpeakToChatSettings::decode_response(T1, &[0xfb, kind.inquiry(), 0, 0]).is_err());
    }
    for kind in [SystemKind::SpeakToChatType1, SystemKind::SpeakToChatType2] {
        assert!(kind.has_capability_query());
        assert!(kind.has_extended_parameters());
        assert!(matches!(
            SystemCapability::fixed(kind),
            Err(CommandError::UnsupportedOperation { .. })
        ));
    }
}

#[test]
fn preview_writes_require_support_but_reports_remain_readable() {
    for (kind, desired) in [
        (
            SystemKind::SpeakToChatType1,
            SystemParameters::SpeakToChatType1 {
                enabled: true,
                preview: true,
            },
        ),
        (
            SystemKind::SpeakToChatType2,
            SystemParameters::SpeakToChatType2 {
                enabled: false,
                preview: true,
            },
        ),
    ] {
        let unsupported = capability(kind, false);
        assert!(matches!(
            desired.encode_set(&unsupported),
            Err(CommandError::InvalidParameter { .. })
        ));
        let read = SystemParameters::decode_response(T1, &[0xf7, kind.inquiry(), 0, 0]).unwrap();
        assert_eq!(read.preview(), Some(true));
        assert_eq!(desired.encode_set(&capability(kind, true)).unwrap()[3], 0);
        let preview_off =
            SystemParameters::decode_response(T1, &[0xf7, kind.inquiry(), 0, 1]).unwrap();
        assert_eq!(
            preview_off.encode_set(&unsupported).unwrap(),
            [0xf8, kind.inquiry(), 0, 1]
        );
    }
}

#[test]
fn all_fixed_toggles_use_inverted_on_off_without_extra_fields() {
    for (on, off, inquiry) in [
        (
            SystemParameters::Vibrator { enabled: true },
            SystemParameters::Vibrator { enabled: false },
            0,
        ),
        (
            SystemParameters::PlaybackControlByWearing { enabled: true },
            SystemParameters::PlaybackControlByWearing { enabled: false },
            1,
        ),
        (
            SystemParameters::AutoVolume { enabled: true },
            SystemParameters::AutoVolume { enabled: false },
            10,
        ),
        (
            SystemParameters::HeadGesture { enabled: true },
            SystemParameters::HeadGesture { enabled: false },
            15,
        ),
    ] {
        let descriptor = SystemCapability::fixed(on.kind()).unwrap();
        assert_eq!(on.encode_set(&descriptor).unwrap(), [0xf8, inquiry, 0]);
        assert_eq!(off.encode_set(&descriptor).unwrap(), [0xf8, inquiry, 1]);
        assert_eq!(
            SystemParameters::decode_response(T1, &[0xf7, inquiry, 0]).unwrap(),
            on
        );
        assert_eq!(
            SystemParameters::decode_notification(T1, &[0xf9, inquiry, 1]).unwrap(),
            off
        );
    }
}

#[test]
fn availability_and_talking_effect_have_distinct_boolean_encodings() {
    for inquiry in [2, 12] {
        for (available, availability_byte) in [(true, 0), (false, 1)] {
            for (talking, effect_byte) in [(true, 1), (false, 0)] {
                let status = SystemStatus::decode_response(
                    T1,
                    &[0xf3, inquiry, availability_byte, effect_byte],
                )
                .unwrap();
                assert_eq!(status.enabled, available);
                assert_eq!(status.talking, Some(talking));
                assert_eq!(
                    SystemStatus::decode_notification(
                        T1,
                        &[0xf5, inquiry, availability_byte, effect_byte]
                    )
                    .unwrap(),
                    status
                );
            }
        }
    }
}

#[test]
fn setters_reject_descriptors_from_other_layouts() {
    let first = capability(SystemKind::SpeakToChatType1, true);
    let second = capability(SystemKind::SpeakToChatType2, true);
    let params = SystemParameters::SpeakToChatType1 {
        enabled: true,
        preview: false,
    };
    let ext = SpeakToChatSettings::Type1 {
        sensitivity: DetectionSensitivity::Auto,
        voice_focus: true,
        mode_out: ModeOutTime::Mid,
    };
    assert!(matches!(
        params.encode_set(&second),
        Err(CommandError::CapabilityMismatch {
            expected: 2,
            actual: 12
        })
    ));
    assert!(ext.encode_set(&second).is_err());
    let wearing = SystemCapability::fixed(SystemKind::PlaybackControlByWearing).unwrap();
    assert!(params.encode_set(&wearing).is_err());
    assert!(
        SystemParameters::Vibrator { enabled: true }
            .encode_set(&wearing)
            .is_err()
    );
    assert!(
        SystemParameters::PlaybackControlByWearing { enabled: true }
            .encode_set(&first)
            .is_err()
    );
}

#[test]
fn editing_extended_fields_preserves_the_other_fields_and_variant() {
    let mut type1 = SpeakToChatSettings::decode_response(T1, &[0xfb, 2, 2, 0, 3]).unwrap();
    let SpeakToChatSettings::Type1 { sensitivity, .. } = &mut type1 else {
        panic!()
    };
    *sensitivity = DetectionSensitivity::High;
    assert_eq!(
        type1
            .encode_set(&capability(SystemKind::SpeakToChatType1, false))
            .unwrap(),
        [0xfc, 2, 1, 0, 3]
    );
    let mut type2 = SpeakToChatSettings::decode_notification(T1, &[0xfd, 12, 2, 1]).unwrap();
    let SpeakToChatSettings::Type2 { mode_out, .. } = &mut type2 else {
        panic!()
    };
    *mode_out = ModeOutTime::None;
    assert_eq!(
        type2
            .encode_set(&capability(SystemKind::SpeakToChatType2, false))
            .unwrap(),
        [0xfc, 12, 2, 3]
    );
}

#[test]
fn capability_timing_values_are_metadata_not_selector_bytes() {
    // No imposed second unit, ordering, minimum, or relation to selector codes.
    let cap = SystemCapability::decode_response(T1, &[0xf1, 12, 0, 255, 0, 128]).unwrap();
    assert_eq!(cap.speak_to_chat().unwrap().mode_out_values, [255, 0, 128]);
    for (mode_out, byte) in [
        (ModeOutTime::Fast, 0),
        (ModeOutTime::Mid, 1),
        (ModeOutTime::Slow, 2),
        (ModeOutTime::None, 3),
    ] {
        let setting = SpeakToChatSettings::Type2 {
            sensitivity: DetectionSensitivity::Auto,
            mode_out,
        };
        assert_eq!(setting.encode_set(&cap).unwrap(), [0xfc, 12, 0, byte]);
    }
}

#[test]
fn every_closed_field_rejects_values_outside_its_domain() {
    for invalid in [2, 3, 127, 128, 255] {
        assert!(SystemCapability::decode_response(T1, &[0xf1, 12, invalid, 5, 15, 30]).is_err());
        for inquiry in [0, 1, 2, 10, 12, 15] {
            let mut status = vec![0xf3, inquiry, invalid];
            let mut params = vec![0xf7, inquiry, invalid];
            if matches!(inquiry, 2 | 12) {
                status.push(0);
                params.push(0);
                assert!(SystemStatus::decode_response(T1, &[0xf3, inquiry, 0, invalid]).is_err());
                assert!(
                    SystemParameters::decode_response(T1, &[0xf7, inquiry, 0, invalid]).is_err()
                );
            }
            assert!(SystemStatus::decode_response(T1, &status).is_err());
            assert!(SystemParameters::decode_response(T1, &params).is_err());
        }
        assert!(SpeakToChatSettings::decode_response(T1, &[0xfb, 2, 0, invalid, 0]).is_err());
    }
    for invalid in [3, 4, 255] {
        assert!(DetectionSensitivity::try_from(invalid).is_err());
        assert!(SpeakToChatSettings::decode_response(T1, &[0xfb, 12, invalid, 0]).is_err());
    }
    for invalid in [4, 5, 255] {
        assert!(ModeOutTime::try_from(invalid).is_err());
        assert!(SpeakToChatSettings::decode_response(T1, &[0xfb, 12, 0, invalid]).is_err());
    }
}

#[test]
fn strict_decoders_reject_all_truncations_extra_bytes_tables_and_opcodes() {
    type Decode = fn(CommandTable, &[u8]) -> Result<(), CommandError>;
    let cap: Decode = |t, p| SystemCapability::decode_response(t, p).map(|_| ());
    let status: Decode = |t, p| SystemStatus::decode_response(t, p).map(|_| ());
    let params: Decode = |t, p| SystemParameters::decode_response(t, p).map(|_| ());
    let ext: Decode = |t, p| SpeakToChatSettings::decode_response(t, p).map(|_| ());
    for (decode, payload) in [
        (cap, vec![0xf1, 2, 0, 0, 1, 255]),
        (cap, vec![0xf1, 12, 1, 5, 15, 30]),
        (status, vec![0xf3, 0, 0]),
        (status, vec![0xf3, 1, 1]),
        (status, vec![0xf3, 2, 0, 1]),
        (status, vec![0xf3, 10, 0]),
        (status, vec![0xf3, 12, 1, 0]),
        (status, vec![0xf3, 15, 1]),
        (params, vec![0xf7, 0, 1]),
        (params, vec![0xf7, 1, 0]),
        (params, vec![0xf7, 2, 0, 1]),
        (params, vec![0xf7, 10, 1]),
        (params, vec![0xf7, 12, 1, 0]),
        (params, vec![0xf7, 15, 0]),
        (ext, vec![0xfb, 2, 2, 1, 3]),
        (ext, vec![0xfb, 12, 1, 2]),
    ] {
        assert!(decode(T1, &payload).is_ok(), "{payload:x?}");
        for end in 0..payload.len() {
            assert!(
                decode(T1, &payload[..end]).is_err(),
                "{payload:x?}, end {end}"
            );
        }
        let mut extra = payload.clone();
        extra.push(0);
        assert!(decode(T1, &extra).is_err());
        for table in [CommandTable::TABLE_2, CommandTable::from_raw(255)] {
            assert!(decode(table, &payload).is_err());
        }
        let mut wrong = payload.clone();
        wrong[0] = 0;
        assert!(decode(T1, &wrong).is_err());
        wrong = payload.clone();
        wrong[1] = 0xff;
        assert!(decode(T1, &wrong).is_err());
    }
}

#[test]
fn notifications_cannot_be_read_responses_or_switch_extended_parameters() {
    assert!(SystemStatus::decode_response(T1, &[0xf5, 12, 0, 0]).is_err());
    assert!(SystemStatus::decode_notification(T1, &[0xf3, 12, 0, 0]).is_err());
    assert!(SystemParameters::decode_response(T1, &[0xf9, 12, 0, 0]).is_err());
    assert!(SystemParameters::decode_notification(T1, &[0xf7, 12, 0, 0]).is_err());
    assert!(SpeakToChatSettings::decode_response(T1, &[0xfd, 12, 0, 0]).is_err());
    assert!(SpeakToChatSettings::decode_notification(T1, &[0xfb, 12, 0, 0]).is_err());
    assert!(SpeakToChatSettings::decode_notification(T1, &[0xf9, 12, 0, 0]).is_err());
    assert!(SystemParameters::decode_notification(T1, &[0xfd, 12, 0, 0]).is_err());
    assert!(SpeakToChatSettings::decode_notification(T1, &[0xfd, 2, 0, 0]).is_err());
    assert!(SpeakToChatSettings::decode_notification(T1, &[0xfd, 12, 0, 0, 0]).is_err());
}
