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

Initialization reads protocol information, a caller-selected capability layout, supported function tables, model name, firmware version, and advertised battery endpoints and audio codec. `SessionConfig::capability_query` defaults to the standard two-byte capability request; callers can explicitly select `WithAppType`. The session does not infer Android compatibility rules from a model name, interpret a protocol version byte as an MDR generation, or retry alternative handshake layouts. Critical replies must match the strict `libmdr2` decoder. An unsupported optional model, firmware, battery, or codec reply leaves that value unknown, records a diagnostic, and allows remaining initial reads to continue; an ambiguous response timeout still closes the session.

After those reads, initialization enables fixed device messages (`94 00 00`) only when table 1 function `90` is advertised. `SessionConfig::report_foreground` additionally sends the application foreground report (`94 04 00`) and defaults to `false`; a foreground GUI can set it to `true` before connecting. These controls enable delivery without making any decision on a received message. Decoded messages are exposed regardless of whether this optional function was advertised.

Snapshots retain raw protocol information, `(table, code, priority)` function identities, unknown functions, and the connection generation. `FunctionSupport` describes the advertised feature and implemented read/write paths. Live availability is carried by the feature's status; parameter freshness is separate. Battery values are optional readings for headset, left, right, and case components. Unknown values stay unknown, zero is a valid reading, and known values become stale when the connection closes.

`DeviceSnapshot::message_counters` distinguishes accepted matched read responses from successfully decoded notifications, with separate noise, EQ, audio, system-parameter and system-extended notification totals. Counters reset for each connection generation, saturate at `u64::MAX`, and contain no payloads or device identifiers. ACKs, reliable duplicates, invalid messages, and unmatched response payloads do not increment these counters. These diagnostics can establish notification reception independently of explicit GET operations; they do not attribute which external controller caused a change or confirm a particular SET.

`DeviceSnapshot::audio_codec` is present only when table 1 function `12` advertises the device's current codec report. Its `codec` is `None` until a valid read or notification arrives; `AudioCodec::UNSETTLED` is an explicit device report, and unknown codec bytes remain intact. `refresh_audio_codec` queries this report without changing settings. Initialization reads it once, notifications keep it current, and disconnect retains the last value with `Freshness::Stale`. The re-exported `common` module provides codec identities. This status describes the device's reported codec, independently of sound-quality preferences; it does not enumerate supported encoders, select a host codec, or establish which multipoint source is active.

## Operations

`refresh_battery`, `refresh_noise`, and `refresh_equalizer` read advertised features. Noise and EQ details are queried on demand. `set_noise` edits newly read parameters while preserving unrelated fields, then checks capability and current availability. `set_equalizer` validates the supplied setting and selects and confirms an editable preset before writing a curve when necessary. Both setters finish with an explicit readback and return `Confirmed` or `Adjusted`; a link ACK alone never confirms a setting. Verification retains the matched read response separately from the latest snapshot, so a later notification can update displayed state without changing that operation's result. Unknown noncritical feature responses fail that operation while preserving the session and other known state.

Use `set_equalizer_preset(kind, preset)` for a preset selection that preserves the freshly read ULT mode. Use `set_equalizer_band(kind, expected_preset, index, value)` to replace one native value while preserving fresh ULT and every other band. The current preset must match `expected_preset`; pass `None` for standalone custom EQ. A preset curve must already be in an advertised editable slot, and values are validated without clamping. The full-setting API intentionally replaces every supplied value and is appropriate when the caller owns the complete target state. The `equalizer` and `noise` modules are re-exported through `ezsc_core` so views can consume these typed values without owning a protocol session.

`refresh_equalizer_band_info(kind)` explicitly reads optional band metadata against a fresh capability. It is independent of regular parameter refresh and settings, and fills `EqualizerState::band_info` only when inquiry and band count agree. Capability changes that invalidate its count discard the old association. Preserve device ordering, identify Clear Bass by its metadata, and use generic band numbers when metadata is missing or unknown. Preset labels may be empty; the core retains their IDs and never invents device-supplied names. An unanswered metadata query follows the same response-timeout isolation rule as other GETs.

