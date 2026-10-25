use libmdr2::{
    battery::BatteryKind,
    command::{
        CapabilityInfo, CapabilityInfoVariant, CapabilityQuery, CommandError, Notification,
        ProtocolInfo, Request, Response, SupportedFunctions,
    },
    types::CommandTable,
};

#[test]
fn all_battery_queries_route_only_the_requested_response_variant() {
    for (kind, inquiry, body) in [
        (BatteryKind::Single, 0x00, vec![50, 0]),
        (BatteryKind::LeftRight, 0x01, vec![50, 0, 60, 1]),
        (BatteryKind::Cradle, 0x02, vec![70, 2]),
        (BatteryKind::SingleWithThreshold, 0x08, vec![50, 0, 10]),
        (
            BatteryKind::LeftRightWithThreshold,
            0x09,
            vec![50, 0, 60, 1, 10, 20],
        ),
        (BatteryKind::CradleWithThreshold, 0x0a, vec![70, 3, 15]),
    ] {
        let request = Request::BatteryStatus(kind).encode().unwrap();
        assert_eq!(request.table, T1);
        assert_eq!(request.payload, [0x22, inquiry]);
        let mut reply = vec![0x23, inquiry];
        reply.extend_from_slice(&body);
        let Response::BatteryStatus(status) = request.expected_response.decode(T1, &reply).unwrap()
        else {
            panic!("incorrect response family");
        };
        assert_eq!(status.kind, kind);
        assert!(!request.expected_response.matches(T2, &reply));
        assert!(request.expected_response.decode(T2, &reply).is_err());
        assert!(Notification::decode(T1, &reply).is_err());
        reply[0] = 0x25;
        assert!(!request.expected_response.matches(T1, &reply));
        assert!(request.expected_response.decode(T1, &reply).is_err());
        let Notification::BatteryStatus(notified) = Notification::decode(T1, &reply).unwrap()
        else {
            panic!("incorrect notification family");
        };
        assert_eq!(notified, status);
        reply[0] = 0x23;
        reply[1] ^= 1;
        assert!(!request.expected_response.matches(T1, &reply));
        assert!(request.expected_response.decode(T1, &reply).is_err());
    }
}

const T1: CommandTable = CommandTable::TABLE_1;
const T2: CommandTable = CommandTable::TABLE_2;

#[test]
fn queries_have_exact_wire_payloads_and_response_descriptors() {
    let cases = [
        (Request::ProtocolInfo, T1, vec![0x00, 0x00], 0x01, 0x00),
        (
            Request::CapabilityInfo(CapabilityQuery::Standard),
            T1,
            vec![0x02, 0x00],
            0x03,
            0x00,
        ),
        (
            Request::CapabilityInfo(CapabilityQuery::WithAppType(0xa5)),
            T1,
            vec![0x02, 0x01, 0xa5],
            0x03,
            0x01,
        ),
        (
            Request::SupportedFunctions(T1),
            T1,
            vec![0x06, 0x00],
            0x07,
            0x00,
        ),
        (
            Request::SupportedFunctions(T2),
            T2,
            vec![0x06, 0x00],
            0x07,
            0x00,
        ),
    ];
    for (request, table, payload, opcode, inquiry) in cases {
        let encoded = request.encode().unwrap();
        assert_eq!(encoded.table, table);
        assert_eq!(encoded.payload, payload);
        assert_eq!(encoded.expected_response.table(), table);
        assert_eq!(encoded.expected_response.opcode(), opcode);
        assert_eq!(encoded.expected_response.inquiry(), inquiry);
    }
}

