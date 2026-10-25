use libmdr2::{
    audio::UpscalingType,
    command::{Notification, Request, Response},
    common::{
        AudioCodec, CommonKind, CommonStatus, ConnectionState, DeviceMode, UpscalingEffectState,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn published_codec_and_dsee_effect_bodies_decode_with_reconstructed_headers() {
    // sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39, sonyctl/sim.py
    // INITIAL: author-reported WH-1000XM6 firmware 3.1.5 bodies 10 and 0202.
    // Only the RET opcode and inquiry are added here.
    assert_eq!(
        CommonStatus::decode_response(T1, &[0x13, 2, 0x10]).unwrap(),
        CommonStatus::AudioCodec(AudioCodec::LDAC)
    );
    assert_eq!(
        CommonStatus::decode_response(T1, &[0x13, 3, 2, 2]).unwrap(),
        CommonStatus::UpscalingEffect {
            kind: UpscalingType::DSEE_EXTREME,
            state: UpscalingEffectState::Invalid
        }
    );
}

#[test]
fn connection_states_are_independent_and_invalid_states_are_not_disconnected() {
    assert_eq!(
        CommonStatus::decode_response(T1, &[0x13, 1, 1, 0]).unwrap(),
        CommonStatus::LeftRightConnection {
            left: ConnectionState::Connected,
            right: ConnectionState::NotConnected
        }
    );
    assert_eq!(
        CommonStatus::decode_notification(T1, &[0x15, 1, 0, 1]).unwrap(),
        CommonStatus::LeftRightConnection {
            left: ConnectionState::NotConnected,
            right: ConnectionState::Connected
        }
    );
    for invalid in 2..=u8::MAX {
        assert!(CommonStatus::decode_response(T1, &[0x13, 1, invalid, 1]).is_err());
        assert!(CommonStatus::decode_response(T1, &[0x13, 1, 0, invalid]).is_err());
    }
}

#[test]
fn unknown_identities_survive_without_becoming_known_states() {
    for raw in 0..=u8::MAX {
        let CommonStatus::AudioCodec(codec) =
            CommonStatus::decode_response(T1, &[0x13, 2, raw]).unwrap()
        else {
            panic!()
        };
        assert_eq!(codec.as_raw(), raw);
        let CommonStatus::UpscalingEffect { kind, state } =
            CommonStatus::decode_notification(T1, &[0x15, 3, raw, 1]).unwrap()
        else {
            panic!()
        };
        assert_eq!(kind.as_raw(), raw);
        assert_eq!(state, UpscalingEffectState::Valid);
    }
    for invalid in 3..=u8::MAX {
        assert!(CommonStatus::decode_response(T1, &[0x13, 3, 2, invalid]).is_err());
    }
}

#[test]
fn device_mode_count_is_unsigned_and_preserves_order_duplicates_and_unknowns() {
    assert_eq!(
        CommonStatus::decode_response(T1, &[0x13, 6, 0]).unwrap(),
        CommonStatus::DeviceModes(vec![])
    );
    assert_eq!(
        CommonStatus::decode_response(T1, &[0x13, 6, 4, 1, 0xe7, 1, 0]).unwrap(),
        CommonStatus::DeviceModes(vec![
            DeviceMode::TWO_DEVICES,
            DeviceMode::from_raw(0xe7),
            DeviceMode::TWO_DEVICES,
            DeviceMode::NORMAL
        ])
    );
    let mut payload = vec![0x13, 6, 255];
    payload.extend(0..255);
    let CommonStatus::DeviceModes(modes) = CommonStatus::decode_response(T1, &payload).unwrap()
    else {
        panic!()
    };
    assert_eq!(modes.len(), 255);
    for (raw, mode) in (0..255).zip(modes) {
        assert_eq!(mode.as_raw(), raw);
    }
    payload[2] = 254;
    assert!(CommonStatus::decode_response(T1, &payload).is_err());
}

#[test]
fn report_lengths_and_headers_are_exact_for_every_layout() {
    for payload in [
        vec![0x13, 1, 0, 1],
        vec![0x13, 2, 0x30],
        vec![0x13, 3, 2, 0],
        vec![0x13, 6, 3, 0, 1, 0x80],
    ] {
        assert!(CommonStatus::decode_response(T1, &payload).is_ok());
        assert!(CommonStatus::decode_response(T2, &payload).is_err());
        assert!(CommonStatus::decode_notification(T1, &payload).is_err());
        for end in 0..payload.len() {
            assert!(CommonStatus::decode_response(T1, &payload[..end]).is_err());
        }
        let mut extra = payload.clone();
        extra.push(0);
        assert!(CommonStatus::decode_response(T1, &extra).is_err());
        extra = payload.clone();
        extra[1] = 0xff;
        assert!(CommonStatus::decode_response(T1, &extra).is_err());
        extra = payload.clone();
        extra[0] = 0x15;
        assert!(CommonStatus::decode_notification(T1, &extra).is_ok());
    }
}

#[test]
fn queries_route_by_function_table_and_inquiry_and_do_not_consume_updates() {
    for (kind, code, inquiry, body) in [
        (CommonKind::LeftRightConnection, 0x11, 1, vec![0, 1]),
        (CommonKind::AudioCodec, 0x12, 2, vec![0x30]),
        (CommonKind::UpscalingEffect, 0x13, 3, vec![2, 2]),
        (CommonKind::DeviceModes, 0x18, 6, vec![1, 1]),
    ] {
        assert_eq!(kind.function_id(), FunctionId { table: T1, code });
        assert_eq!(
            CommonKind::from_function(FunctionId { table: T1, code }),
            Some(kind)
        );
        assert_eq!(
            CommonKind::from_function(FunctionId { table: T2, code }),
            None
        );
        let query = Request::CommonStatus(kind).encode().unwrap();
        assert_eq!(query.payload, [0x12, inquiry]);
        let mut payload = vec![0x13, inquiry];
        payload.extend(body);
        let Response::CommonStatus(status) = query.expected_response.decode(T1, &payload).unwrap()
        else {
            panic!()
        };
        assert_eq!(status.kind(), kind);
        payload[0] = 0x15;
        assert!(!query.expected_response.matches(T1, &payload));
        assert!(matches!(
            Notification::decode(T1, &payload),
            Ok(Notification::CommonStatus(_))
        ));
    }
    assert_eq!(
        CommonKind::from_function(FunctionId {
            table: T1,
            code: 0xff
        }),
        None
    );
}