`NoiseChange::Adaptive` and `AdaptiveSensitivity` edit the adaptation fields of the advertised adaptive layout while preserving effect, selected mode, ambient settings, and the other adaptation field. Level, voice-focus, and adaptation edits do not implicitly enable an effect. `Mode` enables the selected effect only for layouts with supported unambiguous mode semantics; button-cycle editing and ambiguous mode selection in layouts without an explicit mode field remain unsupported.

`refresh_audio` and `set_audio` support `Dsee`, `DseeWithReasons`, `ConnectionPriority`, `ConnectionPriorityWithStatus`, and `ClassicLePriority`. `DeviceSnapshot::audio` contains only these advertised layouts. Other AUDIO functions retain their raw advertised identities without becoming readable or writable through this core. The `audio` module is re-exported for their typed descriptions. Inquiry `00`/`02` connection priorities use the protocol's static capability and skip the unsupported dynamic capability query; the other selected layouts query their capability. Every refresh then reads availability and parameters.

`AudioChange::Dsee` selects off/auto, independently of any active-effect indicator. `AudioChange::Priority` selects a known advertised priority; unknown identities remain readable but cannot be written. For Classic/LE inquiry `05`, this uses the protocol library's default `Enabled` control byte (`00`), including device prompts when required. Setters repeat the capability/status/parameter preflight, require `status.enabled`, encode against the current capability, and confirm the configuration through a matched parameter GET. The secondary status flag is retained without inventing an extra availability rule or interpreting it as a codec. Classic/LE priority notification transition fields remain available in the snapshot but are not compared with its shorter GET response. If a priority change interrupts the control link before readback, the operation returns an error and its state becomes stale; the core does not infer success, reconnect automatically, or replay that operation.

`refresh_system` and `set_system` support `SpeakToChatType1` and `SpeakToChatType2`, exposed in `DeviceSnapshot::system` and through the re-exported `system` module. A refresh reads capability, availability, ordinary parameters, then extended parameters. `SystemChange::Enabled` and `Preview` read the current ordinary parameter pair before patching one switch, preserving the other. Enabling preview requires advertised support; an incompatible retained preview value causes validation failure instead of being silently cleared. `Sensitivity`, `VoiceFocus`, and `ModeOut` read and patch the complete extended parameter set without writing the ordinary switches. Type 2 rejects voice focus, and end-mode selectors remain distinct from the raw values reported in capability metadata.

Both Speak-to-Chat paths refresh capability and status before writing, require availability, and confirm every field in the written parameter set using its own GET response. Ordinary and extended notifications update independent values and freshness fields; they cannot satisfy those reads. Disconnect marks every known parameter set stale while retaining unknown values as unknown. Other SYSTEM layouts remain outside this core's implemented controls.

Each device owns a bounded operation queue and one actor containing its decoder and protocol session. Complete frame writes remain ordered while reads, timers, and cancellation progress concurrently. Expected responses are registered before sending; notifications update snapshots independently and cannot satisfy a GET response. Configuration provides separate queue, operation, transport-write, response, connection, and close budgets. `libmdr2::session::RetryPolicy` independently controls ACK attempts. A response timeout closes the session because a later identical GET cannot reliably distinguish a delayed reply.

Dropping a queued operation prevents it from being sent. Dropping an active operation closes its session to avoid ambiguous command ownership. Explicit reconnect creates a fresh generation, decoder, handshake, and state; it never replays queued settings. Different devices run independently.

## Device prompts

Observe `DeviceSnapshot::pending_prompt` for the latest `DevicePrompt`. Its `PromptId` combines the connection generation and a monotonically increasing serial. A new message replaces the previous prompt and invalidates its ID, including when its payload is identical. Reliable frame retransmissions are deduplicated by the protocol session before projection. The re-exported `alert` module provides typed message content for presentation.

