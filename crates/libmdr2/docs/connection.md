# Connection messages and capability discovery

## Routing and scope

These layouts are MDR v2 command payloads, not complete wire frames. T1 uses data type `0c`; T2 uses `0e`. Every typed query starts with an opcode and an inquiry byte. The data type supplies the command table; the payload alone cannot distinguish two tables that reuse the same opcode.

| Query | Table | Request | Matching response |
| --- | --- | --- | --- |
| Protocol info | T1 | `00 00` | `01 00 ...` |
| Standard capability info | T1 | `02 00` | `03 00 ...` |
| App-type capability info | T1 | `02 01 app_type` | `03 01 ...` |
| Supported functions | T1 or T2 | `06 00` | `07 00 ...` on the same table |
| Device info | T1 | `04 kind` | `05 kind ...` |
| Battery status | T1 | `22 kind` | `23 kind ...` |
| Noise capability | T1 | `60 kind` | `61 kind ...` |
| Noise availability | T1 | `62 kind` | `63 kind ...` |
| Noise parameters | T1 | `66 kind` | `67 kind ...` |
| EQ capability | T1 | `50 kind language` | `51 kind ...` |
| EQ availability | T1 | `52 kind` | `53 kind ...` |
| EQ parameters | T1 | `56 kind` | `57 kind ...` |
| EQ band information | T1 | `5a kind` | `5b kind ...` |
| Common status | T1 | `12 kind` | `13 kind ...` |
| Audio capability / availability / parameters | T1 | `e0/e2/e6 kind` | `e1/e3/e7 kind ...` respectively |
| Power capability / availability / parameters | Kind-dependent T1 or T2 | `20/22/26 kind` | `21/23/27 kind ...` on the same table |
| System capability / availability / parameters | T1 | `f0/f2/f6 kind` | `f1/f3/f7 kind ...` respectively |
| Speak-to-Chat extended parameters | T1 | `fa kind` | `fb kind ...` |
| General capability / availability / parameters | T1 | `d0 kind language` / `d2 kind` / `d6 kind` | `d1/d3/d7 kind ...` |
| Playback capability / availability / metadata or volume | T1 | `a0/a2/a6 kind` | `a1/a3/a7 kind ...` |
| Assignment capability / availability / presets / custom actions | T1 | `f0/f2/f6/fa kind` | `f1/f3/f7/fb kind ...` |
| Peripheral capability / availability / parameters | T2 | `30/32/36 kind` | `31/33/37 kind ...` |
| Voice capability / parameters | T2 | `40/46 kind` | `41/47 kind ...` |
| Voice availability | T2 | `42 kind 00` for `00..02`; `42 kind` otherwise | `43 kind ...` |
| Sound-effect capability | T1 | `50 30 language` / `50 33` / `50 40` | `51 kind ...` |
| Sound-effect availability / parameters / bands | T1 | `52/56/5a kind` where defined | `53/57/5b kind ...` |
| Table-2 system capability / status / parameters | T2 | `f0/f2/f6 kind` where defined | `f1/f3/f7 kind ...` |
| LE Audio alert availability | T1 | `92 05` | `93 05 available` |

Feature bodies are specified in the [device](device-information.md), [battery](battery.md), [noise](noise-control.md) and [equalizer](equalizer.md) chapters. NC-only inquiry `01` has a fieldless capability and does not use a capability query. Notifications use `25` for battery, `65/69` for noise availability/parameters and `55/59` for EQ availability/parameters; `Notification` decodes them separately from query responses. Command opcode assignments are family-specific; arithmetic on known opcodes does not define a valid operation.

The [audio](audio.md), [system](system.md), [power](power.md) and [common](common.md) chapters describe their bodies and supported queries. Some kinds use a fixed capability descriptor and reject a dynamic capability query. System extended queries in `system` are defined only for Speak-to-Chat kinds. The `assignment` module uses its own extended-parameter layouts for inquiries `03/0e`. Voice status queries for `00..02` explicitly select the on/off state with selector `00`; the volume layout `20` has no status query. Family chapters define which operations exist for each inquiry; the table above does not imply that every operation exists for every inquiry.

`Notification` also dispatches common `15`, audio `e5/e9`, system `f5/f9/fd`, and power `25/29`. Opcode `25` is shared: T1 battery inquiries select battery reports, other implemented T1 power inquiries select setting availability, and T2 `00/01` select standby/charging availability. A malformed battery body returns an error without being passed to another family's decoder. Query response matching uses the same table-aware distinction.

AUDIO inquiry `05` has different bodies in a response and a notification: the notification appends a stream-transition field. Sharing an inquiry does not imply that replacing an opcode produces another valid message. Voice inquiry `01`, peripheral inquiry `01` and customizable sound effects also have different response and notification layouts.

