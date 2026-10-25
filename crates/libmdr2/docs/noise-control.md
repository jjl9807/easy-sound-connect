# Noise cancellation and ambient sound

The `noise` module implements table-1 NCASM capability, availability, and parameter messages. Select the layout with `NoiseKind::from_function` using the negotiated supported-function list. A product name does not identify a layout, and multiple advertised functions do not imply a priority order.

## Commands

All payloads use command table 1 (`DataType::DATA_MDR`, `0c`). An inquiry is one byte, denoted `tt` below. Payload lengths exclude transport framing.

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `60 tt` | `61 tt ...` | None |
| Availability | `62 tt` | `63 tt availability` | `65 tt availability` |
| Read parameters | `66 tt` | `67 tt ...` | `69 tt ...` |
| Write parameters | `68 tt ...` | No dedicated SET response | State may be reported by `69` |

`NoiseAvailability` uses `00` for available and `01` for unavailable. `NoiseSwitch` uses `00` off and `01` on. Availability is independent of the configured effect: a control can be available while its total effect is switched off.

`Request::NoiseCapability`, `Request::NoiseStatus`, and `Request::NoiseParameters` create typed read descriptors. Each descriptor matches the expected table, opcode, and inquiry. Notifications are decoded separately, so a notification cannot satisfy a pending read.

## Layout selection

| `NoiseKind` | Inquiry | Supported function | Capability body |
| --- | --- | --- | --- |
| `NcOnly` | `01` | `61` | Fixed descriptor; capability query unsupported |
| `NcModesAndAmbientSwitch` | `12` | `63` | Ambient-mode list |
| `NcAndAmbientLevel` | `13` | `64` | Ambient modes with level ranges |
| `NcModesAndAmbientLevel` | `14` | `65` | Ambient modes with level ranges |
| `AutoNcAndAmbientLevel` | `15` | `68` | Ambient modes with level ranges |
| `DualSingleNcAndAmbientLevel` | `16` | `6a` | Ambient modes with level ranges |
| `DualNcAndAmbientLevel` | `17` | `6b` | Ambient modes with level ranges |
| `AdaptiveNcAndAmbientLevel` | `19` | `6d` | Ambient modes with level ranges |
| `AmbientLevel` | `22` | `67` | Ambient modes with level ranges |
| `ButtonCycle` | `30` | `69` | One physical-key byte |

Function identities include their command table. The same numeric function on table 2 does not select any of these layouts. Inquiries `11`, `18`, `21`, and `40` are not implemented; unknown inquiries return an error rather than being interpreted as a similar layout. The module does not implement NC test mode, NCSS, or phone-managed activity/location automation.

For NC-only control, use `NoiseCapability::nc_only()` after discovering function `61`. `NoiseKind::NcOnly.has_capability_query()` is false and its typed capability query is rejected. The decoder accepts the fieldless response `61 01` for callers decoding existing payloads.

## Capability bodies

The ambient-mode list is `count {mode}*count`. A list with level ranges is `count {mode minimum maximum step}*count`. Counts and levels are unsigned bytes. Both layouts require their exact count-derived length; trailing data is not ignored. Ambient modes are `00` normal and `01` voice focus.

`AmbientLevelRange` requires a minimum in `0..=254`, a maximum in `1..=255`, a nonzero step, and `minimum <= maximum`. Permitted levels are regular increments from the minimum, plus the maximum as a terminal value. For example, minimum `1`, maximum `6`, step `2` permits `1`, `3`, `5`, and `6`. A single-valued range is valid when its minimum equals its maximum.

The decoder retains the order of entries, including empty lists and duplicates. A write must resolve its selected ambient mode to exactly one entry. An empty list cannot authorize an ambient setting, and duplicate entries for the selected mode are rejected as ambiguous. The library does not invent a range when a mode is absent.

For button cycling, the capability body is one `ButtonKey`: left `00`, right `01`, custom `02`, C `03`, NC/AMB `04`, or NC/AMBIENT `05`. This identifies a control; it does not contain an additional list of allowed cycle values.

## Parameter fields

All control layouts except button cycling start with:

| Offset | Field | Values |
| --- | --- | --- |
| 0 | Opcode | `67` response, `68` write, `69` notification |
| 1 | Inquiry | The selected layout |
| 2 | Change status | `00` changing, `01` changed |
| 3 | Total effect | `00` off, `01` on |