#[test]
fn protocol_information_preserves_version_and_decodes_inverted_flags() {
    // Independent literal fixture: version 0x03003032, both function lists enabled.
    let payload = [0x01, 0x00, 0x03, 0x00, 0x30, 0x32, 0x00, 0x00];
    let info = ProtocolInfo::decode(T1, &payload).unwrap();
    assert_eq!(info.version.as_raw(), 0x03003032);
    assert!(info.table_1_supported);
    assert!(info.table_2_supported);

    // An unknown version is data, not a whitelist failure or a v1 fallback.
    let info = ProtocolInfo::decode(T1, &[1, 0, 0xde, 0xad, 0xbe, 0xef, 1, 0]).unwrap();
    assert_eq!(info.version.as_raw(), 0xdeadbeef);
    assert!(!info.table_1_supported);
    assert!(info.table_2_supported);
    assert!(
        !ProtocolInfo::decode(T1, &[1, 0, 1, 2, 3, 4, 0, 1])
            .unwrap()
            .table_2_supported
    );
}

#[test]
fn protocol_information_rejects_bad_table_header_flags_and_lengths() {
    let payload = [0x01, 0x00, 0x03, 0x00, 0x30, 0x32, 0x00, 0x00];
    for length in 0..payload.len() {
        assert!(ProtocolInfo::decode(T1, &payload[..length]).is_err());
    }
    let mut trailing = payload.to_vec();
    trailing.push(0);
    assert!(matches!(
        ProtocolInfo::decode(T1, &trailing),
        Err(CommandError::InvalidLength {
            expected: 8,
            actual: 9
        })
    ));
    assert!(matches!(
        ProtocolInfo::decode(T2, &payload),
        Err(CommandError::WrongTable {
            expected: T1,
            actual: T2
        })
    ));
    for offset in [0, 1, 6, 7] {
        let mut bad = payload;
        bad[offset] = 0xff;
        assert!(ProtocolInfo::decode(T1, &bad).is_err());
    }
}

#[test]
fn capability_variants_preserve_counter_and_opaque_identifier_bytes() {
    let standard = [0x03, 0x00, 0xff, 0x03, 0x00, 0x80, 0xff];
    let info = CapabilityInfo::decode(T1, &standard).unwrap();
    assert_eq!(info.variant, CapabilityInfoVariant::Standard);
    assert_eq!(info.counter, 255);
    assert_eq!(info.unique_id, [0, 0x80, 0xff]);

    let app_type = [0x03, 0x01, 0x09, 0x03, b'a', b'b', b'c'];
    let info = CapabilityInfo::decode(T1, &app_type).unwrap();
    assert_eq!(info.variant, CapabilityInfoVariant::WithAppType);
    assert_eq!(info.counter, 9);
    assert_eq!(info.unique_id, b"abc");
}

#[test]
fn capability_identifier_bound_is_explicit_and_never_truncates() {
    for inquiry in [0, 1] {
        let empty = CapabilityInfo::decode(T1, &[3, inquiry, 7, 0]).unwrap();
        assert!(empty.unique_id.is_empty());

        let mut maximum = vec![3, inquiry, 7, 128];
        maximum.extend(0..128);
        let info = CapabilityInfo::decode(T1, &maximum).unwrap();
        assert_eq!(info.unique_id, (0..128).collect::<Vec<u8>>());

        let mut oversized = vec![3, inquiry, 7, 129];
        oversized.extend(0..129);
        assert!(matches!(
            CapabilityInfo::decode(T1, &oversized),
            Err(CommandError::IdentifierTooLong {
                length: 129,
                maximum: 128
            })
        ));
    }
}

#[test]
fn capability_payloads_require_complete_exact_bodies() {
    let good = [3, 0, 9, 3, b'a', b'b', b'c'];
    for length in 0..good.len() {
        assert!(CapabilityInfo::decode(T1, &good[..length]).is_err());
    }
    for bad in [
        vec![3, 0, 9, 0, 0], // Trailing byte after an empty identifier.
        vec![3, 0, 9, 1],    // Missing identifier byte.
        vec![3, 2, 9, 0],    // Unknown response layout.
        vec![4, 0, 9, 0],    // Wrong opcode.
    ] {
        assert!(CapabilityInfo::decode(T1, &bad).is_err());
    }
    assert!(CapabilityInfo::decode(T2, &good).is_err());
}

