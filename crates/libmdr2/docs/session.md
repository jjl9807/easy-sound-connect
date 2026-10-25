# Reliable MDR v2 sessions

## Sequence and acknowledgement rules

T1 `0c` and T2 `0e` use a shared stop-and-wait transmit sequence. The initial transmit sequence is zero; the initial receive history is empty. For a reliable frame with sequence `s`, its ACK has type `01`, an empty payload and sequence `1 - s`. Only a matching ACK advances the next transmit sequence.

The receive direction has its own history. Reliable repeated frames are ACKed again, because a previous ACK may have been lost, but are normally delivered only once. A structurally valid T1 protocol-info response is an exception: it is delivered even when its sequence repeats. A malformed payload beginning with the same opcode does not qualify for this exception.

## Sending state transitions

| State | Input | Effect / next state |
| --- | --- | --- |
| Idle | `send(table, payload, now)` | Queue one data frame; emit `Transmit` if no write/ACK is ahead |
| Queued first send | ACK with the expected sequence | Ignore it: this frame has not been written |
| Writing data | Matching ACK | Remember it; remain writing until the token completes |
| Writing data | `complete_write(token, now)` | Emit `LinkAcknowledged` if early ACK exists; otherwise start ACK deadline |
| Awaiting ACK | Matching ACK | Emit `LinkAcknowledged`, toggle TX sequence and become idle |
| Awaiting ACK | `advance(now)` reaches deadline | Queue one same-sequence retry, or close if retry budget exhausted |
| Queued retry | Matching ACK | Cancel unsent retry and acknowledge the original data |
| Writing any frame | `fail_write(token, now)` | Close the session, including when an early ACK was seen |
| Any active state | `cancel(now)` | Clear work, emit `Closed`, require a new connection and session |

Incoming reliable data continues to produce `Received` and queued ACKs while outgoing data is writing or waiting. The state machine never requests two overlapping transport writes. Pending ACKs take precedence over a data write that has not started; they do not interrupt a partially written frame.

## Effect handling and ordering

`SessionEvent::Transmit` carries a full logical frame and an opaque `WriteToken`. Encode it, split it only as required by the transport, write all parts in order, then call `complete_write`. Report a partial-write failure through `fail_write`, then close the underlying transport. `fail_write` terminates the protocol session; the caller closes the transport. ACK writes use the same completion mechanism and ordering.

Each method returns all effects caused by that input. Process every returned effect, including those returned by `complete_write`, which may schedule an ACK or the next queued data frame. Discarding such effects can stall the session. Do not execute a new write concurrently merely because a `Received` event or `LinkAcknowledged` was also returned.

Tokens distinguish attempts in the same session; an old token cannot complete a new retry. They are not globally unique across sessions. The caller must route callbacks to their originating connection generation. A new `Session` after reconnect does not make callbacks from the old connection safe to reuse.

## Time, retries and limits

All input timestamps are elapsed monotonic `Duration` values from one origin. Rejected inputs do not advance the stored clock. Going backward is an error. The implementation does not read time, sleep, poll or create tasks.

The default policy uses 750 ms and at most ten additional retries, for a maximum of eleven actual data writes. `RetryPolicy` makes both values configurable. The ACK timeout does not bound the time required for a command response.

An ACK deadline starts after each **complete** successful data write. The outgoing data frame has no ACK deadline while its write is unfinished; callers must enforce I/O timeouts. Writing an ACK for incoming data does not suspend an existing outgoing-data deadline. `advance` schedules at most one retry per input, even after a large time jump. The actual retry uses the same type, sequence and payload. If delayed behind an ACK write, it can still be cancelled by the original frame's late ACK.

Timers are event-driven: `advance` makes the timeout decision when the caller delivers that timer event. A received ACK processed before that decision can still complete the outstanding frame. Serialize clock, receive and write-completion inputs consistently rather than invoking the state machine concurrently.

At most one data frame and 32 queued ACK frames are retained, in addition to a currently executing write. Overflow closes the session instead of growing an unbounded queue. Unrepresentable deadline/token arithmetic also terminates with an explicit reason. Frame payload sizes are bounded by the framing policy; the session's caller must apply that policy to outgoing payloads before writing.

## Example: an ACK is not a command response

```rust
use std::time::Duration;
use libmdr2::{
    frame::{Frame, Sequence},
    session::{Session, SessionEvent},
    types::CommandTable,
};

let mut session = Session::default();
let effects = session.send(CommandTable::TABLE_1, vec![0x00, 0x00], Duration::ZERO)?;
let [SessionEvent::Transmit { token, .. }] = effects.as_slice() else {
    panic!("expected initial write");
};
let token = *token;
// The actual transport must write the frame before reporting this success.
assert!(session.complete_write(token, Duration::ZERO)?.is_empty());
assert_eq!(session.next_deadline(), Some(Duration::from_millis(750)));
let effects = session.receive(
    Frame::acknowledgement(Sequence::Zero),
    Duration::from_millis(10),
)?;
assert!(matches!(effects.as_slice(), [SessionEvent::LinkAcknowledged { .. }]));
// No ProtocolInfo response has arrived yet. Its deadline belongs to the caller.
# Ok::<(), libmdr2::session::SessionError>(())
```

## Response confirmation and failure recovery

The session does not hold `ExpectedResponse` descriptors or command-response deadlines. A `Received` payload may be a notification, a valid response, an unknown command or a malformed command body. Route and validate it at the command layer before making state changes. A link ACK confirms frame delivery only; it does not confirm that a setting took effect.

A response timeout does not undo a command. After a timeout or reconnect, establish the current device state before deciding whether to retry a setting. If a late reply cannot be distinguished from a new reply of the same family, re-establish known state or reconnect before treating a subsequent operation as successful. One-bit ACK sequences also cannot distinguish arbitrarily delayed traffic from earlier exchanges that used the same sequence.

On `Closed`, terminate the underlying transport and fail outstanding higher-level waiters. Further session calls return `SessionError::Closed`. Unsupported session frame categories return typed errors. The caller decides whether such an error requires closing the transport.
