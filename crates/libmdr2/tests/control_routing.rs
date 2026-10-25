use libmdr2::{
    command::{CommandError, Notification, Request, Response},
    equalizer::EqualizerKind,
    noise::NoiseKind,
    types::{CommandTable, DisplayLanguage},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn noise_reads_encode_the_selected_capability_layout() {
    for (kind, inquiry) in [
        (NoiseKind::NcOnly, 0x01),
        (NoiseKind::NcModesAndAmbientSwitch, 0x12),
        (NoiseKind::NcAndAmbientLevel, 0x13),
        (NoiseKind::NcModesAndAmbientLevel, 0x14),
        (NoiseKind::AutoNcAndAmbientLevel, 0x15),
        (NoiseKind::DualSingleNcAndAmbientLevel, 0x16),
        (NoiseKind::DualNcAndAmbientLevel, 0x17),
        (NoiseKind::AdaptiveNcAndAmbientLevel, 0x19),
        (NoiseKind::AmbientLevel, 0x22),
        (NoiseKind::ButtonCycle, 0x30),
    ] {
        for (query, command, response) in [
            (Request::NoiseStatus(kind), 0x62, 0x63),
            (Request::NoiseParameters(kind), 0x66, 0x67),
        ] {
            let encoded = query.encode().unwrap();
            assert_eq!(encoded.table, T1);
            assert_eq!(encoded.payload, [command, inquiry]);
            assert_eq!(encoded.expected_response.opcode(), response);
            assert_eq!(encoded.expected_response.inquiry(), inquiry);
        }
        let capability = Request::NoiseCapability(kind).encode();
        if kind == NoiseKind::NcOnly {
            assert!(matches!(
                capability,
                Err(CommandError::UnsupportedOperation { .. })
            ));
        } else {
            let encoded = capability.unwrap();
            assert_eq!(encoded.payload, [0x60, inquiry]);
            assert_eq!(encoded.expected_response.opcode(), 0x61);
        }
    }
}

#[test]
fn eq_capability_language_does_not_change_read_response_identity() {
    for (kind, inquiry) in [
        (EqualizerKind::Preset, 0x00),
        (EqualizerKind::PresetNonCustomizable, 0x02),
        (EqualizerKind::PresetWithUlt, 0x03),
        (EqualizerKind::PresetWithErrorCodes, 0x04),
        (EqualizerKind::Custom, 0x31),
    ] {
        let english = Request::EqualizerCapability {
            kind,
            language: DisplayLanguage::ENGLISH,
        }
        .encode()
        .unwrap();
        let chinese = Request::EqualizerCapability {
            kind,
            language: DisplayLanguage::SIMPLIFIED_CHINESE,
        }
        .encode()
        .unwrap();
        assert_eq!(english.table, T1);
        assert_eq!(english.payload, [0x50, inquiry, 0x01]);
        assert_eq!(chinese.payload, [0x50, inquiry, 0x0c]);
        // The wire response does not echo language. These queries must be serialized.
        assert_eq!(english.expected_response, chinese.expected_response);
        assert_eq!(english.expected_response.opcode(), 0x51);
        assert_eq!(english.expected_response.inquiry(), inquiry);
        for (query, command, response) in [
            (Request::EqualizerStatus(kind), 0x52, 0x53),
            (Request::EqualizerParameters(kind), 0x56, 0x57),
            (Request::EqualizerBandInfo(kind), 0x5a, 0x5b),
        ] {
            let encoded = query.encode().unwrap();
            assert_eq!(encoded.payload, [command, inquiry]);
            assert_eq!(encoded.expected_response.opcode(), response);
        }
    }
}

#[test]
fn parameter_notifications_never_match_read_responses() {
    for (request, payload) in [
        (
            Request::NoiseParameters(NoiseKind::AdaptiveNcAndAmbientLevel),
            vec![0x67, 0x19, 1, 1, 1, 1, 20, 0, 0],
        ),
        (
            Request::EqualizerParameters(EqualizerKind::PresetWithErrorCodes),
            vec![0x57, 4, 0xa2, 2, 6, 9],
        ),
    ] {
        let expected = request.encode().unwrap().expected_response;
        assert!(expected.decode(T1, &payload).is_ok());
        assert!(!expected.matches(T2, &payload));
        assert!(expected.decode(T2, &payload).is_err());
        assert!(Notification::decode(T1, &payload).is_err());
        let mut update = payload.clone();
        update[0] += 2;
        assert!(!expected.matches(T1, &update));
        assert!(expected.decode(T1, &update).is_err());
        assert!(Notification::decode(T1, &update).is_ok());
        assert!(Notification::decode(T2, &update).is_err());
        update[1] = 0xff;
        assert!(Notification::decode(T1, &update).is_err());
    }
}

#[test]
fn status_dispatch_is_distinct_from_parameter_and_capability_dispatch() {
    let noise = Request::NoiseStatus(NoiseKind::AdaptiveNcAndAmbientLevel)
        .encode()
        .unwrap();
    assert!(matches!(
        noise.expected_response.decode(T1, &[0x63, 0x19, 0]),
        Ok(Response::NoiseStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T1, &[0x65, 0x19, 1]),
        Ok(Notification::NoiseStatus(_))
    ));
    assert!(
        noise
            .expected_response
            .decode(T1, &[0x65, 0x19, 1])
            .is_err()
    );
    let eq = Request::EqualizerStatus(EqualizerKind::PresetWithErrorCodes)
        .encode()
        .unwrap();
    assert!(matches!(
        eq.expected_response.decode(T1, &[0x53, 4, 1, 1, 0]),
        Ok(Response::EqualizerStatus(_))
    ));
    assert!(matches!(
        Notification::decode(T1, &[0x55, 4, 0, 0]),
        Ok(Notification::EqualizerStatus(_))
    ));
    assert!(eq.expected_response.decode(T1, &[0x53, 0, 1]).is_err());
    assert!(Notification::decode(T1, &[0x51, 4]).is_err());
    assert!(Notification::decode(T1, &[0xff, 0]).is_err());
    assert!(Notification::decode(T1, &[]).is_err());
}

#[test]
fn capability_and_frequency_reads_use_full_body_validation() {
    let noise = Request::NoiseCapability(NoiseKind::AdaptiveNcAndAmbientLevel)
        .encode()
        .unwrap();
    let payload = [0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1];
    assert!(matches!(
        noise.expected_response.decode(T1, &payload),
        Ok(Response::NoiseCapability(_))
    ));
    assert!(noise.expected_response.matches(T1, &payload[..2]));
    assert!(noise.expected_response.decode(T1, &payload[..2]).is_err());
    let eq = Request::EqualizerCapability {
        kind: EqualizerKind::Custom,
        language: DisplayLanguage::ENGLISH,
    }
    .encode()
    .unwrap();
    assert!(matches!(
        eq.expected_response.decode(T1, &[0x51, 0x31, 2, 13]),
        Ok(Response::EqualizerCapability(_))
    ));
    let bands = Request::EqualizerBandInfo(EqualizerKind::Custom)
        .encode()
        .unwrap();
    assert!(matches!(
        bands
            .expected_response
            .decode(T1, &[0x5b, 0x31, 2, 1, 0, 31, 0x10, 0, 1]),
        Ok(Response::EqualizerBandInfo(_))
    ));
    assert!(
        bands
            .expected_response
            .decode(T1, &[0x5b, 0x31, 2, 1, 0, 31])
            .is_err()
    );
}
