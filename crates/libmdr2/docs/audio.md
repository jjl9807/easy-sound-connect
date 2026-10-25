# Audio settings

The `audio` module implements MDR v2 table-1 audio settings. Select each layout from its advertised function identity. A configured setting, its current availability and its currently active effect are separate values. DSEE effect indicators are reported by COMMON messages, independently of AUDIO settings.

## Layouts and commands

| Function | Inquiry | `AudioKind` |
| --- | --- | --- |
| `e1` | `00` | `ConnectionPriority` |
| `e2` | `01` | `Dsee` |
| `e3` | `02` | `ConnectionPriorityWithStatus` |
| `e4` | `03` | `Bgm` |
| `e5` | `04` | `UpmixCinema` |
| `e7` | `05` | `ClassicLePriority` |
| `e9` | `07` | `SoundLeakageReduction` |
| `ea` | `08` | `ListeningOptions` |
| `eb` | `09` | `BgmWithReasons` |
| `ec` | `0a` | `UpmixSeries` |
| `ed` | `0b` | `DseeWithReasons` |

Function codes and inquiry codes are different namespaces. These functions must be advertised on table 1; the same code on table 2 does not select these layouts.

| Operation | Request | Response | Notification |
| --- | --- | --- | --- |
| Capability | `e0 inquiry` | `e1 inquiry ...` | — |
| Availability | `e2 inquiry` | `e3 inquiry ...` | `e5 inquiry ...` |
| Parameters | `e6 inquiry` | `e7 inquiry ...` | `e9 inquiry ...` |
| Set parameters | `e8 inquiry ...` | No dedicated SET response | State reports use `e9` |

All query payloads are two bytes. Inquiries `00` and `02` have no dynamic capability query: after discovering the function, use `AudioCapability::static_connection`. Their fixed priorities are sound quality and connection quality. The library rejects dynamic capability operations for these inquiries. Capability queries for the other layouts are available through `Request::AudioCapability`.

The module does not implement voice-contents inquiry `06`, personalized upmix inquiry `0c`, or head-tracking inquiry `0d`. Local cinema/game/music selection does not perform personalization, install profiles or configure external services.

## Capabilities

The following bodies follow `[e1, inquiry]`. Counts are unsigned bytes, and every body must end exactly after its declared records.

| Inquiry | Capability body |
| --- | --- |
| `01`, `0b` | `upscaling_type` |
| `03`, `04`, `07`, `09` | `exclusive_count, exclusive_function_ids...` |
| `05` | `priority_count, priorities..., ldac_exclusive_count, ldac_exclusive_ids...` |
| `08` | `flexibility, option_count, {option, mandatory}...` |
| `0a` | `item_count, items..., exclusive_count, exclusive_function_ids...` |

`UpscalingType` identifies DSEE HX (`00`), DSEE (`01`), DSEE Extreme (`02`) or DSEE Ultimate (`03`). Unknown technology IDs are preserved. The technology is independent of whether DSEE is configured as off or automatic.

Inquiry `05` requires at least two priority entries. Known priorities are sound quality (`00`), connection quality (`01`) and LE Audio low-latency priority (`02`, designated beta). Unknown entries remain available on reads. A setting write requires both a known priority and membership in the capability list. The LDAC exclusion identity `00` represents GATT connectivity; unknown exclusion identities are preserved. This metadata does not switch the host audio transport.

Inquiry `08` preserves `flexibility` as fixed (`00`) or flexible (`01`). Each option's `mandatory` byte is inverted: `00` mandatory, `01` optional. Options are standard (`00`), BGM (`01`), cinema (`02`), game (`03`) and music (`04`). The library checks advertised membership and mandatory inclusion on writes. The flexibility flag is retained as metadata without deriving a separate permission rule from it.

Upmix items for inquiry `0a` are none (`00`), cinema (`01`), game (`02`) and music (`03`). Exclusive-function lists describe compatibility with other device functions; the caller must reconcile them with current state. The library does not automatically disable conflicting settings.

## Availability

Every availability body begins with `availability`: `00` available and `01` unavailable. Additional fields are selected by inquiry:

