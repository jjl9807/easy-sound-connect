# Reference protocol-data fixtures

These files contain request/response payloads and complete frames. Device and firmware labels are metadata supplied by the upstream authors. The table records the source revision and transformations for each fixture. For test coverage, see the [testing guide](../../docs/testing.md).

| Source tag / file | Upstream snapshot and location | Reported device | Transformation |
| --- | --- | --- | --- |
| `xm6-sim` | [sonymdr_open `sonyctl/sim.py`, 44adbf9](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py#L20) | WH-1000XM6 / FW 3.1.5 | Request/response payload bytes retained; private capability identifier omitted |
| `xm6-initial-body` | [same file, initial power state](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py#L33) | WH-1000XM6 / FW 3.1.5 | Published `51 00` body prefixed with the `23 00` response header |
| Noise/EQ literals in `tests/noise.rs` and `tests/equalizer.rs` | [same file, `INITIAL` entries](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py) | WH-1000XM6 / FW 3.1.5 | Initial-state bodies retained; corresponding RET opcode and inquiry prepended; SET expectations use synthetic edits |
| Audio/system/power/common literals in their integration tests | [same file, `INITIAL` entries](https://github.com/Shiven-saini/sonymdr_open/blob/44adbf92df1525571561bbebc0485ea9c436ae39/sonyctl/sim.py) | WH-1000XM6 / FW 3.1.5 | DSEE/priority/listening, Speak-to-Chat, auto-off, codec and effect bodies wrapped with their RET headers; timing, additional layouts and write expectations are synthetic |
| `xm5-command` | [sound-connect SONY command tests, 749e544](https://github.com/aecca/sound-connect/blob/749e54438ec7be055a45021fea30dff87de22975/crates/core/src/drivers/sony/command.rs#L466) | WH-1000XM5 / FW 2.4.1 | Response payload bytes retained |
| `wf-xm5-functions-t2.hex` | [sound-connect SONY frame test, 749e544](https://github.com/aecca/sound-connect/blob/749e54438ec7be055a45021fea30dff87de22975/crates/core/src/drivers/sony/frame.rs#L183) | WF-1000XM5 / FW 6.1.0, attributed upstream to SonyHeadphonesClient | Complete frame retained, including delimiters, escape, sequence and checksum |

The TSV stores six tab-separated fields: case ID, source tag, table number, request payload hex, response payload hex and expected interpretation. Firmware/model text is product data. Device addresses, capability IDs and paired-device names are not included. All boundary/malformed payloads in the Rust tests are separately identified as synthetic cases.

The complete T2 frame has a 25-byte payload, checksum `ee`, and 11 function entries. Its first priority is `3d`, represented on the wire by `3d 2d`. The same byte code can occur in different command tables; the fixture's table-2 identity is part of the expectation, not something inferred from the headphone model name.

General, playback, assignment, peripheral, voice-guidance, sound-effect, table-2 system, alert and link-event tests use independently constructed synthetic payloads. Their device addresses and display names are test data; these cases add no device or firmware compatibility claims.
