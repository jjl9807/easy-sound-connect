# Device messages and confirmations

ALERT messages use table 1. `AlertMessage` decodes notifications, retains unknown message identities and requires an explicit `AlertDecision` when encoding a supported reply. It performs no I/O and never automatically accepts a request.

| Operation | Payload |
| --- | --- |
| Enable fixed-message delivery | `94 00 00` |
| Report application foreground | `94 04 00` |
| Fixed/foreground message | `99 kind message action_type`, kind `00` or `04` |
| Flexible message | `99 06 message count items... action_type` |
| Answer a supported message | `98 kind message decision` |
| LE Audio notice query | `92 05` |
| LE Audio notice response/notification | `93/95 05 on_off` |
| Confirm LE Audio notice | `94 05 confirmation` |

All values above are hexadecimal. Flexible-message counts are unsigned bytes; their exact total length is `5 + count`. Unknown items, duplicates and order are retained. Fixed messages have exactly four bytes. Unsupported inquiry layouts, truncation, trailing bytes and invalid closed fields return `CommandError`.

## Reply contract

Action types are display-only confirmation (`00`), accept/reject (`01`) and positive confirmation with a reply (`02`). Decisions are reject (`00`) or accept (`01`). Display-only messages do not generate a reply. A positive-only message cannot be rejected through the encoder.

Replies are restricted to known messages for implemented local operations. Fixed message identities with reply support are:

| Identities | Meaning |
| --- | --- |
| `00`, `01` | Disconnection caused by a connection-priority or key-assignment change |
| `06`, `07` | Multipoint changes, including disabling LDAC |
| `08`, `2b` | Battery consumption from EQ/DSEE or other local sound functions |
| `09` | Duplicate playback assignment |
| `0a`, `0b`, `0c`, `0d` | Disable/enable the touch panel, with or without reconnection |
| `1c`, `2a`, `30`, `53` | Sound-quality priority, unavailable LDAC, a Classic connection change or an LDAC 990 warning |
| `59`, `5a`, `5b` | Enable automatic volume, including interaction with existing safe-volume settings |
| `5c`, `90` | EQ/BGM exclusivity or disabling BGM |
| `5e`, `5f`, `65`, `66` | Left/right playback assignment or an assignment enabling/disabling repeated taps |
| `70`, `71`, `72`, `73` | Multipoint with LDAC, quality priority or an LDAC 990 warning |
| `74`, `75`, `76`, `77` | Classic sound/connection-quality priority transitions, including from LE Audio |
| `86`, `87` | Enable a sound effect while stamina is already on, with or without lighting |

The last group confirms a sound-effect change; it does not expose a stamina setting. Likewise, automatic-volume alerts do not provide safe-volume setup. Other fixed identities remain readable but cannot be answered by this API.

Flexible-message reply support is restricted independently of its item list:

| Identity | Meaning |
| --- | --- |
| `00` | Battery consumption from three simultaneous settings |
| `01` | Features exclusive with multipoint |
| `07` | Battery-consumption notice requiring a confirmation reply |
| `08`, `09` | Features exclusive with BGM or EQ |
| `0b` | Battery consumption from activating all listed settings |
| `0d`, `11` | A connection change or change to LE Audio with listed restrictions |

Every listed item must also have reply support: EQ (`00`), DSEE (`01`), Speak-to-Chat (`02`), automatic volume (`03`), LDAC (`06`), sound-quality priority (`07`), multipoint (`0b`), BGM (`0d`), LE Audio (`10`), NC (`14`) or connection mode (`18`). An unsupported message stays read-only even when all its items are supported. An unknown or unsupported item makes an otherwise supported message read-only. GATT activation, pairing, Bluetooth standby, assistant and service-configuration messages do not gain reply support through the flexible layout. Original item bytes, order and duplicates are retained.

Reply support does not establish feature availability or identify the operation that caused a message. Associate the decoded snapshot with the current device and outstanding local operation before presenting a decision or encoding a reply.

Left/right messages (`99 02 message default_side`) are decoded for diagnostics; the known identities configure voice-assistant assignment. They have no reply API. `default_side` must be left (`00`) or right (`01`). Assistant, firmware and service configuration workflows are not implemented.

```rust
use libmdr2::{alert::{AlertDecision, AlertMessage}, types::CommandTable};

let message = AlertMessage::decode_notification(
    CommandTable::TABLE_1, &[0x99, 0x00, 0x07, 0x01],
)?;
// Encode only after the application has obtained this explicit decision.
let payload = message.encode_reply(AlertDecision::Reject)?;
assert_eq!(payload, [0x98, 0x00, 0x07, 0x00]);
# Ok::<(), libmdr2::command::CommandError>(())
```

A reply uses the inquiry and message identity from the decoded snapshot. It has no general transaction ID. The caller owns prompt lifetime, connection generation, timeouts and operation correlation. A reply may permit a setting to change or a connection to close; neither its link ACK nor a disconnection proves the original setting succeeded. Reconcile device state through the relevant feature reports.

## LE Audio notices

`LeAudioAlertStatus` reports an inverted on/off flag (`00` on, `01` off). `Request::LeAudioAlertStatus` selects the `93 05` response; a `95 05` notification cannot satisfy that query. Discover function `94` before using this flow.

`LeAudioConfirmation` encodes `00` for confirmation or `01` for confirmation with the request not to show the notice again. The latter requests a change to device notice behavior and must reflect the caller's explicit choice. No firmware, language package or assistant notice flow is provided by these operations.
