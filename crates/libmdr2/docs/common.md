# Connection and audio indicators

The `common` module decodes read-only information reported by the device. Select a layout through `CommonKind::from_function` before submitting `Request::CommonStatus`. All four layouts use command table 1.

| Function | Inquiry | Report |
| --- | --- | --- |
| `11` | `01` | Left and right component connections |
| `12` | `02` | Audio codec |
| `13` | `03` | Upscaling effect |
| `18` | `06` | Device operating modes |

Queries are `[12, inquiry]`. Responses start with `[13, inquiry]`, and notifications with `[15, inquiry]`. Notifications never satisfy a pending query's `ExpectedResponse`. There are no capability queries or writes for these reports.

## Payload bodies

The following bytes follow the two-byte header. Byte values are hexadecimal; lengths are decimal byte counts, and each count field is an unsigned byte.

| Inquiry | Body | Total length |
| --- | --- | --- |
| `01` | `left, right` | 4 |
| `02` | `codec` | 3 |
| `03` | `upscaling_type, effect_state` | 4 |
| `06` | `count, modes...` | `3 + count` |

Connection fields are `00` not connected and `01` connected. Invalid values produce an error. These fields describe earbud components, not the host's audio profile or control transport. They do not derive from a battery percentage: zero battery is a valid reading, and a missing connection report leaves the connection state unknown.

Codec identities include `00` unsettled, `01` SBC, `02` AAC, `10` LDAC, `20` aptX, `21` aptX HD, `30` LC3 and `ff` other. `AudioCodec` retains unknown bytes without mapping them to another codec. This report neither configures a host encoder nor provides a list of codecs supported by the device.

Upscaling types share `audio::UpscalingType`: `00` DSEE HX, `01` DSEE, `02` DSEE Extreme and `03` DSEE Ultimate. The type identity preserves unknown values. Effect states are `00` off, `01` valid and `02` invalid; other values are rejected. The effect state is independent of the DSEE auto/off preference in AUDIO and does not supply a reason when invalid.

Device modes are identities: `00` normal, `01` two-device mode, `02` stereo pair, `03` USB dongle, `04` Party Connect, `05/06` Party Chain transmitter/receiver and `07/08` Auracast transmitter/receiver. Empty lists, unknown entries, duplicates and wire order are retained. These observations do not authorize operations for those modes or identify connected source devices.

## Decoding and routing

```rust
use libmdr2::{
    command::{Notification, Request, Response},
    common::{AudioCodec, CommonKind, CommonStatus},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
let query = Request::CommonStatus(CommonKind::AudioCodec).encode()?;
assert_eq!(query.payload, [0x12, 0x02]);
let response = query.expected_response.decode(table, &[0x13, 0x02, 0x10])?;
assert_eq!(response, Response::CommonStatus(CommonStatus::AudioCodec(AudioCodec::LDAC)));
let update = [0x15, 0x02, 0x30];
assert!(!query.expected_response.matches(table, &update));
assert_eq!(Notification::decode(table, &update)?,
    Notification::CommonStatus(CommonStatus::AudioCodec(AudioCodec::LC3)));
# Ok::<(), libmdr2::command::CommandError>(())
```

All decoders validate the table, opcode, inquiry and exact body length before returning a report. Counted mode lists are bounded by the one-byte count; trailing bytes are rejected. Unsupported COMMON inquiries are not interpreted as one of these layouts. The caller owns report freshness and must refresh state after reconnecting.
