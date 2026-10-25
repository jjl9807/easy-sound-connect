use libmdr2::{
    command::{Notification, Request, Response},
    sound_effect::{
        ExclusiveFunction, SoundEffect, SoundEffectBandInfo, SoundEffectCapability,
        SoundEffectCapabilityData, SoundEffectCurveSetting, SoundEffectKind,
        SoundEffectParameterData, SoundEffectParameters, SoundEffectSetting, SoundEffectStatus,
    },
    types::{CommandTable, DisplayLanguage, FunctionId},
};

const TABLE: CommandTable = CommandTable::TABLE_1;

fn customizable() -> SoundEffectCapability {
    SoundEffectCapability::decode_response(TABLE, &[0x51, 0x33, 0xa7, 13, 2, 2, 3]).unwrap()
}

fn bands() -> SoundEffectBandInfo {
    SoundEffectBandInfo::decode_response(TABLE, &[0x5b, 0x33, 2, 1, 0, 63, 2, 0, 4]).unwrap()
}

#[test]
fn advertised_layouts_define_request_shapes() {
    for (kind, function, inquiry) in [
        (SoundEffectKind::Standard, 0x54, 0x30),
        (SoundEffectKind::Customizable, 0x59, 0x33),
        (SoundEffectKind::UltButton, 0x58, 0x40),
    ] {
        assert_eq!(
            kind.function_id(),
            FunctionId {
                table: TABLE,
                code: function
            }
        );
        assert_eq!(
            SoundEffectKind::from_function(kind.function_id()),
            Some(kind)
        );
        assert_eq!(SoundEffectKind::try_from(inquiry).unwrap(), kind);
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(
            kind.capability_uses_language(),
            kind == SoundEffectKind::Standard
        );
        assert_eq!(kind.has_status_query(), kind == SoundEffectKind::Standard);
        assert_eq!(
            kind.has_band_info_query(),
            kind == SoundEffectKind::Customizable
        );
        assert_eq!(
            SoundEffectKind::from_function(FunctionId {
                table: CommandTable::TABLE_2,
                code: function
            }),
            None
        );
    }
    for inquiry in [0, 3, 0x31, 0x32, 0x34, 0x35, 0x36, 0x37, 0x50] {
        assert!(SoundEffectKind::try_from(inquiry).is_err());
    }
}

#[test]
fn capabilities_preserve_identity_and_uninterpreted_data() {
    let cap =
        SoundEffectCapability::decode_response(TABLE, &[0x51, 0x30, 3, 0, 2, 0x9a, 2, 0, 0xab])
            .unwrap();
    let SoundEffectCapabilityData::Standard {
        effects,
        exclusive_functions,
    } = cap.data()
    else {
        panic!()
    };
    assert_eq!(effects[2].as_raw(), 0x9a);
    assert_eq!(
        exclusive_functions,
        &[
            ExclusiveFunction::STAMINA,
            ExclusiveFunction::from_raw(0xab)
        ]
    );
    assert_eq!(
        SoundEffectSetting::Select(SoundEffect::ULT_1)
            .encode_set(&cap)
            .unwrap(),
        [0x58, 0x30, 2]
    );
    for effect in [
        SoundEffect::ULT_2,
        SoundEffect::from_raw(0x9a),
        SoundEffect::NONE,
    ] {
        assert!(SoundEffectSetting::Select(effect).encode_set(&cap).is_err());
    }
    assert!(
        SoundEffectSetting::Reset(SoundEffect::ULT_1)
            .encode_set(&cap)
            .is_err()
    );
    let custom = customizable();
    assert!(matches!(
        custom.data(),
        SoundEffectCapabilityData::Customizable {
            capability_byte: 0xa7,
            level_steps: 13,
            ..
        }
    ));
}

#[test]
fn capabilities_reject_truncation_trailing_data_and_invalid_bounds() {
    for payload in [
        vec![0x51, 0x30, 255],
        vec![0x51, 0x30, 1, 0, 255],
        vec![0x51, 0x30, 0, 0, 1],
        vec![0x51, 0x33, 2, 13, 255],
        vec![0x51, 0x33, 2, 13, 1, 2, 3],
        vec![0x51, 0x40, 3, 2],
        vec![0x51, 0x40, 1, 2, 3],
    ] {
        assert!(
            SoundEffectCapability::decode_response(TABLE, &payload).is_err(),
            "{payload:02x?}"
        );
    }
    let duplicate =
        SoundEffectCapability::decode_response(TABLE, &[0x51, 0x33, 2, 13, 2, 2, 2]).unwrap();
    assert!(
        SoundEffectSetting::Select(SoundEffect::ULT_1)
            .encode_set(&duplicate)
            .is_err()
    );
}

