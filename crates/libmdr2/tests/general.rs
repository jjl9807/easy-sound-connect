use libmdr2::{
    command::{CommandError, Notification, Request, Response},
    general::{
        GeneralCapability, GeneralFeature, GeneralKind, GeneralParameters, GeneralSettingType,
        GeneralStatus, GeneralStringFormat, GeneralValue,
    },
    types::{CommandTable, DisplayLanguage, FunctionId},
};

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

// sonymdr_open 44adbf92df1525571561bbebc0485ea9c436ae39 sonyctl/sim.py
// INITIAL: author-reported WH-1000XM6 FW 3.1.5 bodies. RET headers added.
const SIDETONE: &[u8] = b"\xd1\xd1\x00\x01\x10SIDETONE_SETTING\x18SIDETONE_SETTING_SUMMARY";
const MULTIPOINT: &[u8] =
    b"\xd1\xd2\x00\x01\x12MULTIPOINT_SETTING\x29MULTIPOINT_SETTING_SUMMARY_LDAC_AVAILABLE";
const TOUCH_PANEL: &[u8] = b"\xd1\xd3\x00\x01\x13TOUCH_PANEL_SETTING\x00";

fn info(format: u8, title: &[u8], summary: &[u8]) -> Vec<u8> {
    let mut result = vec![format, u8::try_from(title.len()).unwrap()];
    result.extend_from_slice(title);
    result.push(u8::try_from(summary.len()).unwrap());
    result.extend_from_slice(summary);
    result
}

fn list_capability(count: u8) -> Vec<u8> {
    // Synthetic capability: stable local feature with device-defined list options.
    let mut payload = b"\xd1\xd4\x01\x01\x10SIDETONE_SETTING\x00".to_vec();
    payload.push(count);
    for n in 0..count {
        payload.extend(info(0, &[b'a' + n % 26], &[]));
    }
    payload
}

#[test]
fn published_capabilities_resolve_by_stable_identity_and_preserve_summaries() {
    for (payload, expected, title) in [
        (SIDETONE, GeneralFeature::Sidetone, "SIDETONE_SETTING"),
        (MULTIPOINT, GeneralFeature::Multipoint, "MULTIPOINT_SETTING"),
        (
            TOUCH_PANEL,
            GeneralFeature::TouchPanel,
            "TOUCH_PANEL_SETTING",
        ),
    ] {
        let cap = GeneralCapability::decode_response(T1, payload).unwrap();
        assert_eq!(cap.feature(), Some(expected));
        assert_eq!(cap.info().title_utf8().unwrap(), title);
        assert_eq!(cap.info().format(), GeneralStringFormat::ENUM_NAME);
        assert_eq!(cap.setting_type(), GeneralSettingType::Boolean);
        assert_eq!(cap.options(), None);
        for (value, wire) in [(true, 0), (false, 1)] {
            let setting = GeneralParameters::new(cap.kind(), GeneralValue::Boolean(value)).unwrap();
            assert_eq!(
                setting.encode_set(&cap).unwrap(),
                [0xd8, payload[1], 0, wire]
            );
        }
    }
    assert_eq!(
        GeneralCapability::decode_response(T1, MULTIPOINT)
            .unwrap()
            .info()
            .summary_utf8()
            .unwrap(),
        "MULTIPOINT_SETTING_SUMMARY_LDAC_AVAILABLE"
    );
    assert!(
        GeneralCapability::decode_response(T1, TOUCH_PANEL)
            .unwrap()
            .info()
            .summary_bytes()
            .is_empty()
    );
}

#[test]
fn slot_numbers_never_define_features() {
    for slot in 0xd1..=0xd4 {
        let mut payload = SIDETONE.to_vec();
        payload[1] = slot;
        let cap = GeneralCapability::decode_response(T1, &payload).unwrap();
        assert_eq!(cap.feature(), Some(GeneralFeature::Sidetone));
        assert_eq!(cap.kind().inquiry(), slot);
        let params = GeneralParameters::new(cap.kind(), GeneralValue::Boolean(false)).unwrap();
        assert_eq!(params.encode_set(&cap).unwrap(), [0xd8, slot, 0, 1]);
    }
    let cap = GeneralCapability::decode_response(T1, SIDETONE).unwrap();
    let wrong = GeneralParameters::new(GeneralKind::Setting2, GeneralValue::Boolean(true)).unwrap();
    assert!(matches!(
        wrong.encode_set(&cap),
        Err(CommandError::CapabilityMismatch { .. })
    ));
}

#[test]
fn unknown_and_raw_names_are_readable_but_never_authorize_writes() {
    for (format, title) in [
        (0, b"SIDETONE_SETTING".as_slice()),
        (0xff, b"SIDETONE_SETTING".as_slice()),
        (1, b"VOICE_ASSISTANT_SETTING".as_slice()),
        (1, b"QUICK_ACCESS_SETTING".as_slice()),
        (1, b"NEW_SETTING".as_slice()),
        (1, b"sidetone_setting".as_slice()),
        (1, b"SIDETONE_SETTING\0".as_slice()),
    ] {
        let mut payload = vec![0xd1, 0xd1, 0];
        payload.extend(info(format, title, b"SIDETONE_SETTING"));
        let cap = GeneralCapability::decode_response(T1, &payload).unwrap();
        assert_eq!(cap.feature(), None);
        assert_eq!(cap.info().format().as_raw(), format);
        assert_eq!(cap.info().title_bytes(), title);
        let params = GeneralParameters::new(cap.kind(), GeneralValue::Boolean(true)).unwrap();
        assert!(params.validate_capability(&cap).is_ok());
        assert!(matches!(
            params.encode_set(&cap),
            Err(CommandError::UnsupportedOperation { .. })
        ));
    }
}

