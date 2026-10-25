# Voice guidance and prompt settings

Voice guidance operates in command table 2. This module provides local prompt switches, prompt volume, battery announcements, power sounds and the ULT beep. Language identities are read-only. Package transfer, language selection, update status and package information commands are outside this module's API.

Select a `VoiceProfile` from an advertised `FunctionId`. Functions `41` and `42` share inquiry `01`, but only `42` also advertises volume inquiry `20`. A capability reply alone cannot establish that optional volume support exists.

| Table-2 function | Primary inquiry | Additional inquiry | Local setting |
| --- | --- | --- | --- |
| `40` | `00` | — | Prompt switch |
| `41` | `01` | — | Prompt switch with language report |
| `42` | `01` | `20` | Prompt switch and volume |
| `43` | `21` | — | Prompt volume |
| `44` | `02` | — | Prompt switch retaining its language field |
| `45` | `03` | — | Prompt switch |
| `46` | `30` | — | Battery announcement switch |
| `47` | `31` | — | Power-sound switch |
| `48` | `32` | — | ULT-beep switch |

## Reads and availability

Commands use `40/41` for capability requests/replies, `42/43` for status requests/replies, `45` for status notifications, `46/47` for parameter requests/replies and `49` for parameter notifications. All layouts require an exact payload length. Unknown inquiry layouts, malformed state bytes and trailing bytes are errors.

Only inquiries `00`, `01` and `02` have capability queries. Their status requests include the `ON_OFF` selector: `[42, inquiry, 00]`. Status replies and notifications then contain `[opcode, inquiry, 00, enabled]`. The decoder rejects language-update status selector `01`. The other status-capable inquiries use `[42, inquiry]` and `[opcode, inquiry, enabled]`. Volume inquiry `20` has no status query; use the companion prompt setting's availability. `VoiceKind::status_request` handles these differences explicitly.

For inquiries without a capability query, call `VoiceCapability::fixed` only after identifying the advertised function. Fixed capability information does not establish current availability. Capability and status are distinct: a supported setting may be temporarily unavailable.

Capabilities have the following bodies after `[41, inquiry]`:

| Inquiry | Body |
| --- | --- |
| `00` | `header[4], supports_switch` |
| `01` | `header[4], supports_switch, count, language[count]` |
| `02` | `supports_switch, count, language[count]` |

`supports_switch` is `00` for unsupported and `01` for supported. Availability uses `00` for available and `01` for unavailable; on/off parameters use `00` for on and `01` for off.

The four-byte capability header for inquiries `00/01` is retained verbatim. The payload must contain at least `3 + header[0]` bytes, and the other three header bytes must each be `00` or `01`. These fields do not enable package operations.

Language counts are unsigned bytes. Language order, duplicates and unknown language identities remain intact; these read-only identities do not share an enum with other protocol families.

## Parameter layouts and writes

| Inquiry | `RET_PARAM` body | `NTFY_PARAM` body | `SET_PARAM` body |
| --- | --- | --- | --- |
| `00`, `03`, `30`, `31`, `32` | `on_off` | `on_off` | `on_off` |
| `01` | `on_off, language` | `on_off` | `on_off` |
| `02` | `on_off, language` | `on_off, language` | `on_off, retained_language` |
| `20`, `21` | `volume` | `volume` | `volume, feedback` |

`VoiceParameters::language()` returns `None` for an inquiry-`01` notification. That means this packet carries no language field; it does not invalidate a previously received language report. Switch writes use the latest parameters and a matching capability. For inquiry `02`, `encode_switch` resends the unchanged language from the report and requires that identity to remain present in the capability. There is no setter accepting a replacement language. Use reports and capabilities from the same device session and refresh them after reconnection.

Volume is a signed, native integer step in `-2..=2`, encoded in two's complement. It is not a percentage or a decibel value. A volume write always includes the fourth byte: feedback `00` asks for an audible preview, and `01` disables preview. `VoiceVolumeSetting::encode_set` requires an advertised profile with volume support; inquiry-`01` capability information alone is insufficient.

```rust
use libmdr2::{
    types::CommandTable,
    voice_guidance::{VoiceCapability, VoiceKind, VoiceParameters},
};

let kind = VoiceKind::BatteryAnnouncement;
let capability = VoiceCapability::fixed(kind)?;
let current = VoiceParameters::decode_response(
    CommandTable::TABLE_2, &[0x47, 0x30, 0x00],
)?;
assert_eq!(current.encode_switch(false, &capability)?, [0x48, 0x30, 0x01]);
# Ok::<(), libmdr2::command::CommandError>(())
```

Encoders return command payloads; framing and delivery remain the caller's responsibility. A link ACK confirms frame delivery only. Observe a parameter reply or notification to confirm the current setting. The library performs no device I/O and does not play preview sounds itself.