#[test]
fn function_lists_preserve_table_unknown_codes_priority_order_and_duplicates() {
    let payload = [7, 0, 3, 0xfe, 255, 0x20, 0, 0xfe, 1];
    let first = SupportedFunctions::decode(T1, &payload).unwrap();
    let second = SupportedFunctions::decode(T2, &payload).unwrap();
    assert_eq!(first.table, T1);
    assert_eq!(second.table, T2);
    for (list, table) in [(&first, T1), (&second, T2)] {
        let entries: Vec<_> = list
            .functions
            .iter()
            .map(|entry| (entry.id.table, entry.id.code, entry.priority))
            .collect();
        assert_eq!(
            entries,
            [(table, 0xfe, 255), (table, 0x20, 0), (table, 0xfe, 1)]
        );
    }
    assert_ne!(first.functions[1].id, second.functions[1].id);
    assert!(
        SupportedFunctions::decode(T1, &[7, 0, 0])
            .unwrap()
            .functions
            .is_empty()
    );
}

#[test]
fn function_count_is_unsigned_and_exact() {
    let mut maximum = vec![7, 0, 255];
    for code in 0..255u8 {
        maximum.extend([code, 255 - code]);
    }
    let functions = SupportedFunctions::decode(T2, &maximum).unwrap();
    assert_eq!(functions.functions.len(), 255);
    assert_eq!(functions.functions[254].id.code, 254);
    assert_eq!(functions.functions[254].priority, 1);
    assert!(SupportedFunctions::decode(T2, &maximum[..maximum.len() - 1]).is_err());
    maximum.push(0);
    assert!(SupportedFunctions::decode(T2, &maximum).is_err());

    for bad in [
        vec![],
        vec![7],
        vec![7, 0],
        vec![7, 0, 1, 0x20],
        vec![7, 0, 0, 0x20, 0],
        vec![8, 0, 0],
        vec![7, 1, 0],
    ] {
        assert!(SupportedFunctions::decode(T1, &bad).is_err());
    }
}

#[test]
fn unsupported_tables_cannot_encode_or_decode_function_queries() {
    for raw in [0, 3, 255] {
        let table = CommandTable::from_raw(raw);
        assert_eq!(
            Request::SupportedFunctions(table).encode().unwrap_err(),
            CommandError::UnsupportedTable(raw)
        );
        assert_eq!(
            SupportedFunctions::decode(table, &[7, 0, 0]).unwrap_err(),
            CommandError::UnsupportedTable(raw)
        );
    }
}

#[test]
fn response_descriptor_routes_by_table_and_variant_then_validates_body() {
    let expected = Request::SupportedFunctions(T2)
        .encode()
        .unwrap()
        .expected_response;
    let payload = [7, 0, 1, 0x20, 4];
    assert!(!expected.matches(T1, &payload));
    assert!(expected.decode(T1, &payload).is_err());
    assert!(!expected.matches(T2, &[9, 0, 0]));
    assert!(!expected.matches(T2, &[7, 1, 0]));
    assert!(!expected.matches(T2, &[7]));
    assert!(expected.matches(T2, &[7, 0]));
    assert!(expected.decode(T2, &[7, 0]).is_err());
    let Response::SupportedFunctions(list) = expected.decode(T2, &payload).unwrap() else {
        panic!("wrong typed response");
    };
    assert_eq!(list.functions[0].id.table, T2);

    let expected = Request::CapabilityInfo(CapabilityQuery::WithAppType(0))
        .encode()
        .unwrap()
        .expected_response;
    assert!(!expected.matches(T1, &[3, 0, 9, 0]));
    assert!(expected.decode(T1, &[3, 0, 9, 0]).is_err());
    assert!(matches!(
        expected.decode(T1, &[3, 1, 9, 0]),
        Ok(Response::CapabilityInfo(_))
    ));
    assert!(matches!(
        Request::ProtocolInfo
            .encode()
            .unwrap()
            .expected_response
            .decode(T1, &[1, 0, 3, 0, 0x30, 0x32, 0, 0]),
        Ok(Response::ProtocolInfo(_))
    ));
}
