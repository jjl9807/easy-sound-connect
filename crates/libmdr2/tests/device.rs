use libmdr2::{
    command::{CommandError, Request, Response},
    device::{DeviceInfo, DeviceInfoKind, MAX_DEVICE_TEXT_LEN, ModelColor, ModelSeries},
    types::CommandTable,
};

const T1: CommandTable = CommandTable::TABLE_1;

#[test]
fn queries_select_exact_device_information_inquiries() {
    for (kind, inquiry) in [
        (DeviceInfoKind::ModelName, 1),
        (DeviceInfoKind::FirmwareVersion, 2),
        (DeviceInfoKind::SeriesAndColor, 3),
    ] {
        let encoded = Request::DeviceInfo(kind).encode().unwrap();
        assert_eq!(kind.inquiry(), inquiry);
        assert_eq!(encoded.table, T1);
        assert_eq!(encoded.payload, [0x04, inquiry]);
        assert_eq!(encoded.expected_response.table(), T1);
        assert_eq!(encoded.expected_response.opcode(), 0x05);
        assert_eq!(encoded.expected_response.inquiry(), inquiry);
    }
}

#[test]
fn upstream_reported_xm6_captures_decode_without_model_routing() {
    // Protocol data from sonymdr_open 44adbf9, sonyctl/sim.py HANDSHAKE.
    // Dataset metadata: WH-1000XM6 FW 3.1.5; see fixtures/README.md.
    let model = DeviceInfo::decode(T1, b"\x05\x01\x0aWH-1000XM6").unwrap();
    assert_eq!(model.kind(), DeviceInfoKind::ModelName);
    let DeviceInfo::ModelName(name) = model else {
        panic!("expected model name");
    };
    assert_eq!(name.as_bytes(), b"WH-1000XM6");
    assert_eq!(name.as_utf8().unwrap(), "WH-1000XM6");

    let firmware = DeviceInfo::decode(T1, b"\x05\x02\x053.1.5").unwrap();
    assert_eq!(firmware.kind(), DeviceInfoKind::FirmwareVersion);
    let DeviceInfo::FirmwareVersion(version) = firmware else {
        panic!("expected firmware version");
    };
    assert_eq!(version.into_bytes(), b"3.1.5");

    let color = DeviceInfo::decode(T1, &[0x05, 0x03, 0x30, 0x03]).unwrap();
    assert_eq!(color.kind(), DeviceInfoKind::SeriesAndColor);
    let DeviceInfo::SeriesAndColor(appearance) = color else {
        panic!("expected appearance");
    };
    assert_eq!(appearance.series, ModelSeries::PREMIUM);
    assert_eq!(appearance.color.as_raw(), 3);
}

#[test]
fn another_upstream_firmware_capture_does_not_require_an_xm6_name() {
    // Protocol data from aecca/sound-connect 749e544, SONY command test data.
    // Upstream identifies the data set as WH-1000XM5 firmware 2.4.1.
    let DeviceInfo::FirmwareVersion(version) =
        DeviceInfo::decode(T1, b"\x05\x02\x052.4.1").unwrap()
    else {
        panic!("expected firmware version");
    };
    assert_eq!(version.as_utf8().unwrap(), "2.4.1");
}

#[test]
fn unknown_series_and_color_are_preserved_instead_of_defaulted() {
    for (series, color) in [(0xfe, 0xfd), (0xff, 0xff), (0, 0)] {
        let DeviceInfo::SeriesAndColor(appearance) =
            DeviceInfo::decode(T1, &[0x05, 3, series, color]).unwrap()
        else {
            panic!("expected appearance");
        };
        assert_eq!(appearance.series, ModelSeries::from_raw(series));
        assert_eq!(appearance.series.as_raw(), series);
        assert_eq!(appearance.color, ModelColor::from_raw(color));
        assert_eq!(appearance.color.as_raw(), color);
    }
}

