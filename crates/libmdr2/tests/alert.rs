use libmdr2::{
    alert::{
        AlertContents, AlertControl, AlertDecision, AlertMessage, LeAudioAlertStatus,
        LeAudioConfirmation,
    },
    command::{Notification, Request, Response},
    types::CommandTable,
};

const T1: CommandTable = CommandTable::TABLE_1;

#[test]
fn explicit_decisions_keep_message_and_inquiry_identity() {
    for inquiry in [0, 4] {
        let message = AlertMessage::decode_notification(T1, &[0x99, inquiry, 7, 1]).unwrap();
        assert_eq!(message.inquiry(), inquiry);
        assert_eq!(
            message.encode_reply(AlertDecision::Accept).unwrap(),
            [0x98, inquiry, 7, 1]
        );
        assert_eq!(
            message.encode_reply(AlertDecision::Reject).unwrap(),
            [0x98, inquiry, 7, 0]
        );
    }
    assert_eq!(AlertControl::EnableFixedMessages.encode(), [0x94, 0, 0]);
    assert_eq!(AlertControl::ApplicationForeground.encode(), [0x94, 4, 0]);
}

#[test]
fn display_only_and_positive_only_actions_have_distinct_reply_contracts() {
    let display = AlertMessage::decode_notification(T1, &[0x99, 0, 7, 0]).unwrap();
    assert!(display.encode_reply(AlertDecision::Accept).is_err());
    assert!(display.encode_reply(AlertDecision::Reject).is_err());
    let positive = AlertMessage::decode_notification(T1, &[0x99, 0, 7, 2]).unwrap();
    assert!(positive.encode_reply(AlertDecision::Accept).is_ok());
    assert!(positive.encode_reply(AlertDecision::Reject).is_err());
    for invalid in 3..=u8::MAX {
        assert!(AlertMessage::decode_notification(T1, &[0x99, 0, 7, invalid]).is_err());
    }
}

#[test]
fn unknown_and_excluded_fixed_messages_remain_readable_but_cannot_be_answered() {
    // No pairing, GATT, service, standby, stamina or unrelated settings approval.
    for id in [
        2, 3, 5, 0x0e, 0x1d, 0x20, 0x23, 0x24, 0x25, 0x26, 0x2c, 0x2d, 0x2e, 0x2f, 0x31, 0x34,
        0x36, 0x38, 0x40, 0x54, 0x55, 0x60, 0x61, 0x62, 0x63, 0x64, 0x67, 0x68, 0x80, 0x81, 0x82,
        0x83, 0x84, 0x85, 0x88, 0x93, 0xfe, 0xff,
    ] {
        let alert = AlertMessage::decode_notification(T1, &[0x99, 0, id, 1]).unwrap();
        let AlertContents::Fixed { message, .. } = alert.contents() else {
            panic!()
        };
        assert_eq!(message.as_raw(), id);
        assert!(alert.encode_reply(AlertDecision::Accept).is_err());
        assert!(alert.encode_reply(AlertDecision::Reject).is_err());
    }
}