| Inquiry | Additional fields after availability |
| --- | --- |
| `00`, `01`, `03`, `04`, `07` | None |
| `02`, `05` | `secondary_availability` (`00` enabled, `01` disabled) |
| `09`, `0b` | `reason_count, reason_ids...` |
| `08` | `assigned` (`00` yes, `01` no) |
| `0a` | `preferred_sound_effect` |

`AudioStatus::enabled` exposes the first flag as `true` when the control is available. It does not indicate whether the setting is switched on. `AudioStatusDetails` retains the additional fields. The second connection flag is exposed as `secondary_enabled`; it does not identify the currently negotiated codec.

Reason identities include calling (`00`), demo mode (`01`), LE Audio connected (`02`), LE Audio playing (`03`), control over GATT (`04`) and nonstandard listening mode (`0b`). Unknown reasons are retained. Preferred effects are headphone upmix (`00`) and BRAVIA 3D Surround (`01`), with unknown IDs preserved.

## Parameter layouts

The following bodies follow `[e7|e9, inquiry]`:

| Inquiry | Response body (`e7`) | Notification body (`e9`) |
| --- | --- | --- |
| `00`, `02` | `priority` | Same |
| `01`, `0b` | `dsee_mode` | Same |
| `03`, `09` | `on_off, room_size` | Same |
| `04`, `07` | `on_off` | Same |
| `05` | `priority` | `priority, switching_stream` |
| `08` | `count, listening_options...` | Same |
| `0a` | `upmix_item` | Same |

**DSEE uses `00` off, `01` auto. Other on/off fields in this module use `00` on, `01` off.** Room sizes are small (`00`), middle (`01`) and large (`02`). Room size remains present while BGM is off and must be retained when changing its switch.

For inquiry `05`, the notification's transition byte is none (`00`), LE Audio (`01`) or Classic Audio (`02`). `AudioParameterValue::ConnectionPriority` exposes this as `switching_stream: Option<SwitchingStream>`: `None` means the field is absent, while `Some(SwitchingStream::None)` means a notification reports no transition. A four-byte `e7 05` response or a three-byte `e9 05` notification is invalid. A notification is never accepted as a query response.

Priority, upmix and listening-option IDs remain lossless even when unknown. Closed switches, room sizes and transition values reject undefined bytes. Parameter decoding does not require a previously obtained capability descriptor.

## Writing settings

`AudioSetting::encode_set` validates against an `AudioCapability` and returns a table-1 payload for `Session::send`. The capability determines the exact inquiry.

```rust
use libmdr2::{
    audio::{AudioCapability, AudioSetting, DseeMode},
    types::CommandTable,
};

let capability = AudioCapability::decode_response(
    CommandTable::TABLE_1,
    &[0xe1, 0x0b, 0x02],
)?;
assert_eq!(
    AudioSetting::Dsee(DseeMode::Auto).encode_set(&capability)?,
    [0xe8, 0x0b, 0x01],
);
# Ok::<(), libmdr2::command::CommandError>(())
```

SET bodies use the response fields except for inquiry `05`, whose body is `priority, control`. The supported write form uses control byte `00`; the API does not infer a stream-switching meaning for that byte.

```rust
use libmdr2::{
    audio::{AudioCapability, AudioSetting, PriorityMode},
    types::CommandTable,
};

let capability = AudioCapability::decode_response(
    CommandTable::TABLE_1,
    &[0xe1, 0x05, 3, 0, 1, 2, 1, 0],
)?;
assert_eq!(
    AudioSetting::ConnectionPriority(PriorityMode::LOW_LATENCY)
        .encode_set(&capability)?,
    [0xe8, 0x05, 0x02, 0x00],
);
# Ok::<(), libmdr2::command::CommandError>(())
```

Listening assignment writes replace the whole list. Each selected option must be known, advertised exactly once, and distinct in the requested list; every mandatory option must remain included. A conflicting duplicate descriptor is not silently resolved. Selection order is retained. Upmix and connection writes likewise require known, advertised choices. No value is clamped or substituted.

The caller must check current availability, associate the capability with the same device connection and handle conflicting features. A link ACK confirms frame delivery only. Check parameter notifications or issue a serialized parameter read to confirm the resulting state; reports have no general transaction ID.
