use libmdr2::{
    command::{Notification, Request, Response},
    system2::{
        MicrophoneKey, System2Capability, System2Kind, System2Parameters, System2Setting,
        System2Status, System2StatusValue, System2Value, VolumeLimitation, WearingState,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn functions_disambiguate_table_and_microphone_from_input_source() {
    for (kind, code, inquiry) in [
        (System2Kind::WearingStatus, 0xf0, 0),
        (System2Kind::AutoVolumeOptimizer, 0xf3, 3),
        (System2Kind::AutoVolumeWithLimitations, 0xf4, 4),
        (System2Kind::MicrophoneOperation, 0xf9, 0x0b),
    ] {
        assert_eq!(kind.function_id(), FunctionId { table: T2, code });
        assert_eq!(System2Kind::from_function(kind.function_id()), Some(kind));
        assert_eq!(
            System2Kind::from_function(FunctionId { table: T1, code }),
            None
        );
        assert_eq!(kind.inquiry(), inquiry);
    }
    assert_eq!(
        System2Kind::from_function(FunctionId {
            table: T2,
            code: 0xfa
        }),
        None
    );
}

#[test]
fn wearing_is_a_state_not_an_on_off_setting() {
    for (raw, expected) in [
        (0, WearingState::Normal),
        (1, WearingState::InvalidPosition),
        (2, WearingState::LeftNotWorn),
        (3, WearingState::RightNotWorn),
        (4, WearingState::NeitherWorn),
    ] {
        let status = System2Status::decode_response(T2, &[0xf3, 0, raw]).unwrap();
        assert_eq!(status.value, System2StatusValue::Wearing(expected));
        assert_eq!(
            System2Status::decode_notification(T2, &[0xf5, 0, raw]).unwrap(),
            status
        );
    }
    for value in 5..=u8::MAX {
        assert!(System2Status::decode_response(T2, &[0xf3, 0, value]).is_err());
    }
    assert!(System2Parameters::decode_response(T2, &[0xf7, 0, 0]).is_err());
    assert!(System2Capability::fixed(System2Kind::WearingStatus).is_err());
}

#[test]
fn limitations_are_read_only_unsigned_and_preserve_unknown_identifiers() {
    let decoded = System2Parameters::decode_response(T2, &[0xf7, 4, 0, 3, 0, 0xc1, 0]).unwrap();
    assert_eq!(
        decoded.value,
        System2Value::AutoVolume {
            enabled: true,
            limitations: vec![
                VolumeLimitation::SAFE_VOLUME,
                VolumeLimitation::from_raw(0xc1),
                VolumeLimitation::SAFE_VOLUME
            ],
        }
    );
    assert_eq!(VolumeLimitation::from_raw(0xc1).as_raw(), 0xc1);
    let mut maximum = vec![0xf7, 4, 1, 255];
    maximum.extend(0..=254);
    assert!(System2Parameters::decode_response(T2, &maximum).is_ok());
    maximum.push(255);
    assert!(System2Parameters::decode_response(T2, &maximum).is_err());
    let cap = System2Capability::fixed(System2Kind::AutoVolumeWithLimitations).unwrap();
    assert_eq!(
        System2Setting::AutoVolume { enabled: false }
            .encode_set(&cap)
            .unwrap(),
        [0xf8, 4, 1]
    );
    assert!(System2Parameters::decode_response(T2, &[0xf7, 4, 0, 0]).is_ok());
}

#[test]
fn microphone_key_and_two_switch_order_are_explicit() {
    for (raw, expected) in [
        (0, MicrophoneKey::TouchSensor),
        (1, MicrophoneKey::Button),
        (2, MicrophoneKey::FaceTap),
    ] {
        let cap = System2Capability::decode_response(T2, &[0xf1, 0x0b, raw]).unwrap();
        assert_eq!(cap.microphone_key(), Some(expected));
        assert_eq!(
            System2Setting::MicrophoneOperation {
                enabled: true,
                mute_sound: false
            }
            .encode_set(&cap)
            .unwrap(),
            [0xf8, 0x0b, 0, 1]
        );
        assert_eq!(
            System2Setting::MicrophoneOperation {
                enabled: false,
                mute_sound: true
            }
            .encode_set(&cap)
            .unwrap(),
            [0xf8, 0x0b, 1, 0]
        );
        assert!(
            System2Setting::AutoVolume { enabled: false }
                .encode_set(&cap)
                .is_err()
        );
    }
    let decoded = System2Parameters::decode_response(T2, &[0xf7, 0x0b, 0, 1]).unwrap();
    assert_eq!(
        decoded.value,
        System2Value::MicrophoneOperation {
            enabled: true,
            mute_sound: false
        }
    );
    assert!(System2Capability::fixed(System2Kind::MicrophoneOperation).is_err());
    let cap = System2Capability::fixed(System2Kind::AutoVolumeOptimizer).unwrap();
    assert!(
        System2Setting::MicrophoneOperation {
            enabled: true,
            mute_sound: true
        }
        .encode_set(&cap)
        .is_err()
    );
    assert_eq!(
        System2Setting::AutoVolume { enabled: true }
            .encode_set(&cap)
            .unwrap(),
        [0xf8, 3, 0]
    );
}

#[test]
fn closed_fields_tables_lengths_and_directions_are_checked() {
    for raw in 2..=u8::MAX {
        assert!(System2Status::decode_response(T2, &[0xf3, 3, raw]).is_err());
        assert!(System2Parameters::decode_response(T2, &[0xf7, 3, raw]).is_err());
        assert!(System2Parameters::decode_response(T2, &[0xf7, 0x0b, raw, 0]).is_err());
        assert!(System2Parameters::decode_response(T2, &[0xf7, 0x0b, 0, raw]).is_err());
    }
    for raw in 3..=u8::MAX {
        assert!(System2Capability::decode_response(T2, &[0xf1, 0x0b, raw]).is_err());
    }
    for sample in [
        &[0xf7, 3, 0][..],
        &[0xf7, 4, 1, 2, 0, 0x80],
        &[0xf7, 0x0b, 1, 0],
    ] {
        for length in 0..sample.len() {
            assert!(System2Parameters::decode_response(T2, &sample[..length]).is_err());
        }
        let mut longer = sample.to_vec();
        longer.push(0);
        assert!(System2Parameters::decode_response(T2, &longer).is_err());
        assert!(System2Parameters::decode_response(T1, sample).is_err());
        assert!(System2Parameters::decode_notification(T2, sample).is_err());
        let mut update = sample.to_vec();
        update[0] = 0xf9;
        assert_eq!(
            System2Parameters::decode_response(T2, sample).unwrap(),
            System2Parameters::decode_notification(T2, &update).unwrap()
        );
    }
    assert!(System2Capability::decode_response(T2, &[0xf1, 3, 0]).is_err());
    assert!(System2Status::decode_response(T2, &[0xf3, 0x11, 0]).is_err());
}

#[test]
fn table_two_status_and_parameters_do_not_collide_with_table_one() {
    let read = Request::System2Status(System2Kind::WearingStatus)
        .encode()
        .unwrap();
    assert_eq!(read.payload, [0xf2, 0]);
    assert_eq!(read.table, T2);
    assert!(matches!(
        read.expected_response.decode(T2, &[0xf3, 0, 4]),
        Ok(Response::System2Status(_))
    ));
    assert!(read.expected_response.decode(T1, &[0xf3, 0, 0]).is_err());
    assert!(!read.expected_response.matches(T2, &[0xf5, 0, 4]));
    assert!(matches!(
        Notification::decode(T2, &[0xf5, 0, 4]),
        Ok(Notification::System2Status(_))
    ));
    assert!(matches!(
        Notification::decode(T1, &[0xf5, 0, 0]),
        Ok(Notification::SystemStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T2, &[0xf9, 3, 1]),
        Ok(Notification::System2Parameters(_))
    ));
    let cap = Request::System2Capability(System2Kind::MicrophoneOperation)
        .encode()
        .unwrap();
    assert_eq!(cap.payload, [0xf0, 0x0b]);
    assert!(matches!(
        cap.expected_response.decode(T2, &[0xf1, 0x0b, 0]),
        Ok(Response::System2Capability(_))
    ));
    let params = Request::System2Parameters(System2Kind::AutoVolumeWithLimitations)
        .encode()
        .unwrap();
    assert_eq!(params.payload, [0xf6, 4]);
    assert!(matches!(
        params.expected_response.decode(T2, &[0xf7, 4, 0, 0]),
        Ok(Response::System2Parameters(_))
    ));
    assert!(
        Request::System2Capability(System2Kind::AutoVolumeOptimizer)
            .encode()
            .is_err()
    );
    assert!(
        Request::System2Parameters(System2Kind::WearingStatus)
            .encode()
            .is_err()
    );
}
