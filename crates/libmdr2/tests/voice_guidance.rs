use libmdr2::{
    types::{CommandTable, FunctionId},
    voice_guidance::{
        VoiceCapability, VoiceKind, VoiceLanguage, VoiceParameters, VoiceProfile, VoiceStatus,
        VoiceVolumeSetting,
    },
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// All voice packets in this file are synthetic wire cases, not device captures.

#[test]
fn advertised_profiles_keep_shared_inquiries_and_optional_volume_distinct() {
    for (profile, code, inquiry, volume) in [
        (VoiceProfile::OnOffWithMetadata, 0x40, 0, None),
        (VoiceProfile::OnOffWithLanguageReport, 0x41, 1, None),
        (
            VoiceProfile::OnOffWithLanguageAndVolume,
            0x42,
            1,
            Some(0x20),
        ),
        (VoiceProfile::FixedVolume, 0x43, 0x21, Some(0x21)),
        (VoiceProfile::OnOffWithRetainedLanguage, 0x44, 2, None),
        (VoiceProfile::OnOff, 0x45, 3, None),
        (VoiceProfile::BatteryAnnouncement, 0x46, 0x30, None),
        (VoiceProfile::PowerSound, 0x47, 0x31, None),
        (VoiceProfile::UltBeep, 0x48, 0x32, None),
    ] {
        assert_eq!(profile.function_id(), FunctionId { table: T2, code });
        assert_eq!(
            VoiceProfile::from_function(FunctionId { table: T2, code }),
            Some(profile)
        );
        assert_eq!(
            VoiceProfile::from_function(FunctionId { table: T1, code }),
            None
        );
        assert_eq!(profile.kind().inquiry(), inquiry);
        assert_eq!(profile.volume_kind().map(VoiceKind::inquiry), volume);
    }
    assert_eq!(
        VoiceProfile::from_function(FunctionId {
            table: T2,
            code: 0x49
        }),
        None
    );
    for byte in [4, 0x22, 0x33, 0xff] {
        assert!(VoiceKind::try_from(byte).is_err());
    }
}

#[test]
fn local_status_queries_have_an_explicit_selector_only_where_required() {
    for byte in [0, 1, 2, 3, 0x20, 0x21, 0x30, 0x31, 0x32] {
        let kind = VoiceKind::try_from(byte).unwrap();
        assert_eq!(kind.inquiry(), byte);
        assert_eq!(kind.has_capability_query(), byte <= 2);
        assert_eq!(kind.has_status_query(), byte != 0x20);
        if byte == 0x20 {
            assert!(kind.status_request().is_err());
        } else if byte <= 2 {
            assert_eq!(kind.status_request().unwrap(), [0x42, byte, 0]);
        } else {
            assert_eq!(kind.status_request().unwrap(), [0x42, byte]);
        }
        assert_eq!(VoiceCapability::fixed(kind).is_ok(), byte > 2);
    }
}

#[test]
fn capability_support_bit_is_not_the_on_off_boolean_convention() {
    let capability = VoiceCapability::decode_response(T2, &[0x41, 0, 4, 0, 1, 0, 1]).unwrap();
    assert_eq!(capability.kind(), VoiceKind::OnOffWithMetadata);
    assert!(capability.supports_switch());
    assert_eq!(capability.header_bytes(), Some([4, 0, 1, 0]));
    assert!(capability.languages().is_empty());
    let no_switch = VoiceCapability::decode_response(T2, &[0x41, 0, 4, 0, 1, 0, 0]).unwrap();
    assert!(!no_switch.supports_switch());
    let current = VoiceParameters::decode_response(T2, &[0x47, 0, 0]).unwrap();
    assert_eq!(current.enabled(), Some(true));
    assert_eq!(
        current.encode_switch(false, &capability).unwrap(),
        [0x48, 0, 1]
    );
    assert!(current.encode_switch(false, &no_switch).is_err());
}

#[test]
fn language_ids_and_order_are_preserved_without_guessing_other_family_enums() {
    for packet in [
        vec![0x41, 1, 4, 1, 0, 1, 1, 4, 0xf0, 0x81, 1, 0x81],
        vec![0x41, 2, 1, 4, 0xf0, 0x81, 1, 0x81],
    ] {
        let capability = VoiceCapability::decode_response(T2, &packet).unwrap();
        let raw: Vec<_> = capability
            .languages()
            .iter()
            .map(|value| value.as_raw())
            .collect();
        assert_eq!(raw, [0xf0, 0x81, 1, 0x81]);
        assert_eq!(capability.languages()[1], VoiceLanguage::from_raw(0x81));
    }
    let mut packet = vec![0x41, 2, 1, 255];
    packet.extend(0..=254);
    let capability = VoiceCapability::decode_response(T2, &packet).unwrap();
    assert_eq!(capability.languages().len(), 255);
    assert_eq!(capability.languages()[254].as_raw(), 254);
    packet[3] = 254;
    assert!(VoiceCapability::decode_response(T2, &packet).is_err());
    assert!(
        VoiceCapability::decode_response(T2, &[0x41, 2, 1, 0])
            .unwrap()
            .languages()
            .is_empty()
    );
}

#[test]
fn capability_flags_and_header_extent_are_checked_independently() {
    for offset in 3..=6 {
        let mut packet = [0x41, 0, 4, 0, 1, 0, 1];
        packet[offset] = 2;
        assert!(VoiceCapability::decode_response(T2, &packet).is_err());
    }
    assert!(VoiceCapability::decode_response(T2, &[0x41, 0, 5, 0, 1, 0, 1]).is_err());
    // The leading header byte is retained rather than fixed to a guessed version.
    let capability = VoiceCapability::decode_response(T2, &[0x41, 0, 0, 0, 1, 0, 1]).unwrap();
    assert_eq!(capability.header_bytes(), Some([0, 0, 1, 0]));
    assert!(VoiceCapability::decode_response(T2, &[0x41, 2, 2, 0]).is_err());
    assert!(VoiceCapability::decode_response(T2, &[0x41, 3, 1]).is_err());
}

#[test]
fn status_decoding_separates_local_availability_from_language_update_status() {
    for byte in [0, 1, 2, 3, 0x21, 0x30, 0x31, 0x32] {
        for enabled in [true, false] {
            let wire = if enabled { 0 } else { 1 };
            let mut packet = if byte <= 2 {
                vec![0x43, byte, 0, wire]
            } else {
                vec![0x43, byte, wire]
            };
            let status = VoiceStatus::decode_response(T2, &packet).unwrap();
            assert_eq!(status.kind().inquiry(), byte);
            assert_eq!(status.enabled, enabled);
            packet[0] = 0x45;
            assert_eq!(
                VoiceStatus::decode_notification(T2, &packet).unwrap(),
                status
            );
            assert!(VoiceStatus::decode_response(T2, &packet).is_err());
            if byte <= 2 {
                packet[2] = 1;
                assert!(VoiceStatus::decode_notification(T2, &packet).is_err());
            }
        }
    }
    assert!(VoiceStatus::decode_response(T2, &[0x43, 0x20, 0]).is_err());
    assert!(VoiceStatus::decode_response(T2, &[0x43, 0, 0]).is_err());
    assert!(VoiceStatus::decode_response(T2, &[0x43, 3, 2]).is_err());
}

#[test]
fn inquiry_one_notifications_omit_language_and_writes_never_add_it() {
    let capability =
        VoiceCapability::decode_response(T2, &[0x41, 1, 4, 0, 1, 0, 1, 1, 0xf0]).unwrap();
    let response = VoiceParameters::decode_response(T2, &[0x47, 1, 0, 0xf0]).unwrap();
    let notification = VoiceParameters::decode_notification(T2, &[0x49, 1, 1]).unwrap();
    assert_eq!(response.language().unwrap().as_raw(), 0xf0);
    assert_eq!(notification.language(), None);
    assert_eq!(notification.enabled(), Some(false));
    assert_eq!(
        response.encode_switch(false, &capability).unwrap(),
        [0x48, 1, 1]
    );
    assert_eq!(
        notification.encode_switch(true, &capability).unwrap(),
        [0x48, 1, 0]
    );
    assert!(VoiceParameters::decode_response(T2, &[0x47, 1, 0]).is_err());
    assert!(VoiceParameters::decode_notification(T2, &[0x49, 1, 0, 0xf0]).is_err());
}

#[test]
fn inquiry_two_switch_writes_preserve_the_received_advertised_language() {
    let capability = VoiceCapability::decode_response(T2, &[0x41, 2, 1, 2, 1, 0xf0]).unwrap();
    for (opcode, raw) in [(0x47, 1), (0x49, 0xf0)] {
        let packet = [opcode, 2, 0, raw];
        let current = if opcode == 0x47 {
            VoiceParameters::decode_response(T2, &packet)
        } else {
            VoiceParameters::decode_notification(T2, &packet)
        }
        .unwrap();
        assert_eq!(current.language().unwrap().as_raw(), raw);
        assert_eq!(
            current.encode_switch(false, &capability).unwrap(),
            [0x48, 2, 1, raw]
        );
    }
    let current = VoiceParameters::decode_response(T2, &[0x47, 2, 0, 0x81]).unwrap();
    assert!(current.encode_switch(false, &capability).is_err());
    let capability = VoiceCapability::decode_response(T2, &[0x41, 2, 1, 1, 0x81]).unwrap();
    assert_eq!(
        current.encode_switch(false, &capability).unwrap(),
        [0x48, 2, 1, 0x81]
    );
    // Raw unknown identities are preserved; no API accepts a replacement language.
    assert!(VoiceParameters::decode_response(T2, &[0x47, 2, 0]).is_err());
}

#[test]
fn fixed_sound_switches_share_boolean_encoding_but_not_capability_identity() {
    for byte in [3, 0x30, 0x31, 0x32] {
        let kind = VoiceKind::try_from(byte).unwrap();
        let capability = VoiceCapability::fixed(kind).unwrap();
        let current = VoiceParameters::decode_response(T2, &[0x47, byte, 1]).unwrap();
        assert_eq!(current.kind(), kind);
        assert_eq!(current.enabled(), Some(false));
        assert_eq!(current.volume(), None);
        assert!(capability.supports_switch());
        assert_eq!(
            current.encode_switch(true, &capability).unwrap(),
            [0x48, byte, 0]
        );
    }
    let battery = VoiceParameters::decode_response(T2, &[0x47, 0x30, 1]).unwrap();
    assert!(
        battery
            .encode_switch(
                true,
                &VoiceCapability::fixed(VoiceKind::PowerSound).unwrap()
            )
            .is_err()
    );
}

#[test]
fn all_signed_volume_bytes_are_validated_as_five_native_steps() {
    for byte in 0..=255 {
        let valid = matches!(byte, 0..=2 | 0xfe | 0xff);
        for inquiry in [0x20, 0x21] {
            let decoded = VoiceParameters::decode_response(T2, &[0x47, inquiry, byte]);
            assert_eq!(decoded.is_ok(), valid);
            if let Ok(parameters) = decoded {
                assert_eq!(parameters.volume(), Some(byte as i8));
                assert_eq!(parameters.enabled(), None);
                assert_eq!(parameters.language(), None);
                assert_eq!(
                    VoiceParameters::decode_notification(T2, &[0x49, inquiry, byte]).unwrap(),
                    parameters
                );
                let capability = VoiceCapability::fixed(parameters.kind()).unwrap();
                assert!(!capability.supports_switch());
                assert!(parameters.encode_switch(true, &capability).is_err());
            }
        }
    }
}

#[test]
fn volume_writes_require_the_optional_function_and_always_include_preview() {
    for (level, byte) in [(-2, 0xfe), (-1, 0xff), (0, 0), (1, 1), (2, 2)] {
        for (feedback, flag) in [(true, 0), (false, 1)] {
            let setting = VoiceVolumeSetting { level, feedback };
            assert_eq!(
                setting
                    .encode_set(VoiceProfile::OnOffWithLanguageAndVolume)
                    .unwrap(),
                [0x48, 0x20, byte, flag]
            );
            assert_eq!(
                setting.encode_set(VoiceProfile::FixedVolume).unwrap(),
                [0x48, 0x21, byte, flag]
            );
            assert!(
                setting
                    .encode_set(VoiceProfile::OnOffWithLanguageReport)
                    .is_err()
            );
            assert!(setting.encode_set(VoiceProfile::UltBeep).is_err());
        }
    }
    for level in [-128, -3, 3, 127] {
        assert!(
            VoiceVolumeSetting {
                level,
                feedback: false
            }
            .encode_set(VoiceProfile::FixedVolume)
            .is_err()
        );
    }
}

#[test]
fn exact_packets_reject_every_truncation_and_any_trailing_byte() {
    type Decoder = fn(CommandTable, &[u8]) -> bool;
    let cases: &[(Decoder, &[u8])] = &[
        (
            |t, p| VoiceCapability::decode_response(t, p).is_ok(),
            &[0x41, 0, 4, 0, 1, 0, 1],
        ),
        (
            |t, p| VoiceCapability::decode_response(t, p).is_ok(),
            &[0x41, 1, 4, 0, 1, 0, 1, 2, 1, 0xf0],
        ),
        (
            |t, p| VoiceCapability::decode_response(t, p).is_ok(),
            &[0x41, 2, 1, 1, 0xf0],
        ),
        (
            |t, p| VoiceStatus::decode_response(t, p).is_ok(),
            &[0x43, 0, 0, 1],
        ),
        (
            |t, p| VoiceStatus::decode_notification(t, p).is_ok(),
            &[0x45, 0x31, 0],
        ),
        (
            |t, p| VoiceParameters::decode_response(t, p).is_ok(),
            &[0x47, 1, 0, 1],
        ),
        (
            |t, p| VoiceParameters::decode_notification(t, p).is_ok(),
            &[0x49, 1, 0],
        ),
        (
            |t, p| VoiceParameters::decode_notification(t, p).is_ok(),
            &[0x49, 2, 1, 0xf0],
        ),
        (
            |t, p| VoiceParameters::decode_response(t, p).is_ok(),
            &[0x47, 0x20, 0xfe],
        ),
    ];
    for (decode, packet) in cases {
        assert!(decode(T2, packet));
        assert!(!decode(T1, packet));
        for end in 0..packet.len() {
            assert!(!decode(T2, &packet[..end]));
        }
        let mut longer = packet.to_vec();
        longer.push(0);
        assert!(!decode(T2, &longer));
        let mut wrong_opcode = packet.to_vec();
        wrong_opcode[0] = 0;
        assert!(!decode(T2, &wrong_opcode));
    }
    for byte in [0, 1, 2, 3, 0x30, 0x31, 0x32] {
        let packet = if byte == 1 || byte == 2 {
            vec![0x47, byte, 2, 1]
        } else {
            vec![0x47, byte, 2]
        };
        assert!(VoiceParameters::decode_response(T2, &packet).is_err());
    }
}
