use libmdr2::{
    command::{CommandError, Notification, Request, Response},
    equalizer::{
        BandInformationType, EqualizerBandInfo, EqualizerCapability, EqualizerErrorCode,
        EqualizerKind, EqualizerParameters, EqualizerSetting, EqualizerStatus, PresetId, UltMode,
    },
    types::{CommandTable, DisplayLanguage, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn published_ten_band_bodies_decode_without_model_specific_rules() {
    // sonymdr_open 44adbf92, sonyctl/sim.py INITIAL, GET 50/04, 56/04, 5a/04.
    // Upstream attributes these bodies to WH-1000XM6 FW 3.1.5. RET headers
    // are reconstructed from their corresponding command definitions.
    let cap = EqualizerCapability::decode_response(
        T1,
        &[
            0x51, 0x04, 0x0a, 0x0d, 0x09, 0x00, 0x00, 0x30, 0x00, 0x31, 0x00, 0x32, 0x00, 0x33,
            0x00, 0x20, 0x00, 0xa0, 0x00, 0xa1, 0x00, 0xa2, 0x00,
        ],
    )
    .unwrap();
    assert_eq!(cap.kind(), EqualizerKind::PresetWithErrorCodes);
    assert_eq!(cap.band_count(), 10);
    assert_eq!(cap.level_steps(), 13);
    assert_eq!(cap.ult2_additional_step(), None);
    assert_eq!(
        cap.presets()
            .iter()
            .map(|p| p.id.as_raw())
            .collect::<Vec<_>>(),
        [0, 0x30, 0x31, 0x32, 0x33, 0x20, 0xa0, 0xa1, 0xa2]
    );
    assert!(cap.presets().iter().all(|p| p.name_utf8() == Ok("")));

    let params = EqualizerParameters::decode_response(
        T1,
        &[
            0x57, 0x04, 0xa2, 0x0a, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x09, 0x09, 0x07, 0x06,
        ],
    )
    .unwrap();
    assert_eq!(params.preset(), Some(PresetId::USER_2));
    assert_eq!(params.levels(), [6, 6, 6, 6, 6, 6, 9, 9, 7, 6]);
    params.validate_capability(&cap).unwrap();
    let info = EqualizerBandInfo::decode_response(
        T1,
        &[
            0x5b, 0x04, 0x0a, 0x01, 0x00, 0x1f, 0x01, 0x00, 0x3f, 0x01, 0x00, 0x7d, 0x01, 0x00,
            0xfa, 0x01, 0x01, 0xf4, 0x01, 0x03, 0xe8, 0x01, 0x07, 0xd0, 0x01, 0x0f, 0xa0, 0x01,
            0x1f, 0x40, 0x01, 0x3e, 0x80,
        ],
    )
    .unwrap();
    info.validate_capability(&cap).unwrap();
    assert_eq!(
        info.bands
            .iter()
            .map(|b| b.frequency_hz().unwrap())
            .collect::<Vec<_>>(),
        [31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]
    );
    assert!(info.bands.iter().all(|b| !b.is_clear_bass()));

    let status = EqualizerStatus::decode_response(T1, &[0x53, 4, 0, 0]).unwrap();
    assert!(status.enabled);
    assert!(status.errors.is_empty());
}

#[test]
fn inquiry_and_function_identity_are_not_interchangeable() {
    let cases = [
        (EqualizerKind::Preset, 0x00, 0x50),
        (EqualizerKind::PresetNonCustomizable, 0x02, 0x52),
        (EqualizerKind::PresetWithUlt, 0x03, 0x53),
        (EqualizerKind::PresetWithErrorCodes, 0x04, 0x57),
        (EqualizerKind::Custom, 0x31, 0x55),
    ];
    for (kind, inquiry, code) in cases {
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(EqualizerKind::try_from(inquiry).unwrap(), kind);
        assert_eq!(kind.function_id(), FunctionId { table: T1, code });
        assert_eq!(EqualizerKind::from_function(kind.function_id()), Some(kind));
        assert_eq!(
            EqualizerKind::from_function(FunctionId { table: T2, code }),
            None
        );
    }
    for code in [1, 0x30, 0x32, 0x37, 0xff] {
        assert!(EqualizerKind::try_from(code).is_err());
    }
    assert_eq!(
        EqualizerKind::from_function(FunctionId {
            table: T1,
            code: 0x51
        }),
        None
    );
}

#[test]
fn all_capability_queries_include_language_and_match_the_exact_inquiry() {
    for kind in [
        EqualizerKind::Preset,
        EqualizerKind::PresetNonCustomizable,
        EqualizerKind::PresetWithUlt,
        EqualizerKind::PresetWithErrorCodes,
        EqualizerKind::Custom,
    ] {
        for (language, byte) in [
            (DisplayLanguage::ENGLISH, 1),
            (DisplayLanguage::SIMPLIFIED_CHINESE, 12),
        ] {
            let query = Request::EqualizerCapability { kind, language }
                .encode()
                .unwrap();
            assert_eq!(query.table, T1);
            assert_eq!(query.payload, [0x50, kind.inquiry(), byte]);
            assert!(query.expected_response.matches(T1, &[0x51, kind.inquiry()]));
            assert!(!query.expected_response.matches(T2, &[0x51, kind.inquiry()]));
            assert!(!query.expected_response.matches(T1, &[0x51, 0xfe]));
            assert!(
                query
                    .expected_response
                    .decode(T1, &[0x51, kind.inquiry()])
                    .is_err()
            );
        }
        for (query, get, ret) in [
            (Request::EqualizerStatus(kind), 0x52, 0x53),
            (Request::EqualizerParameters(kind), 0x56, 0x57),
            (Request::EqualizerBandInfo(kind), 0x5a, 0x5b),
        ] {
            let encoded = query.encode().unwrap();
            assert_eq!(encoded.payload, [get, kind.inquiry()]);
            assert_eq!(encoded.expected_response.opcode(), ret);
            assert_eq!(encoded.expected_response.inquiry(), kind.inquiry());
        }
    }
}

#[test]
fn capabilities_retain_utf8_empty_invalid_and_unknown_preset_labels() {
    let cap = EqualizerCapability::decode_response(
        T1,
        &[
            0x51, 0, 2, 21, 4, 0x77, 3, 0xe4, 0xb8, 0xad, 0xa0, 0, 0x77, 1, 0xff, 1, 2, b'A', 0,
        ],
    )
    .unwrap();
    assert_eq!(cap.presets()[0].id.as_raw(), 0x77);
    assert_eq!(cap.presets()[0].name_utf8(), Ok("中"));
    assert_eq!(cap.presets()[1].name_utf8(), Ok(""));
    assert_eq!(cap.presets()[2].name_bytes(), [0xff]);
    assert!(cap.presets()[2].name_utf8().is_err());
    assert_eq!(cap.presets()[3].name_utf8(), Ok("A\0"));
    assert_eq!(cap.presets()[2].id, cap.presets()[0].id);
    assert_eq!(
        EqualizerSetting::Preset {
            preset: PresetId::from_raw(0x77),
            levels: None
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 0, 0x77, 0]
    );
}

#[test]
fn capabilities_enforce_name_limit_record_count_and_exact_total_length() {
    let mut payload = vec![0x51, 0, 1, 21, 1, 0xa0, 128];
    payload.extend([b'x'; 128]);
    assert_eq!(
        EqualizerCapability::decode_response(T1, &payload)
            .unwrap()
            .presets()[0]
            .name_bytes()
            .len(),
        128
    );
    payload[6] = 129;
    payload.push(b'x');
    assert!(EqualizerCapability::decode_response(T1, &payload).is_err());
    for payload in [
        &[0x51, 0, 1, 21, 2, 0xa0, 0][..],
        &[0x51, 0, 1, 21, 0, 0xa0, 0][..],
        &[0x51, 0, 1, 21, 1, 0xa0, 2, b'x'][..],
        &[0x51, 0, 1, 21, 1, 0xa0][..],
    ] {
        assert!(
            EqualizerCapability::decode_response(T1, payload).is_err(),
            "{payload:02x?}"
        );
    }
}

#[test]
fn ult_and_custom_capabilities_use_distinct_fixed_headers() {
    let ult = EqualizerCapability::decode_response(T1, &[0x51, 3, 6, 21, 4, 1, 0xa1, 0]).unwrap();
    assert_eq!(ult.band_count(), 6);
    assert_eq!(ult.level_steps(), 21);
    assert_eq!(ult.ult2_additional_step(), Some(4));
    assert_eq!(ult.presets()[0].id, PresetId::USER_1);
    let custom = EqualizerCapability::decode_response(T1, &[0x51, 0x31, 3, 19]).unwrap();
    assert_eq!(custom.band_count(), 3);
    assert_eq!(custom.level_steps(), 19);
    assert!(custom.presets().is_empty());
    assert_eq!(custom.ult2_additional_step(), None);
}

#[test]
fn all_response_and_notification_layouts_decode_with_separate_opcodes() {
    for inquiry in [0, 2, 3, 4, 0x31] {
        let mut status = vec![0x53, inquiry, 1];
        if inquiry == 4 {
            status.extend_from_slice(&[2, 0, 0xf7]);
        }
        let expected = EqualizerStatus::decode_response(T1, &status).unwrap();
        assert!(!expected.enabled);
        assert_eq!(expected.kind().inquiry(), inquiry);
        assert!(EqualizerStatus::decode_notification(T1, &status).is_err());
        status[0] = 0x55;
        assert_eq!(
            EqualizerStatus::decode_notification(T1, &status).unwrap(),
            expected
        );
        assert!(matches!(
            Notification::decode(T1, &status).unwrap(),
            Notification::EqualizerStatus(_)
        ));

        let mut params = vec![0x57, inquiry];
        if inquiry != 0x31 {
            params.push(0x77);
        }
        if inquiry == 3 {
            params.push(2);
        }
        params.extend_from_slice(&[3, 0, 127, 255]);
        let expected = EqualizerParameters::decode_response(T1, &params).unwrap();
        assert_eq!(expected.levels(), [0, 127, 255]);
        assert_eq!(
            expected.preset(),
            (inquiry != 0x31).then_some(PresetId::from_raw(0x77))
        );
        assert_eq!(expected.ult_mode(), (inquiry == 3).then_some(UltMode::Ult2));
        assert!(EqualizerParameters::decode_notification(T1, &params).is_err());
        params[0] = 0x59;
        assert_eq!(
            EqualizerParameters::decode_notification(T1, &params).unwrap(),
            expected
        );
        assert!(matches!(
            Notification::decode(T1, &params).unwrap(),
            Notification::EqualizerParameters(_)
        ));
    }
}

#[test]
fn error_status_preserves_codes_and_enforces_count() {
    let status =
        EqualizerStatus::decode_response(T1, &[0x53, 4, 1, 5, 0, 1, 2, 0xfe, 0x99]).unwrap();
    assert_eq!(
        status.errors,
        [
            EqualizerErrorCode::CALLING,
            EqualizerErrorCode::DEMO_MODE,
            EqualizerErrorCode::LISTENING_MODE,
            EqualizerErrorCode::OTHER,
            EqualizerErrorCode::from_raw(0x99)
        ]
    );
    assert_eq!(status.errors[4].as_raw(), 0x99);
    for payload in [
        &[0x53, 4, 1][..],
        &[0x53, 4, 1, 1][..],
        &[0x53, 4, 1, 0, 0][..],
        &[0x53, 0, 2][..],
    ] {
        assert!(EqualizerStatus::decode_response(T1, payload).is_err());
    }
}

#[test]
fn parameter_validation_checks_capability_but_decode_preserves_raw_values() {
    let cap = EqualizerCapability::decode_response(T1, &[0x51, 0, 2, 13, 1, 0xa0, 0]).unwrap();
    for payload in [
        &[0x57, 0, 0xa0, 2, 6, 13][..],
        &[0x57, 0, 0x77, 2, 6, 6][..],
        &[0x57, 0, 0xa0, 1, 6][..],
        &[0x57, 4, 0xa0, 2, 6, 6][..],
    ] {
        let params = EqualizerParameters::decode_response(T1, payload).unwrap();
        assert!(params.validate_capability(&cap).is_err());
    }
    let params = EqualizerParameters::decode_response(T1, &[0x57, 0, 0xa0, 2, 0, 12]).unwrap();
    params.validate_capability(&cap).unwrap();
    assert!(EqualizerParameters::decode_response(T1, &[0x57, 3, 0xa0, 3, 0]).is_err());
}

#[test]
fn metadata_identifies_clear_bass_at_its_actual_index_and_preserves_unknowns() {
    let info = EqualizerBandInfo::decode_response(
        T1,
        &[
            0x5b, 0, 5, 1, 0x01, 0xf4, 0x10, 0, 1, 2, 0xff, 0xff, 0x10, 0, 2, 0x99, 0xab, 0xcd,
        ],
    )
    .unwrap();
    assert_eq!(info.kind(), EqualizerKind::Preset);
    assert_eq!(info.bands[0].frequency_hz(), Some(500));
    assert!(!info.bands[0].is_clear_bass());
    assert!(info.bands[1].is_clear_bass());
    assert_eq!(info.bands[1].frequency_hz(), None);
    assert_eq!(info.bands[2].frequency_hz(), Some(65_535_000));
    assert!(!info.bands[3].is_clear_bass());
    assert_eq!(info.bands[4].information_type.as_raw(), 0x99);
    assert_eq!(info.bands[4].value, 0xabcd);
    assert_eq!(info.bands[4].frequency_hz(), None);
    assert_eq!(
        info.bands[1].information_type,
        BandInformationType::SPECIFIC
    );
    let cap = EqualizerCapability::decode_response(T1, &[0x51, 0, 5, 13, 0]).unwrap();
    info.validate_capability(&cap).unwrap();
    let wrong_count = EqualizerCapability::decode_response(T1, &[0x51, 0, 4, 13, 0]).unwrap();
    assert!(info.validate_capability(&wrong_count).is_err());
    let wrong_kind = EqualizerCapability::decode_response(T1, &[0x51, 0x31, 5, 13]).unwrap();
    assert!(matches!(
        info.validate_capability(&wrong_kind),
        Err(CommandError::CapabilityMismatch { .. })
    ));
}

#[test]
fn selected_preset_and_full_curve_have_distinct_literal_encodings() {
    for inquiry in [0, 4] {
        let cap =
            EqualizerCapability::decode_response(T1, &[0x51, inquiry, 6, 21, 2, 1, 0, 0xa0, 0])
                .unwrap();
        assert_eq!(
            EqualizerSetting::Preset {
                preset: PresetId::ROCK,
                levels: None
            }
            .encode_set(&cap)
            .unwrap(),
            [0x58, inquiry, 1, 0]
        );
        assert_eq!(
            EqualizerSetting::Preset {
                preset: PresetId::CUSTOM,
                levels: Some(vec![13, 10, 10, 10, 10, 10])
            }
            .encode_set(&cap)
            .unwrap(),
            [0x58, inquiry, 0xa0, 6, 13, 10, 10, 10, 10, 10]
        );
    }
}

#[test]
fn curve_writes_validate_presets_range_shape_and_editability() {
    let cap =
        EqualizerCapability::decode_response(T1, &[0x51, 0, 2, 13, 3, 0, 0, 0xa0, 0, 0x77, 0])
            .unwrap();
    for setting in [
        EqualizerSetting::Preset {
            preset: PresetId::USER_1,
            levels: None,
        },
        EqualizerSetting::Preset {
            preset: PresetId::OFF,
            levels: Some(vec![6, 6]),
        },
        EqualizerSetting::Preset {
            preset: PresetId::from_raw(0x77),
            levels: Some(vec![6, 6]),
        },
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![]),
        },
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![6]),
        },
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![6, 6, 6]),
        },
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![6, 13]),
        },
        EqualizerSetting::PresetWithUlt {
            preset: PresetId::CUSTOM,
            ult_mode: UltMode::Off,
            levels: None,
        },
        EqualizerSetting::Custom { levels: vec![6, 6] },
    ] {
        assert!(setting.encode_set(&cap).is_err(), "{setting:?}");
    }
    assert_eq!(
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![0, 12])
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 0, 0xa0, 2, 0, 12]
    );
    let fixed = EqualizerCapability::decode_response(T1, &[0x51, 2, 2, 13, 1, 0xa0, 0]).unwrap();
    assert_eq!(
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: None
        }
        .encode_set(&fixed)
        .unwrap(),
        [0x58, 2, 0xa0, 0]
    );
    assert!(matches!(
        EqualizerSetting::Preset {
            preset: PresetId::CUSTOM,
            levels: Some(vec![6, 6])
        }
        .encode_set(&fixed),
        Err(CommandError::UnsupportedOperation { .. })
    ));
}