#[test]
fn standard_status_checks_availability_and_capability_membership() {
    let cap = SoundEffectCapability::decode_response(TABLE, &[0x51, 0x30, 2, 2, 3, 0]).unwrap();
    let status =
        SoundEffectStatus::decode_response(TABLE, &[0x53, 0x30, 0, 2, 2, 0, 3, 1]).unwrap();
    assert!(status.enabled);
    assert!(status.effects[0].enabled);
    assert!(!status.effects[1].enabled);
    status.validate_capability(&cap).unwrap();
    let mut payload = vec![0x55, 0x30, 1, 1, 0xa0, 0];
    let status = SoundEffectStatus::decode_notification(TABLE, &payload).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.effects[0].effect.as_raw(), 0xa0);
    assert!(status.validate_capability(&cap).is_err());
    payload[5] = 2;
    assert!(SoundEffectStatus::decode_notification(TABLE, &payload).is_err());
    assert!(SoundEffectStatus::decode_response(TABLE, &[0x53, 0x33, 0, 0]).is_err());
}

#[test]
fn full_custom_response_maps_curves_in_capability_order() {
    let params =
        SoundEffectParameters::decode_response(TABLE, &[0x57, 0x33, 3, 0, 2, 2, 6, 7, 2, 8, 9])
            .unwrap();
    params.validate_capability(&customizable()).unwrap();
    bands().validate_parameters(&params).unwrap();
    let SoundEffectParameterData::Customizable {
        effect,
        exclusive_functions,
        curves: Some(curves),
    } = params.data()
    else {
        panic!()
    };
    assert_eq!(*effect, SoundEffect::ULT_2);
    assert!(exclusive_functions.is_empty());
    assert_eq!(curves, &[vec![6, 7], vec![8, 9]]);

    let wrong_count =
        SoundEffectParameters::decode_response(TABLE, &[0x57, 0x33, 2, 0, 1, 2, 1, 2]).unwrap();
    assert!(wrong_count.validate_capability(&customizable()).is_err());
    let wrong_levels =
        SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x35, 2, 2, 0, 13]).unwrap();
    assert!(wrong_levels.validate_capability(&customizable()).is_err());
    let wrong_dimension =
        SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x35, 2, 1, 0]).unwrap();
    assert!(bands().validate_parameters(&wrong_dimension).is_err());
}

#[test]
fn custom_response_and_selection_notification_are_distinct() {
    let notification = [0x59, 0x33, 0xff, 2, 0, 0x9b];
    let params = SoundEffectParameters::decode_notification(TABLE, &notification).unwrap();
    params.validate_capability(&customizable()).unwrap();
    assert!(matches!(
        params.data(),
        SoundEffectParameterData::Customizable {
            effect: SoundEffect::NONE,
            curves: None,
            ..
        }
    ));
    let mut response = notification;
    response[0] = 0x57;
    assert!(SoundEffectParameters::decode_response(TABLE, &response).is_err());
    assert!(SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x33, 2, 1, 0]).is_err());
    assert!(SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x33, 2, 0, 0]).is_err());
}

#[test]
fn reset_custom_and_external_updates_retain_their_inquiries() {
    for inquiry in [0x34, 0x35] {
        let payload = [0x59, inquiry, 2, 2, 1, 12];
        let params = SoundEffectParameters::decode_notification(TABLE, &payload).unwrap();
        assert_eq!(params.kind(), SoundEffectKind::Customizable);
        assert_eq!(params.inquiry(), inquiry);
        params.validate_capability(&customizable()).unwrap();
        bands().validate_parameters(&params).unwrap();
        let mut response = payload;
        response[0] = 0x57;
        assert!(SoundEffectParameters::decode_response(TABLE, &response).is_err());
    }
    let params =
        SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x36, 0xff, 1, 0xb8, 2, 5, 6])
            .unwrap();
    params.validate_capability(&customizable()).unwrap();
    bands().validate_parameters(&params).unwrap();
    assert_eq!(params.inquiry(), 0x36);
    assert!(
        SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x36, 2, 1, 0, 2, 5, 6]).is_err()
    );
}

#[test]
fn nested_curve_counts_and_all_valid_packet_prefixes_are_bounded() {
    let valid = [0x57, 0x33, 2, 0, 2, 2, 1, 2, 2, 3, 4];
    for length in 0..valid.len() {
        assert!(SoundEffectParameters::decode_response(TABLE, &valid[..length]).is_err());
    }
    for payload in [
        vec![0x57, 0x33, 2, 0, 255],
        vec![0x57, 0x33, 2, 0, 2, 255],
        vec![0x57, 0x33, 2, 0, 2, 0, 255],
        vec![0x57, 0x33, 2, 0, 0],
        vec![0x57, 0x33, 0xff, 255],
        vec![0x57, 0x40, 255],
    ] {
        assert!(
            SoundEffectParameters::decode_response(TABLE, &payload).is_err(),
            "{payload:02x?}"
        );
    }
    let mut trailing = valid.to_vec();
    trailing.push(0);
    assert!(SoundEffectParameters::decode_response(TABLE, &trailing).is_err());
}

