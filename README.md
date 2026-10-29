# Easy Sound Connect

Easy Sound Connect (ezsc) is a Rust desktop client for SONY MDR v2 headphones, with native Bluetooth transports and a shared GPUI interface. The client is under development; device and platform compatibility requires hardware validation.

The product display name is **Easy Sound Connect** and the desktop application identifier is **`com.jjl9807.ezsc`**. Window grouping, desktop entries, login registrations and notification identities use this identifier. Distribution metadata must use the same values; see [packaging metadata](packaging/README.md).

## Workspace

| Crate | Responsibility |
| --- | --- |
| [`libmdr2`](crates/libmdr2) | Public protocol codecs, typed commands and transport-independent session state machine |
| [`bluetooth`](crates/bluetooth) | Native discovery, control connections and ordered byte transport |
| [`core`](crates/core) | Per-device sessions, capability/state projection and confirmed operations |
| `app` | Connection probe, optional control checks, GPUI interface, tray, client preferences and product artwork retrieval/cache |

Only `libmdr2` is publishable. The client does not implement MDR v1 or legacy fallback. Model names do not enable features; advertised protocol capabilities select supported layouts.

## Run

Pair the headset in system Bluetooth settings first. Enumerate paired devices and use a returned platform ID and advertised transport:

```sh
cargo run -p app -- --list
cargo run -p app -- --device '<platform-id>' --transport gatt --watch 30
# Use --transport rfcomm when that service is available.
cargo run -p app -- --device '<platform-id>' --transport rfcomm --controls --band-info --watch 0
```

The default `ezsc-probe` binary performs a complete handshake, reads model/firmware, battery state and the reported audio codec, enables device messages when advertised, watches notifications, and releases the control connection. It does not pair devices or change sound settings. `--controls` additionally reads advertised noise, EQ, DSEE, connection-priority and Speak-to-Chat capabilities, availability and parameters. Speak-to-Chat includes its separate extended settings. `--band-info` requests optional EQ band metadata; an unanswered request closes the session because delayed replies cannot be safely correlated with a later identical request. Ctrl+C requests cleanup. The probe reports device prompts but does not answer them automatically.

The explicitly requested `--exercise-controls` check temporarily changes supported noise effect, mode, ambient level, voice focus, adaptation and sensitivity fields, then selects another advertised EQ preset and changes one band in an editable curve when available. Each independent noise check restores and verifies its full saved baseline before proceeding. Device-adjusted or failed noise checks are collected only after verified restoration and produce a failing exit status before the EQ checks; an unverified restoration stops subsequent checks immediately. The probe prints saved values before writing, confirms changes by GET readback, and restores a custom slot's own curve before restoring the originally active EQ preset. Ctrl+C waits for the current check and restoration. Forced process termination or a lost device can prevent recovery.

## Desktop interface

```sh
cargo run -p app --features gui --bin easy-sound-connect
```

The desktop interface uses GPUI Kit `0.8.1`, its Component controls and the matching `gpui-pre`/`gpui-pre-platform` `0.3.10` packages, with a shared custom theme. Bluetooth and protocol work run on Tokio rather than the UI thread. Navigation has seven pages:

| Page | Current content |
| --- | --- |
| My Device | Product image, connection state, battery components, audio codec, noise/ambient controls and Speak-to-Chat |
| Sound | Capability-driven EQ presets, curve and band steps, DSEE and Bluetooth connection quality |
| Scenes | Availability notice; scene editing, execution and automatic triggers are not implemented |
| Settings | Headphone information and control-connection actions; reserved for device settings |
| Activity | Recent connection and setting results, with diagnostic copying |
| Preferences | Client language, theme, tray visibility, close-to-tray behavior, login startup, startup visibility and notifications; accessible without a headset |
| About | Application version, project link and dependency information |

The main window integrates window controls into the interface without a separate title strip. Its transparent upper margin supports window dragging and platform double-click actions. Windows uses native window-control hit testing; macOS retains its native traffic-light buttons and positioning. Linux follows the left/right button layout supplied by the desktop settings portal, updates when that layout changes, and exposes only supported window actions; desktops without that setting use GPUI's default layout. When the window manager supplies server decorations, the application avoids duplicate controls. Page actions remain below the window controls while content scrolls. The compact tray panel has no window buttons. Closing the main window follows the configured close-to-tray behavior.

