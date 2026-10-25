# Battery status

The `battery` module supports six battery-status layouts. They are read-only POWER messages on **command table 1**. Select the layout from the device's advertised function list.

All hexadecimal values below are bytes in the **unframed payload**. The frame's table identity must remain attached to the payload during dispatch. Equal opcode or function numbers on table 2 do not identify these battery messages.

## Commands and capability selection

| Operation | Payload | Meaning |
| --- | --- | --- |
| `POWER_GET_STATUS` | `22 inquiry` | Query the selected battery layout |
| `POWER_RET_STATUS` | `23 inquiry body...` | Response to the query |
| `POWER_NTFY_STATUS` | `25 inquiry body...` | Asynchronous status notification |

Battery readings use the **status** command family. There is no battery-setting write operation in this module, and no separate `GET_CAPABILITY` request for these six layouts. The supported-function list advertises which inquiry and body layout apply.

| `BatteryKind` | T1 function code | Inquiry | Exact response/notification length |
| --- | --- | --- | --- |
| `Single` | `20` | `00` | 4 |
| `LeftRight` | `21` | `01` | 6 |
| `Cradle` | `22` | `02` | 4 |
| `SingleWithThreshold` | `28` | `08` | 5 |
| `LeftRightWithThreshold` | `29` | `09` | 8 |
| `CradleWithThreshold` | `2a` | `0a` | 5 |

`BatteryKind::from_function` accepts a `FunctionId`, and returns `None` for table-2 functions, including code `20`. `function_id` and `inquiry` expose the corresponding identities. The library does not choose precedence between multiple advertised battery layouts or infer that every TWS product has a cradle reading.

## Single and cradle layouts

Single and cradle batteries have identical body fields but distinct inquiry and endpoint identities:

| Offset | Width | Field | Accepted values |
| --- | --- | --- | --- |
| 0 | 1 | Opcode | `23` for response, `25` for notification |
| 1 | 1 | Inquiry | `00` single, `02` cradle |
| 2 | 1 | Remaining level | `0..=100` percent |
| 3 | 1 | Charging status | Defined below |

For threshold inquiries `08` and `0a`, append one byte at offset 4 containing the reported threshold in `0..=100`. Total length is five bytes. The library exposes this value without deriving notification behavior or comparison rules from it.

## Left and right layout

The two level/status pairs precede the optional two thresholds. Thresholds are **not** interleaved into each pair.

| Offset | Width | Field | Accepted values |
| --- | --- | --- | --- |
| 0 | 1 | Opcode | `23` or `25` |
| 1 | 1 | Inquiry | `01` without thresholds, `09` with thresholds |
| 2 | 1 | Left level | `0..=100` |
| 3 | 1 | Left charging status | `00..=03` |
| 4 | 1 | Right level | `0..=100` |
| 5 | 1 | Right charging status | `00..=03` |
| 6 | 1 | Left threshold | Only for `09`, `0..=100` |
| 7 | 1 | Right threshold | Only for `09`, `0..=100` |

For example, the synthetic payload `23 09 2f 01 52 03 0a 14` means left 47%, charging, threshold 10%; right 82%, charged, threshold 20%.

## Charging, percentage and connectivity

| Code | `ChargingStatus` | Meaning |
| --- | --- | --- |
| `00` | `NotCharging` | Not charging |
| `01` | `Charging` | Charging |
| `02` | `Unknown` | Charging state unknown |
| `03` | `Charged` | Charged |

Every level and threshold becomes a validated `Percentage`. `00` is a real zero; `ff` is not an unknown-level sentinel in these layouts and is rejected. A charged state does not cause the library to replace the reported level with 100.

The battery payloads contain **no connectivity flag**. An unknown charging state, a zero level, or a missing cradle update does not establish that a component is offline. A caller must keep separate state for not-yet-read or stale values and, when available, process the independent COMMON left/right connection status. The module does not synthesize missing readings as zero or omit a reading merely because it is zero.

## Rust API and dispatch

`BatteryStatus::decode_response(table, payload)` accepts only opcode `23`. `decode_notification(table, payload)` accepts only `25`. Both return the exact `BatteryKind` plus `BatteryReadings`, whose variants distinguish `Single`, `LeftRight { left, right }`, and `Cradle`. Each `BatteryReading` contains the level, charging state and optional threshold.

```rust
use libmdr2::{battery::{BatteryReadings, BatteryStatus}, types::CommandTable};

let status = BatteryStatus::decode_notification(
    CommandTable::TABLE_1,
    &[0x25, 0x09, 47, 1, 82, 3, 10, 20],
)?;
if let BatteryReadings::LeftRight { left, right } = status.batteries {
    assert_eq!(left.level.as_u8(), 47);
    assert_eq!(right.threshold.unwrap().as_u8(), 20);
}
# Ok::<(), libmdr2::command::CommandError>(())
```

A pending response matcher must include table, response opcode and inquiry. Notifications use a separate path; a `25` update must not complete a pending `22` query even when its body matches the response layout. A link ACK is neither a battery response nor a setting confirmation.

The decoders reject wrong tables, unsupported inquiries, wrong opcodes, truncated or extended payloads, percentages above 100 and charging codes outside `00..03`. They do not guess another layout when the expected number of bytes is absent.

## Payload examples

`23 00 51 00` is a single-battery response reporting 81%, not charging. `25 02 64 03` is a cradle notification reporting 100%, charged. The first example uses a published body with a reconstructed response header; the second is synthetic. Fixture provenance and coverage are documented in the [testing guide](testing.md).
