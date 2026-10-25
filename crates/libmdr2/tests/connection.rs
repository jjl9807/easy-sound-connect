use libmdr2::{
    command::Notification,
    connection::{ConnectionNotice, HostLeAudioState, LeAudioStateReport, ReconnectionPhase},
    types::{BluetoothAddress, CommandTable},
};

#[test]
fn address_parsing_validates_every_digit_and_separator_without_reversing_octets() {
    let raw = b"02:Ab:CD:ef:01:23";
    let address = BluetoothAddress::parse_ascii(raw).unwrap();
    assert_eq!(address.octets(), [2, 0xab, 0xcd, 0xef, 1, 0x23]);
    assert_eq!(address.to_ascii(), *b"02:AB:CD:EF:01:23");
    assert_eq!(address.to_string(), "02:AB:CD:EF:01:23");
    for i in 0..17 {
        let mut bad = *raw;
        bad[i] = if i % 3 == 2 { b'-' } else { b'g' };
        assert!(BluetoothAddress::parse_ascii(&bad).is_err());
    }
    for end in 0..17 {
        assert!(BluetoothAddress::parse_ascii(&raw[..end]).is_err());
    }
    assert!(BluetoothAddress::parse_ascii(b"02:AB:CD:EF:01:23:").is_err());
}

#[test]
fn reconnect_requests_retain_phase_address_and_table_identity() {
    for (raw, phase) in [
        (0, ReconnectionPhase::UnderConfirmation),
        (1, ReconnectionPhase::Execute),
    ] {
        let mut payload = vec![0x19, 8, raw];
        payload.extend_from_slice(b"02:00:00:00:00:01");
        let notice =
            ConnectionNotice::decode_notification(CommandTable::TABLE_1, &payload).unwrap();
        assert_eq!(
            notice,
            ConnectionNotice::Reconnect {
                phase,
                target: BluetoothAddress::from_octets([2, 0, 0, 0, 0, 1])
            }
        );
        assert_eq!(notice.function_id().code, 0x1a);
        assert_eq!(
            Notification::decode(CommandTable::TABLE_1, &payload).unwrap(),
            Notification::ConnectionNotice(notice)
        );
        for end in 0..payload.len() {
            assert!(
                ConnectionNotice::decode_notification(CommandTable::TABLE_1, &payload[..end])
                    .is_err()
            );
        }
        assert!(ConnectionNotice::decode_notification(CommandTable::TABLE_2, &payload).is_err());
        payload.push(0);
        assert!(ConnectionNotice::decode_notification(CommandTable::TABLE_1, &payload).is_err());
        payload.pop();
        payload[2] = 2;
        assert!(ConnectionNotice::decode_notification(CommandTable::TABLE_1, &payload).is_err());
    }
}

#[test]
fn gatt_disconnect_notice_is_exactly_two_bytes_and_does_not_alias_time_sync() {
    let notice = ConnectionNotice::decode_notification(CommandTable::TABLE_1, &[0x19, 10]).unwrap();
    assert_eq!(notice, ConnectionNotice::DisconnectGatt);
    assert_eq!(notice.function_id().code, 0x1c);
    for payload in [&[0x19, 10, 0][..], &[0x19, 5], &[0x18, 10], &[0x19]] {
        assert!(ConnectionNotice::decode_notification(CommandTable::TABLE_1, payload).is_err());
    }
}

#[test]
fn host_profile_reports_use_table_two_and_have_no_query_or_notification_route() {
    for (state, raw) in [
        (HostLeAudioState::Connecting, 0),
        (HostLeAudioState::Connected, 1),
        (HostLeAudioState::Disconnecting, 2),
        (HostLeAudioState::Disconnected, 3),
    ] {
        let report = LeAudioStateReport {
            state,
            target: BluetoothAddress::from_octets([2, 0, 0, 0, 0, 1]),
        };
        let payload = report.encode();
        let mut expected = vec![0x68, 0, raw];
        expected.extend_from_slice(b"02:00:00:00:00:01");
        assert_eq!(payload.as_slice(), expected);
        assert_eq!(LeAudioStateReport::table(), CommandTable::TABLE_2);
        assert_eq!(LeAudioStateReport::function_id().code, 0x60);
        assert!(Notification::decode(CommandTable::TABLE_2, &payload).is_err());
    }
}