#[test]
fn list_values_use_advertised_indices_without_sorting_options() {
    let cap = GeneralCapability::decode_response(T1, &list_capability(3)).unwrap();
    assert_eq!(cap.setting_type(), GeneralSettingType::List);
    assert_eq!(cap.feature(), Some(GeneralFeature::Sidetone));
    assert_eq!(
        cap.options()
            .unwrap()
            .iter()
            .map(|o| o.title_bytes())
            .collect::<Vec<_>>(),
        vec![b"a".as_slice(), b"b".as_slice(), b"c".as_slice()]
    );
    let params = GeneralParameters::new(cap.kind(), GeneralValue::List(2)).unwrap();
    assert_eq!(params.encode_set(&cap).unwrap(), [0xd8, 0xd4, 1, 2]);
    for index in 3..=63 {
        assert!(
            GeneralParameters::new(cap.kind(), GeneralValue::List(index))
                .unwrap()
                .encode_set(&cap)
                .is_err()
        );
    }
    assert!(
        GeneralParameters::new(cap.kind(), GeneralValue::Boolean(true))
            .unwrap()
            .encode_set(&cap)
            .is_err()
    );
    let boolean = GeneralCapability::decode_response(T1, SIDETONE).unwrap();
    assert!(
        GeneralParameters::new(boolean.kind(), GeneralValue::List(0))
            .unwrap()
            .encode_set(&boolean)
            .is_err()
    );
}

#[test]
fn list_count_bounds_allow_all_sixty_four_indices_and_reject_extra_records() {
    for count in [1, 64] {
        let cap = GeneralCapability::decode_response(T1, &list_capability(count)).unwrap();
        assert_eq!(cap.options().unwrap().len(), usize::from(count));
        for index in 0..count {
            assert!(
                GeneralParameters::new(cap.kind(), GeneralValue::List(index))
                    .unwrap()
                    .encode_set(&cap)
                    .is_ok()
            );
        }
    }
    for count in [0, 65, 128, 255] {
        assert!(GeneralCapability::decode_response(T1, &list_capability(count)).is_err());
    }
    let mut duplicate = list_capability(1);
    let count_offset = duplicate.len() - 5;
    duplicate[count_offset] = 2;
    duplicate.extend(info(0, b"a", &[]));
    let cap = GeneralCapability::decode_response(T1, &duplicate).unwrap();
    assert_eq!(cap.options().unwrap()[0], cap.options().unwrap()[1]);
    duplicate[count_offset] = 1;
    assert!(GeneralCapability::decode_response(T1, &duplicate).is_err());
}

#[test]
fn title_and_summary_are_byte_bounded_and_preserve_non_utf8() {
    for length in [0, 128] {
        let mut payload = vec![0xd1, 0xd1, 0];
        payload.extend(info(0, &vec![0xff; length], &vec![b'x'; length]));
        let cap = GeneralCapability::decode_response(T1, &payload).unwrap();
        assert_eq!(cap.info().title_bytes(), vec![0xff; length]);
        assert_eq!(cap.info().summary_bytes(), vec![b'x'; length]);
        if length != 0 {
            assert!(cap.info().title_utf8().is_err());
        }
    }
    for (title, summary) in [(vec![b'a'; 129], vec![]), (vec![], vec![b'b'; 129])] {
        let mut payload = vec![0xd1, 0xd1, 0];
        payload.extend(info(0, &title, &summary));
        assert!(GeneralCapability::decode_response(T1, &payload).is_err());
    }
    let mut payload = vec![0xd1, 0xd1, 0];
    payload.extend(info(0, "侧音".as_bytes(), "耳机设置".as_bytes()));
    let cap = GeneralCapability::decode_response(T1, &payload).unwrap();
    assert_eq!(cap.info().title_utf8().unwrap(), "侧音");
    assert_eq!(cap.info().summary_utf8().unwrap(), "耳机设置");
    assert_eq!(cap.feature(), None);
}

#[test]
fn nested_capability_lengths_reject_every_truncation_and_trailing_byte() {
    for payload in [
        SIDETONE.to_vec(),
        MULTIPOINT.to_vec(),
        TOUCH_PANEL.to_vec(),
        list_capability(3),
    ] {
        for end in 0..payload.len() {
            assert!(GeneralCapability::decode_response(T1, &payload[..end]).is_err());
        }
        let mut extra = payload.clone();
        extra.push(0);
        assert!(GeneralCapability::decode_response(T1, &extra).is_err());
        assert!(GeneralCapability::decode_response(T2, &payload).is_err());
        extra = payload.clone();
        extra[0] = 0xd3;
        assert!(GeneralCapability::decode_response(T1, &extra).is_err());
        extra = payload.clone();
        extra[1] = 0xff;
        assert!(GeneralCapability::decode_response(T1, &extra).is_err());
        extra = payload;
        extra[2] = 2;
        assert!(GeneralCapability::decode_response(T1, &extra).is_err());
    }
}