#[test]
fn curve_writes_use_band_metadata_native_steps_and_notification_flag() {
    let cap = customizable();
    assert_eq!(
        SoundEffectSetting::Select(SoundEffect::ULT_2)
            .encode_set(&cap)
            .unwrap(),
        [0x58, 0x33, 3]
    );
    assert_eq!(
        SoundEffectSetting::Reset(SoundEffect::ULT_2)
            .encode_set(&cap)
            .unwrap(),
        [0x58, 0x34, 3]
    );
    let mut setting = SoundEffectCurveSetting {
        effect: SoundEffect::ULT_1,
        request_notification: true,
        levels: vec![0, 12],
    };
    assert_eq!(
        setting.encode_set(&cap, &bands()).unwrap(),
        [0x58, 0x35, 0, 2, 2, 0, 12]
    );
    setting.request_notification = false;
    assert_eq!(
        setting.encode_set(&cap, &bands()).unwrap(),
        [0x58, 0x35, 1, 2, 2, 0, 12]
    );
    setting.levels[0] = 13;
    assert!(setting.encode_set(&cap, &bands()).is_err());
    setting.levels = vec![0];
    assert!(setting.encode_set(&cap, &bands()).is_err());
    setting.levels.clear();
    let empty = SoundEffectBandInfo::decode_response(TABLE, &[0x5b, 0x33, 0]).unwrap();
    assert!(setting.encode_set(&cap, &empty).is_err());
    setting.levels = vec![0, 0];
    setting.effect = SoundEffect::CUSTOM;
    assert!(setting.encode_set(&cap, &bands()).is_err());
    let zero_steps =
        SoundEffectCapability::decode_response(TABLE, &[0x51, 0x33, 2, 0, 1, 4]).unwrap();
    assert!(setting.encode_set(&zero_steps, &bands()).is_err());
}

#[test]
fn band_metadata_retains_unknown_types_and_uses_big_endian_values() {
    let metadata = SoundEffectBandInfo::decode_response(
        TABLE,
        &[0x5b, 0x33, 3, 0x10, 0, 1, 2, 0, 4, 0xa0, 0x12, 0x34],
    )
    .unwrap();
    assert!(metadata.bands()[0].is_clear_bass());
    assert_eq!(metadata.bands()[1].frequency_hz(), Some(4000));
    assert_eq!(metadata.bands()[2].information_type.as_raw(), 0xa0);
    assert_eq!(metadata.bands()[2].value, 0x1234);
    assert_eq!(metadata.bands()[2].frequency_hz(), None);
    assert!(SoundEffectBandInfo::decode_response(TABLE, &[0x5b, 0x33, 255]).is_err());
    assert!(SoundEffectBandInfo::decode_response(TABLE, &[0x5b, 0x33, 0, 0]).is_err());
    assert!(SoundEffectBandInfo::decode_response(TABLE, &[0x5b, 0x30, 0]).is_err());
}

#[test]
fn ult_button_edit_preserves_wire_order_unknown_entries_and_states() {
    let cap = SoundEffectCapability::decode_response(TABLE, &[0x51, 0x40, 1, 3]).unwrap();
    let current = SoundEffectParameters::decode_response(
        TABLE,
        &[0x57, 0x40, 4, 0x9b, 0, 3, 1, 2, 0, 0xff, 1],
    )
    .unwrap();
    assert_eq!(
        current
            .encode_button_change(&cap, SoundEffect::ULT_2, true)
            .unwrap(),
        [0x58, 0x40, 4, 0x9b, 0, 3, 0, 2, 0, 0xff, 1]
    );
    assert!(
        current
            .encode_button_change(&cap, SoundEffect::from_raw(0x9b), false)
            .is_err()
    );
    assert!(
        current
            .encode_button_change(&cap, SoundEffect::NONE, true)
            .is_err()
    );
    assert!(
        current
            .encode_button_change(&cap, SoundEffect::LIVE, true)
            .is_err()
    );
    assert!(
        SoundEffectSetting::Select(SoundEffect::ULT_1)
            .encode_set(&cap)
            .is_err()
    );
}

