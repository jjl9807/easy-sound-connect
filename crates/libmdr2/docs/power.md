# Power settings

The `power` module covers device-local power settings on command tables 1 and 2. Battery readings use the separate `battery` module. Select each layout from its table-aware function identity; an inquiry byte alone is insufficient for routing.

## Layouts and commands

| `PowerKind` | Table | Function | Inquiry | Capability |
| --- | --- | --- | --- | --- |
| `AutoPowerOff` | 1 | `24` | `04` | Automatic power-off option list |
| `AutoPowerOffWithWearingDetection` | 1 | `25` | `05` | Option list including removal from ears |
| `PowerSaving` | 1 | `26` | `06` | Fixed on/off layout; no capability query |
| `CaringCharge` | 1 | `2c` | `0c` | Fixed on/off layout; no capability query |
| `AutoStandby` | 2 | `20` | `00` | Standby candidate |
| `CaringChargeWithThreshold` | 2 | `22` | `01` | Read-only charge threshold |

All byte codes in this chapter are hexadecimal; durations and percentages are decimal. Queries consist of `[opcode, inquiry]` on the selected command table.

| Operation | Query | Response | Notification |
| --- | --- | --- | --- |
| Capability | `20` | `21` | — |
| Availability | `22` | `23` | `25` |
| Parameters | `26` | `27` | `29` |
| Set parameters | `28` | No dedicated RET | State reports use `29` |

The query API rejects capability queries for `PowerSaving` and `CaringCharge`. After discovering those functions, construct their fieldless descriptors with `PowerCapability::fixed`. Every setting supports availability and parameter reads.

Table-1 battery notifications and power-setting notifications share opcode `25`. For example, table 1 inquiry `00` means battery status, while table 2 inquiry `00` means automatic-standby availability. Route by table, opcode and inquiry before interpreting the body.

## Automatic power-off

Capability responses for inquiries `04` and `05` contain `[21, inquiry, count, option * count]`. Count is unsigned; exact length is required. The decoder retains device order, duplicate options and unknown option identities.

| Option | Meaning |
| --- | --- |
| `00` | 5 minutes |
| `01` | 30 minutes |
| `02` | 60 minutes |
| `03` | 180 minutes |
| `04` | 15 minutes |
| `10` | When removed from ears; inquiry `05` only |
| `11` | Automatic power-off disabled |

Parameter responses, notifications and writes use `[opcode, inquiry, active_option, selected_time]`. The second option field is stored independently of the active option and must be retained when changing an unrelated field. An option identity is not a minute count; use `PowerOffOption::minutes` to interpret known timer IDs.

Encoding requires an understood, advertised active option. If the capability contains timer candidates, `selected_time` must be one of those timers. If the device offers only removal and disabled options, the payload still carries the second field. Its known value can be preserved even though the capability offers no timer. Unknown identities remain available for reading but cannot be written. No option is silently substituted or normalized.

```rust
use libmdr2::{
    power::{PowerCapability, PowerOffOption, PowerParameters, PowerSettings},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
// This device offers removal/disabled only, but still reports the time field.
let capability = PowerCapability::decode_response(table, &[0x21, 5, 2, 0x10, 0x11])?;
let mut parameters = PowerParameters::decode_response(table, &[0x27, 5, 0x10, 0])?;
if let PowerSettings::AutoPowerOff { active, .. } = &mut parameters.settings {
    *active = PowerOffOption::DISABLED;
}
assert_eq!(parameters.encode_set(&capability)?, [0x28, 5, 0x11, 0]);
# Ok::<(), libmdr2::command::CommandError>(())
```

## Switches, standby and caring charge

Power-saving, caring-charge and automatic-standby parameters use `[opcode, inquiry, on_off]`, where `00` is on and `01` is off. The Rust API uses `PowerSettings::Switch { enabled }`; it does not expose an inverted Boolean.

Table-2 automatic-standby capability is `[21, 00, candidate]`. Candidate `00` means 15 minutes. Unknown candidates remain available through `standby_candidate`, but do not authorize writes because their meaning is unknown. The timer is capability data; the writable parameter is only an on/off switch.

Table-2 caring-charge capability is `[21, 01, threshold]`, with threshold in `1..=100` percent. `charge_threshold` exposes that read-only value. Its parameter only controls whether caring charge is enabled; it cannot set a new percentage.

Availability is `[23|25, inquiry, enabled]`, where `00` means available and `01` means unavailable. Table-2 caring charge appends a notice byte: `00` necessary, `01` unnecessary. `PowerStatus::battery_notice_required` exposes it independently of availability. Neither field reports battery charge percentage or charging state. A control can be available while its setting is switched off.

## User power-off

`PowerOff::encode()` returns the fixed table-1 payload `24 03 01`. Before sending it, the caller must verify that the device advertises `PowerOff::function_id()` (table 1 function `23`); encoding does not perform this check. This is a user power-off action, with no parameter read or dedicated response. The API does not expose factory power-off. The application owns user interaction, transmission policy and connection teardown. A transport disconnect may accompany power-off but is not, by itself, proof that the requested action succeeded.

## Validation and confirmation

All decoders validate opcode, table/inquiry layout, exact length and closed flag values. Parameter decoding does not require cached capabilities; original option IDs remain available even if capabilities are missing or changed. Encoding checks the descriptor's table and layout, field shape and supported options. The caller must associate the capability with the same physical connection and check current availability before sending.

Setting encoders return payload bytes for `Session::send` on `kind().table()`. They do not create response descriptors. A link ACK confirms frame delivery, not that a setting was applied. Check parameter notifications or issue a serialized parameter read to confirm the resulting state. Notifications have no transaction identifier and are not guaranteed after every write. Command deadlines and reconnect policy belong to the caller.