#[test]
fn parameters_and_availability_are_separate_with_strict_closed_values() {
    let status = GeneralStatus::decode_response(T1, &[0xd3, 0xd1, 0]).unwrap();
    assert!(status.enabled);
    assert_eq!(status.kind(), GeneralKind::Setting1);
    let params = GeneralParameters::decode_response(T1, &[0xd7, 0xd1, 0, 1]).unwrap();
    assert_eq!(params.value, GeneralValue::Boolean(false));
    for raw in 2..=255 {
        assert!(GeneralStatus::decode_response(T1, &[0xd3, 0xd1, raw]).is_err());
        assert!(GeneralParameters::decode_response(T1, &[0xd7, 0xd1, 0, raw]).is_err());
    }
    for raw in 64..=255 {
        assert!(GeneralParameters::decode_response(T1, &[0xd7, 0xd1, 1, raw]).is_err());
        assert!(GeneralParameters::new(GeneralKind::Setting1, GeneralValue::List(raw)).is_err());
    }
    for raw in 0..=63 {
        assert_eq!(
            GeneralParameters::decode_response(T1, &[0xd7, 0xd1, 1, raw])
                .unwrap()
                .value,
            GeneralValue::List(raw)
        );
    }
    // Public edits cannot bypass validation after construction.
    let mut params = params;
    params.value = GeneralValue::List(255);
    assert!(
        params
            .encode_set(&GeneralCapability::decode_response(T1, SIDETONE).unwrap())
            .is_err()
    );
}

#[test]
fn parameter_and_status_notifications_require_their_own_headers_and_exact_lengths() {
    for payload in [
        vec![0xd3, 0xd1, 0],
        vec![0xd7, 0xd1, 0, 0],
        vec![0xd7, 0xd4, 1, 63],
    ] {
        let is_status = payload[0] == 0xd3;
        let decode = |p: &[u8]| {
            if is_status {
                GeneralStatus::decode_response(T1, p).map(|_| ())
            } else {
                GeneralParameters::decode_response(T1, p).map(|_| ())
            }
        };
        for end in 0..payload.len() {
            assert!(decode(&payload[..end]).is_err());
        }
        let mut changed = payload.clone();
        changed.push(0);
        assert!(decode(&changed).is_err());
        changed = payload.clone();
        changed[0] += 2;
        assert!(decode(&changed).is_err());
        assert!(Notification::decode(T1, &changed).is_ok());
        assert!(Notification::decode(T2, &changed).is_err());
    }
}

#[test]
fn queries_include_language_and_keep_same_slot_response_identity() {
    for slot in 0xd1..=0xd4 {
        let kind = GeneralKind::try_from(slot).unwrap();
        assert_eq!(
            kind.function_id(),
            FunctionId {
                table: T1,
                code: slot
            }
        );
        assert_eq!(
            GeneralKind::from_function(FunctionId {
                table: T1,
                code: slot
            }),
            Some(kind)
        );
        assert_eq!(
            GeneralKind::from_function(FunctionId {
                table: T2,
                code: slot
            }),
            None
        );
        let english = Request::GeneralCapability {
            kind,
            language: DisplayLanguage::ENGLISH,
        }
        .encode()
        .unwrap();
        let chinese = Request::GeneralCapability {
            kind,
            language: DisplayLanguage::SIMPLIFIED_CHINESE,
        }
        .encode()
        .unwrap();
        assert_eq!(english.payload, [0xd0, slot, 1]);
        assert_eq!(chinese.payload, [0xd0, slot, 12]);
        assert_eq!(english.expected_response, chinese.expected_response);
        let mut payload = SIDETONE.to_vec();
        payload[1] = slot;
        assert!(matches!(
            english.expected_response.decode(T1, &payload),
            Ok(Response::GeneralCapability(_))
        ));
        assert!(english.expected_response.decode(T2, &payload).is_err());
        for (request, opcode, response, body) in [
            (Request::GeneralStatus(kind), 0xd2, 0xd3, vec![0]),
            (Request::GeneralParameters(kind), 0xd6, 0xd7, vec![0, 1]),
        ] {
            let query = request.encode().unwrap();
            assert_eq!(query.payload, [opcode, slot]);
            let mut reply = vec![response, slot];
            reply.extend(body);
            assert!(query.expected_response.decode(T1, &reply).is_ok());
            reply[0] += 2;
            assert!(!query.expected_response.matches(T1, &reply));
            assert!(Notification::decode(T1, &reply).is_ok());
        }
    }
    assert!(GeneralKind::try_from(0xd0).is_err());
    assert_eq!(
        GeneralKind::from_function(FunctionId {
            table: T1,
            code: 0xd5
        }),
        None
    );
}
