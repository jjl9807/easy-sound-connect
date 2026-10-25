//! Independently authored tests using only published protocol-data vectors.
//! Provenance and transformations are documented in fixtures/README.md.

use std::{collections::VecDeque, time::Duration};

use libmdr2::{
    battery::{BatteryKind, BatteryReadings},
    command::{Notification, Request, Response, SupportedFunctions},
    device::{DeviceInfo, DeviceInfoKind},
    frame::{DataType, Decoder, Sequence},
    session::{Session, SessionEvent},
    types::CommandTable,
};

fn hex(text: &str) -> Vec<u8> {
    let text = text.trim();
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let pair = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(pair, 16).unwrap()
        })
        .collect()
}

fn request(table: CommandTable, bytes: &[u8]) -> Request {
    match bytes {
        [0, 0] => Request::ProtocolInfo,
        [4, 1] => Request::DeviceInfo(DeviceInfoKind::ModelName),
        [4, 2] => Request::DeviceInfo(DeviceInfoKind::FirmwareVersion),
        [4, 3] => Request::DeviceInfo(DeviceInfoKind::SeriesAndColor),
        [6, 0] => Request::SupportedFunctions(table),
        [0x22, 0] => Request::BatteryStatus(BatteryKind::Single),
        _ => panic!("fixture request not covered: {bytes:?}"),
    }
}

#[test]
fn published_payloads_match_typed_queries_and_independent_expectations() {
    let mut count = 0;
    for line in include_str!("fixtures/reference-payloads.tsv").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        let [id, source, table, query, payload, expected] = fields.as_slice() else {
            panic!("invalid fixture row: {line}");
        };
        assert!(matches!(
            *source,
            "xm6-sim" | "xm6-initial-body" | "xm5-command"
        ));
        let table = CommandTable::from_raw(table.parse().unwrap());
        let query = hex(query);
        let payload = hex(payload);
        let encoded = request(table, &query).encode().unwrap();
        assert_eq!(encoded.table, table, "{id}");
        assert_eq!(encoded.payload, query, "{id}");
        let response = encoded.expected_response.decode(table, &payload).unwrap();
        let observed = match response {
            Response::ProtocolInfo(info) => {
                assert!(info.table_1_supported && info.table_2_supported, "{id}");
                format!("version:{:08x}", info.version.as_raw())
            }
            Response::DeviceInfo(
                DeviceInfo::ModelName(text) | DeviceInfo::FirmwareVersion(text),
            ) => {
                format!("text:{}", text.as_utf8().unwrap())
            }
            Response::DeviceInfo(DeviceInfo::SeriesAndColor(appearance)) => {
                format!(
                    "appearance:{:02x},{:02x}",
                    appearance.series.as_raw(),
                    appearance.color.as_raw()
                )
            }
            Response::BatteryStatus(status) => {
                let BatteryReadings::Single(reading) = status.batteries else {
                    panic!("unexpected endpoint");
                };
                assert!(reading.threshold.is_none());
                format!(
                    "battery:{},{}",
                    reading.level.as_u8(),
                    reading.charging.as_u8()
                )
            }
            Response::SupportedFunctions(list) => {
                assert!(
                    list.functions
                        .iter()
                        .all(|function| function.id.table == table)
                );
                if *id == "wh-xm6-functions-t1" {
                    assert!(
                        list.functions
                            .iter()
                            .any(|entry| entry.id.code == 0x6d && entry.priority == 0x0b)
                    );
                }
                format!("functions:{}", list.functions.len())
            }
            other => panic!("unexpected fixture response: {other:?}"),
        };
        assert_eq!(&observed, expected, "{id}");
        count += 1;
    }
    assert_eq!(count, 10);
}