#[test]
fn ult_writes_include_mode_without_adjusting_native_levels() {
    let cap = EqualizerCapability::decode_response(T1, &[0x51, 3, 2, 21, 8, 1, 0xa1, 0]).unwrap();
    assert_eq!(
        EqualizerSetting::PresetWithUlt {
            preset: PresetId::USER_1,
            ult_mode: UltMode::Ult2,
            levels: None
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 3, 0xa1, 2, 0]
    );
    assert_eq!(
        EqualizerSetting::PresetWithUlt {
            preset: PresetId::USER_1,
            ult_mode: UltMode::Ult1,
            levels: Some(vec![0, 20])
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 3, 0xa1, 1, 2, 0, 20]
    );
    assert!(
        EqualizerSetting::PresetWithUlt {
            preset: PresetId::USER_1,
            ult_mode: UltMode::Ult2,
            levels: Some(vec![0, 21])
        }
        .encode_set(&cap)
        .is_err()
    );
    assert!(
        EqualizerSetting::Preset {
            preset: PresetId::USER_1,
            levels: None
        }
        .encode_set(&cap)
        .is_err()
    );
    assert_eq!(UltMode::try_from(0).unwrap(), UltMode::Off);
    assert!(UltMode::try_from(0xff).is_err());
}

#[test]
fn standalone_custom_writes_have_no_preset_and_handle_full_u8_counts() {
    let cap = EqualizerCapability::decode_response(T1, &[0x51, 0x31, 3, 19]).unwrap();
    assert_eq!(
        EqualizerSetting::Custom {
            levels: vec![0, 9, 18]
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 0x31, 3, 0, 9, 18]
    );
    assert!(
        EqualizerSetting::Custom {
            levels: vec![0, 9, 19]
        }
        .encode_set(&cap)
        .is_err()
    );
    let maximum = EqualizerCapability::decode_response(T1, &[0x51, 0x31, 255, 255]).unwrap();
    let wire = EqualizerSetting::Custom {
        levels: vec![254; 255],
    }
    .encode_set(&maximum)
    .unwrap();
    assert_eq!(&wire[..3], [0x58, 0x31, 255]);
    assert_eq!(wire.len(), 258);
    let mut response = wire;
    response[0] = 0x57;
    let decoded = EqualizerParameters::decode_response(T1, &response).unwrap();
    decoded.validate_capability(&maximum).unwrap();
    assert!(decoded.preset().is_none());
    assert!(
        EqualizerSetting::Custom {
            levels: vec![0; 256]
        }
        .encode_set(&maximum)
        .is_err()
    );
}