Notification dispatch uses both table and inquiry when opcodes overlap: assignment versus system (`f5/f9/fd`), sound effects versus EQ (`55/59`), playback metadata versus volume (`a9`), and table-2 versus table-1 system messages. Alerts (`99`), connection requests (`19`) and peripheral operation results (`3d`) are unsolicited messages with their own decoders, not read responses. The outbound LE Audio state report (`68 00`) is not a device notification.

## Protocol information

`01 00 version:u32be table_1:u8 table_2:u8` is exactly eight bytes:

| Payload offset | Meaning |
| --- | --- |
| 0 / 1 | Response opcode `01` / inquiry `00` |
| 2–5 | Protocol version, preserved as an uninterpreted 32-bit value |
| 6 | Table-1 support: `00` enabled, `01` disabled |
| 7 | Table-2 support: `00` enabled, `01` disabled |

Other support flag values, missing bytes and trailing bytes are rejected. These flags use inverted Boolean encoding: `00` means enabled and `01` means disabled. The version prefix is not a reliable MDR generation test. A table's absence is different from an empty function list returned by a supported table.

Example: `01 00 03 00 30 32 00 00` reports version `0x03003032` and support for both command tables. Decoding the version does not establish compatibility with every message layout advertised by the device.

## Capability information variants

Both response variants use `03 inquiry counter:u8 id_length:u8 id_bytes...`. The total length must equal `4 + id_length`. Standard inquiry is `00`; app-type inquiry is `01`. The app-type request adds an opaque byte, but the response does not echo it. The caller must explicitly choose the request layout.

`counter` is an unsigned capability revision/counter field, independent of the frame sequence. The ID is retained as opaque bytes without assuming UTF-8 encoding, address formatting or uniqueness across physical devices.

Both layouts accept capability IDs of `0..=128` bytes. The decoder requires the exact body length and preserves all identifier bytes; oversized identifiers are rejected. The caller selects the request layout and app-type value explicitly.

## Supported functions

The response is `07 00 count:u8 (function_code:u8 priority:u8)*count`, with total length exactly `3 + 2 * count`. Count is unsigned and can encode 255 entries. There is no sort or deduplication step in the decoder.

Each entry becomes a `FunctionId { table, code }` plus its original priority. Unknown codes, repeated entries and wire order are retained. Some advertised codes describe restrictions or connection behavior rather than settings; an application must select the implemented operations for each recognized function.

For example, T1 `20` identifies the basic single-battery layout. The same code on T2 is a different function. A higher-level capability projection may group several known variants into one feature, but must retain the selected wire shape.

## Request and response API

`Request::encode` returns a payload, its table and an `ExpectedResponse` descriptor. Register the descriptor before making the transport write. `matches` only checks table/opcode/inquiry; `decode` validates the entire body. A positive match alone does not establish a valid response or successful operation.

```rust
use libmdr2::command::{Request, Response};
use libmdr2::types::CommandTable;

let query = Request::SupportedFunctions(CommandTable::TABLE_2).encode()?;
assert_eq!(query.payload, [0x06, 0x00]);
let payload = [0x07, 0x00, 0x02, 0x20, 0x01, 0xfe, 0xff];
assert!(!query.expected_response.matches(CommandTable::TABLE_1, &payload));
let Response::SupportedFunctions(list) =
    query.expected_response.decode(CommandTable::TABLE_2, &payload)? else {
    panic!("unexpected response family");
};
assert_eq!(list.functions[1].id.code, 0xfe);
assert_eq!(list.functions[1].priority, 0xff);
# Ok::<(), libmdr2::command::CommandError>(())
```

No generic transaction ID exists here. Two identical requests have identical response descriptors, so response concurrency, late replies and timeouts remain caller responsibilities. `Session` serializes frame delivery. After a link ACK, a command response may still be pending; wait for or resolve that response before sending another query with the same response descriptor. EQ, general and standard sound-effect capabilities do not echo the requested display language, so queries for different languages also share a response descriptor and must be serialized.

## Suggested discovery sequence and caching boundary

After a supported v2 transport is ready, query protocol information, select a verified capability-info layout, query enabled function tables, then request details/state for supported features. Device name/firmware queries can supply presentation and diagnostics without determining all capabilities.

The library exposes capability IDs, counters and display-language codes; the caller manages capability caching. Display language `01` is English and `0c` is Simplified Chinese. These codes select device-provided capability labels. A capability cache should include the ID, counter, display language and parser schema in its validity checks. Changing language invalidates cached labels without changing device settings. Live state must be synchronized after reconnect; a missing or invalid cache requires fresh capability queries.
