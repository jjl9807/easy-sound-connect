# Device information

MDR v2 table 1 provides read-only model-name, firmware-version and appearance queries. These responses describe the product. Capability discovery determines which controls are supported.

The structures below describe **command payloads**, before framing, escaping or checksum. They travel in reliable table-1 frames (`DataType::DATA_MDR`, `0c`).

## Wire definitions

| Direction | Payload | Size |
| --- | --- | --- |
| Query | `04 inquiry` | Exactly 2 bytes |
| Model-name response | `05 01 n text[n]` | Exactly `3 + n` bytes |
| Firmware-version response | `05 02 n text[n]` | Exactly `3 + n` bytes |
| Series/color response | `05 03 series color` | Exactly 4 bytes |

`n`, `series` and `color` are unsigned bytes. Model and firmware text fields contain `1..=128` **bytes**, not characters. Empty fields, missing bytes, trailing bytes and mismatched declared lengths are rejected.

`DeviceText` retains the exact text bytes without validating their encoding. Use `as_utf8()` to interpret them as UTF-8; it reports invalid UTF-8 instead of substituting replacement characters. The parser does not strip NULs or whitespace, normalize product names, truncate overlong text, or interpret a firmware string as a semantic version. Display fallback belongs to the caller.

The instruction-guide inquiry `04` is unsupported. An unsupported inquiry returns a typed error.

## Series and color

`ModelSeries` and `ModelColor` retain their raw bytes, including unfamiliar codes. The following series codes have named constants:

| Byte | Series | Rust constant |
| --- | --- | --- |
| `00` | No named series | `ModelSeries::NONE` |
| `10` | EXTRA BASS | `ModelSeries::EXTRA_BASS` |
| `11` | ULT POWER SOUND | `ModelSeries::ULT_POWER_SOUND` |
| `20` | h.ear | `ModelSeries::HEAR` |
| `30` | Premium | `ModelSeries::PREMIUM` |
| `40` | Sports | `ModelSeries::SPORTS` |
| `50` | Casual | `ModelSeries::CASUAL` |
| `60` | LinkBuds | `ModelSeries::LINK_BUDS` |
| `70` | Neckband | `ModelSeries::NECKBAND` |
| `80` | Linkpod | `ModelSeries::LINKPOD` |
| `90` | Gaming | `ModelSeries::GAMING` |

The color field is exposed as a raw product identifier through `ModelColor::as_raw()`. The library does not infer RGB values or map unknown colors to a default. Product-asset selection belongs to the caller.

## Rust API

Use `Request::DeviceInfo(DeviceInfoKind)` to obtain a table-1 request and matching response descriptor. Register the descriptor before sending the request. Its expected response uses opcode `05` and the requested inquiry; a model-name response cannot satisfy a firmware query, and a table-2 packet cannot satisfy either one.

```rust
use libmdr2::{
    command::{Request, Response},
    device::{DeviceInfo, DeviceInfoKind},
    types::CommandTable,
};

let request = Request::DeviceInfo(DeviceInfoKind::FirmwareVersion).encode()?;
assert_eq!(request.payload, [0x04, 0x02]);
assert_eq!(request.table, CommandTable::TABLE_1);

let reply = b"\x05\x02\x053.1.5";
let decoded = request.expected_response.decode(CommandTable::TABLE_1, reply)?;
let Response::DeviceInfo(DeviceInfo::FirmwareVersion(version)) = decoded else {
    unreachable!()
};
assert_eq!(version.as_utf8().unwrap(), "3.1.5");
# Ok::<(), libmdr2::command::CommandError>(())
```

For already-routed payloads, `DeviceInfo::decode(table, payload)` validates the complete response directly. `DeviceInfo::kind()` identifies its variant, while `DeviceText::as_bytes()`/`into_bytes()` preserve text without conversion. A link ACK confirms frame delivery; only a matching, valid `05` payload completes the device-information query.

## Payload examples

The fixture suite includes the following published payloads:

| Upstream-reported device | Payload | Expected interpretation |
| --- | --- | --- |
| WH-1000XM6, firmware 3.1.5 | `05 01 0a 57 48 2d 31 30 30 30 58 4d 36` | Model `WH-1000XM6` |
| WH-1000XM6, firmware 3.1.5 | `05 02 05 33 2e 31 2e 35` | Firmware `3.1.5` |
| WH-1000XM6, firmware 3.1.5 | `05 03 30 03` | Premium-series code `30`, color code `03` |
| WH-1000XM5, firmware 2.4.1 | `05 02 05 32 2e 34 2e 31` | Firmware `2.4.1` |

XM6 provenance: [sonymdr_open `sonyctl/sim.py`, commit 44adbf9](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py#L20). XM5 provenance: [sound-connect SONY command tests, commit 749e544](https://github.com/aecca/sound-connect/blob/749e54438ec7be055a45021fea30dff87de22975/crates/core/src/drivers/sony/command.rs#L466).

Synthetic cases cover multibyte text, invalid UTF-8, zero/128/129-byte boundaries, unknown appearance codes, wrong tables, unexpected inquiries, truncation and trailing bytes. See the [testing guide](testing.md) for the full coverage matrix.