#[test]
fn text_preserves_raw_bytes_and_explicit_utf8_errors() {
    // Two Chinese characters use six bytes, not a two-byte character count.
    let payload = [vec![0x05, 1, 6], "耳机".as_bytes().to_vec()].concat();
    let DeviceInfo::ModelName(name) = DeviceInfo::decode(T1, &payload).unwrap() else {
        panic!("expected model name");
    };
    assert_eq!(name.as_utf8().unwrap(), "耳机");

    let DeviceInfo::ModelName(name) =
        DeviceInfo::decode(T1, &[0x05, 1, 4, 0, b' ', 0xff, b' ']).unwrap()
    else {
        panic!("expected model name");
    };
    assert_eq!(name.as_bytes(), [0, b' ', 0xff, b' ']);
    assert!(name.as_utf8().is_err());
    assert_eq!(name.into_bytes(), [0, b' ', 0xff, b' ']);
}

#[test]
fn text_length_bounds_apply_to_both_variants() {
    assert_eq!(MAX_DEVICE_TEXT_LEN, 128);
    for inquiry in [1, 2] {
        for length in [1, 128] {
            let mut payload = vec![0x05, inquiry, length];
            payload.extend(std::iter::repeat_n(b'x', length as usize));
            assert!(DeviceInfo::decode(T1, &payload).is_ok());
        }
        for length in [0, 129, 255] {
            let mut payload = vec![0x05, inquiry, length];
            payload.extend(std::iter::repeat_n(b'x', length as usize));
            assert_eq!(
                DeviceInfo::decode(T1, &payload),
                Err(CommandError::InvalidValue {
                    field: "device text length",
                    value: length,
                })
            );
        }
    }
}

#[test]
fn exact_lengths_reject_truncation_and_trailing_bytes() {
    for payload in [
        vec![],
        vec![0x05],
        vec![0x05, 1],
        vec![0x05, 1, 2, b'a'],
        vec![0x05, 1, 1, b'a', b'b'],
        vec![0x05, 2],
        vec![0x05, 2, 2, b'a'],
        vec![0x05, 2, 1, b'a', b'b'],
        vec![0x05, 3],
        vec![0x05, 3, 0x30],
        vec![0x05, 3, 0x30, 3, 0],
    ] {
        assert!(DeviceInfo::decode(T1, &payload).is_err(), "{payload:02x?}");
    }
}

#[test]
fn device_response_routing_checks_table_inquiry_and_full_body() {
    let expected = Request::DeviceInfo(DeviceInfoKind::FirmwareVersion)
        .encode()
        .unwrap()
        .expected_response;
    assert!(!expected.matches(CommandTable::TABLE_2, b"\x05\x02\x053.1.5"));
    assert!(!expected.matches(T1, b"\x05\x01\x04Demo"));
    assert!(expected.matches(T1, &[0x05, 2]));
    assert!(expected.decode(T1, &[0x05, 2]).is_err());
    assert!(matches!(
        expected.decode(T1, b"\x05\x02\x053.1.5").unwrap(),
        Response::DeviceInfo(DeviceInfo::FirmwareVersion(_))
    ));
}

#[test]
fn unsupported_headers_and_tables_fail_without_guessing() {
    assert!(matches!(
        DeviceInfo::decode(CommandTable::TABLE_2, &[0x05, 3, 0x30, 3]),
        Err(CommandError::WrongTable { .. })
    ));
    assert!(matches!(
        DeviceInfo::decode(T1, &[0x15, 3, 0x30, 3]),
        Err(CommandError::UnexpectedOpcode { .. })
    ));
    // Instruction-guide metadata is a known but unimplemented inquiry here.
    for inquiry in [0, 4, 0xff] {
        assert_eq!(
            DeviceInfo::decode(T1, &[0x05, inquiry, 0]),
            Err(CommandError::InvalidValue {
                field: "device information inquiry",
                value: inquiry,
            })
        );
    }
}
