# Physical-key and touch-action assignments

Assignments use table 1 and two capability-selected layouts. Function `f3` selects inquiry `03`; function `fe` selects inquiry `0e`. The latter adds a limitation identity to the capability header. No model name or fixed number of keys is used.

## Commands

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capabilities | `f0 tt` | `f1 tt ...` | — |
| Per-key availability | `f2 tt` | `f3 tt ...` | `f5 tt ...` |
| Current presets | `f6 tt` | `f7 tt ...` | `f9 tt ...` |
| Current custom maps | `fa tt` | `fb tt ...` | `fd tt ...` |
| Replace preset list | `f8 tt ...` | No dedicated RET | State reports use `f9` |
| Replace custom maps | `fc tt ...` | No dedicated RET | State reports use `fd` |

`tt` is `03` or `0e`. Identity values below are hexadecimal. Count fields are unsigned bytes; stated lengths are decimal byte counts. Every decoder checks the table, opcode, inquiry and complete length. Notifications do not satisfy read queries.

## Nested capability layout

Inquiry `03` starts with `[f1, 03, key_count]`; `0e` starts with `[f1, 0e, limitation, key_count]`. Each key then contains:

```text
key, key_type, default_preset, preset_count
  preset, fixed_count, customizable_count
    (action, function) * fixed_count
    (action, default_function, function_count, functions...) * customizable_count
```

The preset record repeats `preset_count` times for each key. Key, preset and function counts are nonzero. A preset must contain at least one fixed or customizable action; either count may individually be zero. Counts are bounded by their one-byte representation, and bytes are checked before each nested read.

Keys `00..05` identify left, right, custom, C, NC/AMB and NC/AMBIENT controls. Key types are `00` touch sensor, `01` button and `02` face tap. Limitation `00` adds no description; `01/02` describe assistant-preset/action restrictions during LE Audio. The library preserves unknown identities, list order and duplicates when reading. Writes require an unambiguous target and a known limitation.

Capability defaults are metadata. They must not replace the device's current presets or custom action maps.

## Current state

Availability is `[f3|f5, tt, count, availability...]`, where `00` is enabled and `01` is disabled. Presets are `[f7|f9, tt, count, preset...]`. Both lists follow capability key order and must have the same key count when associated with a capability. The decoder retains unknown preset identities.

Extended parameters are:

```text
fb|fd, tt, record_count,
  preset, binding_count, (action, function) * binding_count
```

Every record and binding count is nonzero. Extended records are identified by **preset**, not by a physical key or key-list position. Their order is retained. To modify one key's current map, the preset must occur on exactly one key in the current preset list and exactly one extended record. Ambiguous associations are rejected rather than assigning a record to the first matching key.

## Editing local assignments

The write API encodes a command from a decoded snapshot with one assignment changed. The snapshot itself is unchanged. The API does not expose a setter that replaces an arbitrary preset list or custom map.

- `AssignmentParameters::encode_preset_change` encodes a change to one key and retains every other preset byte. The target preset must be advertised and known to represent local controls. All functions newly activated by the preset must be local.
- A preset with customizable actions requires a decoded custom snapshot with its complete map. Default capability functions are not a substitute for that state.
- `AssignmentCustomParameters::encode_function_change` encodes a change to one existing, customizable action of the current preset. It requires a known action and a local function advertised for that exact key, preset and action.
- Unrelated unknown or service presets, functions and records are retained with their original order. They cannot be newly selected or assigned through these methods. Fixed actions cannot be replaced by the custom writer.

Local functions include NC/ambient cycling, quick attention, playback, track and volume controls, local listening-mode controls, microphone mute and game/chat balance. Phone assistants, Quick Access services, unsupported NC test workflows and unknown functions cannot be newly assigned. A preset identity alone is not sufficient: for example, an advertised playback preset may contain an assistant binding and therefore fail the activation check.

```rust
use libmdr2::{
    assignment::{AssignmentCapability, AssignmentKey, AssignmentParameters, AssignmentPreset},
    types::CommandTable,
};

let table = CommandTable::TABLE_1;
// One button with two fixed, local presets: NC/ambient and playback.
let capability = AssignmentCapability::decode_response(table, &[
    0xf1, 3, 1, 2, 1, 0, 2,
    0, 1, 0, 0, 2,
    0x20, 1, 0, 0, 0x20,
])?;
let current = AssignmentParameters::decode_response(table, &[0xf7, 3, 1, 0])?;
let payload = current.encode_preset_change(
    &capability, AssignmentKey::CUSTOM, AssignmentPreset::PLAYBACK, None,
)?;
assert_eq!(payload, [0xf8, 3, 1, 0x20]);
# Ok::<(), libmdr2::command::CommandError>(())
```

Returned bytes are table-1 command payloads. The caller associates snapshots with the same device connection and capability revision, checks current per-key availability, and serializes changes.

A link ACK confirms frame delivery only. Reconcile notifications or perform a subsequent readback; neither report carries a transaction identifier. Refresh stale or inconsistent snapshots instead of substituting defaults or dropping unrelated fields.
