# Link requests and host LE Audio state

The `connection` module decodes device requests to reconnect or release the GATT control link and encodes host LE Audio state reports. The caller decides how to handle these requests and manages the transport.

| Direction | Table | Function | Payload |
| --- | --- | --- | --- |
| Device to caller | T1 | `1a` | `19 08 phase address[17]` |
| Device to caller | T1 | `1c` | `19 0a` |
| Caller to device | T2 | `60` | `68 00 state address[17]` |

Byte values are hexadecimal. Addresses contain exactly 17 ASCII bytes in `XX:XX:XX:XX:XX:XX` notation. `BluetoothAddress` parses either hexadecimal case, retains the six address octets in display order and encodes uppercase notation. Malformed separators, nonhexadecimal characters and incorrect lengths are errors.

## Device requests

Reconnection phases are under confirmation (`00`) and execute (`01`); other phase values are rejected. `ConnectionNotice::Reconnect` retains both the phase and target address. The caller must associate the notice with its current physical connection and validate the target before acting. Under-confirmation is not an instruction to immediately reconnect. There is no typed reply to this notice.

`DisconnectGatt` has no body after its two-byte header. It requests release of the GATT control link and does not describe the state of the audio profile. On a transport change, the connection owner disposes of the old session, isolates late callbacks and performs fresh capability/state synchronization.

```rust
use libmdr2::{
    command::Notification,
    connection::ConnectionNotice,
    types::CommandTable,
};
assert_eq!(Notification::decode(CommandTable::TABLE_1, &[0x19, 0x0a])?,
    Notification::ConnectionNotice(ConnectionNotice::DisconnectGatt));
# Ok::<(), libmdr2::command::CommandError>(())
```

## Host profile state reports

`LeAudioStateReport` sends the host's observed profile state: connecting (`00`), connected (`01`), disconnecting (`02`) or disconnected (`03`). The address is the accessory address associated with that observation. This is an outgoing message, not a query or a device-to-host notification. A control-channel connection does not establish that the LE Audio profile is connected.

Discover T2 function `60` and obtain a reliable platform observation before sending. The payload belongs to table 2 even though its opcode also appears in table-1 noise control. Its link ACK confirms frame delivery only. The message expects no RET response and has no `ExpectedResponse` descriptor. The library neither implements an audio profile nor changes the host audio route.