#[test]
fn local_touch_assignment_audio_and_volume_confirmations_keep_explicit_decisions() {
    // Synthetic message bodies keyed by independently documented local triggers.
    // These exercise changes that may stall waiting for a device confirmation.
    let cases: &[(&str, &[u8])] = &[
        ("touch panel switch/reconnect", &[0x0a, 0x0b, 0x0c, 0x0d]),
        (
            "connection quality and LDAC",
            &[0x1c, 0x2a, 0x30, 0x53, 0x74, 0x75, 0x76, 0x77],
        ),
        (
            "multipoint with LDAC",
            &[0x06, 0x07, 0x70, 0x71, 0x72, 0x73],
        ),
        ("local sound consumption and BGM", &[0x08, 0x2b, 0x5c, 0x90]),
        ("automatic volume", &[0x59, 0x5a, 0x5b]),
        (
            "playback and repeat-tap assignment",
            &[0x09, 0x5e, 0x5f, 0x65, 0x66],
        ),
        ("sound effect while stamina already on", &[0x86, 0x87]),
    ];
    for (trigger, identities) in cases {
        for &message in *identities {
            for inquiry in [0, 4] {
                let alert =
                    AlertMessage::decode_notification(T1, &[0x99, inquiry, message, 1]).unwrap();
                assert_eq!(
                    alert.encode_reply(AlertDecision::Accept).unwrap(),
                    [0x98, inquiry, message, 1],
                    "{trigger}"
                );
                assert_eq!(
                    alert.encode_reply(AlertDecision::Reject).unwrap(),
                    [0x98, inquiry, message, 0],
                    "{trigger}"
                );
                let display =
                    AlertMessage::decode_notification(T1, &[0x99, inquiry, message, 0]).unwrap();
                assert!(
                    display.encode_reply(AlertDecision::Accept).is_err(),
                    "{trigger}"
                );
                let confirm =
                    AlertMessage::decode_notification(T1, &[0x99, inquiry, message, 2]).unwrap();
                assert!(
                    confirm.encode_reply(AlertDecision::Accept).is_ok(),
                    "{trigger}"
                );
                assert!(
                    confirm.encode_reply(AlertDecision::Reject).is_err(),
                    "{trigger}"
                );
            }
        }
    }
}

#[test]
fn flexible_replies_require_supported_message_and_items() {
    let alert = AlertMessage::decode_notification(T1, &[0x99, 6, 1, 3, 6, 0x10, 6, 1]).unwrap();
    let AlertContents::Flexible { message, items, .. } = alert.contents() else {
        panic!()
    };
    assert_eq!(message.as_raw(), 1);
    assert_eq!(
        items.iter().map(|item| item.as_raw()).collect::<Vec<_>>(),
        [6, 0x10, 6]
    );
    assert_eq!(
        alert.encode_reply(AlertDecision::Accept).unwrap(),
        [0x98, 6, 1, 1]
    );
    for payload in [
        vec![0x99, 6, 2, 1, 0, 1],
        vec![0x99, 6, 1, 1, 0x0a, 1],
        vec![0x99, 6, 1, 1, 0xff, 1],
    ] {
        let alert = AlertMessage::decode_notification(T1, &payload).unwrap();
        assert!(alert.encode_reply(AlertDecision::Accept).is_err());
    }
}

#[test]
fn flexible_local_settings_validate_message_and_each_affected_feature_independently() {
    // A complete local item list includes the automatic-volume control and NC.
    let items = [0, 1, 2, 3, 6, 7, 0x0b, 0x0d, 0x10, 0x14, 0x18];
    for message in [0, 1, 7, 8, 9, 0x0b, 0x0d, 0x11] {
        let mut payload = vec![0x99, 6, message, items.len() as u8];
        payload.extend_from_slice(&items);
        payload.push(1);
        let alert = AlertMessage::decode_notification(T1, &payload).unwrap();
        assert_eq!(
            alert.encode_reply(AlertDecision::Accept).unwrap(),
            [0x98, 6, message, 1]
        );
        assert_eq!(
            alert.encode_reply(AlertDecision::Reject).unwrap(),
            [0x98, 6, message, 0]
        );
        // One excluded item among valid local items makes the whole reply invalid.
        for unsupported in [4, 5, 8, 9, 10, 12, 14, 15, 17, 18, 19, 21, 22, 23, 25, 255] {
            let mut mixed = payload.clone();
            mixed[5] = unsupported;
            let mixed = AlertMessage::decode_notification(T1, &mixed).unwrap();
            let AlertContents::Flexible { items, .. } = mixed.contents() else {
                panic!()
            };
            assert_eq!(items[1].as_raw(), unsupported);
            assert!(mixed.encode_reply(AlertDecision::Accept).is_err());
            assert!(mixed.encode_reply(AlertDecision::Reject).is_err());
        }
    }
}

