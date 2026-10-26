# core

Device sessions and shared state for easy-sound-connect. The Cargo package is named `core`; its Rust library is named `ezsc_core` to avoid shadowing Rust's standard `core` crate. It depends on the platform transport contract in `bluetooth` and the platform-independent MDR v2 codecs and session state machine in `libmdr2`.

## Connecting and observing state

Run the manager on a Tokio runtime. A successful `connect` returns a handle in `ConnectionState::Connected`; subscribe before calling `initialize` to observe negotiation and state synchronization. Watch receivers contain the latest snapshot and may coalesce intermediate updates.

```rust,no_run
use std::sync::Arc;
use bluetooth::{Backend, DeviceId, TransportKind};
use ezsc_core::{DeviceManager, SessionConfig};

async fn read_device(backend: Arc<dyn Backend>, id: DeviceId) -> ezsc_core::Result<()> {
    let manager = DeviceManager::new(backend);
    let device = manager.connect(id, TransportKind::Gatt, SessionConfig::default()).await?;
    let _state = device.subscribe();
    let initialized = device.initialize().await;
    println!("{:?}", device.snapshot());
    manager.shutdown().await?;
    initialized?;
    Ok(())
}
```

Initialization reads protocol information, a caller-selected capability layout, supported function tables, model name, firmware version, and advertised battery endpoints. `SessionConfig::capability_query` defaults to the standard two-byte capability request; callers can explicitly select `WithAppType`. The session does not infer Android compatibility rules from a model name, interpret a protocol version byte as an MDR generation, or retry alternative handshake layouts. Critical replies must match the strict `libmdr2` decoder. An unsupported optional model, firmware, or battery reply leaves that value unknown, records a diagnostic, and allows remaining initial reads to continue; an ambiguous response timeout still closes the session.

Snapshots retain raw protocol information, `(table, code, priority)` function identities, unknown functions, and the connection generation. `FunctionSupport` describes the advertised feature and implemented read/write paths. Live availability is carried by the feature's status; parameter freshness is separate. Battery values are optional readings for headset, left, right, and case components. Unknown values stay unknown, zero is a valid reading, and known values become stale when the connection closes.

`DeviceSnapshot::message_counters` distinguishes accepted matched read responses from successfully decoded notifications, with separate noise/EQ parameter notification totals. Counters reset for each connection generation, saturate at `u64::MAX`, and contain no payloads or device identifiers. ACKs, reliable duplicates, invalid messages, and unmatched response payloads do not increment these counters. These diagnostics can establish notification reception independently of explicit GET operations; they do not attribute which external controller caused a change or confirm a particular SET.

## Operations

`refresh_battery`, `refresh_noise`, and `refresh_equalizer` read advertised features. Noise and EQ details are queried on demand. `set_noise` edits newly read parameters while preserving unrelated fields, then checks capability and current availability. `set_equalizer` validates the supplied setting and selects and confirms an editable preset before writing a curve when necessary. Both setters finish with an explicit readback and return `Confirmed` or `Adjusted`; a link ACK alone never confirms a setting. Verification retains the matched read response separately from the latest snapshot, so a later notification can update displayed state without changing that operation's result. Unknown noncritical feature responses fail that operation while preserving the session and other known state.

Use `set_equalizer_preset(kind, preset)` for a preset selection that preserves the freshly read ULT mode. Use `set_equalizer_band(kind, expected_preset, index, value)` to replace one native value while preserving fresh ULT and every other band. The current preset must match `expected_preset`; pass `None` for standalone custom EQ. A preset curve must already be in an advertised editable slot, and values are validated without clamping. The full-setting API intentionally replaces every supplied value and is appropriate when the caller owns the complete target state. The `equalizer` and `noise` modules are re-exported through `ezsc_core` so views can consume these typed values without owning a protocol session.

`refresh_equalizer_band_info(kind)` explicitly reads optional band metadata against a fresh capability. It is independent of regular parameter refresh and settings, and fills `EqualizerState::band_info` only when inquiry and band count agree. Capability changes that invalidate its count discard the old association. Preserve device ordering, identify Clear Bass by its metadata, and use generic band numbers when metadata is missing or unknown. Preset labels may be empty; the core retains their IDs and never invents device-supplied names. An unanswered metadata query follows the same response-timeout isolation rule as other GETs.

`NoiseChange::Adaptive` and `AdaptiveSensitivity` edit the adaptation fields of the advertised adaptive layout while preserving effect, selected mode, ambient settings, and the other adaptation field. Level, voice-focus, and adaptation edits do not implicitly enable an effect. `Mode` enables the selected effect only for layouts with supported unambiguous mode semantics; button-cycle editing and ambiguous mode selection in layouts without an explicit mode field remain unsupported.

Each device owns a bounded operation queue and one actor containing its decoder and protocol session. Complete frame writes remain ordered while reads, timers, and cancellation progress concurrently. Expected responses are registered before sending; notifications update snapshots independently and cannot satisfy a GET response. Configuration provides separate queue, operation, transport-write, response, connection, and close budgets. `libmdr2::session::RetryPolicy` independently controls ACK attempts. A response timeout closes the session because a later identical GET cannot reliably distinguish a delayed reply.

Dropping a queued operation prevents it from being sent. Dropping an active operation closes its session to avoid ambiguous command ownership. Explicit reconnect creates a fresh generation, decoder, handshake, and state; it never replays queued settings. Different devices run independently.

## Shutdown and validation

`DeviceHandle::close` is idempotent and waits for transport cleanup within the configured close budget. `DeviceManager::disconnect` also waits for pending native setup to acknowledge cleanup. Owned supervisors retain setup futures and identity reservations after caller cancellation or abandonment. `shutdown` cancels discovery and connection setup, waits for cleanup acknowledgements within the configured budgets, and closes connected sessions. A close deadline reports an error but does not release ownership: a background supervisor keeps waiting, and reconnect remains blocked until native cleanup actually finishes. Backend setup futures must complete only after releasing native resources, or return a ready connection that the supervisor can explicitly close. Dropping the manager signals cancellation; call and await `shutdown` before stopping the Tokio runtime to observe cleanup errors.

The integration suite uses independent synthetic peers at the raw frame boundary, with split reads and controlled write completion. It covers handshake and table identity, notifications before responses, ACK/response/write timeout distinctions, bounded queues, dropped operations, late callbacks after reconnect, independent devices, noise/EQ readback, unavailable and unknown feature layouts, setup cancellation with detached native workers, delayed cleanup after a deadline, and conflicting notifications after readback. These tests do not establish compatibility with a physical headset or platform Bluetooth service.

```sh
cargo test -p core
cargo clippy -p core --all-targets -- -D warnings
```