The sidebar device card shows the selected headset's product image, model, control-connection status and battery summary. Its dropdown lists saved headphones and provides Add headphones. The connected device's page provides icon buttons to refresh, release the control connection and remove the current headset from the local list; disconnected devices have a connect button. Startup restores the saved list and last selection without enumerating or connecting devices. Selecting a saved headset connects directly using its platform identity and last successful transport; Add headphones enumerates system-paired devices advertising MDR v2 services. New headphones must be paired in system Bluetooth settings first. Devices are saved after successful MDR initialization, independently of optional control refreshes. Switching waits for the previous control connection to release before opening the next one. Removing the current headset waits for its control connection to close and does not unpair the device. Disconnected cards show only their connection status; connected cards use the lowest current left/right battery reading, excluding the charging case, or the headset's own reading for single-battery devices.

The Preferences page offers English and Simplified Chinese in a language dropdown, with Light, Dark and System themes. The initial language follows a Chinese system locale and otherwise defaults to English. Application theme changes apply across windows, and language changes also update tray menus. Changing language also refreshes already loaded EQ display labels through the shared read-only command queue, without reconnecting or changing headphone settings. Additional device settings, scene automation and a local EQ curve library are not integrated yet.

Application UI text, device-prompt descriptions, accessibility labels and notifications use the same `rust-i18n` runtime as GPUI Kit. English and Simplified Chinese catalogs live in [crates/app/locales](crates/app/locales), are embedded at compile time and fall back to English. Translation keys describe presentation; protocol IDs and permitted prompt replies remain independent of locale coverage. Device-provided text and technical diagnostic details retain their original values. Activity records and completed operation messages retain their originating language.

On Linux, the interface prefers an installed Simplified Chinese Source Han Sans family (`Source Han Sans SC` or `Source Han Sans CN`) for Chinese, Latin text and digits. Static Regular, Medium and Bold fonts are recommended; the current Linux text backend must be able to resolve distinct regular and bold faces. If no usable family is found, the existing system-font fallback remains active and a warning appears once per launch when the main window is shown. Background startup defers the warning until the main window is opened. Install the fonts through the system and restart the application to apply them. Fonts are not bundled, downloaded or stored in the application configuration directory. Windows and macOS retain their platform font selection.

Important warnings and errors appear briefly at the bottom of the sidebar, in yellow and red respectively. Messages dismiss automatically after eight seconds or can be closed manually; clicking the message opens its details in Activity. Ordinary successful updates do not produce these messages. Repeated visible messages do not extend their lifetime, and a warning cannot replace a visible error. Connection, operation and configuration diagnostics remain available in Activity after the message disappears.

Product artwork retrieval and presentation live in internal `app` modules. On the first normal launch without a valid local catalog, the application sends a JSON-serialized GraphQL POST to `https://v1.api.data-gateway.seeds.services/graphql`; subsequent launches reuse the catalog under `~/.ezsc/cache/products/`. An indexed lookup matches the protocol-reported model name and raw color code, which are also saved with known devices. Bluetooth aliases do not substitute for model identity, and an unknown color is not assigned a default variant. Images are downloaded only when needed and then reused from the local cache. Catalog loading, downloads, disk access and image decoding run in the background without blocking startup, rendering or the Bluetooth command queue.

Device cards and the disconnected home-page product area use the static image. Once the control connection is ready, the home-page image plays the corresponding animation, when available, once and retains its final frame. Returning to My Device from another page or bringing the main window back to the foreground while My Device is open replays the animation once using the decoded cache. Ordinary redraws and selecting the already active page do not restart it. APNG resources are supported even when their URL ends in `.png`. The static image remains the fallback while an animation loads or if it fails. Missing model/color matches, unavailable artwork and failed downloads or decoding fall back to the embedded product image for the selected theme. Default images and tray icons from `crates/app/assets/` remain embedded in the binary; device control does not require network access. Cache writes are atomic, corrupt resources can be downloaded again, and disk/decoded-image limits bound resource use.

The desktop window supports saved-device selection, connection progress, battery and audio-codec updates, noise/ambient controls including adaptation and sensitivity, EQ preset selection and native band steps, DSEE Auto/Off, Bluetooth connection quality and Speak-to-Chat settings. Bluetooth connection quality offers the advertised sound-quality and stable-connection choices in Classic Audio. LE Audio connection settings are not exposed; an existing LE Audio value remains visible without enabling mode changes. Controls follow advertised capabilities and current availability. Stored sensitivity can be edited independently of the adaptation switch. Changes remain pending until readback reports confirmation or device adjustment. Frequency labels load automatically with the initial sound-control sync. Valid metadata is reused; refreshing sound controls retries missing labels. When metadata is unavailable, the equalizer uses numbered bands.

The audio-codec indicator uses headset reports, including SBC, AAC and LDAC, when advertised. It updates from notifications and manual state refreshes; disconnected values are marked stale. An unsettled codec is shown as not determined, and unrecognized identities retain their raw code. This is separate from the Bluetooth connection-quality preference and does not select or configure a host encoder.