#[test]
fn excluded_flexible_workflows_cannot_use_supported_or_empty_items_to_gain_a_reply() {
    // Assistant, GATT activation, Bluetooth standby, pairing and unknown messages.
    for message in [2, 3, 4, 5, 6, 10, 12, 14, 15, 16, 18, 255] {
        for payload in [
            vec![0x99, 6, message, 0, 1],
            vec![0x99, 6, message, 2, 0x06, 0x14, 1],
        ] {
            let alert = AlertMessage::decode_notification(T1, &payload).unwrap();
            let AlertContents::Flexible {
                message: parsed, ..
            } = alert.contents()
            else {
                panic!()
            };
            assert_eq!(parsed.as_raw(), message);
            assert!(alert.encode_reply(AlertDecision::Accept).is_err());
            assert!(alert.encode_reply(AlertDecision::Reject).is_err());
        }
    }
}

#[test]
fn left_right_assistant_notices_are_diagnostic_only() {
    for side in [0, 1] {
        let alert = AlertMessage::decode_notification(T1, &[0x99, 2, 0, side]).unwrap();
        assert!(
            matches!(alert.contents(), AlertContents::LeftRight { default_right, .. } if *default_right == (side == 1))
        );
        assert!(alert.encode_reply(AlertDecision::Accept).is_err());
        assert!(alert.encode_reply(AlertDecision::Reject).is_err());
    }
    assert!(AlertMessage::decode_notification(T1, &[0x99, 2, 0, 2]).is_err());
}

#[test]
fn flexible_counts_are_bounded_unsigned_and_lengths_are_exact() {
    let mut largest = vec![0x99, 6, 1, 255];
    largest.extend([0; 255]);
    largest.push(1);
    let alert = AlertMessage::decode_notification(T1, &largest).unwrap();
    let AlertContents::Flexible { items, .. } = alert.contents() else {
        panic!()
    };
    assert_eq!(items.len(), 255);
    for payload in [
        vec![0x99, 0, 7, 1],
        vec![0x99, 4, 1, 0],
        vec![0x99, 2, 0, 1],
        vec![0x99, 6, 1, 0, 1],
        largest,
    ] {
        for end in 0..payload.len() {
            assert!(AlertMessage::decode_notification(T1, &payload[..end]).is_err());
        }
        assert!(AlertMessage::decode_notification(CommandTable::TABLE_2, &payload).is_err());
        let mut bad = payload.clone();
        bad.push(0);
        assert!(AlertMessage::decode_notification(T1, &bad).is_err());
        bad = payload.clone();
        bad[0] = 0x98;
        assert!(AlertMessage::decode_notification(T1, &bad).is_err());
    }
}

#[test]
fn le_audio_notice_queries_and_confirmations_do_not_alias_prompt_replies() {
    let request = Request::LeAudioAlertStatus.encode().unwrap();
    assert_eq!(request.payload, [0x92, 5]);
    assert_eq!(
        request.expected_response.decode(T1, &[0x93, 5, 0]).unwrap(),
        Response::LeAudioAlertStatus(LeAudioAlertStatus { enabled: true })
    );
    assert_eq!(
        Notification::decode(T1, &[0x95, 5, 1]).unwrap(),
        Notification::LeAudioAlertStatus(LeAudioAlertStatus { enabled: false })
    );
    assert!(!request.expected_response.matches(T1, &[0x95, 5, 0]));
    assert_eq!(LeAudioConfirmation::Confirm.encode(), [0x94, 5, 0]);
    assert_eq!(
        LeAudioConfirmation::ConfirmAndDoNotShowAgain.encode(),
        [0x94, 5, 1]
    );
    assert!(LeAudioAlertStatus::decode_response(T1, &[0x93, 5, 2]).is_err());
    assert!(LeAudioAlertStatus::decode_response(T1, &[0x93, 3, 0]).is_err());
    assert!(matches!(
        Notification::decode(T1, &[0x99, 0, 7, 1]),
        Ok(Notification::AlertMessage(_))
    ));
}
