# MDR v2 protocol and implementation guide

This guide describes the SONY MDR v2 wire formats and APIs supported by `libmdr2`. It is an interoperability reference, not an official specification. All byte values in wire layouts are hexadecimal unless stated otherwise; lengths and offsets are byte counts. `u8` is unsigned and `u32be` is big-endian.

## Reading order

1. [Architecture and API boundaries](architecture.md): ownership, integration, compatibility policy, and the distinction between a frame and an operation.
2. [Framing](framing.md): exact byte layout, escaping, checksum, limits, and recovery.
3. [Reliable session](session.md): sequence bits, ACKs, ordered writes, retries, notifications, cancellation, and caller responsibilities.
4. [Connection and capabilities](connection.md): protocol version, capability identities, function lists, and response routing.
5. [Device information](device-information.md): model, firmware, series, and color.
6. [Battery status](battery.md): six capability-selected layouts and notifications.
7. [Noise cancellation and ambient sound](noise-control.md): ten layouts, level ranges, button cycles, availability and validated writes.
8. [Equalizer](equalizer.md): five layouts, presets, native curves, ULT, band information, availability and validated writes.
9. [Audio settings](audio.md): DSEE, connection priority and local listening modes.
10. [System settings](system.md): Speak-to-Chat, playback control based on wearing state and local switches.
11. [Power settings](power.md): automatic power-off, standby, caring charge and user power-off.
12. [Connection and audio indicators](common.md): component connections, codec, upscaling effects and operating modes.
13. [General settings](general.md): labeled Boolean/list capabilities and local controls.
14. [Playback](playback.md): playback actions, metadata, volume and mute reporting.
15. [Key assignments](assignment.md): nested candidates and preserving unrelated bindings.
16. [Peripheral control](peripheral.md): device lists, source selection and operation results.
17. [Voice guidance](voice-guidance.md): local switches, volume and read-only language metadata.
18. [Sound effects](sound-effect.md): presets, custom curves and ULT-button assignments.
19. [Table-2 system settings](system2.md): wearing state, automatic volume and microphone controls.
20. [Alerts](alert.md): local confirmation requests and explicit responses.
21. [Link events](link-events.md): reconnect requests and host LE Audio state reports.
22. [Testing](testing.md): fixture provenance, boundary coverage and reproducible checks.

The family-specific documents are also embedded into their Rust module docs; their Rust examples are executed as doctests. Markdown links between guide files refer to the source documentation in this directory. In generated API docs, navigate through the `frame`, `session`, `command`, `device`, `battery`, `noise`, `equalizer`, `audio`, `system`, `power`, `common`, `general`, `playback`, `assignment`, `peripheral`, `voice_guidance`, `sound_effect`, `system2`, `alert` and `connection` modules. The crate package includes the Markdown guide for offline use.

## Protocol coverage

| Area | Wire/API coverage | Limits and integration requirements |
| --- | --- | --- |
| Framing | Header, checksum, escaping, bounded incremental decode | Raw unknown types preserved, not automatically supported by sessions |
| Reliable session | T1 `0c`, T2 `0e`, ACK `01`, one shared sequence space | No SHOT, large-data workflow, transport, or command-response timer |
| Protocol info | T1 `00/01`, inquiry `00` | Raw version and table flags; layout selection uses advertised capabilities |
| Capability info | T1 `02/03`, standard `00`, app-type `01` | Caller-selected layout; opaque identifier and caller-managed cache |
| Supported functions | T1/T2 `06/07`, inquiry `00` | Table/code/priority/order preserved; command semantics are defined by individual families |
| Device info | T1 `04/05`, inquiries `01/02/03` | Model, firmware, series/color only |
| Battery | T1 `22/23/25`, inquiries `00/01/02/08/09/0a` | Read responses and updates; no connection inference or battery writes |
| Noise control | T1 `60/61/62/63/65/66/67/68/69`, ten inquiry layouts | NC, ambient sound, adaptation and button cycles; no NCSS, test mode or phone automation |
| Equalizer | T1 `50/51/52/53/55/56/57/58/59/5a/5b`, five inquiry layouts | Presets, custom curves, ULT and band information; no Find Your EQ workflow |
| Audio settings | T1 `e0/e1/e2/e3/e5/e6/e7/e8/e9`, eleven inquiry layouts | DSEE, connection priority and local listening modes; no host encoder or personalized 360 workflow |
| System settings | T1 `f0/f1/f2/f3/f5/f6/f7/f8/f9/fa/fb/fc/fd`, six inquiry layouts | Two Speak-to-Chat variants and four local switches; key assignments use a separate module |
| Power settings | T1/T2 `20/21/22/23/25/26/27/28/29`, six inquiry layouts; T1 user power-off `24 03 01` | Automatic power-off/standby, power saving and caring charge; power-off encoding is limited to the user command |
| Common indicators | T1 `12/13/15`, inquiries `01/02/03/06` | Read-only L/R connections, codec, upscaling effect and device modes |
| General settings | T1 `d0/d1/d2/d3/d5/d6/d7/d8/d9`, four advertised slots | Boolean/list reads; recognized sidetone, multipoint and touch-panel writes only |
| Playback | T1 `a0/a1/a2/a3/a5/a6/a7/a8/a9`, four function profiles | Actions, metadata and music/call volume; mute reported, not set |
| Key assignments | T1 `f0/f1/f2/f3/f5/f6/f7/f8/f9/fa/fb/fc/fd`, inquiries `03/0e` | Presets/custom actions; unrelated bindings retained, new service actions rejected |
| Peripheral control | T2 `30/31/32/33/35/36/37/38/39/3c/3d`, inquiries `00..03` | Device lists, connect/disconnect, source selection/keep and handover; no pairing/unpairing workflow |
| Voice guidance | T2 `40/41/42/43/45/46/47/48/49`, nine layouts | Local on/off, volume and sound switches; no language switching or package transfer |
| Sound effects | T1 `50/51/52/53/55/56/57/58/59/5a/5b`, inquiries `30/33/40` plus partial updates | Standard/custom effects and ULT-button assignments; opaque capability metadata retained |
| Table-2 system | T2 `f0/f1/f2/f3/f5/f6/f7/f8/f9`, inquiries `00/03/04/0b` | Wearing reports, automatic volume and microphone controls; limitations read-only |
| Alerts | T1 `92/93/94/95/98/99`, selected layouts | Explicit local confirmations; unknown/service alerts readable without reply permission |
| Link events | T1 `19` inquiries `08/0a`; outbound T2 `68 00` | Reconnect/disconnect notices and host LE Audio states; no transport execution |
| Other functions | Raw reliable payloads remain available | An advertised unknown function does not authorize a typed query or write |

Preserving an advertised function code does not provide its command semantics. Applications must select a supported payload layout before querying a function.

## Extending a family

Each variant specifies its `(table, function code)` relationship, inquiry, request/response/notification opcodes, exact lengths, units and allowed values. Identity types preserve unknown values when the payload layout remains known; decoders reject unknown discriminants that change that layout. Writable variants also specify capability validation and confirmation behavior. Coverage includes fixed vectors, malformed input, response routing and executable API examples.
