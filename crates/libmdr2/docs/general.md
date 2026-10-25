# Device-described local settings

The GENERAL_SETTING family exposes four table-1 slots, inquiries `d1..d4`. Their advertised function codes are also `d1..d4`. A slot does not have a fixed meaning: discover its descriptor before treating it as sidetone, multipoint or another feature.

## Commands and routing

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `d0 slot language` | `d1 slot ...` | — |
| Availability | `d2 slot` | `d3 slot enabled` | `d5 slot enabled` |
| Read parameters | `d6 slot` | `d7 slot type value` | `d9 slot type value` |
| Set parameters | `d8 slot type value` | No dedicated RET | State reports use `d9` |

`GeneralKind` selects only a slot. `Request::GeneralCapability` includes `DisplayLanguage`, such as `01` for English or `0c` for Simplified Chinese. Responses do not echo language, so requests for different languages on the same slot must be serialized. Unknown slots are rejected.

## Capability and labels

A capability starts with `[d1, slot, type, info...]`. Type `00` is Boolean and ends after the setting's `info`. Type `01` is a list and adds `count, option_info...`, with `1..=64` option records.

Each `info` record is:

```text
format:u8 title_length:u8 title_bytes... summary_length:u8 summary_bytes...
```

Both lengths are byte counts in `0..=128`. Empty text is valid. Decoding retains raw title and summary bytes; explicit UTF-8 accessors report invalid encoding without replacing or truncating data. The decoder checks every nested length and rejects incomplete records, inconsistent counts and trailing bytes.

Format `00` (`RAW_NAME`) means display text. Format `01` (`ENUM_NAME`) means a stable identifier for caller-side translation. The format applies to both title and summary. Unknown formats and their labels remain readable. Options retain wire order and duplicates because their zero-based positions are the parameter indices; the library does not sort or deduplicate them.

`GeneralCapability::feature` recognizes these exact setting titles only when the format is `ENUM_NAME`:

| Identifier | Feature |
| --- | --- |
| `SIDETONE_SETTING` | Sidetone |
| `MULTIPOINT_SETTING` | Multipoint |
| `TOUCH_PANEL_SETTING` | Touch-panel controls |

Feature recognition uses neither the summary, translated display text nor the slot number. Recognition does not change the advertised Boolean/list layout. Unknown settings are readable but have no typed write operation in this module.

## Availability and parameter values

Availability is `00` available or `01` unavailable. It is independent of the configured on/off value. Parameters are exactly four bytes including the header:

| Type | Value |
| --- | --- |
| `00` Boolean | `00` on, `01` off |
| `01` List | An index in `0..=63`, additionally bounded by the advertised list |

Other type bytes, Boolean values or out-of-domain indices are rejected. Response and notification decoders work without a capability cache. Use `validate_capability` to check slot, layout and option membership across messages; this validation also works for readable settings with unrecognized feature names.

## Writing a recognized setting

```rust
use libmdr2::{
    general::{GeneralCapability, GeneralFeature, GeneralParameters, GeneralValue},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
// Synthetic Boolean sidetone descriptor in slot 4; its summary is empty.
let capability = GeneralCapability::decode_response(
    table, b"\xd1\xd4\x00\x01\x10SIDETONE_SETTING\x00",
)?;
assert_eq!(capability.feature(), Some(GeneralFeature::Sidetone));
let desired = GeneralParameters::new(capability.kind(), GeneralValue::Boolean(true))?;
assert_eq!(desired.encode_set(&capability)?, [0xd8, 0xd4, 0, 0]);
# Ok::<(), libmdr2::command::CommandError>(())
```

`encode_set` requires a recognized local feature, matching slot and matching value layout. Lists must use an existing capability index; values are never clamped. Unknown identifiers, service-specific titles and `RAW_NAME` labels cannot authorize writes, even if their text resembles a recognized identifier.

Submit the payload on table 1 after checking current availability. The caller tracks descriptor freshness, associates descriptors with the current connection, and handles device confirmations and operation timeouts.

A link ACK confirms frame delivery only. Decode parameter notifications or perform a serialized readback to reconcile state; notifications are not substitutes for matching query responses.