DSEE Auto configures automatic processing; it does not report whether processing is currently active. Speak-to-Chat exposes its configured switch, supported preview control, sensitivity, automatic end mode and voice focus where the layout provides it. Each edit preserves other freshly read fields. Connection-priority changes can end the control connection; release control and reconnect to read the actual setting. An interrupted operation remains unconfirmed and is not replayed automatically.

Device prompts appear in a modal dialog. Supported requests expose the device's allowed confirmation or cancellation actions; informational and unsupported messages can only be closed locally. Sound controls are blocked while a prompt is pending. After an explicit reply, Core reads the setting again before reporting its result. Prompts belong to one control connection and cannot be reused after replacement, reply, timeout or disconnection. An unanswered prompt times out after two minutes and closes the control session; reconnect explicitly to reconcile the device state. Connection-priority writes retain the default protocol control byte `00` and use this confirmation flow when requested by the device.

Pass `--diagnostics` after `--bin easy-sound-connect --` to print connection state, control values and per-generation response/notification counters to stderr. Diagnostics are opt-in and do not record raw frames. On disconnection, controls become unavailable and cached values are marked stale; release control before reconnecting. Reconnection is explicit, not automatic.

The main window and compact tray panel observe one application-owned Core session and submit commands through the same bridge. Closing the panel does not disconnect the headset. Closing the main window keeps the application running only when close-to-tray is enabled and a tray host is available; otherwise it exits. Quit shuts down the shared connection. Disabling the tray or losing its host restores access to the main window. A second normal launch activates the existing instance. Linux uses `ksni` over D-Bus for StatusNotifierItem integration, without adding GTK/AppIndicator to the tray implementation; Windows and macOS use native tray APIs through `tray-icon`. Panel placement and tray availability depend on the desktop host.

Tray icon colors follow the system tray host independently of the application theme. Windows reads the taskbar's `SystemUsesLightTheme` setting, macOS lets AppKit recolor a template image, and Linux publishes a named symbolic SVG for hosts that support symbolic recoloring. Linux also provides a neutral bitmap fallback for hosts that cannot load the symbolic icon; these hosts do not gain automatic panel-color matching from the fallback.

The native tray menu contains only Open main window and Quit, without device summaries, separators or submenus. Linux hosts such as GNOME AppIndicator show this native menu on a primary click; hosts that send an activation request still open the GPUI panel. No desktop extension or setting changes are required.

Client state is stored under `.ezsc` in the operating system's user home directory on all platforms. `preferences.json` contains versioned client preferences; `devices.json` contains versioned saved device identities, display/model names, raw product colors, successful transports and the last selection. Device records do not store live connection state, battery readings or sound settings. Saves replace files atomically, and malformed or unsupported files are preserved. Device-record writes run off the UI thread and pending writes are drained at shutdown. Login startup is opt-in and uses the operating system's login registration location; it registers the current executable path, so after moving the binary, disable and re-enable that preference. The login entry passes `--background`, which starts in the tray only when the startup-visibility preference and tray availability permit it. Desktop notifications report pending device confirmations and lost control connections, subject to system notification support.

### Offline visual preview

```sh
cargo run -p app --features gui --bin easy-sound-connect -- --preview
cargo run -p app --features gui --bin easy-sound-connect -- --preview --headband --zh --light --page=sound
cargo run -p app --features gui --bin easy-sound-connect -- --preview --empty --page=preferences
cargo run -p app --features gui --bin easy-sound-connect -- --preview --panel
cargo run -p app --features gui --bin easy-sound-connect -- --preview --with-tray
cargo run -p app --features gui --bin easy-sound-connect -- --preview --preview-product-image=/absolute/path/image.png --preview-product-animation=/absolute/path/animation.png
```

Preview mode uses synthetic `Preview headset` state, disables device controls and does not start Bluetooth, modify saved preferences or change login startup. Its default layout has left/right/case batteries and ten EQ bands; `--headband` selects a single battery and five bands. These fixtures exercise layouts and do not describe or validate any real headphone model. Preview defaults to English and Dark; `--zh` selects Simplified Chinese and `--light` selects Light. Use `--empty` for the empty device list, `--device-picker` to open the device dropdown and `--panel` to open the compact panel. Preview device selection and removal use in-memory fixtures only. The `--page=` option accepts `sound`, `preferences`, `settings`, `activity`, `scenes` or `about` for the initial main page; My Device is the default. The ordinary preview skips system tray registration and the normal single-instance lock. Add `--with-tray` to exercise native tray registration, menu actions and window lifecycle while retaining synthetic device state and an isolated temporary instance directory.

