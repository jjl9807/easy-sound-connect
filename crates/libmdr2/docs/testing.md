# Testing

## Scope and limits

The test suite verifies wire layouts, fixed vectors, malformed-input handling and deterministic session behavior without Bluetooth hardware or an async runtime. These tests exercise codec and state-machine behavior; device, firmware and operating-system compatibility require separate hardware validation.

Published fixtures retain their upstream device/firmware labels. Synthetic cases cover additional valid layouts, boundary values and invalid inputs. Frame wrappers or timing added to a published payload are identified separately from the original bytes.

## Sources and provenance

| Source | Fixed revision / identity | What is used |
| --- | --- | --- |
| [sonymdr_open simulator data](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py#L20) | `44adbf92df1525571561bbebc0485ea9c436ae39` | Author-reported WH-1000XM6 / FW 3.1.5 handshake and initial-status payloads |
| [sound-connect command data](https://github.com/aecca/sound-connect/blob/749e54438ec7be055a45021fea30dff87de22975/crates/core/src/drivers/sony/command.rs#L466) | `749e54438ec7be055a45021fea30dff87de22975` | Author-reported WH-1000XM5 / FW 2.4.1 protocol, firmware and battery payloads |
| [sound-connect frame data](https://github.com/aecca/sound-connect/blob/749e54438ec7be055a45021fea30dff87de22975/crates/core/src/drivers/sony/frame.rs#L183) | Same fixed revision | Complete T2 frame reported as WF-1000XM5 / FW 6.1.0 and attributed there to SonyHeadphonesClient |

Fixture files live in `tests/fixtures/`, with per-source metadata in [their README](../tests/fixtures/README.md). `reference-payloads.tsv` contains ten small request/response pairs. `wf-xm5-functions-t2.hex` contains the complete reported T2 frame, including escaping and checksum. Device addresses, capability IDs and paired-device names from the source fixtures are omitted.

The XM6 battery source publishes body `51 00`; the payload fixture adds response header `23 00`. Integration tests that wrap payloads in frames or schedule notifications use synthetic wrappers and timing. The WF-XM5 complete-frame fixture is retained as published.

## Tests by boundary

| Test file | Main assertions |
| --- | --- |
| `tests/frame.rs` | Fixed ACK/query/checksum vectors, every split boundary, all byte values, corrupt lengths/checksums, canonical escaping, limits, truncation and recovery from noise |
| `tests/session.rs` | ACK deadlines measured from completed writes, early ACKs, same-sequence retries, exact retry counts, duplicate re-ACK, valid protocol-info exception, interleaved traffic, bounded ACK backlog, cancellation, stale tokens and monotonic time |
| `tests/command.rs` | Exact connection queries, inverted table flags, capability layout/ID bounds, 255-entry unsigned counts, unknown functions and correct table/opcode/inquiry routing |
| `tests/device.rs` | Model/firmware captures, nonempty byte-length bounds, UTF-8 handling, unknown series/color, truncation and mismatched response descriptors |
| `tests/battery.rs` | All six layouts, separate left/right thresholds, all charging states, valid zero, invalid percentage/code, exact length, capability mapping and response/notification separation |
| `tests/noise.rs` | Ten parameter layouts, capability ranges, closed fields, terminal maximum steps, availability, preserved inactive fields and capability-checked writes |
| `tests/equalizer.rs` | Five layouts, advertised presets, native curve ranges, ULT, band metadata, raw unknown identities, strict lengths and capability-checked writes |
| `tests/control_routing.rs` | Capability-selected queries, language selection, availability and parameter notification separation, exact inquiry/table routing and malformed bodies |
| `tests/audio.rs` | Eleven audio layouts, capability choices, DSEE encoding, asymmetric LE/classic priority messages, listening assignments, exact counts and invalid writes |
| `tests/system.rs` | Six system layouts, preview capability checks, two Speak-to-Chat extended bodies, closed selectors and notification separation |
| `tests/power.rs` | Six power-setting layouts across T1/T2, advertised options, retained inactive timer fields, charge thresholds and fixed user power-off bytes |
| `tests/common.rs` | Component connection states, codec/effect fixtures, unknown identities, unsigned mode counts and strict status decoding |
| `tests/settings_routing.rs` | Shared battery/power opcodes, table-aware dispatch, unsupported dynamic queries, separate extended notifications and asymmetric audio reports |
| `tests/reference_vectors.rs` | Published payloads through typed requests; complete WF T2 frame across every split boundary; synthetic notification → ACK → read-response exchange |
| `tests/general.rs` | Boolean/list descriptors, language-independent identities, raw text preservation, unsigned bounds and restricted writes |
| `tests/playback.rs` | Profile-dependent volume kinds, control inquiry asymmetry, metadata, mute reports and bounded volume writes |
| `tests/assignment.rs` | Nested counts, candidate validation, reordered custom records and preservation of unknown/service bindings |
| `tests/peripheral.rs` | Device-list layouts, ASCII addresses, duplicate targets, asymmetric reports and operation-result correlation |
| `tests/voice_guidance.rs` | Status selectors, signed volume, response/notification differences and preservation of reported language |
| `tests/sound_effect.rs` | Effect lists, partial notifications, custom curves, opaque metadata and ULT-button constraints |
| `tests/system2.rs` | Table-aware wearing states, read-only limitations, microphone field order and invalid operations |
| `tests/alert.rs` | Explicit confirmation decisions, known local replies and read-only unknown/service alerts |
| `tests/connection.rs` | Host/device message directions, reconnect phases, strict ASCII addresses and fixed lengths |
| `tests/extended_routing.rs` | Cross-family query/response/notification dispatch, inquiry-specific shapes and unsupported queries |
| Rustdoc examples | API examples embedded from the README and protocol guide compile and execute |

Raw frame payload types and unknown identities are tested separately from session support. A raw `DataType` value does not provide session behavior for that frame category. Likewise, a function-list fixture can contain features without a typed command API.

## Fixture coverage

| Data | Coverage | Fixture type |
| --- | --- | --- |
| WH-1000XM6 / firmware 3.1.5 | Protocol information, function lists, identity, battery, noise, EQ, audio, system, auto-off, codec and DSEE effect reports | Published payloads; initial-state response headers reconstructed where the source supplies bodies |
| WH-1000XM5 / firmware 2.4.1 | Protocol information, firmware and single battery | Published payloads |
| WF-1000XM5 / firmware 6.1.0 | Table-2 function list, framing and escaping | Published complete frame |
| All six battery variants | Field ordering, threshold placement and charging states | Synthetic payloads |
| Ten noise and five EQ variants | Capability/parameter layouts, availability, band metadata and setting constraints | Synthetic payloads plus published XM6 initial-state bodies |
| Audio, system, power and common variants | Capability choices, fixed descriptors, configured settings versus current availability, inactive fields, table identity and message asymmetry | Synthetic payloads plus published XM6 initial-state bodies |
| General, playback, assignment, peripheral, voice, sound-effect, table-2 system, alert and link-event layouts | Query contracts, local writes, nested records, operation outcomes, unknown identities and message directions | Independently constructed synthetic payloads; no additional hardware compatibility claim |
| Invalid payloads and session interleavings | Boundary values, truncation, malformed lengths, ACK ordering and timeout behavior | Synthetic payloads and events |

## Reproducible checks

From the workspace root:

```sh
cargo fmt --all -- --check
cargo test -p libmdr2 --offline
cargo clippy -p libmdr2 --all-targets --offline -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p libmdr2 --no-deps --offline
cargo run -p libmdr2 --example handshake --offline
cargo run -p libmdr2 --example settings --offline
cargo package -p libmdr2 --offline
```

When adding data, record its source revision, reported hardware/firmware, whether it is a payload or complete frame, and every transformation. Keep expected fields independent from the implementation under test. Add mutations/truncations where the decoder could otherwise accept ambiguous lengths. Hardware validation records should identify the device, firmware, operating system and transport separately.
