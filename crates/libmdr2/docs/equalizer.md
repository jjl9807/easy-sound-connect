# Equalizer

Equalizer messages use command table 1. Select the inquiry from the advertised function identity; model names, preset counts and band counts are not selectors.

| Function | Inquiry | Layout |
| --- | --- | --- |
| `50` | `00` | Presets with editable custom/user curves |
| `52` | `02` | Preset selection without curve editing |
| `53` | `03` | Presets, editable curves and ULT mode |
| `57` | `04` | Presets and editable curves with status error codes |
| `55` | `31` | Standalone custom curve without a preset |

All numbers in wire tables are hexadecimal unless stated otherwise. All counts and levels are unsigned bytes. These layouts belong to MDR v2; identical opcode values in another protocol do not establish compatibility.

## Commands

Every payload begins with `[opcode, inquiry]`.

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `50` | `51` | — |
| Availability | `52` | `53` | `55` |
| Parameters | `56` | `57` | `59` |
| Band information | `5a` | `5b` | — |
| Set parameters | `58` | — | Parameter reports use `59` |

Capability requests include a third byte, `DisplayLanguage`, selecting the language of device-supplied preset labels. English is `01`; Simplified Chinese is `0c`. All other queries contain only the two-byte header.

## Capabilities and preset names

The following bodies follow `[51, inquiry]`:

| Inquiry | Body |
| --- | --- |
| `00`, `02`, `04` | `band_count, level_steps, preset_count, presets...` |
| `03` | `band_count, level_steps, ult2_additional_step, preset_count, presets...` |
| `31` | `band_count, level_steps` |

Each preset record is `[preset_id, name_length, name_bytes...]`. The name length is at most 128 bytes, and zero is valid. `Preset::name_bytes` preserves the bytes; `Preset::name_utf8` validates UTF-8 without replacing invalid sequences. The library neither substitutes a name for an empty label nor translates preset IDs. Unknown IDs, duplicate entries and device ordering are preserved.

`band_count` determines the entire curve length. No five-, six- or ten-band layout is assumed. `level_steps` is the number of native steps: writable levels are in `0..level_steps`. For example, 13 steps admit values 0 through 12. The protocol reports native steps; this API does not assign them a decibel scale. A caller displaying a centered scale can use `i16::from(raw) - (i16::from(level_steps) - 1) / 2` after checking that the step count is nonzero. Use signed arithmetic so values below the center cannot underflow an unsigned byte.

`ult2_additional_step` is a display adjustment associated with ULT 2. It does not extend the native level range or get added to values sent in a curve update. Zero band/step counts are retained on decoding, but cannot authorize curve writes.

## Availability and parameters

Availability bodies for `00`, `02`, `03` and `31` contain one byte: `00` means available and `01` means unavailable. Inquiry `04` adds `error_count, error_codes...`. Known reason codes are calling (`00`), demo mode (`01`), listening mode (`02`) and other (`fe`). Unknown reasons are retained. Availability describes whether the function can be operated; it does not indicate whether the OFF preset is active.

Parameter responses and notifications share the following bodies:

| Inquiry | Body |
| --- | --- |
| `00`, `02`, `04` | `preset_id, band_count, levels...` |
| `03` | `preset_id, ult_mode, band_count, levels...` |
| `31` | `band_count, levels...` |

ULT modes are off (`00`), ULT 1 (`01`) and ULT 2 (`02`). The decoder rejects other mode values. Parameter decoding validates the payload structure independently of capabilities and preserves every native level byte. Use `validate_capability` to check the inquiry, curve length, level range and advertised preset. The caller must associate reports with the capability from the same connection.

## Band information and Clear Bass

The extended-information body is `[count, records...]`; each record contains a type byte and an unsigned, big-endian 16-bit value.

| Type | Value interpretation |
| --- | --- |
| `00` | No band information |
| `01` | Frequency in hertz |
| `02` | Frequency in kilohertz |
| `10` | Specific-information ID: `0001` identifies Clear Bass |

Both unknown types and unknown specific-information values are preserved. `BandInformation::frequency_hz` converts only Hz/kHz records. `BandInformation::is_clear_bass` identifies Clear Bass without assuming an index. Clear Bass occupies one element of the same native curve and uses the same level range. `EqualizerBandInfo::validate_capability` checks the inquiry and band count before band metadata is associated with a curve.

## Writing settings

`EqualizerSetting::encode_set` returns a table-1 payload for `Session::send`. The capability selects the inquiry. For preset variants, `levels: None` sends a zero band count and selects the preset without replacing its curve. `Some(levels)` must contain the complete curve and target an advertised custom/user slot (`a0..=a5`). Inquiry `02` accepts selection only. Standalone custom EQ always sends a complete curve. Unknown advertised presets can be selected, but are not assumed to accept curve edits.

```rust
use libmdr2::{
    equalizer::{EqualizerCapability, EqualizerSetting, PresetId},
    types::CommandTable,
};

// A two-band capability with 13 native steps and one editable custom preset.
let capability = EqualizerCapability::decode_response(
    CommandTable::TABLE_1,
    &[0x51, 0x00, 2, 13, 1, 0xa0, 0],
)?;
let payload = EqualizerSetting::Preset {
    preset: PresetId::CUSTOM,
    levels: Some(vec![6, 9]),
}.encode_set(&capability)?;
assert_eq!(payload, [0x58, 0x00, 0xa0, 2, 6, 9]);
# Ok::<(), libmdr2::command::CommandError>(())
```

Encoding checks capability constraints without clamping. Check `EqualizerStatus::enabled` separately before sending. A link ACK confirms frame delivery only; it does not confirm that the device applied a preset or curve. SET opcode `58` has no dedicated response in these layouts. Check parameter notifications or issue a parameter query to confirm the resulting state. The caller schedules requests and enforces response deadlines.

## Decoding contract

All entry points require table 1 and their specific response or notification opcode. They reject unsupported inquiries, missing bytes, inconsistent counts, trailing bytes and invalid closed-enum values. Allocation follows validated, byte-sized counts. Unknown preset IDs, reason codes and band-information records remain available through raw-value types. Payload decoding performs no I/O and requires no runtime, Bluetooth stack or GUI.
