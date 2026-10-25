# Sound effects and ULT button assignments

## Negotiation and messages

Sound effects use the table-1 EQEBB command family. Select a layout from the device's advertised function list.

| Function | Kind | Inquiry | Capability request | Status request | Parameters | Band metadata |
| --- | --- | --- | --- | --- | --- | --- |
| `54` | `Standard` | `30` | `50 30 language` | `52 30` | `56 30` | None |
| `59` | `Customizable` | `33` | `50 33` | None | `56 33` | `5a 33` |
| `58` | `UltButton` | `40` | `50 40` | None | `56 40` | None |

Capability, status, parameter and band responses use opcodes `51`, `53`, `57` and `5b`. Notifications use `55` for status and `59` for parameters. Writes use `58`; there is no dedicated parameter-write response. A link ACK confirms frame delivery only. Observe notifications or request fresh parameters.

The `34`, `35` and `36` inquiries belong to the customizable function. They are update operations, not independently advertised layouts. The API does not expose capability or parameter GET requests for these inquiries.

## Capability data

The following layouts start after the opcode and inquiry. All counts are unsigned bytes; each count must match its complete record list and the total payload.

| Inquiry | Capability data |
| --- | --- |
| `30` | `effect_count effects[effect_count] exclusive_count exclusive_functions[exclusive_count]` |
| `33` | `capability_byte level_steps effect_count effects[effect_count]` |
| `40` | `minimum_enabled maximum_enabled` |

`capability_byte` is retained without interpretation. Read the band count and frequency metadata from `5b 33 count (information_type value:u16be)*`. This uses the same `BandInformation` type as the equalizer module. Unknown information types and values remain available. A curve's native values are unsigned steps in `0..level_steps` (exclusive upper bound), not decibels. The minimum ULT enabled count must not exceed the maximum; these bounds apply to enabled entries, not the list's total length.

Effect IDs `00..=06` identify Off, ULT, ULT 1, ULT 2, Custom, Flat and Live. `ff` means no active effect while a mutually exclusive function is active. The known exclusive-function ID `00` identifies Stamina. Unknown IDs are preserved in capabilities and reports. Selection, reset and curve writes require a known local effect uniquely advertised by the corresponding capability. ULT button edits instead validate the target against the current entry list and the capability's enabled-count bounds. `NONE` cannot be selected or used as the target of a write.

## Status and parameter data

Standard status is `enabled count (effect enabled)*`; available is `00`, unavailable is `01`. Both the overall control and individual effects have availability. Check current availability before presenting or issuing a selection.

| Inquiry | Parameter response / notification data | Write data |
| --- | --- | --- |
| `30` | `effect` | `effect` |
| `33`, response | `effect exclusive_count exclusive_functions* curve_count (band_count levels*)*` | `effect` |
| `33`, notification | `effect exclusive_count exclusive_functions*` | `effect` |
| `34`, notification | `effect band_count levels*` | `effect`, resetting this curve |
| `35`, notification | `effect band_count levels*` | `request_notification effect band_count levels*` |
| `36`, notification | `effect exclusive_count exclusive_functions* band_count levels*` | None |
| `40` | `count (effect enabled)*` | `count (effect enabled)*` |

For `33` and `36`, a nonempty exclusive-function list requires effect `ff`. A `33` response has at least one curve. Curves correspond to the capability's effect order. The `33` notification does not contain curves; retain the previous curves until a curve notification or full response replaces them. `inquiry()` distinguishes resets, customizations and external updates.

For a `35` write, `request_notification` is `00` to request a device notification or `01` to omit it. It does not enable or disable the sound effect. Writes always contain the entire curve. `SoundEffectCurveSetting::encode_set` requires band metadata from the same device to validate its dimension, plus the capability to check effect membership and native step bounds. Empty metadata cannot authorize a curve write.

Decoding checks wire structure and closed enabled-state values independently of negotiation. `validate_capability` checks reported membership, curve slot count and native levels. `SoundEffectBandInfo::validate_parameters` checks curve lengths. Keep the capability, metadata and reports associated with the same device and negotiation; these standalone values do not carry a device identity.

## Updating ULT button assignments

`SoundEffectParameters::encode_button_change` encodes a change to one existing entry from the latest decoded `40` state. It leaves the decoded snapshot unchanged. The target must occur exactly once and identify a known local effect. The encoder preserves every other identity, state and list position, including unknown effects, and checks the resulting enabled count. There is no API to add, reorder or replace the entire list. A complete current parameter response or notification is required before making a change.

```rust
use libmdr2::{
    sound_effect::{SoundEffect, SoundEffectCapability, SoundEffectParameters},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
let capability = SoundEffectCapability::decode_response(table, &[0x51, 0x40, 1, 2])?;
let current = SoundEffectParameters::decode_response(
    table, &[0x57, 0x40, 3, 2, 0, 3, 1, 5, 1],
)?;
let write = current.encode_button_change(&capability, SoundEffect::ULT_2, true)?;
assert_eq!(write, [0x58, 0x40, 3, 2, 0, 3, 0, 5, 1]);
# Ok::<(), libmdr2::command::CommandError>(())
```

These APIs cover local effects and button cycling. Server-supplied EQ data, listening-comparison procedures, channel-balance correction and EBB commands are outside this module's API.
