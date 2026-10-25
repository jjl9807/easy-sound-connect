use libmdr2::{
    audio::AudioKind,
    battery::BatteryKind,
    command::{CommandError, Notification, Request, Response},
    power::PowerKind,
    system::SystemKind,
    types::CommandTable,
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn shared_power_opcodes_route_by_table_and_inquiry_not_opcode_alone() {
    let battery = Request::BatteryStatus(BatteryKind::Single)
        .encode()
        .unwrap();
    let standby = Request::PowerStatus(PowerKind::AutoStandby)
        .encode()
        .unwrap();
    assert_eq!(battery.payload, standby.payload);
    assert_eq!(battery.payload, [0x22, 0]);
    assert_ne!(battery.expected_response, standby.expected_response);
    assert!(matches!(
        battery.expected_response.decode(T1, &[0x23, 0, 80, 0]),
        Ok(Response::BatteryStatus(_))
    ));
    assert!(matches!(
        standby.expected_response.decode(T2, &[0x23, 0, 0]),
        Ok(Response::PowerStatus(_))
    ));
    assert!(standby.expected_response.decode(T1, &[0x23, 0, 0]).is_err());
    assert!(
        battery
            .expected_response
            .decode(T2, &[0x23, 0, 80, 0])
            .is_err()
    );

    assert!(matches!(
        Notification::decode(T1, &[0x25, 0, 80, 0]),
        Ok(Notification::BatteryStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T2, &[0x25, 0, 0]),
        Ok(Notification::PowerStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T2, &[0x25, 1, 0, 1]),
        Ok(Notification::PowerStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T1, &[0x25, 4, 0]),
        Ok(Notification::PowerStatus(_))
    ));
    // A malformed known battery message must not fall through to another decoder.
    assert!(Notification::decode(T1, &[0x25, 0, 0]).is_err());
    assert!(Notification::decode(T1, &[0x25, 1, 0, 1]).is_err());
    assert!(Notification::decode(T2, &[0x25, 4, 0]).is_err());
    assert!(Notification::decode(CommandTable::from_raw(3), &[0x25, 0, 0]).is_err());
}

#[test]
fn new_capability_queries_decode_complete_bodies_without_fabricated_operations() {
    for (query, bytes, reply) in [
        (
            Request::AudioCapability(AudioKind::Dsee),
            vec![0xe0, 1],
            vec![0xe1, 1, 2],
        ),
        (
            Request::PowerCapability(PowerKind::AutoPowerOffWithWearingDetection),
            vec![0x20, 5],
            vec![0x21, 5, 2, 0x10, 0x11],
        ),
        (
            Request::PowerCapability(PowerKind::CaringChargeWithThreshold),
            vec![0x20, 1],
            vec![0x21, 1, 80],
        ),
        (
            Request::SystemCapability(SystemKind::SpeakToChatType2),
            vec![0xf0, 0x0c],
            vec![0xf1, 0x0c, 1, 5, 15, 30],
        ),
    ] {
        let encoded = query.encode().unwrap();
        assert_eq!(encoded.payload, bytes);
        assert!(
            encoded
                .expected_response
                .decode(encoded.table, &reply)
                .is_ok()
        );
        assert!(
            encoded
                .expected_response
                .matches(encoded.table, &reply[..2])
        );
        assert!(
            encoded
                .expected_response
                .decode(encoded.table, &reply[..2])
                .is_err()
        );
    }
    for query in [
        Request::AudioCapability(AudioKind::ConnectionPriority),
        Request::AudioCapability(AudioKind::ConnectionPriorityWithStatus),
        Request::PowerCapability(PowerKind::PowerSaving),
        Request::PowerCapability(PowerKind::CaringCharge),
        Request::SystemCapability(SystemKind::PlaybackControlByWearing),
        Request::SystemExtendedParameters(SystemKind::HeadGesture),
    ] {
        assert!(matches!(
            query.encode(),
            Err(CommandError::UnsupportedOperation { .. })
        ));
    }
}

#[test]
fn new_notifications_are_not_responses_even_when_bodies_match() {
    for (query, mut reply, notification) in [
        (
            Request::AudioStatus(AudioKind::Dsee),
            vec![0xe3, 1, 0],
            0xe5,
        ),
        (
            Request::AudioParameters(AudioKind::Dsee),
            vec![0xe7, 1, 1],
            0xe9,
        ),
        (
            Request::PowerParameters(PowerKind::AutoStandby),
            vec![0x27, 0, 0],
            0x29,
        ),
        (
            Request::PowerParameters(PowerKind::AutoPowerOffWithWearingDetection),
            vec![0x27, 5, 0x10, 0],
            0x29,
        ),
        (
            Request::SystemStatus(SystemKind::SpeakToChatType2),
            vec![0xf3, 0x0c, 0, 0],
            0xf5,
        ),
        (
            Request::SystemParameters(SystemKind::SpeakToChatType2),
            vec![0xf7, 0x0c, 1, 1],
            0xf9,
        ),
        (
            Request::SystemExtendedParameters(SystemKind::SpeakToChatType2),
            vec![0xfb, 0x0c, 0, 0],
            0xfd,
        ),
    ] {
        let encoded = query.encode().unwrap();
        let expected = encoded.expected_response;
        assert!(expected.decode(encoded.table, &reply).is_ok());
        assert!(Notification::decode(encoded.table, &reply).is_err());
        reply[0] = notification;
        assert!(!expected.matches(encoded.table, &reply));
        assert!(expected.decode(encoded.table, &reply).is_err());
        assert!(Notification::decode(encoded.table, &reply).is_ok());
    }
}

#[test]
fn le_priority_notification_requires_its_extra_field_and_never_routes_as_a_reply() {
    let query = Request::AudioParameters(AudioKind::ClassicLePriority)
        .encode()
        .unwrap();
    assert_eq!(query.payload, [0xe6, 5]);
    assert!(matches!(
        query.expected_response.decode(T1, &[0xe7, 5, 0]),
        Ok(Response::AudioParameters(_))
    ));
    assert!(
        query
            .expected_response
            .decode(T1, &[0xe7, 5, 0, 0])
            .is_err()
    );
    assert!(Notification::decode(T1, &[0xe9, 5, 0]).is_err());
    assert!(matches!(
        Notification::decode(T1, &[0xe9, 5, 0, 2]),
        Ok(Notification::AudioParameters(_))
    ));
    assert!(Notification::decode(T2, &[0xe9, 5, 0, 2]).is_err());
}

#[test]
fn system_extended_layouts_are_selected_by_the_advertised_variant() {
    let type1 = Request::SystemExtendedParameters(SystemKind::SpeakToChatType1)
        .encode()
        .unwrap();
    let type2 = Request::SystemExtendedParameters(SystemKind::SpeakToChatType2)
        .encode()
        .unwrap();
    assert_eq!(type1.payload, [0xfa, 2]);
    assert_eq!(type2.payload, [0xfa, 0x0c]);
    assert!(matches!(
        type1.expected_response.decode(T1, &[0xfb, 2, 0, 0, 0]),
        Ok(Response::SystemExtendedParameters(_))
    ));
    assert!(
        type1
            .expected_response
            .decode(T1, &[0xfb, 0x0c, 0, 0])
            .is_err()
    );
    assert!(matches!(
        Notification::decode(T1, &[0xfd, 2, 0, 0, 0]),
        Ok(Notification::SystemExtendedParameters(_))
    ));
    assert!(Notification::decode(T1, &[0xfd, 2, 0, 0]).is_err());
}