#[test]
fn published_wf_frame_decodes_at_every_fragment_boundary_without_model_routing() {
    let bytes = hex(include_str!("fixtures/wf-xm5-functions-t2.hex"));
    let payload = hex("07000b423d53ff54fff1ff60ff61fff2ff63fff830320431ff");
    for split in 0..=bytes.len() {
        let mut decoder = Decoder::new(512).unwrap();
        let mut frames: Vec<_> = decoder
            .feed(&bytes[..split])
            .collect::<Result<_, _>>()
            .unwrap();
        frames.extend(
            decoder
                .feed(&bytes[split..])
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
        assert_eq!(frames.len(), 1, "split {split}");
        let frame = &frames[0];
        assert_eq!(frame.data_type, DataType::DATA_MDR_NO2);
        assert_eq!(frame.sequence, Sequence::One);
        assert_eq!(frame.payload, payload);
        assert_eq!(frame.encode(512).unwrap(), bytes);
        let functions =
            SupportedFunctions::decode(frame.data_type.command_table().unwrap(), &frame.payload)
                .unwrap();
        assert_eq!(functions.functions.len(), 11);
        assert_eq!(functions.functions[0].id.table, CommandTable::TABLE_2);
        assert_eq!(functions.functions[0].id.code, 0x42);
        assert_eq!(functions.functions[0].priority, 0x3d);
        assert_eq!(functions.functions[10].id.code, 0x31);
        assert_eq!(functions.functions[10].priority, 0xff);
        decoder.finish().unwrap();
    }
}

#[test]
fn a_battery_notification_before_the_reply_does_not_complete_the_query() {
    let query = Request::BatteryStatus(BatteryKind::Single)
        .encode()
        .unwrap();
    let expected = query.expected_response;
    let mut session = Session::default();
    let effects = session
        .send(query.table, query.payload, Duration::ZERO)
        .unwrap();
    let [SessionEvent::Transmit { token, frame }] = effects.as_slice() else {
        panic!("expected query");
    };
    // Published WH-XM5 query-frame bytes; the replay schedule below is synthetic.
    assert_eq!(frame.encode(512).unwrap(), hex("3e0c00000000022200303c"));
    session.complete_write(*token, Duration::ZERO).unwrap();

    // Synthetic NTFY (50%, charging), ACK, then published XM6 battery body
    // reframed with seq=1. These event timings and frame wrappers are not captures.
    let stream = hex("3e0c000000000425003201683c3e010100000000023c3e0c010000000423005100853c");
    let mut decoder = Decoder::new(512).unwrap();
    let mut observations = Vec::new();
    let mut confirmations = 0;
    for fragment in stream.chunks(2) {
        for decoded in decoder.feed(fragment) {
            let now = Duration::from_millis(1);
            let mut events = VecDeque::from(session.receive(decoded.unwrap(), now).unwrap());
            while let Some(event) = events.pop_front() {
                match event {
                    SessionEvent::Transmit { token, frame } => {
                        assert_eq!(frame.data_type, DataType::ACK);
                        events.extend(session.complete_write(token, now).unwrap());
                    }
                    SessionEvent::Received(frame) => {
                        let table = frame.data_type.command_table().unwrap();
                        if expected.matches(table, &frame.payload) {
                            let Response::BatteryStatus(status) =
                                expected.decode(table, &frame.payload).unwrap()
                            else {
                                panic!("wrong response");
                            };
                            let BatteryReadings::Single(reading) = status.batteries else {
                                panic!("wrong endpoint");
                            };
                            assert_eq!(reading.level.as_u8(), 81);
                            confirmations += 1;
                            observations.push("response");
                        } else {
                            assert_eq!(confirmations, 0);
                            assert!(expected.decode(table, &frame.payload).is_err());
                            let Notification::BatteryStatus(status) =
                                Notification::decode(table, &frame.payload).unwrap()
                            else {
                                panic!("wrong notification");
                            };
                            let BatteryReadings::Single(reading) = status.batteries else {
                                panic!("wrong endpoint");
                            };
                            assert_eq!(reading.level.as_u8(), 50);
                            observations.push("notification");
                        }
                    }
                    SessionEvent::LinkAcknowledged { .. } => {
                        assert_eq!(confirmations, 0);
                        observations.push("ack");
                    }
                    other => panic!("unexpected session event: {other:?}"),
                }
            }
        }
    }
    assert_eq!(observations, ["notification", "ack", "response"]);
    assert_eq!(confirmations, 1);
    decoder.finish().unwrap();
}