The remaining fields are:

| Inquiry | Fields from offset 4 | Total length |
| --- | --- | --- |
| `01` | NC switch | 5 |
| `12` | NC value, ambient mode, ambient switch | 7 |
| `13` | NC switch, ambient mode, ambient level | 7 |
| `14` | NC value, ambient mode, ambient level | 7 |
| `15` | effect mode, NC value, ambient mode, ambient level | 8 |
| `16` | effect mode, NC value, ambient mode, ambient level | 8 |
| `17` | effect mode, ambient mode, ambient level | 7 |
| `19` | effect mode, ambient mode, ambient level, adaptation switch, sensitivity | 9 |
| `22` | ambient mode, ambient level | 6 |

The effect mode is `00` NC or `01` ambient sound. These fields remain present while the total effect is off. Inquiries `15`, `16`, `17`, and `19` therefore select NC when total effect is on and mode is `00`, ambient when it is on and mode is `01`, and neither when total effect is off.

The shared NC-value field recognizes `00` off, `01` single, `02` dual, `03` auto, `04` auto single, and `05` auto dual. These values are preserved without collapsing the automatic states. Capability messages do not provide a separate enumeration of permitted NC values; encoding validates the layout, typed NC value, advertised ambient mode, and ambient range. Choosing which NC choices to expose in a product remains a caller policy.

Inquiries `13` and `14` have no separate effect-mode field. Ambient level `00` represents inactive ambient sound when inquiry `13` has NC on, or inquiry `14` has single or dual NC selected. This value is valid for those NC states even if the advertised ambient range starts above zero. A write still requires the ambient processing mode to be advertised. The exception does not permit arbitrary out-of-range levels and does not apply to layouts with a separate effect-mode field.

For inquiry `19`, adaptation uses `NoiseSwitch`: `00` off, `01` on. Sensitivity is `00` standard, `01` high, or `02` low. These fields describe the device's noise-adaptation setting and do not require phone-managed activity or location tracking.

Inquiry `30` has the independent three-byte layout `opcode 30 cycle`. There is no change-status or total-effect field. `ButtonCycle` values are:

| Value | Included effects |
| --- | --- |
| `01` | NC, ambient sound, off |
| `02` | NC, ambient sound |
| `03` | NC, off |
| `04` | Ambient sound, off |

## Encoding changes

Decode the current parameters before changing a field. This retains values that remain meaningful when their effect is temporarily inactive, including ambient level and adaptation sensitivity. `encode_set` never clears, normalizes, or clamps another field.

```rust
use libmdr2::{
    command::CommandError,
    noise::{NoiseCapability, NoiseParameters, NoiseSwitch},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
let capability = NoiseCapability::decode_response(
    table,
    &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1],
)?;
let mut parameters = NoiseParameters::decode_response(
    table,
    &[0x67, 0x19, 1, 1, 1, 1, 20, 0, 2],
)?;
if let NoiseParameters::Control { effect, .. } = &mut parameters {
    *effect = NoiseSwitch::Off;
}
let payload = parameters.encode_set(&capability)?;
assert_eq!(payload, [0x68, 0x19, 1, 0, 1, 1, 20, 0, 2]);
# Ok::<(), CommandError>(())
```

Response and notification decoders validate table, opcode, layout, exact length, and closed enum fields. They accept the full unsigned byte domain for an ambient level without requiring a previously obtained capability. This allows reported state to be inspected even if capability information is unavailable or has changed.

Writes additionally require a capability of the exact same inquiry, an unambiguous advertised ambient mode, and a permitted level, including the inactive-zero rule for inquiries `13` and `14` described above. These checks also apply to stored values while total effect is off. Out-of-range values return `CommandError::InvalidParameter`; mismatched layouts return `CommandError::CapabilityMismatch`. Obtain fresh capability and parameter data when they disagree, rather than substituting a model-specific default.

`encode_set` returns payload bytes only. Callers select the table-1 frame, check current availability, schedule transmission, and decide how to observe the result. A link ACK confirms frame delivery only; it does not confirm that the device applied the setting. A parameter notification reports device state but carries no transaction identifier. A subsequent parameter query provides a readback through the normal response matcher; it must obey the same serialization and late-response policy as other reads.
