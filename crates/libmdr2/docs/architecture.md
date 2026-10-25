# Library architecture and API boundaries

## Layers and ownership

```text
request types -> payload + expected response
                            |
                    Session::send
                            |
              Transmit(token, Frame)
                            |
                Frame::encode(limit)
                            |
           caller-owned ordered transport writes
                            |
                complete_write / fail_write

transport fragments -> Decoder -> Session::receive -> Received(Frame)
                                                     |
                           response matcher / notification decoder
```

`frame` handles byte framing, `session` handles reliable delivery and write order, and `command` handles typed queries and response routing. Feature modules define their payload layouts and write constraints. Depending on the operation, an encoder checks a capability descriptor, an advertised profile or a decoded state snapshot. `types` holds identities shared across families.

The library requires `std` and has no third-party dependencies. Bluetooth I/O, scheduling, clocks, storage and user interfaces belong to the application. The crate forbids unsafe Rust.

A caller supplies elapsed monotonic `Duration` values, feeds transport bytes and executes emitted writes. The protocol state machine can be driven by a blocking loop, an async runtime or a deterministic test harness.

## Responsibilities that stay outside this library

| Caller responsibility | Why it is separate |
| --- | --- |
| Device discovery, pairing, RFCOMM/GATT service selection | Depends on the device and operating-system Bluetooth APIs |
| GATT notification/MTU/write-limit setup and write fragmentation | A frame is not a Bluetooth read, notification or characteristic write |
| I/O deadlines and errors | The session only knows about writes that the caller completes or fails |
| Choosing a compatible handshake and capability variant | A raw version or model name is not enough to prove layout compatibility |
| Per-device command queue and response deadlines | Link ACKs carry no general transaction ID and do not confirm operations |
| Capability caching and live-state projection | Cached descriptors are not current battery/settings state |
| User-facing language and labels | Protocol display-language codes differ from application translations |
| Reconnect and connection-generation isolation | Old callbacks must never affect a replacement session |

MDR v2 framing can be carried over RFCOMM or GATT. Known service identifiers are:

| Transport resource | UUID |
| --- | --- |
| RFCOMM service | `956c7b26-d49a-4ba8-b03f-b17d393cb6e2` |
| GATT service | `5b833e20-6bc7-4802-8e9a-723ceca4bd8f` |
| GATT write characteristic | `5b833c60-6bc7-4802-8e9a-723ceca4bd8f` |
| GATT notify characteristic | `5b833c61-6bc7-4802-8e9a-723ceca4bd8f` |

Availability depends on the device and operating system. A GATT adapter must configure notifications and negotiate writable length/MTU before sending frames.

## Capability-driven compatibility

A function is identified by a table **and** a code. For example, T1 `20` can describe a battery layout; T2 `20` has a different meaning. The library retains unknown function codes and their priorities, but does not create a writable operation for them. `BatteryKind::from_function`, `NoiseKind::from_function` and `EqualizerKind::from_function` map known T1 identities to their payload layouts. The other feature kinds or profiles provide the same explicit selection. `PowerKind` additionally owns the command table: T1 battery and T2 standby can share an opcode and inquiry while carrying different bodies. Playback and voice profiles retain advertised function identities when several functions share one inquiry but authorize different operations. General-setting slots additionally require a recognized descriptor identity before permitting writes.

Applications must track the device's advertised functions, the library's supported layouts, current function availability and state freshness separately. A timeout does not establish that a function is unsupported. Model names supply presentation and diagnostics; advertised function identities select payload layouts.

## Typed errors and unknown information

Each layer returns its own typed error. The framing layer may recover at a new delimiter; the caller decides whether continued use of the link is appropriate. Frame types unsupported by `Session` produce errors. Terminal session failures emit `Closed` and require transport teardown.

Known payloads enforce their exact shapes. Unknown identity codes within known shapes can be preserved (for example, series/color or advertised functions). Unknown discriminants that change a layout are rejected. Opaque text/identifier bytes are retained without truncation; explicit UTF-8 interpretation can fail without discarding the original bytes.

## Integration contract

Retain a query's `ExpectedResponse` before submitting its payload. Execute every `Transmit` in order and report full-write success/failure using its token. Feed both responses and notifications while a query is outstanding. Check the table, opcode and inquiry before fully decoding a matching response. A battery update with opcode `25` is a notification, not a response to the `22` query.

`LinkAcknowledged` reports acknowledgment of a transmitted frame. A decoded `Response` validates a query reply; confirming the resulting device state may require a separate parameter report. For settings without a dedicated write response, check current availability, encode against the matching capability or profile, then observe notifications or request the current parameters. A notification has no transaction identifier, so a matching value cannot be attributed to a specific write.

Peripheral operation results use a separate matcher for inquiry, action and target address. Inspect the matched outcome to distinguish success, progress, busy, failure and unknown results. Alert replies encode an explicit caller decision. Connection notices describe requested transport actions, and outbound LE Audio reports describe host-observed profile state; the application handles the corresponding system APIs.

The user power-off command may end the control connection. Disconnection alone does not confirm that power-off succeeded. More generally, a timeout does not establish whether an operation took effect. Serialize commands with indistinguishable responses and synchronize state or reconnect when a late reply could be confused with a later operation.

On disconnect, cancel outstanding work, dispose of the decoder/session and isolate late transport callbacks. Create a new `Session` for each connection; session instances have no reset operation.

## API compatibility and documentation

The public API may change in later minor releases before version `1.0`. `#[non_exhaustive]` error and message enums require a fallback match arm so callers can handle new variants. Unknown identity values remain available as raw bytes.

Detailed family documents are embedded in their module docs, so their examples are checked by `cargo test --doc`. The crate package includes the protocol guide, tests, fixtures and executable examples.