Use `DevicePrompt::allows` to select legal `PromptReply` actions, then call `DeviceHandle::respond_prompt(id, reply)` only after an explicit user decision. The actor validates the current ID and deadline again and encodes its own received message, never caller-provided content. Supported accept/reject messages allow their declared actions; positive-only messages cannot be rejected. Informational, unknown, assistant-selection, and unsupported messages allow only `Dismiss`, which sends no protocol reply. The core never accepts, rejects, or replays a setting automatically.

A new prompt interrupts an active operation with `AwaitingConfirmation` after its current exchange can be safely separated. Unsent steps are discarded, including any remaining stages of a multi-step EQ edit. A prompt received during initialization remains visible while the bounded initialization completes. Pending prompts block new settings with `Error::PromptPending`; independent reads remain allowed unless an earlier query is waiting for a prompt response.

If a GET has been link-acknowledged but the device waits for a decision before returning its response, the actor retains that GET's matcher and releases the interrupted operation. An explicit reply can then be transmitted. The old response is consumed only for synchronization; after it is drained, a new matched GET verifies the decision. The old response cannot confirm the setting. Its response deadline is paused while a prompt awaits a decision and resumes after the reply's ACK. A missing old response then closes the session within the normal response budget.

Association with a setting requires an actual transmitted SET and a supported prompt matching that setting's meaning; it is not inferred from timing alone. Late, replacement, unrelated, and flexible messages are handled independently. After an associated acceptance, a fresh matched GET returns `Confirmed` only if the requested state is present, otherwise `Adjusted`. After an associated rejection, the same check returns `Cancelled` only if the preflight value is restored, otherwise `Adjusted`. Independent replies synchronize all implemented control parameters and the advertised codec report, then return `Synchronized`; local dismissal performs the same reads and returns `Dismissed`. Neither result asserts that an earlier requested setting succeeded. Callers must reconcile reported state and explicitly retry any unsent work.

The prompt is consumed before transmission, preventing repeated decisions. Replaced, consumed, or previous-generation IDs return `Error::StalePrompt`; actions outside the message contract return `Error::InvalidPromptReply`. `SessionConfig::prompt_timeout` defaults to 120 seconds. Expiry closes the control session with `TimeoutStage::Prompt`, clears the prompt, and marks known state stale without transmitting an implicit decision. Reply failures do not imply setting success. If the control link closes during a reply or its readback, reconnect explicitly before making further changes.

## Shutdown and validation

`DeviceHandle::close` is idempotent and waits for transport cleanup within the configured close budget. `DeviceManager::disconnect` also waits for pending native setup to acknowledge cleanup. Owned supervisors retain setup futures and identity reservations after caller cancellation or abandonment. `shutdown` cancels discovery and connection setup, waits for cleanup acknowledgements within the configured budgets, and closes connected sessions. A close deadline reports an error but does not release ownership: a background supervisor keeps waiting, and reconnect remains blocked until native cleanup actually finishes. Backend setup futures must complete only after releasing native resources, or return a ready connection that the supervisor can explicitly close. Dropping the manager signals cancellation; call and await `shutdown` before stopping the Tokio runtime to observe cleanup errors.

The integration suite uses independent synthetic peers at the raw frame boundary, with split reads and controlled write completion. It covers handshake and table identity, notifications before responses, ACK/response/write timeout distinctions, bounded queues, dropped operations, late callbacks after reconnect, independent devices, noise/EQ/audio/Speak-to-Chat readback, unavailable and unknown feature layouts, setup cancellation with detached native workers, delayed cleanup after a deadline, and conflicting notifications after readback. Prompt tests cover capability-gated delivery, explicit decisions, action validation, fresh readback after held queries, message replacement, interrupted multi-step edits, expiry, and connection generations. These tests do not establish compatibility with a physical headset or platform Bluetooth service.

```sh
cargo test -p core
cargo clippy -p core --all-targets -- -D warnings
```