Use `--preview-product-image=/absolute/path` and `--preview-product-animation=/absolute/path` to load local image fixtures in preview mode. The static fixture appears in device cards and the home-page product area; the optional animation exercises playback for the synthetic connected headset. These options read local files only and do not fetch the catalog, download artwork or write application configuration/cache files.

### Build prerequisites

On Debian/Ubuntu, the probe needs a C toolchain, `pkg-config` and `libdbus-1-dev`. The GUI also needs Fontconfig/FreeType, XKB, Wayland and X11 development libraries, a graphical desktop session and a working Vulkan driver. Package names vary by distribution. Follow the [GPUI Kit installation guide](https://gpui-kit.com/docs/installation/) for upstream toolchain and platform prerequisites; its general package list also covers optional features that this application does not enable. No package-installation script is provided here.

The upstream guide lists macOS 15 or later with Xcode command-line tools, and Windows 10 or later with the MSVC Rust toolchain, Visual Studio C++ build tools, a Windows SDK and CMake. These are upstream requirements, not this project's verified minimum operating-system versions. Native builds and runtime checks are still required on each target platform; a cross-compilation check does not establish GUI, tray or Bluetooth compatibility.

## Validation

```sh
cargo fmt --all --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check -p app --features gui
```

Deterministic transports exercise handshake, fragmented I/O, ACK/response ordering, cancellation, stale connection isolation, noise controls and equalizer readback. Linux and Windows have RFCOMM/GATT backends; macOS currently has an RFCOMM preview using paired SDP records, with GATT explicitly unavailable. The macOS preview limits native open attempts to 16 per process to bound retained callback objects and any unfinished native write buffers; restart the app when that limit is reached. These are implementation boundaries, not full hardware compatibility claims.

The hardware checks below cover the existing protocol and control flows on the stated headset/platform combination. They do not establish acceptance of the GPUI Kit migration, every new page, tray lifecycle, scaling or all language/theme combinations. Offline previews support visual review; complete reference-image comparison and cross-platform GUI acceptance remain pending.

Hardware validation on WF-1000XM6 firmware 2.0.5, Debian 13.7 / BlueZ 5.82 over RFCOMM covers discovery, handshake, identity/battery reads, noise/EQ capabilities, frequency labels and clean release. GUI checks confirmed NC/ambient selection; ambient level, voice focus, adaptation and all three sensitivity choices in ambient mode; EQ preset selection; and independent edits to all ten custom bands. Original settings and the edited custom curve were restored and verified. Some requests produced device-adjusted readback: disabling the effect from ambient mode also changed the reported mode, and edits to ambient-related fields in NC mode retained their previous values. These results are reported as `Adjusted` without model-specific normalization.

An external NC change reached the GUI through a notification without a GET. Two phone EQ handoffs coincided with peer resets; reconnection read the changed preset, but external EQ notification synchronization remains unverified. Both peer-reset recovery and a deliberate control-profile disconnection were followed by successful explicit reconnection in the same GUI process. A silent PCM stream remained active at sampled points during controls and reconnection; audible quality was not assessed. Sleep/resume, long-running reliability, GATT hardware behavior, Windows, macOS and WH-1000XM6 validation remain pending.

DSEE, connection-priority and Speak-to-Chat integration is covered by synthetic protocol peers, including both Speak-to-Chat parameter layouts, field preservation, unavailable controls, notification/readback separation and disconnection during a priority change. On the WF-1000XM6/Linux RFCOMM combination above, GUI writes and GET readback confirmed DSEE Off/Auto and Speak-to-Chat Type2 on/off, preview, all three sensitivity choices and all four automatic-end modes. These settings were restored and checked in an independent connection. DSEE processing and speech-triggered effects were not assessed.

Device-prompt tests cover capability-gated initialization, explicit decisions, replacement and expired prompts, connection generations, unsupported messages, interrupted EQ steps, and reads withheld until a decision arrives. The production GUI on the same WF/Linux combination displayed and rejected the sound-quality prompt, preserving stable-connection priority. A new request produced two successive confirmations; accepting both read back sound-quality priority, with Linux reporting LDAC. Explicit reconnection retained the setting. The reverse prompt restored stable-connection priority and SBC, followed by an independent connection confirming the restored setting. These checks validate configuration and prompt handling, not sustained audible quality, LE Audio playback or latency.

The probe and production GUI both read and displayed LDAC directly from the headset on the same WF/Linux combination without changing sound settings. Synthetic peers cover codec notifications, refresh/readback separation, unknown and unsettled identities, stale values and fresh connection generations. Live codec-transition notifications were not exercised in this check.
