# MDR v2 framing

## Unescaped layout

Each frame starts with `3e` and ends with `3c`. Offsets below refer to the decoded body, excluding the delimiters and escape bytes.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | Data type |
| 1 | 1 | Sequence, `00` or `01` |
| 2 | 4 | Payload length, unsigned big-endian |
| 6 | `length` | Payload bytes |
| `6 + length` | 1 | Sum8 checksum |

The body length must be exactly `7 + payload length`. The checksum is the sum of all preceding unescaped body bytes, modulo 256. It excludes the delimiters and the checksum byte itself. Sum8 is an additive checksum, not a polynomial CRC.

| Data type | Meaning | Reliable-session support |
| --- | --- | --- |
| `01` | ACK | Supported; payload must be empty |
| `0c` | MDR v2 table 1 | Supported, requires ACK |
| `0e` | MDR v2 table 2 | Supported, requires ACK |
| `1c` / `1e` | SHOT table 1 / table 2 | Preserved by codec, rejected by session |
| Other byte | Uninterpreted frame category | Preserved by codec, rejected by session |

The codec can decode and re-encode well-formed frames with unknown data types. Reliability and command handling remain separate from framing. All data types use the codec's one-bit sequence validation.

## Escaping

Compute the checksum before escaping. Escape every reserved byte in the body, including type, length and checksum; delimiters themselves remain unescaped.

| Body byte | Encoded pair |
| --- | --- |
| `3c` | `3d 2c` |
| `3d` | `3d 2d` |
| `3e` | `3d 2e` |

Any other byte following `3d` produces an invalid-escape error.

## Examples

The protocol-info query `00 00` on T1 with sequence zero is:

```text
3e 0c 00 00 00 00 02 00 00 0e 3c
   type seq  length    data sum
```

Its checksum is `0c + 02 = 0e`. ACK sequence one, confirming data sequence zero, is `3e 01 01 00 00 00 00 02 3c`; ACK sequence zero is `3e 01 00 00 00 00 00 01 3c`.

For payload `3c 3d 3e`, the full encoded frame is:

```text
3e 0c 00 00 00 00 03 3d 2c 3d 2d 3d 2e c6 3c
```

The checksum uses the original payload bytes: `0c + 03 + 3c + 3d + 3e = c6`. Checksum bytes equal to a delimiter/escape are themselves escaped.

## Incremental decoding and recovery

`Decoder::push` consumes one byte and returns at most one frame or error. `Decoder::feed` exposes the same behavior as a lazy iterator over a fragment. Consume it fully to process every byte; dropping it early leaves remaining bytes unconsumed. Neither method retains an unbounded event queue.

1. Outside a frame, ignore bytes until a start delimiter arrives.
2. Inside a frame, decode only canonical escape pairs and validate the sequence.
3. Once all six header bytes arrive, validate the declared payload length and ACK shape, without allocating based solely on the declaration.
4. At the end delimiter, require the exact body length and correct checksum.
5. On error, discard the partial frame and seek a new start delimiter. If an unexpected start caused the error, that same delimiter starts a replacement.

A stream may split an escape pair or header across arbitrary fragments, or deliver several frames together. `finish()` is for the end of the stream: it reports an unfinished frame and resets the decoder. Calling it between reads or GATT notifications would incorrectly reject ordinary fragmentation.

```rust
use libmdr2::frame::{Decoder, Frame, Sequence};

let mut decoder = Decoder::new(512)?;
let first = [0x3e, 0x01, 0x01, 0x00];
assert_eq!(decoder.feed(&first).count(), 0);
let rest = [0x00, 0x00, 0x00, 0x02, 0x3c];
let frame = decoder.feed(&rest).next().unwrap()?;
assert_eq!(frame, Frame::acknowledgement(Sequence::Zero));
decoder.finish()?;
# Ok::<(), libmdr2::frame::FrameError>(())
```

## Limits and invalid input

The caller supplies a maximum payload for both encoding and decoding. This is an application limit, not a universal device limit. It must fit the wire `u32` field and the local body-length arithmetic. A zero-byte payload limit permits only empty payloads, including ACK frames. Stored body bytes never exceed the payload limit plus seven; allocator capacity may exceed the current content length. Process decoder results incrementally to keep caller-owned buffering bounded.

`Frame::encode_into` appends a full frame and leaves its destination unchanged on validation error. It does not fragment writes or assign the next session sequence.

The following frame has a valid checksum but an invalid length:

```text
3e 0c 00 00 00 00 05 07 18 3c
```

It declares five payload bytes but contains one. It is rejected as `LengthMismatch`; `18` is the checksum and cannot be treated as payload.

Frame errors do not themselves close a physical connection. The caller decides whether to continue after resynchronization.
