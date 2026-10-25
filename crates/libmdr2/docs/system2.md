# Table-2 system settings

These commands use command table 2. Select `System2Kind` from an advertised `FunctionId`; the function code and command table are both significant. `SYSTEM` opcodes overlap table-1 system and assignment commands.

| Function (T2) | Inquiry | Layout | Capability | Status | Parameters |
| --- | --- | --- | --- | --- | --- |
| `f0` | `00` | Wearing status | — | yes | — |
| `f3` | `03` | Automatic volume optimizer | fixed | yes | yes |
| `f4` | `04` | Automatic volume with limitations | fixed | yes | yes |
| `f9` | `0b` | Microphone control by headphone operation | query | yes | yes |

The microphone layout requires function `(2, f9)`. The inquiry `0b` alone is insufficient to select it: it also occurs in a different input-source layout. No input-source switching, assistant, Quick Access or speaker-link control is provided by this module.

## Commands and fields

| Operation | Request | Response / notification |
| --- | --- | --- |
| Microphone capability | `f0 0b` | `f1 0b key` |
| Status | `f2 inquiry` | `f3/f5 inquiry status` |
| Automatic-volume parameters | `f6 03` | `f7/f9 03 on_off` |
| Limited automatic-volume parameters | `f6 04` | `f7/f9 04 on_off count limitation[count]` |
| Microphone parameters | `f6 0b` | `f7/f9 0b control_on_off mute_sound_on_off` |
| Set automatic volume | `f8 03/04 on_off` | Reconcile through parameters |
| Set microphone controls | `f8 0b control_on_off mute_sound_on_off` | Reconcile through parameters |

`key` is `00` touch sensor, `01` button or `02` face tap. Each `on_off` is `00` on or `01` off; other values are rejected. Status uses `00` available / `01` unavailable for writable layouts, independently of their configured switch. For inquiry `00`, status instead describes wearing: `00` normal, `01` invalid position, `02` left not worn, `03` right not worn, `04` neither worn.

The limitation list is read-only. Identifier `00` indicates a safe-volume limitation; unknown identifiers and their order are retained. The count is an unsigned byte, and exact payload length is required. A `SET` for inquiry `04` contains only the on/off value, never the limitation count or list.

## API use

```rust
use libmdr2::{
    system2::{System2Capability, System2Kind, System2Parameters, System2Setting},
    types::CommandTable,
};

let capability = System2Capability::fixed(System2Kind::AutoVolumeWithLimitations)?;
let current = System2Parameters::decode_response(
    CommandTable::TABLE_2, &[0xf7, 0x04, 0x00, 0x01, 0x00],
)?;
assert_eq!(current.kind(), capability.kind());
let payload = System2Setting::AutoVolume { enabled: false }.encode_set(&capability)?;
assert_eq!(payload, [0xf8, 0x04, 0x01]);
# Ok::<(), libmdr2::command::CommandError>(())
```

Obtain the microphone-operation capability from `System2Capability::decode_response`. When changing either microphone switch, preserve the other value from the latest parameter report; the wire format writes both together. The caller must check current availability and retain capability/state per connection. A link ACK confirms frame delivery only; read parameters or observe parameter notifications to confirm the resulting setting. The wearing report does not change automatic-pause preferences or imply Bluetooth connection state.