#[test]
fn zero_capability_fields_are_readable_but_cannot_authorize_curve_writes() {
    for body in [[0x51, 0x31, 0, 13], [0x51, 0x31, 1, 0]] {
        let cap = EqualizerCapability::decode_response(T1, &body).unwrap();
        assert!(matches!(
            EqualizerSetting::Custom {
                levels: vec![0; usize::from(body[2])]
            }
            .encode_set(&cap),
            Err(CommandError::InvalidCapability { .. })
        ));
    }
    let cap = EqualizerCapability::decode_response(T1, &[0x51, 2, 0, 0, 1, 0, 0]).unwrap();
    assert_eq!(
        EqualizerSetting::Preset {
            preset: PresetId::OFF,
            levels: None
        }
        .encode_set(&cap)
        .unwrap(),
        [0x58, 2, 0, 0]
    );
}

#[test]
fn decoders_reject_wrong_table_opcode_all_truncations_and_trailing_bytes() {
    type Decode = fn(CommandTable, &[u8]) -> Result<(), CommandError>;
    let cases: &[(Decode, &[u8])] = &[
        (
            |t, p| EqualizerCapability::decode_response(t, p).map(|_| ()),
            &[0x51, 0, 2, 13, 1, 0xa0, 1, b'A'],
        ),
        (
            |t, p| EqualizerCapability::decode_response(t, p).map(|_| ()),
            &[0x51, 3, 2, 13, 4, 1, 0xa0, 0],
        ),
        (
            |t, p| EqualizerCapability::decode_response(t, p).map(|_| ()),
            &[0x51, 0x31, 2, 13],
        ),
        (
            |t, p| EqualizerStatus::decode_response(t, p).map(|_| ()),
            &[0x53, 0, 0],
        ),
        (
            |t, p| EqualizerStatus::decode_response(t, p).map(|_| ()),
            &[0x53, 4, 1, 1, 0],
        ),
        (
            |t, p| EqualizerParameters::decode_response(t, p).map(|_| ()),
            &[0x57, 0, 0xa0, 2, 0, 12],
        ),
        (
            |t, p| EqualizerParameters::decode_response(t, p).map(|_| ()),
            &[0x57, 3, 0xa0, 1, 2, 0, 12],
        ),
        (
            |t, p| EqualizerParameters::decode_response(t, p).map(|_| ()),
            &[0x57, 0x31, 2, 0, 12],
        ),
        (
            |t, p| EqualizerBandInfo::decode_response(t, p).map(|_| ()),
            &[0x5b, 0, 2, 1, 0, 31, 0x10, 0, 1],
        ),
    ];
    for &(decode, payload) in cases {
        decode(T1, payload).unwrap();
        assert!(matches!(
            decode(T2, payload),
            Err(CommandError::WrongTable { .. })
        ));
        for length in 0..payload.len() {
            assert!(
                decode(T1, &payload[..length]).is_err(),
                "truncation {length}: {payload:02x?}"
            );
        }
        let mut trailing = payload.to_vec();
        trailing.push(0);
        assert!(decode(T1, &trailing).is_err());
        let mut opcode = payload.to_vec();
        opcode[0] = 0xfe;
        assert!(matches!(
            decode(T1, &opcode),
            Err(CommandError::UnexpectedOpcode { .. })
        ));
        let mut inquiry = payload.to_vec();
        inquiry[1] = 0x32;
        assert!(decode(T1, &inquiry).is_err());
    }
}

#[test]
fn expected_response_dispatch_retains_typed_equalizer_result() {
    let expected = Request::EqualizerCapability {
        kind: EqualizerKind::Custom,
        language: DisplayLanguage::ENGLISH,
    }
    .encode()
    .unwrap()
    .expected_response;
    let Response::EqualizerCapability(capability) =
        expected.decode(T1, &[0x51, 0x31, 2, 13]).unwrap()
    else {
        panic!("expected an equalizer capability");
    };
    assert_eq!(capability.kind(), EqualizerKind::Custom);
    assert!(expected.decode(T1, &[0x51, 0, 2, 13, 0]).is_err());
    assert!(expected.decode(T1, &[0x59, 0x31, 0]).is_err());
}
