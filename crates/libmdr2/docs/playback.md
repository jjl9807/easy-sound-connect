# Playback, metadata and volume

PLAY messages use command table 1. Select a `PlaybackProfile` from the device's advertised function list. The profile and the wire inquiry are distinct identities.

| Function | Profile | Read inquiry | Music volume | Call volume | Control inquiry |
| --- | --- | --- | --- | --- | --- |
| `a1` | `WithCallVolume` | `01` | `20` | `21` | `01` |
| `a2` | `WithCallVolumeAndMute` | `01` | `30` | `31` | `01` |
| `a3` | `WithCallVolumeAndFunctionChange` | `02` | `20` | `21` | `01` |
| `a4` | `WithFunctionChange` | `03` | `20` | None | `03` |

`PlaybackKind` identifies the three read layouts. `PlaybackProfile` retains the additional advertised distinction between ordinary volume and volume with mute. Capability replies to `a1/a2` do not distinguish those profiles. Callers must retain the selected profile from the same connection as the capability.

## Commands and capability

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `a0 inquiry` | `a1 inquiry ...` | None |
| Availability/state | `a2 inquiry` | `a3 inquiry ...` | `a5 inquiry ...` |
| Metadata | `a6 inquiry` | `a7 inquiry ...` | `a9 inquiry ...` |
| Volume | `a6 volume_inquiry` | `a7 volume_inquiry ...` | `a9 volume_inquiry ...` |
| Playback operation | `a4 control_inquiry 00 action` | No dedicated response | State reports |
| Volume update | `a8 volume_inquiry level` | No dedicated response | Parameter reports |

All numbers in wire layouts are hexadecimal unless stated otherwise. A capability body is `[music_maximum, call_maximum]` for inquiries `01/02`, and `[music_maximum]` for inquiry `03`. Each maximum is a nonzero unsigned byte. Writable native levels are in `0..=maximum`; these values are neither percentages nor decibels.

## Status

Status responses and notifications share these bodies:

| Inquiry | Body |
| --- | --- |
| `01` | `enabled, playback_state, music_call_state` |
| `02` | `enabled, playback_state, music_call_state, control_enabled` |
| `03` | `enabled, playback_state, control_enabled` |

Availability fields use `00` available and `01` unavailable. Playback states are `00` unsettled, `01` playing, `02` paused and `03` stopped. Music/call context is `00` music or `01` call. These fields remain separate: a reported state does not imply that controls are currently available. Undefined state values are rejected.

## Metadata and volume reports

Metadata bodies contain exactly four entries in track, album, artist and genre order. Each entry is `[state, byte_length, name_bytes...]`. States are `00` unsettled, `01` nothing and `02` settled. Each name contains zero through 128 bytes; length is a byte count, not a character count. Empty bytes do not replace the state field. `MetadataText::bytes` preserves original bytes, and `as_utf8` performs explicit UTF-8 validation without replacement characters.

Volume responses for `20/21` contain one unsigned level byte. Responses for `30/31` contain `[level, mute]`, where `00` means muted and `01` means not muted. `PlaybackVolume` can be decoded before a capability response is available. Use `validate_capability` to check the reported level against the selected profile and capability.

## Encoding operations

```rust
use libmdr2::{
    playback::{PlaybackCapability, PlaybackControl, PlaybackProfile, VolumeSetting},
    types::CommandTable,
};

let profile = PlaybackProfile::WithCallVolumeAndFunctionChange;
let capability = PlaybackCapability::decode_response(
    CommandTable::TABLE_1, &[0xa1, 0x02, 31, 16],
)?;
// This profile reads inquiry 02, but playback operations use inquiry 01.
assert_eq!(PlaybackControl::Pause.encode_set(profile, &capability)?, [0xa4, 1, 0, 1]);
let payload = VolumeSetting { kind: profile.music_volume_kind(), level: 31 }
    .encode_set(profile, &capability)?;
assert_eq!(payload, [0xa8, 0x20, 31]);
# Ok::<(), libmdr2::command::CommandError>(())
```

The operation API exposes play (`07`), pause (`01`), next track (`02`) and previous track (`03`). Other protocol action codes are not exposed by `PlaybackControl`. Volume writes always contain three bytes: even `30/31` writes have no mute field. Mute is read-only in these layouts. Playback encoders check profile/layout consistency. Volume encoding also checks music/call support, the selected mute-reporting layout and the inclusive native maximum.

The caller checks current availability, associates reports with the current device connection, serializes operations and enforces response deadlines. An ACK confirms reliable-frame delivery, not a completed playback operation. Observe state/parameter reports or perform a serialized readback. Do not automatically retry track changes after an ambiguous application-level timeout.

All decoders enforce table, opcode, inquiry and exact lengths. Metadata parsing rejects truncated entries, extra entries and trailing bytes. Metadata and volume share `a7/a9`; their inquiries determine the decoder. Repeat/shuffle player modes and host media-service integration are outside these layouts.
