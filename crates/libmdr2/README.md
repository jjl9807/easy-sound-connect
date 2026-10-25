# libmdr2

Native Rust implementation of the SONY MDR v2 headphone control protocol. The library provides typed command payloads, bounded stream decoding and a deterministic state machine for reliable frame delivery. It requires `std`, has no third-party dependencies and leaves Bluetooth I/O, clocks and scheduling to the caller.

## Protocol support

| Module | API coverage |
| --- | --- |
| `frame` | One-bit sequence numbers, raw data types, 8-bit additive checksum, escaping, incremental decoding, payload limits, length validation and resynchronization |
| `command` | Connection and feature queries, typed response descriptors and separate notification dispatch |
| `device` | Model name, firmware version, series/color, raw text with explicit UTF-8 decoding and unknown appearance codes |
| `battery` | Six single/L/R/cradle layouts, optional thresholds, four charging states, responses and notifications |
| `noise` | Ten NC/ambient/button-cycle layouts, capability ranges, availability, parameters and validated setting writes |
| `equalizer` | Five preset/custom/ULT layouts, capability labels, native curves, band metadata, availability and validated setting writes |
| `audio` | Eleven DSEE, connection-priority and local listening-mode layouts, capability choices, availability, parameters and validated writes |
| `system` | Two Speak-to-Chat layouts with sensitivity and automatic-ending controls, plus automatic playback control based on wearing state, vibration, automatic volume and head-gesture switches |
| `power` | Automatic power-off, standby, power saving and caring charge across T1/T2, plus a fixed user power-off command |
| `common` | Read-only L/R connections, codec, DSEE effect state and operating-mode reports |
| `general` | Boolean/list settings with device-provided labels; capability-identified sidetone, multipoint and touch-panel writes |
| `playback` | Playback actions, metadata, music/call volume and distinct mute-reporting profiles |
| `assignment` | Two physical-key layouts, nested action candidates and edits that preserve unrelated assignments |
| `peripheral` | Paired-device lists, local connect/disconnect, source switching, handover settings and correlated operation results |
| `voice_guidance` | Local guidance switches, signed volume, battery announcements and sound switches; language metadata is read-only |
| `sound_effect` | Standard/custom sound effects, curve metadata, validated curve writes and ULT-button effect assignments |
| `system2` | Wearing state, automatic volume with limitations and microphone-operation controls |
| `alert` | Fixed/flexible device alerts, explicit local confirmations and LE Audio confirmation preferences |
| `connection` | Device reconnect/GATT-disconnect requests and host-to-device LE Audio state reports |
| `types` | Table-aware function identities, raw protocol versions, display-language codes and Bluetooth addresses |
| `session` | Reliable T1/T2 frames, ACKs, one outstanding data frame, ordered writes, duplicate handling, ACK deadlines and bounded retransmission |

The codec preserves unknown frame types; the session accepts reliable T1/T2 data and ACKs, and returns typed errors for SHOT and other frame types. Applications select supported layouts from the device's advertised capabilities; model names do not determine protocol support. MDR v1 and legacy fallback are unsupported.

## Detailed protocol documentation

