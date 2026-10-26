# easy-sound-connect

easy-sound-connect (ezsc) is a Rust desktop client for SONY MDR v2 headphones, with native Bluetooth transports and a shared GPUI interface. The client is under development; device and platform compatibility requires hardware validation.

## Workspace

| Crate | Responsibility |
| --- | --- |
| [`libmdr2`](crates/libmdr2) | Public protocol codecs, typed commands and transport-independent session state machine |
| [`bluetooth`](crates/bluetooth) | Native discovery, control connections and ordered byte transport |
| [`core`](crates/core) | Per-device sessions, capability/state projection and confirmed operations |
| `model` | Reserved for display assets and their cache |
| `app` | Connection probe, optional control checks and GPUI desktop window |

Only `libmdr2` is publishable. The client does not implement MDR v1 or legacy fallback. Model names do not enable features; advertised protocol capabilities select supported layouts.

## Run

Pair the headset in system Bluetooth settings first. Enumerate paired devices and use a returned platform ID and advertised transport:

```sh
cargo run -p app -- --list
cargo run -p app -- --device '<platform-id>' --transport gatt --watch 30
# Use --transport rfcomm when that service is available.
cargo run -p app -- --device '<platform-id>' --transport rfcomm --controls --band-info --watch 0
```

The default `ezsc-probe` binary performs a complete handshake, reads model/firmware and battery state, watches notifications, and releases the control connection. It does not pair devices or change settings. `--controls` additionally reads advertised noise and EQ capabilities, availability and parameters. `--band-info` requests optional EQ band metadata; an unanswered request closes the session because delayed replies cannot be safely correlated with a later identical request. Ctrl+C requests cleanup.

The explicitly requested `--exercise-controls` check temporarily changes supported noise effect, mode, ambient level, voice focus, adaptation and sensitivity fields, then selects another advertised EQ preset and changes one band in an editable curve when available. Each independent noise check restores and verifies its full saved baseline before proceeding. Device-adjusted or failed noise checks are collected only after verified restoration and produce a failing exit status before the EQ checks; an unverified restoration stops subsequent checks immediately. The probe prints saved values before writing, confirms changes by GET readback, and restores a custom slot's own curve before restoring the originally active EQ preset. Ctrl+C waits for the current check and restoration. Forced process termination or a lost device can prevent recovery.

```sh
cargo run -p app --features gui --bin easy-sound-connect
```

The desktop window supports paired-device selection, connection progress, battery updates, noise/ambient controls, EQ preset selection and native band steps. Controls follow advertised capabilities and current availability. Changes remain pending until readback reports confirmation or device adjustment. Optional band labels can be read separately; missing metadata uses numbered bands. GPUI packages are pinned to the `gpui-pre` 0.3.7 snapshot of Zed commit `1a28cff4b409169bac058bca40dfbfeb7621d19b`. Bluetooth and protocol work run on Tokio rather than the UI thread. The complete product interface, tray integration, localization and themes are not implemented yet.

Pass `--diagnostics` after `--bin easy-sound-connect --` to print connection state, control values and per-generation response/notification counters to stderr. Diagnostics are opt-in and do not record raw frames. On disconnection, controls become unavailable and cached values are marked stale; release control before reconnecting. Reconnection is explicit, not automatic.

On Debian/Ubuntu, the probe needs a C toolchain, `pkg-config` and `libdbus-1-dev`. The GUI also needs Fontconfig/FreeType, XKB, Wayland and X11 development libraries. A desktop session and a supported graphics driver are needed to run the window. Windows builds use the Windows SDK; macOS builds require Xcode command-line tools.

## Validation

```sh
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p app --features gui
```

Deterministic transports exercise handshake, fragmented I/O, ACK/response ordering, cancellation, stale connection isolation, noise controls and equalizer readback. Linux and Windows have RFCOMM/GATT backends; macOS currently has an RFCOMM preview using paired SDP records, with GATT explicitly unavailable. The macOS preview limits native open attempts to 16 per process to bound retained callback objects and any unfinished native write buffers; restart the app when that limit is reached. These are implementation boundaries, not full hardware compatibility claims.

Hardware validation on WF-1000XM6 firmware 2.0.5, Debian 13.7 / BlueZ 5.82 over RFCOMM covers discovery, handshake, identity/battery reads, noise/EQ capabilities, frequency labels and clean release. GUI checks confirmed NC/ambient selection, ambient level and voice focus in ambient mode, EQ preset selection and independent edits to all ten custom bands. Probe checks also confirmed adaptation and stored sensitivity changes in ambient mode. Original settings and the edited custom curve were restored and verified. Some requests produced device-adjusted readback: disabling the effect from ambient mode also changed the reported mode, and edits to ambient-related fields in NC mode retained their previous values. These results are reported as `Adjusted` without model-specific normalization.

An external NC change reached the GUI through a notification without a GET. Two phone EQ handoffs coincided with peer resets; reconnection read the changed preset, but external EQ notification synchronization remains unverified. Both peer-reset recovery and a deliberate control-profile disconnection were followed by successful explicit reconnection in the same GUI process. A silent PCM stream remained active at sampled points during controls and reconnection; audible quality was not assessed. Sleep/resume, long-running reliability, GATT hardware behavior, Windows, macOS and WH-1000XM6 validation remain pending.
