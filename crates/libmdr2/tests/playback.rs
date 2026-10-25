use libmdr2::{
    command::CommandError,
    playback::{
        MetadataState, MusicCallState, PlaybackCapability, PlaybackControl, PlaybackKind,
        PlaybackMetadata, PlaybackProfile, PlaybackState, PlaybackStatus, PlaybackVolume,
        VolumeKind, VolumeSetting,
    },
    types::{CommandTable, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn published_xm6_initial_bodies_have_independent_expected_values() {
    // sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39, sonyctl/sim.py
    // INITIAL, author-reported WH-1000XM6 firmware 3.1.5: bodies 1f10,
    // 000200, 13, 0a. RET headers below are reconstructed, not captured frames.
    let capability = PlaybackCapability::decode_response(T1, &[0xa1, 1, 0x1f, 0x10]).unwrap();
    assert_eq!(capability.kind(), PlaybackKind::WithCallVolume);
    assert_eq!(capability.music_maximum(), 31);
    assert_eq!(capability.call_maximum(), Some(16));
    let status = PlaybackStatus::decode_response(T1, &[0xa3, 1, 0, 2, 0]).unwrap();
    assert!(status.enabled);
    assert_eq!(status.playback, PlaybackState::Paused);
    assert_eq!(status.music_call, Some(MusicCallState::Music));
    assert_eq!(status.control_enabled, None);
    let music = PlaybackVolume::decode_response(T1, &[0xa7, 0x20, 0x13]).unwrap();
    assert_eq!(music.kind(), VolumeKind::Music);
    assert_eq!((music.level, music.muted), (19, None));
    let call = PlaybackVolume::decode_response(T1, &[0xa7, 0x21, 0x0a]).unwrap();
    assert_eq!((call.level, call.muted), (10, None));
}

#[test]
fn profiles_preserve_function_identity_beyond_the_reply_inquiry() {
    for (profile, code, kind, music, call) in [
        (
            PlaybackProfile::WithCallVolume,
            0xa1,
            PlaybackKind::WithCallVolume,
            VolumeKind::Music,
            Some(VolumeKind::Call),
        ),
        (
            PlaybackProfile::WithCallVolumeAndMute,
            0xa2,
            PlaybackKind::WithCallVolume,
            VolumeKind::MusicWithMute,
            Some(VolumeKind::CallWithMute),
        ),
        (
            PlaybackProfile::WithCallVolumeAndFunctionChange,
            0xa3,
            PlaybackKind::WithCallVolumeAndFunctionChange,
            VolumeKind::Music,
            Some(VolumeKind::Call),
        ),
        (
            PlaybackProfile::WithFunctionChange,
            0xa4,
            PlaybackKind::WithFunctionChange,
            VolumeKind::Music,
            None,
        ),
    ] {
        assert_eq!(profile.function_id(), FunctionId { table: T1, code });
        assert_eq!(
            PlaybackProfile::from_function(FunctionId { table: T1, code }),
            Some(profile)
        );
        assert_eq!(
            PlaybackProfile::from_function(FunctionId { table: T2, code }),
            None
        );
        assert_eq!(profile.kind(), kind);
        assert_eq!(profile.music_volume_kind(), music);
        assert_eq!(profile.call_volume_kind(), call);
    }
    assert_eq!(
        PlaybackProfile::from_function(FunctionId {
            table: T1,
            code: 0xff
        }),
        None
    );
    assert!(PlaybackKind::try_from(0x40).is_err());
}

#[test]
fn control_inquiry_is_not_derived_from_a3_profiles_read_inquiry() {
    for (profile, cap, inquiry) in [
        (PlaybackProfile::WithCallVolume, vec![0xa1, 1, 31, 16], 1),
        (
            PlaybackProfile::WithCallVolumeAndMute,
            vec![0xa1, 1, 31, 16],
            1,
        ),
        (
            PlaybackProfile::WithCallVolumeAndFunctionChange,
            vec![0xa1, 2, 31, 16],
            1,
        ),
        (PlaybackProfile::WithFunctionChange, vec![0xa1, 3, 31], 3),
    ] {
        let capability = PlaybackCapability::decode_response(T1, &cap).unwrap();
        for (control, byte) in [
            (PlaybackControl::Pause, 1),
            (PlaybackControl::NextTrack, 2),
            (PlaybackControl::PreviousTrack, 3),
            (PlaybackControl::Play, 7),
        ] {
            assert_eq!(
                control.encode_set(profile, &capability).unwrap(),
                [0xa4, inquiry, 0, byte]
            );
        }
    }
    let capability = PlaybackCapability::decode_response(T1, &[0xa1, 3, 31]).unwrap();
    assert!(
        PlaybackControl::Pause
            .encode_set(PlaybackProfile::WithCallVolume, &capability)
            .is_err()
    );
}

#[test]
fn volume_ranges_are_native_inclusive_and_profiles_select_the_layout() {
    let cap = PlaybackCapability::decode_response(T1, &[0xa1, 1, 31, 16]).unwrap();
    for (profile, kind, maximum) in [
        (PlaybackProfile::WithCallVolume, VolumeKind::Music, 31),
        (PlaybackProfile::WithCallVolume, VolumeKind::Call, 16),
        (
            PlaybackProfile::WithCallVolumeAndMute,
            VolumeKind::MusicWithMute,
            31,
        ),
        (
            PlaybackProfile::WithCallVolumeAndMute,
            VolumeKind::CallWithMute,
            16,
        ),
    ] {
        for level in [0, 1, maximum] {
            assert_eq!(
                VolumeSetting { kind, level }
                    .encode_set(profile, &cap)
                    .unwrap(),
                [0xa8, kind.inquiry(), level]
            );
        }
        assert!(
            VolumeSetting {
                kind,
                level: maximum + 1
            }
            .encode_set(profile, &cap)
            .is_err()
        );
    }
    assert!(
        VolumeSetting {
            kind: VolumeKind::MusicWithMute,
            level: 1
        }
        .encode_set(PlaybackProfile::WithCallVolume, &cap)
        .is_err()
    );
    assert!(
        VolumeSetting {
            kind: VolumeKind::Call,
            level: 1
        }
        .encode_set(PlaybackProfile::WithCallVolumeAndMute, &cap)
        .is_err()
    );
    let music_only = PlaybackCapability::decode_response(T1, &[0xa1, 3, 255]).unwrap();
    assert_eq!(
        VolumeSetting {
            kind: VolumeKind::Music,
            level: 255
        }
        .encode_set(PlaybackProfile::WithFunctionChange, &music_only)
        .unwrap(),
        [0xa8, 0x20, 255]
    );
    assert!(
        VolumeSetting {
            kind: VolumeKind::Call,
            level: 1
        }
        .encode_set(PlaybackProfile::WithFunctionChange, &music_only)
        .is_err()
    );
    assert!(
        VolumeSetting {
            kind: VolumeKind::Music,
            level: 1
        }
        .encode_set(PlaybackProfile::WithCallVolume, &music_only)
        .is_err()
    );
}

#[test]
fn capability_maxima_are_unsigned_nonzero_and_exact() {
    for (kind, body) in [(1, vec![128, 255]), (2, vec![255, 128]), (3, vec![255])] {
        let mut p = vec![0xa1, kind];
        p.extend(body);
        assert!(PlaybackCapability::decode_response(T1, &p).is_ok());
        for index in 2..p.len() {
            let mut bad = p.clone();
            bad[index] = 0;
            assert!(PlaybackCapability::decode_response(T1, &bad).is_err());
        }
        assert_shape(&p, PlaybackCapability::decode_response);
    }
}

#[test]
fn mute_is_reported_separately_and_not_written() {
    let cap = PlaybackCapability::decode_response(T1, &[0xa1, 1, 31, 16]).unwrap();
    for (kind, inquiry) in [
        (VolumeKind::MusicWithMute, 0x30),
        (VolumeKind::CallWithMute, 0x31),
    ] {
        let muted = PlaybackVolume::decode_response(T1, &[0xa7, inquiry, 7, 0]).unwrap();
        assert_eq!(muted.muted, Some(true));
        let unmuted = PlaybackVolume::decode_notification(T1, &[0xa9, inquiry, 7, 1]).unwrap();
        assert_eq!(unmuted.muted, Some(false));
        assert_eq!(
            VolumeSetting {
                kind,
                level: muted.level
            }
            .encode_set(PlaybackProfile::WithCallVolumeAndMute, &cap)
            .unwrap(),
            [0xa8, inquiry, 7]
        );
        assert!(PlaybackVolume::decode_response(T1, &[0xa7, inquiry, 7]).is_err());
        for invalid in 2..=u8::MAX {
            assert!(PlaybackVolume::decode_response(T1, &[0xa7, inquiry, 7, invalid]).is_err());
        }
    }
}

#[test]
fn reported_levels_can_be_inspected_before_capability_validation() {
    let volume = PlaybackVolume::decode_response(T1, &[0xa7, 0x20, 255]).unwrap();
    assert_eq!(volume.level, 255);
    let cap = PlaybackCapability::decode_response(T1, &[0xa1, 1, 31, 16]).unwrap();
    assert!(
        volume
            .validate_capability(PlaybackProfile::WithCallVolume, &cap)
            .is_err()
    );
    let volume = PlaybackVolume::decode_response(T1, &[0xa7, 0x20, 31]).unwrap();
    assert!(
        volume
            .validate_capability(PlaybackProfile::WithCallVolume, &cap)
            .is_ok()
    );
    assert!(
        volume
            .validate_capability(PlaybackProfile::WithCallVolumeAndMute, &cap)
            .is_err()
    );
}

#[test]
fn metadata_keeps_field_order_status_and_utf8_bytes() {
    // Synthetic data: track 音, album absent, invalid artist UTF-8, empty settled genre.
    let p = [0xa7, 2, 2, 3, 0xe9, 0x9f, 0xb3, 1, 0, 0, 1, 0xff, 2, 0];
    let data = PlaybackMetadata::decode_response(T1, &p).unwrap();
    assert_eq!(data.kind(), PlaybackKind::WithCallVolumeAndFunctionChange);
    assert_eq!(data.track.as_utf8().unwrap(), "音");
    assert_eq!(data.track.state, MetadataState::Settled);
    assert!(data.album.bytes().is_empty());
    assert_eq!(data.album.state, MetadataState::Nothing);
    assert_eq!(data.artist.bytes(), [0xff]);
    assert!(data.artist.as_utf8().is_err());
    assert_eq!(data.artist.state, MetadataState::Unsettled);
    assert_eq!(data.genre.state, MetadataState::Settled);
    assert_eq!(data.genre.as_utf8().unwrap(), "");
    assert_shape(&p, PlaybackMetadata::decode_response);
}

#[test]
fn metadata_accepts_exactly_four_bounded_names() {
    for inquiry in 1..=3 {
        let empty = [0xa7, inquiry, 0, 0, 1, 0, 2, 0, 1, 0];
        assert!(PlaybackMetadata::decode_response(T1, &empty).is_ok());
        for length in [0, 1, 127, 128] {
            let mut p = vec![0xa7, inquiry];
            for _ in 0..4 {
                p.extend([2, length]);
                p.extend(std::iter::repeat_n(b'x', usize::from(length)));
            }
            let data = PlaybackMetadata::decode_response(T1, &p).unwrap();
            assert_eq!(data.genre.bytes().len(), usize::from(length));
            assert_shape(&p, PlaybackMetadata::decode_response);
            p.extend([1, 0]);
            assert!(PlaybackMetadata::decode_response(T1, &p).is_err());
        }
        let mut excessive = vec![0xa7, inquiry, 2, 129];
        excessive.extend(std::iter::repeat_n(b'x', 129));
        excessive.extend([1, 0, 1, 0, 1, 0]);
        assert!(PlaybackMetadata::decode_response(T1, &excessive).is_err());
    }
}

#[test]
fn metadata_rejects_unknown_states_at_each_entry() {
    for offset in [2, 4, 6, 8] {
        for state in 3..=u8::MAX {
            let mut p = [0xa7, 1, 1, 0, 1, 0, 1, 0, 1, 0];
            p[offset] = state;
            assert!(PlaybackMetadata::decode_response(T1, &p).is_err());
        }
    }
}

#[test]
fn all_status_layouts_keep_availability_and_playback_separate() {
    for p in [
        vec![0xa3, 1, 1, 1, 1],
        vec![0xa3, 2, 0, 0, 0, 1],
        vec![0xa3, 3, 1, 3, 0],
    ] {
        let status = PlaybackStatus::decode_response(T1, &p).unwrap();
        assert_eq!(status.enabled, p[2] == 0);
        match p[1] {
            1 => {
                assert_eq!(status.music_call, Some(MusicCallState::Call));
                assert_eq!(status.control_enabled, None);
            }
            2 => {
                assert_eq!(status.music_call, Some(MusicCallState::Music));
                assert_eq!(status.control_enabled, Some(false));
            }
            3 => {
                assert_eq!(status.music_call, None);
                assert_eq!(status.control_enabled, Some(true));
            }
            _ => unreachable!(),
        }
        assert_shape(&p, PlaybackStatus::decode_response);
    }
}

#[test]
fn undefined_status_fields_are_rejected_at_their_wire_boundary() {
    for invalid in 2..=u8::MAX {
        assert!(PlaybackStatus::decode_response(T1, &[0xa3, 1, invalid, 0, 0]).is_err());
        assert!(PlaybackStatus::decode_response(T1, &[0xa3, 1, 0, 0, invalid]).is_err());
        assert!(PlaybackStatus::decode_response(T1, &[0xa3, 2, 0, 0, 0, invalid]).is_err());
        assert!(PlaybackStatus::decode_response(T1, &[0xa3, 3, 0, 0, invalid]).is_err());
    }
    for invalid in 4..=u8::MAX {
        assert!(PlaybackStatus::decode_response(T1, &[0xa3, 1, 0, invalid, 0]).is_err());
    }
}

#[test]
fn notification_decoders_do_not_accept_query_responses() {
    let metadata = [0xa7, 1, 1, 0, 1, 0, 1, 0, 1, 0];
    assert!(PlaybackMetadata::decode_notification(T1, &metadata).is_err());
    let mut update = metadata;
    update[0] = 0xa9;
    assert!(PlaybackMetadata::decode_notification(T1, &update).is_ok());
    assert!(PlaybackMetadata::decode_response(T1, &update).is_err());
    assert!(PlaybackVolume::decode_notification(T1, &[0xa7, 0x20, 1]).is_err());
    assert!(PlaybackVolume::decode_notification(T1, &[0xa9, 0x20, 1]).is_ok());
    assert!(PlaybackStatus::decode_notification(T1, &[0xa3, 1, 0, 2, 0]).is_err());
    assert!(PlaybackStatus::decode_notification(T1, &[0xa5, 1, 0, 2, 0]).is_ok());
    // The shared parameter opcode does not make volume bodies valid metadata.
    assert!(PlaybackMetadata::decode_notification(T1, &[0xa9, 0x20, 1]).is_err());
    assert!(PlaybackVolume::decode_notification(T1, &update).is_err());
}

fn assert_shape<T>(payload: &[u8], decode: fn(CommandTable, &[u8]) -> Result<T, CommandError>) {
    assert!(decode(T1, payload).is_ok());
    assert!(decode(T2, payload).is_err());
    for end in 0..payload.len() {
        assert!(decode(T1, &payload[..end]).is_err());
    }
    let mut changed = payload.to_vec();
    changed.push(0);
    assert!(decode(T1, &changed).is_err());
    changed = payload.to_vec();
    changed[0] = 0xff;
    assert!(decode(T1, &changed).is_err());
    changed = payload.to_vec();
    changed[1] = 0xff;
    assert!(decode(T1, &changed).is_err());
}