#[test]
fn ult_bounds_count_enabled_entries_and_reject_ambiguous_targets() {
    let cap = SoundEffectCapability::decode_response(TABLE, &[0x51, 0x40, 1, 2]).unwrap();
    let current =
        SoundEffectParameters::decode_notification(TABLE, &[0x59, 0x40, 3, 2, 0, 3, 1, 5, 1])
            .unwrap();
    current.validate_capability(&cap).unwrap();
    assert!(
        current
            .encode_button_change(&cap, SoundEffect::ULT_1, false)
            .is_err()
    );
    assert!(
        current
            .encode_button_change(&cap, SoundEffect::ULT_2, true)
            .is_ok()
    );
    let full =
        SoundEffectParameters::decode_response(TABLE, &[0x57, 0x40, 3, 2, 0, 3, 0, 5, 1]).unwrap();
    assert!(
        full.encode_button_change(&cap, SoundEffect::FLAT, true)
            .is_err()
    );
    let duplicate =
        SoundEffectParameters::decode_response(TABLE, &[0x57, 0x40, 2, 2, 0, 2, 1]).unwrap();
    assert!(
        duplicate
            .encode_button_change(&cap, SoundEffect::ULT_1, true)
            .is_err()
    );
    assert!(SoundEffectParameters::decode_response(TABLE, &[0x57, 0x40, 1, 2, 2]).is_err());
}

#[test]
fn decoders_enforce_tables_and_opcodes_and_preserve_unknown_reports() {
    let payload = [0x57, 0x30, 0xc7];
    let current = SoundEffectParameters::decode_response(TABLE, &payload).unwrap();
    assert!(
        matches!(current.data(), SoundEffectParameterData::Standard(effect) if effect.as_raw() == 0xc7)
    );
    assert!(SoundEffectParameters::decode_notification(TABLE, &payload).is_err());
    assert!(SoundEffectParameters::decode_response(CommandTable::TABLE_2, &payload).is_err());
    assert!(
        SoundEffectCapability::decode_response(CommandTable::TABLE_2, &[0x51, 0x40, 1, 2]).is_err()
    );
    assert!(
        SoundEffectStatus::decode_response(CommandTable::TABLE_2, &[0x53, 0x30, 0, 0]).is_err()
    );
    assert!(SoundEffectBandInfo::decode_response(CommandTable::TABLE_2, &[0x5b, 0x33, 0]).is_err());
    assert!(SoundEffectParameters::decode_response(TABLE, &[0x57, 0x32, 0]).is_err());
    assert!(current.validate_capability(&customizable()).is_err());
}

#[test]
fn shared_eqebb_routing_uses_inquiry_and_preserves_query_shapes() {
    for (kind, request_payload, response_payload) in [
        (
            SoundEffectKind::Standard,
            vec![0x50, 0x30, DisplayLanguage::ENGLISH.as_raw()],
            vec![0x51, 0x30, 1, 2, 0],
        ),
        (
            SoundEffectKind::Customizable,
            vec![0x50, 0x33],
            vec![0x51, 0x33, 2, 13, 1, 2],
        ),
        (
            SoundEffectKind::UltButton,
            vec![0x50, 0x40],
            vec![0x51, 0x40, 1, 2],
        ),
    ] {
        let request = Request::SoundEffectCapability {
            kind,
            language: DisplayLanguage::ENGLISH,
        }
        .encode()
        .unwrap();
        assert_eq!(request.table, TABLE);
        assert_eq!(request.payload, request_payload);
        assert!(matches!(
            request.expected_response.decode(TABLE, &response_payload),
            Ok(Response::SoundEffectCapability(cap)) if cap.kind() == kind
        ));
        let mut wrong = response_payload;
        wrong[1] = 0;
        assert!(request.expected_response.decode(TABLE, &wrong).is_err());
        assert_eq!(
            Request::SoundEffectParameters(kind)
                .encode()
                .unwrap()
                .payload,
            [0x56, kind.inquiry()]
        );
    }
    for kind in [SoundEffectKind::Customizable, SoundEffectKind::UltButton] {
        assert!(Request::SoundEffectStatus(kind).encode().is_err());
    }
    for kind in [SoundEffectKind::Standard, SoundEffectKind::UltButton] {
        assert!(Request::SoundEffectBandInfo(kind).encode().is_err());
    }
    assert_eq!(
        Request::SoundEffectBandInfo(SoundEffectKind::Customizable)
            .encode()
            .unwrap()
            .payload,
        [0x5a, 0x33]
    );
    for inquiry in [0x34, 0x35] {
        assert!(matches!(
            Notification::decode(TABLE, &[0x59, inquiry, 2, 1, 6]),
            Ok(Notification::SoundEffectParameters(params)) if params.inquiry() == inquiry
        ));
    }
    assert!(matches!(
        Notification::decode(TABLE, &[0x55, 0x30, 0, 0]),
        Ok(Notification::SoundEffectStatus(_))
    ));
}
