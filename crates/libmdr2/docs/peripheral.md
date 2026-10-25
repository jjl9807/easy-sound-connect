# Audio sources and connection control

The PERIPHERAL command family reports audio-source connections and encodes connection operations and source preferences on command table 2. Select a layout from the device's advertised function list.

| Advertised function | Inquiry | Layout |
| --- | --- | --- |
| `30` | `00` | Device list |
| `31` | `01` | Source switching and keep-source preference |
| `32` or `33` | `02` | Device list with Bluetooth class of device |
| `34` | `03` | Music handover preference |

Functions `32` and `33` share inquiry `02`; retain the discovered function ID separately when that distinction matters. These are table-2 function IDs and must not be matched against the same numeric codes in another table.

## Capability and availability

Device-list inquiries `00` and `02` expose these exchanges:

| Exchange | Payload |
| --- | --- |
| Capability GET / RET | `30 inquiry` / `31 inquiry maxPaired maxConnected transfer` |
| Status GET / RET / NTFY | `32 inquiry` / `33 inquiry mode enabled` / `35 inquiry mode enabled` |

`maxPaired` and `maxConnected` are unsigned advertised capacities, separate from the current number of records or connections. Their values do not constrain the raw connection-slot field. `transfer` is `00` for possible and `01` for impossible; it is read-only metadata, with no file-transfer API.

`mode` is `00` for normal or `01` for pairing. The availability field `enabled` is `00` for available or `01` for unavailable. Other flag values are rejected.

Inquiries `01` and `03` have fixed parameter layouts and no capability or status query. Construct their descriptor with `PeripheralCapability::fixed` after the corresponding function has been advertised. Constructing a fixed descriptor does not verify that the device supports the function.

## Device lists

All four inquiries accept parameter GET `36 inquiry`; RET uses opcode `37` and NTFY uses `39`. A device-list response or notification contains:

```text
opcode inquiry count record[count] playbackRight
00 record: address[17] connectionSlot nameLength name[nameLength]
02 record: address[17] connectionSlot classOfDevice[3] nameLength name[nameLength]
```

Addresses are 17 ASCII bytes in colon-separated hexadecimal form. Parsing normalizes case into a `BluetoothAddress`; encoding emits uppercase hexadecimal. `classOfDevice` is an unsigned 24-bit big-endian value. Friendly names contain 1–128 bytes. `name_bytes` preserves them; `name_utf8` explicitly validates UTF-8.

Record count is an unsigned byte. Decoders require all declared records and exactly one trailing `playbackRight` byte. Empty lists are valid. Record order, duplicate addresses and positive connection-slot values are retained. Zero connection slot means disconnected; a positive value means connected. No stronger slot interpretation is applied. `playback_right_raw` exposes the final byte without interpreting it as a vector index or a source address.

## Source preferences

| Inquiry | RET | NTFY | SET |
| --- | --- | --- | --- |
| Source keep `01` | `37 01 onOff` | `39 01 onOff result` | `38 01 onOff` |
| Music handover `03` | `37 03 onOff` | `39 03 onOff` | `38 03 onOff` |

`onOff` is `00` for on and `01` for off. The library exposes that wire meaning; it does not invert values to match a particular application label. A keep-source notification has one more byte than its response. The two forms cannot be decoded by replacing only their opcode. Its result uses the source-switch result identities below, but contains no target address and is not a targeted `3d` operation result.

## Targeted operations and results

`PeripheralOperation::encode` validates a target against a supplied device list and produces a command payload and an `ExpectedPeripheralResult`. The target must occur exactly once in the list, after address normalization. Connect and disconnect require the list's matching capability layout. Switching requires the source-switch descriptor and a target reported as connected in either list layout. The caller must use a current list and capability from the same device connection and check current availability separately.

| Operation | SET | Result NTFY |
| --- | --- | --- |
| Disconnect | `3c 00/02 00 address[17]` | `3d 00/02 00 result address[17]` |
| Connect | `3c 00/02 01 address[17]` | `3d 00/02 01 result address[17]` |
| Switch source | `3c 01 address[17]` | `3d 01 result address[17]` |

Connection results `00`–`03`, `10`–`13` and `20`–`23` describe disconnection, connection and unpairing respectively. The low nibble is `0` success, `1` error, `2` in progress or `3` busy. The high nibble must match the action before the library classifies the result. Unpairing reports are readable, but this module exposes no unpair command. Pairing-result bytes `30`–`33` and unknown action/result identities remain available as raw values; they do not classify as success for a supported targeted action. No API starts device pairing or implements host Bluetooth pairing services.

Source-switch results are `00` success, `01` failure, `02` a call preventing the switch, `03` an A2DP source that is not connected, and `04` voice-assistant priority preventing the switch. Unknown values are preserved and classify as `OperationOutcome::Unknown`.

Register the expected result before sending the command. `matches` checks the inquiry, target address and connection action; inspect `outcome` separately. In-progress and busy reports are not success. A link ACK confirms frame delivery only; it does not confirm a connection or preference change.

The protocol has no unique operation identifier in these reports: identical repeated operations have identical keys. Serialize them and refresh device state after ambiguous timeouts instead of assigning an old result to a new operation. Reconcile the device-list notification or a subsequent query with the result before presenting final connection state.

```rust
use libmdr2::{
    peripheral::{PeripheralCapability, PeripheralOperation, PeripheralParameters,
                 PeripheralResultReport, OperationOutcome},
    types::{BluetoothAddress, CommandTable},
};

let table = CommandTable::TABLE_2;
let capability = PeripheralCapability::decode_response(table, &[0x31, 0, 8, 2, 0])?;
// Synthetic source address and friendly name, initially disconnected.
let report = b"\x37\x00\x0102:00:00:00:00:01\x00\x03PC1\x00";
let PeripheralParameters::Devices(list) = PeripheralParameters::decode_response(table, report)?
    else { unreachable!() };
let address = BluetoothAddress::parse_ascii(b"02:00:00:00:00:01")?;
let command = PeripheralOperation::Connect(address).encode(&capability, &list)?;
assert_eq!(command.payload, b"\x3c\x00\x0102:00:00:00:00:01");

let progress = PeripheralResultReport::decode_notification(
    table, b"\x3d\x00\x01\x1202:00:00:00:00:01",
)?;
assert!(command.expected_result.matches(&progress));
assert_eq!(progress.outcome(), OperationOutcome::InProgress);
# Ok::<(), libmdr2::command::CommandError>(())
```

Every decoder checks the command table, opcode, inquiry and exact payload length before exposing fields. Unknown result codes and descriptive metadata are preserved; unsupported command layouts and invalid closed flags are errors. The caller sends encoded commands through its transport and processes subsequent results and state reports.