The [protocol guide](https://github.com/jjl9807/easy-sound-connect/blob/main/crates/libmdr2/docs/README.md) is also shipped in the crate's `docs/` directory. It covers library boundaries, wire definitions, framing, ACK/retry rules, capability variants, device information, battery and local-control layouts, notifications, operation results, validation rules and fixture provenance. Protocol chapters are embedded into the corresponding module documentation; their Rust examples are executed as doctests.

For a local checkout, begin with `crates/libmdr2/docs/README.md`. Generate API docs with `cargo doc -p libmdr2 --no-deps --offline`.

## Framing

```rust
use libmdr2::frame::{DataType, Decoder, Frame, Sequence};

let request = Frame {
    data_type: DataType::DATA_MDR,
    sequence: Sequence::Zero,
    payload: vec![0x00, 0x00],
};
let bytes = request.encode(4096)?;
let mut decoder = Decoder::new(4096)?;
let mut decoded = Vec::new();
for fragment in bytes.chunks(2) {
    decoded.extend(decoder.feed(fragment).collect::<Result<Vec<_>, _>>()?);
}
assert_eq!(decoded, vec![request]);
decoder.finish()?;
# Ok::<(), libmdr2::frame::FrameError>(())
```

The limit is selected by the caller and is not a universal device limit. The decoder checks declared length before buffering a payload; it never allocates based solely on an untrusted declared length. Only escape pairs `3d 2c`, `3d 2d` and `3d 2e` are accepted. Invalid sequences, nonempty ACK payloads, checksum errors, length mismatches and oversized frames produce typed errors.

`feed` is lazy: exhaust the iterator to consume its entire fragment. After an error, continue iterating if the connection policy permits recovery. A new start delimiter can replace an incomplete frame; the decoder reports that error and starts the replacement frame. Call `finish` only at end of stream, not between Bluetooth reads or notifications.

## Driving a session

1. Encode a typed `command::Request`, retain its `expected_response`, and register the response matcher **before** sending. A descriptor identifies a family and variant, not a unique transaction.
2. Pass its table and payload to `Session::send` with elapsed monotonic time.
3. For each `Transmit` event, encode the entire frame, then write it in order. The transport handles RFCOMM/GATT write limits and fragmentation.
4. Report complete-write success or failure with its `WriteToken`. Tokens belong to one session; route each callback to the session and physical connection that created it.
5. Feed incoming bytes through `Decoder`, then pass decoded frames to `receive`. Drain **all** returned events, including ACK writes while awaiting a response.
6. Schedule `advance` at `next_deadline`. The library never sleeps or polls. Enforce a separate transport-write timeout and report failure if it expires.
7. On `Received`, route by table and response descriptor, then fully validate the command payload. Other messages may be asynchronous notifications.
8. On `Closed`, close the transport. Reconnect with a new session and decoder; do not replay commands or route old callbacks into the replacement session.

The default ACK policy is 750 ms and ten **additional** retransmissions. Callers can configure both values. ACK deadlines start after each successful complete write of the outbound data frame. An ACK arriving before the corresponding write callback is held until that write succeeds.

`LinkAcknowledged` reports acknowledgment of a transmitted frame. Command responses and confirmation of device state are separate. The caller manages response deadlines, command serialization, compatibility decisions and whether an operation can be retried. The session permits one outstanding reliable data frame and queues at most 32 ACK writes. Queue overflow, write failure, cancellation or exhausted retries close the session.

Duplicate reliable frames are re-ACKed without duplicate delivery, except that a fully valid table-1 protocol-info response is delivered even with a repeated sequence. Unknown commands in reliable frames are delivered as raw payloads.

An ACK carries only one sequence bit, so arbitrarily delayed traffic can reuse the sequence expected by a later exchange. Command replies also lack a general transaction ID. Serialize commands with indistinguishable replies; after an ambiguous timeout, synchronize device state or reconnect before issuing another such command.

Run the offline integration examples:

```sh
cargo run -p libmdr2 --example handshake --offline
cargo run -p libmdr2 --example settings --offline
```

The settings example selects a noise-control layout from an advertised function, checks availability, encodes a change against its capability and validates a subsequent readback. Both examples use synthetic exchanges and perform no Bluetooth I/O.

`NoiseParameters::encode_set` and `EqualizerSetting::encode_set` return table-1 payloads after checking capability constraints. Send these with `Session::send`. SET commands have no dedicated response descriptor in these families; parameter notifications and serialized reads reconcile the reported state. Check current availability separately, and retain unrelated fields when editing a setting.

`audio`, `system`, `general`, `playback`, `assignment` and `sound_effect` use table 1; `system2`, `peripheral` and `voice_guidance` use table 2. Power settings use the table returned by their `PowerKind`. Select the family layout or profile from advertised function identities. Kinds without a dynamic capability query provide an explicit fixed descriptor where settings require one.

Peripheral operations return an `ExpectedPeripheralResult`: match the inquiry, action and target address, then inspect the outcome. A matching progress or busy report is not success. Alerts require an explicit caller decision; there is no automatic acceptance policy. Connection notices request transport actions from the caller without performing Bluetooth I/O.

General settings authorize writes only for recognized device-local identities. Assignment edits preserve unrelated unknown or service bindings but cannot activate new service functions. Voice-guidance language metadata is readable; language switching and package transfer are not supported.

## Capability identities and compatibility

Function identities include `(table, code)`; unknown codes, priorities, repeated entries and wire order are preserved. Protocol-version values are uninterpreted: their numeric prefix does not identify MDR v1 versus v2. The caller explicitly selects the standard or app-type capability-info request.

Capability identifiers are opaque bytes used during capability discovery. Track physical-device and model identities separately. Both capability-info layouts accept identifiers of `0..=128` bytes without truncation. Display-language identifiers select device-provided labels.

## Validation and development

```sh
cargo test -p libmdr2 --offline
cargo clippy -p libmdr2 --all-targets --offline -- -D warnings
```

The test suite covers wire formats, malformed inputs, capability constraints, response routing and session timing. Fixtures include upstream-reported WH-1000XM6, WH-1000XM5 and WF-1000XM5 data alongside synthetic payloads and event sequences. See the [testing guide](https://github.com/jjl9807/easy-sound-connect/blob/main/crates/libmdr2/docs/testing.md) for coverage, fixture provenance and the limits of offline validation.

The public API may change in later minor releases before version `1.0`.
