# Speak-to-Chat and local system controls

These SYSTEM messages use command table 1. Function identities select layouts; device names do not. Availability, a configured on/off switch and an active Speak-to-Chat effect are distinct states.

## Layouts and operations

| Function | Inquiry | `SystemKind` | Capability | Extended parameters |
| --- | --- | --- | --- | --- |
| `f0` | `00` | `Vibrator` | Fixed toggle | None |
| `f1` | `01` | `PlaybackControlByWearing` | Fixed toggle | None |
| `f2` | `02` | `SpeakToChatType1` | Preview support and end-mode values | Sensitivity, voice focus, end mode |
| `fa` | `0a` | `AutoVolume` | Fixed toggle | None |
| `fc` | `0c` | `SpeakToChatType2` | Preview support and end-mode values | Sensitivity, end mode |
| `ff` | `0f` | `HeadGesture` | Fixed toggle | None |

All values in byte layouts are hexadecimal. The following operations use `[opcode, inquiry]` as the header:

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `f0` | `f1` | None |
| Availability and effect | `f2` | `f3` | `f5` |
| Switch parameters | `f6` | `f7` | `f9` |
| Set switch parameters | `f8` | No dedicated response | State reports use `f9` |
| Extended parameters | `fa` | `fb` | `fd` |
| Set extended parameters | `fc` | No dedicated response | State reports use `fd` |

Only the two Speak-to-Chat layouts have capability and extended-parameter queries. For a discovered fixed toggle, construct `SystemCapability::fixed(kind)`. The constructor verifies that the kind is a fixed toggle; the caller verifies that the device advertises its function. A fixed descriptor is not proof of current availability.

## Speak-to-Chat capability

The response is exactly `[f1, inquiry, preview_support, fast, mid, slow]`. Preview support uses `00` unsupported and `01` supported. The last three bytes are unsigned device-supplied values associated with FAST, MID and SLOW. They are retained without asserting units, order or a nonzero lower bound. They are not the selector bytes sent in an extended-parameter update.

`SpeakToChatCapability::mode_out_value` retrieves one of those values. End mode NONE has no capability value and returns `None`. Selecting NONE uses wire value `03`, not a zero duration.

## Availability and configured switches

Fixed-toggle status bodies contain one byte: `00` available, `01` unavailable. Speak-to-Chat adds an effect byte: `00` inactive, `01` active. The latter reports whether Speak-to-Chat is currently active; it is independent of the configured Speak-to-Chat switch. `SystemStatus` exposes `enabled` and optional `talking` separately.

Fixed-toggle parameter bodies contain one byte: `00` on, `01` off. Speak-to-Chat parameter bodies contain `[on_off, preview_on_off]`, each using that same inverted on/off encoding. The complete pair is carried by both reads and writes. When changing one switch, preserve the other value from the latest parameter report.

Reading switch parameters does not require a capability descriptor. Writing requires a descriptor for the exact inquiry. Turning preview on additionally requires advertised preview support; turning it off is permitted without support. The library does not infer preview support from the device model.

## Extended settings

| Inquiry | Body after `[opcode, inquiry]` | Total payload length |
| --- | --- | --- |
| `02` | `sensitivity, voice_focus, end_mode` | 5 bytes |
| `0c` | `sensitivity, end_mode` | 4 bytes |

Sensitivity values are automatic `00`, high `01`, and low `02`. Voice focus uses `00` on and `01` off. End-mode selectors are FAST `00`, MID `01`, SLOW `02`, and NONE `03`. Closed-enum fields reject other values. The type-2 layout has no voice-focus byte; the type-1 layout always includes it.

```rust
use libmdr2::{
    system::{DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemCapability},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
let capability = SystemCapability::decode_response(table, &[0xf1, 0x02, 1, 5, 15, 30])?;
let mut settings = SpeakToChatSettings::decode_response(table, &[0xfb, 0x02, 0, 0, 1])?;
if let SpeakToChatSettings::Type1 { sensitivity, .. } = &mut settings {
    *sensitivity = DetectionSensitivity::Low;
}
assert_eq!(settings.encode_set(&capability)?, [0xfc, 0x02, 2, 0, 1]);
assert_eq!(capability.speak_to_chat().unwrap().mode_out_value(ModeOutTime::Mid), Some(15));
# Ok::<(), libmdr2::command::CommandError>(())
```

Extended writes require the capability for the same Speak-to-Chat layout. They do not modify the ordinary on/off and preview switches. Ordinary and extended notifications have different opcodes and must update their respective state.

## Integration and decoding contract

`encode_set` returns table-1 payload bytes. The caller checks live availability, maintains descriptor freshness, drives the reliable session, and reconciles parameter notifications or serialized reads. A link ACK confirms delivery only; it does not establish that the requested setting took effect. A notification contains no transaction identifier and cannot satisfy a pending read response.

All decoders validate the table, opcode, supported inquiry, exact length and closed fields. Unsupported system inquiries return an error. Button assignments use the separate [assignment](assignment.md) module. The wearing control configures playback behavior; physical wearing state is reported by [table-2 system settings](system2.md). This module does not implement voice assistants, Quick Access, resets, fitting tests or head-gesture training.
