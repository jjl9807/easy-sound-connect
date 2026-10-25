use libmdr2::{
    assignment::AssignmentKind,
    command::{CommandError, Notification, Request, Response},
    connection::{ConnectionNotice, ReconnectionPhase},
    equalizer::EqualizerKind,
    general::GeneralKind,
    peripheral::{PeripheralKind, PeripheralParameters},
    playback::{PlaybackKind, VolumeKind},
    sound_effect::{SoundEffectKind, SoundEffectParameterData},
    system::SystemKind,
    types::{CommandTable, DisplayLanguage},
    voice_guidance::VoiceKind,
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// Synthetic packets test the public routing boundary, independently of module
// codec tests. Request/response/notification differences are intentional.
fn query(request: Request, table: CommandTable, outgoing: &[u8], incoming: &[u8]) -> Response {
    let encoded = request.encode().unwrap();
    assert_eq!(encoded.table, table);
    assert_eq!(encoded.payload, outgoing);
    let expected = encoded.expected_response;
    assert_eq!(expected.table(), table);
    assert_eq!(expected.opcode(), incoming[0]);
    assert_eq!(expected.inquiry(), incoming[1]);
    assert!(expected.matches(table, incoming));
    let other_table = if table == T1 { T2 } else { T1 };
    assert!(!expected.matches(other_table, incoming));
    assert!(expected.decode(other_table, incoming).is_err());
    let mut other_inquiry = incoming.to_vec();
    other_inquiry[1] ^= 0x80;
    assert!(!expected.matches(table, &other_inquiry));
    assert!(expected.decode(table, &other_inquiry).is_err());
    assert!(Notification::decode(table, incoming).is_err());
    expected.decode(table, incoming).unwrap()
}

fn pair(
    request: Request,
    table: CommandTable,
    outgoing: &[u8],
    response: &[u8],
    notification: &[u8],
) -> (Response, Notification) {
    let decoded = query(request, table, outgoing, response);
    let expected = request.encode().unwrap().expected_response;
    assert!(!expected.matches(table, notification));
    assert!(expected.decode(table, notification).is_err());
    let other_table = if table == T1 { T2 } else { T1 };
    assert!(Notification::decode(other_table, notification).is_err());
    (decoded, Notification::decode(table, notification).unwrap())
}

#[test]
fn general_slots_keep_their_routing_identity_across_all_read_exchanges() {
    for (kind, inquiry) in [
        (GeneralKind::Setting1, 0xd1),
        (GeneralKind::Setting2, 0xd2),
        (GeneralKind::Setting3, 0xd3),
        (GeneralKind::Setting4, 0xd4),
    ] {
        let result = query(
            Request::GeneralCapability {
                kind,
                language: DisplayLanguage::SIMPLIFIED_CHINESE,
            },
            T1,
            &[0xd0, inquiry, 0x0c],
            &[0xd1, inquiry, 0, 0, 1, b'X', 0],
        );
        assert!(matches!(result, Response::GeneralCapability(cap) if cap.kind() == kind));
        let (response, notification) = pair(
            Request::GeneralStatus(kind),
            T1,
            &[0xd2, inquiry],
            &[0xd3, inquiry, 0],
            &[0xd5, inquiry, 0],
        );
        assert!(matches!((response, notification),
            (Response::GeneralStatus(a), Notification::GeneralStatus(b)) if a == b));
        let (response, notification) = pair(
            Request::GeneralParameters(kind),
            T1,
            &[0xd6, inquiry],
            &[0xd7, inquiry, 0, 1],
            &[0xd9, inquiry, 0, 1],
        );
        assert!(matches!((response, notification),
            (Response::GeneralParameters(a), Notification::GeneralParameters(b)) if a == b));
    }
}

#[test]
fn playback_shared_parameter_opcode_dispatches_metadata_and_all_volume_layouts() {
    for (kind, inquiry) in [
        (PlaybackKind::WithCallVolume, 1),
        (PlaybackKind::WithCallVolumeAndFunctionChange, 2),
        (PlaybackKind::WithFunctionChange, 3),
    ] {
        let mut capability = vec![0xa1, inquiry, 31];
        if inquiry != 3 {
            capability.push(16);
        }
        assert!(
            matches!(query(Request::PlaybackCapability(kind), T1, &[0xa0, inquiry], &capability),
            Response::PlaybackCapability(cap) if cap.kind() == kind)
        );
        let mut status = vec![0xa3, inquiry, 0, 2, 0];
        if inquiry == 2 {
            status.push(0);
        }
        let mut changed = status.clone();
        changed[0] = 0xa5;
        let (response, notification) = pair(
            Request::PlaybackStatus(kind),
            T1,
            &[0xa2, inquiry],
            &status,
            &changed,
        );
        assert!(matches!((response, notification),
            (Response::PlaybackStatus(a), Notification::PlaybackStatus(b)) if a == b));
        let metadata = [0xa7, inquiry, 1, 0, 1, 0, 1, 0, 1, 0];
        let mut changed = metadata;
        changed[0] = 0xa9;
        let (response, notification) = pair(
            Request::PlaybackMetadata(kind),
            T1,
            &[0xa6, inquiry],
            &metadata,
            &changed,
        );
        assert!(matches!((response, notification),
            (Response::PlaybackMetadata(a), Notification::PlaybackMetadata(b)) if a == b));
        assert!(
            !Request::PlaybackVolume(VolumeKind::Music)
                .encode()
                .unwrap()
                .expected_response
                .matches(T1, &metadata)
        );
    }
    for (kind, inquiry, muted) in [
        (VolumeKind::Music, 0x20, false),
        (VolumeKind::Call, 0x21, false),
        (VolumeKind::MusicWithMute, 0x30, true),
        (VolumeKind::CallWithMute, 0x31, true),
    ] {
        let mut response = vec![0xa7, inquiry, 9];
        if muted {
            response.push(0);
        }
        let mut notification = response.clone();
        notification[0] = 0xa9;
        let (response, notification) = pair(
            Request::PlaybackVolume(kind),
            T1,
            &[0xa6, inquiry],
            &response,
            &notification,
        );
        assert!(matches!((response, notification),
            (Response::PlaybackVolume(a), Notification::PlaybackVolume(b)) if a == b));
    }
    assert!(Notification::decode(T1, &[0xa9, 0x40, 0]).is_err());
}

#[test]
fn assignment_dispatch_does_not_shadow_system_switches_or_speak_to_chat() {
    for (kind, inquiry) in [
        (AssignmentKind::Standard, 3),
        (AssignmentKind::WithLimitations, 0x0e),
    ] {
        let mut capability = vec![0xf1, inquiry];
        if inquiry == 0x0e {
            capability.push(1);
        }
        // One physical key, one preset, one fixed action/function pair.
        capability.extend([1, 0, 1, 0, 1, 0, 1, 0, 0, 2]);
        assert!(
            matches!(query(Request::AssignmentCapability(kind), T1, &[0xf0, inquiry], &capability),
            Response::AssignmentCapability(cap) if cap.kind() == kind)
        );
        let (a, b) = pair(
            Request::AssignmentStatus(kind),
            T1,
            &[0xf2, inquiry],
            &[0xf3, inquiry, 1, 0],
            &[0xf5, inquiry, 1, 0],
        );
        assert!(
            matches!((a,b), (Response::AssignmentStatus(a), Notification::AssignmentStatus(b)) if a == b)
        );
        let (a, b) = pair(
            Request::AssignmentParameters(kind),
            T1,
            &[0xf6, inquiry],
            &[0xf7, inquiry, 1, 0],
            &[0xf9, inquiry, 1, 0],
        );
        assert!(
            matches!((a,b), (Response::AssignmentParameters(a), Notification::AssignmentParameters(b)) if a == b)
        );
        let (a, b) = pair(
            Request::AssignmentCustomParameters(kind),
            T1,
            &[0xfa, inquiry],
            &[0xfb, inquiry, 1, 0x71, 1, 1, 0x21],
            &[0xfd, inquiry, 1, 0x71, 1, 1, 0x21],
        );
        assert!(
            matches!((a,b), (Response::AssignmentCustomParameters(a), Notification::AssignmentCustomParameters(b)) if a == b)
        );
    }
    assert!(matches!(
        Notification::decode(T1, &[0xf5, 0, 0]).unwrap(),
        Notification::SystemStatus(_)
    ));
    assert!(matches!(
        Notification::decode(T1, &[0xf9, 0, 0]).unwrap(),
        Notification::SystemParameters(_)
    ));
    assert!(matches!(
        Notification::decode(T1, &[0xfd, 0x0c, 0, 0]).unwrap(),
        Notification::SystemExtendedParameters(_)
    ));
    let system = Request::SystemParameters(SystemKind::Vibrator)
        .encode()
        .unwrap();
    let assignment = Request::AssignmentParameters(AssignmentKind::Standard)
        .encode()
        .unwrap();
    assert_eq!(
        system.expected_response.opcode(),
        assignment.expected_response.opcode()
    );
    assert!(!system.expected_response.matches(T1, &[0xf7, 3, 1, 0]));
    assert!(!assignment.expected_response.matches(T1, &[0xf7, 0, 0]));
}

#[test]
fn peripheral_queries_separate_short_source_keep_replies_and_targeted_results() {
    for (kind, inquiry) in [
        (PeripheralKind::Devices, 0),
        (PeripheralKind::DevicesWithClass, 2),
    ] {
        assert!(
            matches!(query(Request::PeripheralCapability(kind), T2, &[0x30, inquiry], &[0x31, inquiry, 8, 2, 0]),
            Response::PeripheralCapability(cap) if cap.kind() == kind)
        );
        let (a, b) = pair(
            Request::PeripheralStatus(kind),
            T2,
            &[0x32, inquiry],
            &[0x33, inquiry, 0, 0],
            &[0x35, inquiry, 0, 0],
        );
        assert!(
            matches!((a,b), (Response::PeripheralStatus(a), Notification::PeripheralStatus(b)) if a == b)
        );
        let (a, b) = pair(
            Request::PeripheralParameters(kind),
            T2,
            &[0x36, inquiry],
            &[0x37, inquiry, 0, 0],
            &[0x39, inquiry, 0, 0],
        );
        assert!(
            matches!((a,b), (Response::PeripheralParameters(a), Notification::PeripheralParameters(b)) if a == b)
        );
    }
    let request = Request::PeripheralParameters(PeripheralKind::SourceSwitch);
    let (a, b) = pair(request, T2, &[0x36, 1], &[0x37, 1, 0], &[0x39, 1, 0, 2]);
    assert!(matches!(
        a,
        Response::PeripheralParameters(PeripheralParameters::SourceKeep { result: None, .. })
    ));
    assert!(
        matches!(b, Notification::PeripheralParameters(PeripheralParameters::SourceKeep { result: Some(value), .. }) if value.as_raw() == 2)
    );
    assert!(
        request
            .encode()
            .unwrap()
            .expected_response
            .decode(T2, &[0x37, 1, 0, 2])
            .is_err()
    );
    assert!(Notification::decode(T2, &[0x39, 1, 0]).is_err());
    let (a, b) = pair(
        Request::PeripheralParameters(PeripheralKind::MusicHandover),
        T2,
        &[0x36, 3],
        &[0x37, 3, 1],
        &[0x39, 3, 1],
    );
    assert!(
        matches!((a,b), (Response::PeripheralParameters(a), Notification::PeripheralParameters(b)) if a == b)
    );
    let operation_result = b"\x3d\x01\x0002:00:00:00:00:01";
    assert!(matches!(
        Notification::decode(T2, operation_result).unwrap(),
        Notification::PeripheralResult(_)
    ));
    assert!(
        !request
            .encode()
            .unwrap()
            .expected_response
            .matches(T2, operation_result)
    );
    assert!(Notification::decode(T1, operation_result).is_err());
}

#[test]
fn voice_status_routes_selector_zero_and_never_completes_from_language_status() {
    for inquiry in [0, 1, 2, 3, 0x20, 0x21, 0x30, 0x31, 0x32] {
        let kind = VoiceKind::try_from(inquiry).unwrap();
        let mut parameters = vec![0x47, inquiry, 0];
        if matches!(inquiry, 1 | 2) {
            parameters.push(0xf0);
        }
        let mut notification = parameters.clone();
        notification[0] = 0x49;
        if inquiry == 1 {
            notification.pop();
        }
        let (response, notification) = pair(
            Request::VoiceParameters(kind),
            T2,
            &[0x46, inquiry],
            &parameters,
            &notification,
        );
        let (Response::VoiceParameters(response), Notification::VoiceParameters(notification)) =
            (response, notification)
        else {
            panic!("incorrect voice parameter route")
        };
        assert_eq!(response.kind(), notification.kind());
        if inquiry == 1 {
            assert_eq!(response.language().unwrap().as_raw(), 0xf0);
            assert_eq!(notification.language(), None);
        } else {
            assert_eq!(response, notification);
        }
        if inquiry == 0x20 {
            continue;
        }
        let outgoing = if inquiry <= 2 {
            vec![0x42, inquiry, 0]
        } else {
            vec![0x42, inquiry]
        };
        let status = if inquiry <= 2 {
            vec![0x43, inquiry, 0, 0]
        } else {
            vec![0x43, inquiry, 0]
        };
        let mut notification = status.clone();
        notification[0] = 0x45;
        let (a, b) = pair(
            Request::VoiceStatus(kind),
            T2,
            &outgoing,
            &status,
            &notification,
        );
        assert!(
            matches!((a,b), (Response::VoiceStatus(a), Notification::VoiceStatus(b)) if a == b)
        );
        if inquiry <= 2 {
            let expected = Request::VoiceStatus(kind)
                .encode()
                .unwrap()
                .expected_response;
            // Header matching is only dispatch; full decoding must reject the
            // language-update status rather than satisfy this local query.
            let language_update = [0x43, inquiry, 1, 0];
            assert!(expected.matches(T2, &language_update));
            assert!(expected.decode(T2, &language_update).is_err());
            assert!(Notification::decode(T2, &[0x45, inquiry, 1, 0]).is_err());
        }
    }
    for (kind, incoming) in [
        (VoiceKind::OnOffWithMetadata, vec![0x41, 0, 4, 0, 1, 0, 1]),
        (
            VoiceKind::OnOffWithLanguageReport,
            vec![0x41, 1, 4, 0, 1, 0, 1, 1, 0xf0],
        ),
        (
            VoiceKind::OnOffWithRetainedLanguage,
            vec![0x41, 2, 1, 1, 0xf0],
        ),
    ] {
        assert!(
            matches!(query(Request::VoiceCapability(kind), T2, &[0x40, kind.inquiry()], &incoming), Response::VoiceCapability(cap) if cap.kind() == kind)
        );
    }
}

#[test]
fn sound_effect_queries_use_language_only_for_standard_capabilities() {
    let language = DisplayLanguage::from_raw(0x91);
    for (kind, outgoing, incoming) in [
        (
            SoundEffectKind::Standard,
            vec![0x50, 0x30, 0x91],
            vec![0x51, 0x30, 1, 2, 0],
        ),
        (
            SoundEffectKind::Customizable,
            vec![0x50, 0x33],
            vec![0x51, 0x33, 4, 13, 1, 2],
        ),
        (
            SoundEffectKind::UltButton,
            vec![0x50, 0x40],
            vec![0x51, 0x40, 1, 2],
        ),
    ] {
        assert!(
            matches!(query(Request::SoundEffectCapability { kind, language }, T1, &outgoing, &incoming), Response::SoundEffectCapability(cap) if cap.kind() == kind)
        );
    }
    let (a, b) = pair(
        Request::SoundEffectStatus(SoundEffectKind::Standard),
        T1,
        &[0x52, 0x30],
        &[0x53, 0x30, 0, 1, 2, 0],
        &[0x55, 0x30, 0, 1, 2, 0],
    );
    assert!(
        matches!((a,b), (Response::SoundEffectStatus(a), Notification::SoundEffectStatus(b)) if a == b)
    );
    assert!(
        matches!(query(Request::SoundEffectBandInfo(SoundEffectKind::Customizable), T1,
        &[0x5a, 0x33], &[0x5b, 0x33, 1, 1, 0, 63]), Response::SoundEffectBandInfo(info) if info.bands().len() == 1)
    );
    for (kind, response, notification) in [
        (
            SoundEffectKind::Standard,
            vec![0x57, 0x30, 2],
            vec![0x59, 0x30, 2],
        ),
        (
            SoundEffectKind::Customizable,
            vec![0x57, 0x33, 2, 0, 1, 2, 4, 5],
            vec![0x59, 0x33, 2, 0],
        ),
        (
            SoundEffectKind::UltButton,
            vec![0x57, 0x40, 2, 2, 0, 3, 1],
            vec![0x59, 0x40, 2, 2, 0, 3, 1],
        ),
    ] {
        let (a, b) = pair(
            Request::SoundEffectParameters(kind),
            T1,
            &[0x56, kind.inquiry()],
            &response,
            &notification,
        );
        let (Response::SoundEffectParameters(a), Notification::SoundEffectParameters(b)) = (a, b)
        else {
            panic!("incorrect sound effect route")
        };
        assert_eq!(a.kind(), kind);
        assert_eq!(b.kind(), kind);
        if kind == SoundEffectKind::Customizable {
            assert!(matches!(
                a.data(),
                SoundEffectParameterData::Customizable {
                    curves: Some(_),
                    ..
                }
            ));
            assert!(matches!(
                b.data(),
                SoundEffectParameterData::Customizable { curves: None, .. }
            ));
        } else {
            assert_eq!(a, b);
        }
    }
}

#[test]
fn sound_effect_update_subtypes_do_not_shadow_equalizer_notifications() {
    for (inquiry, bytes) in [
        (0x34, vec![0x59, 0x34, 2, 2, 4, 5]),
        (0x35, vec![0x59, 0x35, 2, 2, 4, 5]),
        (0x36, vec![0x59, 0x36, 2, 0, 2, 4, 5]),
    ] {
        assert!(
            matches!(Notification::decode(T1, &bytes).unwrap(), Notification::SoundEffectParameters(value) if value.inquiry() == inquiry)
        );
        let expected = Request::SoundEffectParameters(SoundEffectKind::Customizable)
            .encode()
            .unwrap()
            .expected_response;
        assert!(!expected.matches(T1, &bytes));
        let mut fake_response = bytes;
        fake_response[0] = 0x57;
        assert!(!expected.matches(T1, &fake_response));
        assert!(expected.decode(T1, &fake_response).is_err());
    }
    let (a, b) = pair(
        Request::EqualizerStatus(EqualizerKind::Preset),
        T1,
        &[0x52, 0],
        &[0x53, 0, 0],
        &[0x55, 0, 0],
    );
    assert!(matches!(
        (a, b),
        (
            Response::EqualizerStatus(_),
            Notification::EqualizerStatus(_)
        )
    ));
    let (a, b) = pair(
        Request::EqualizerParameters(EqualizerKind::PresetNonCustomizable),
        T1,
        &[0x56, 2],
        &[0x57, 2, 0, 1, 6],
        &[0x59, 2, 0, 1, 6],
    );
    assert!(matches!(
        (a, b),
        (
            Response::EqualizerParameters(_),
            Notification::EqualizerParameters(_)
        )
    ));
    assert!(
        !Request::SoundEffectParameters(SoundEffectKind::Standard)
            .encode()
            .unwrap()
            .expected_response
            .matches(T1, &[0x57, 2, 0])
    );
    for unknown in [0x32, 0x37, 0xff] {
        assert!(Notification::decode(T1, &[0x59, unknown, 0]).is_err());
    }
}

#[test]
fn alerts_and_link_requests_are_notifications_not_read_responses() {
    let (a, b) = pair(
        Request::LeAudioAlertStatus,
        T1,
        &[0x92, 5],
        &[0x93, 5, 0],
        &[0x95, 5, 0],
    );
    assert!(
        matches!((a,b), (Response::LeAudioAlertStatus(a), Notification::LeAudioAlertStatus(b)) if a == b)
    );
    let expected = Request::LeAudioAlertStatus
        .encode()
        .unwrap()
        .expected_response;
    let alert = [0x99, 0, 6, 1];
    assert!(
        matches!(Notification::decode(T1, &alert).unwrap(), Notification::AlertMessage(message) if message.inquiry() == 0)
    );
    let reconnect = b"\x19\x08\x0102:00:00:00:00:01";
    assert!(matches!(
        Notification::decode(T1, reconnect).unwrap(),
        Notification::ConnectionNotice(ConnectionNotice::Reconnect {
            phase: ReconnectionPhase::Execute,
            ..
        })
    ));
    assert!(matches!(
        Notification::decode(T1, &[0x19, 0x0a]).unwrap(),
        Notification::ConnectionNotice(ConnectionNotice::DisconnectGatt)
    ));
    for bytes in [&alert[..], &reconnect[..], &[0x19, 0x0a][..]] {
        assert!(!expected.matches(T1, bytes));
        assert!(expected.decode(T1, bytes).is_err());
        assert!(Notification::decode(T2, bytes).is_err());
    }
    assert!(Notification::decode(T1, &[0x19, 0x0a, 0]).is_err());
}

#[test]
fn unsupported_queries_fail_before_an_expected_response_is_created() {
    for request in [
        Request::PeripheralCapability(PeripheralKind::SourceSwitch),
        Request::PeripheralCapability(PeripheralKind::MusicHandover),
        Request::PeripheralStatus(PeripheralKind::SourceSwitch),
        Request::PeripheralStatus(PeripheralKind::MusicHandover),
        Request::VoiceCapability(VoiceKind::OnOff),
        Request::VoiceCapability(VoiceKind::Volume),
        Request::VoiceCapability(VoiceKind::FixedVolume),
        Request::VoiceCapability(VoiceKind::BatteryAnnouncement),
        Request::VoiceCapability(VoiceKind::PowerSound),
        Request::VoiceCapability(VoiceKind::UltBeep),
        Request::VoiceStatus(VoiceKind::Volume),
        Request::SoundEffectStatus(SoundEffectKind::Customizable),
        Request::SoundEffectStatus(SoundEffectKind::UltButton),
        Request::SoundEffectBandInfo(SoundEffectKind::Standard),
        Request::SoundEffectBandInfo(SoundEffectKind::UltButton),
    ] {
        assert!(
            matches!(
                request.encode(),
                Err(CommandError::UnsupportedOperation { .. })
            ),
            "{request:?}"
        );
    }
}
