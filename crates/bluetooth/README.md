# Bluetooth control transports

Internal Rust transport crate for the SONY MDR v2 desktop client. It exposes paired-device discovery, caller-selected RFCOMM/GATT connections, ordered raw receives, complete-frame writes, cancellation and native errors. It has no dependency on `libmdr2` and does not interpret commands, ACKs, settings or headphone model names. It is not published independently.

## API and ownership

`system_backend().await` returns the current platform's `Arc<dyn Backend>`. `Backend::discover` reads paired devices and reported MDR v2 services; it does not pair devices, turn Bluetooth on, or open an MDR control connection. A `DeviceId` is an opaque backend handle, not a model identifier or capability cache key. Discovery results are not a guarantee of current reachability or comprehensive service availability.

The caller selects a `DeviceId` and `TransportKind` and passes them to `Backend::connect`. There is no automatic protocol fallback or selection based on a model name. A successful connection is ready for protocol traffic; GATT preparation happens before success is reported.

`Connection` uniquely owns receiving and the connection lifecycle. `Connection::writer()` returns a cloneable `Writer` whose `write(Vec<u8>)` accepts one complete encoded frame. The backend fragments and serializes these writes. Success means every native fragment completed locally, not that a protocol ACK, command response or setting confirmation arrived. Reads continue while a write is in progress, and received chunks may contain part of a frame or several frames. The caller supplies framing, response matching and separate write/operation deadlines.

The default channels contain at most 32 messages, with configurable capacity limited to 1–1024. Stream and asynchronous receive loops apply backpressure. Native callbacks that cannot wait, including Windows GATT and macOS delegate callbacks, terminate the connection with an explicit error if their queue fills; they never discard bytes and continue as though the stream were intact. Each outgoing frame is limited to 1 MiB as a local allocation policy, distinct from a device's smaller native write limit. The protocol owner should impose its own tighter payload policy.

Cancelling or dropping an enqueued write future cancels the whole connection, because a partial frame cannot safely be followed by another frame. A reported write failure also cancels the connection. `Connection::cancel()` requests shutdown; `close().await` waits for the backend's cleanup result. Dropping `Connection` requests cancellation even when writer clones remain. The caller may apply a shutdown deadline, and must still isolate callbacks and state by connection generation. Connections have no reset operation.

`driver_channel` provides the same bounded contract to platform drivers and deterministic tests. Drivers receive complete `WriteRequest`s, acknowledge their entire writes through a oneshot result, and send ordered raw chunks. They must observe cancellation while reading, writing and waiting for queue capacity, release native resources, then call `Driver::finish`. Dropping an unfinished driver reports a terminal error instead of leaving waiters pending.

## Platform implementations

| Platform | Implemented transports | Discovery and lifecycle boundaries |
| --- | --- | --- |
| Linux | RFCOMM, GATT | BlueZ paired-device UUID cache on powered adapters; does not actively scan. Missing cached service UUIDs are not proof that the headphone lacks a service. |
| Windows | RFCOMM, GATT | Paired-device enumeration and cached native service queries; selected connections perform uncached service/characteristic resolution. Permission and cache failures can omit inaccessible devices or return an error. |
| macOS | RFCOMM | IOBluetooth paired-device SDP cache; no fresh SDP query. GATT is explicitly unsupported and is not advertised by this backend. |

Linux requires BlueZ, access to its system D-Bus APIs, an already paired device and a powered adapter. Building its native bindings requires the D-Bus development library and `pkg-config`. RFCOMM registers one MDR v2 client profile for the backend lifetime; BlueZ resolves the actual SDP channel. Only an explicitly pending device connection is accepted. Closing a device session closes its socket and disconnects that control profile. The shared registration remains available for reconnects and is unregistered when the backend is dropped; BlueZ unregistration is asynchronous. The code never uses a fixed RFCOMM channel or a legacy MDR service UUID.

Linux GATT uses BlueZ `AcquireWrite` to complete native MTU negotiation, reads the device's c91 writable value limit, and then acquires c61 notifications. The effective write limit is the smaller of the device value and the native writer's payload limit. This initial backend requires native support for acquiring notifications and write-without-response; unsupported characteristic/API combinations return an explicit error. Releasing GATT drops this client's acquired descriptors; it does not call the global device disconnect operation that would tear down unrelated audio profiles. BlueZ may retain the underlying shared Bluetooth connection.

Windows keeps WinRT objects on its native runtime thread. RFCOMM uses the service-provided host and channel. GATT requires an active native session with a valid current ATT MTU, caps the c91 limit by `MaxPduSize - 3`, and enables the c61 CCCD before returning. The OS owns MTU negotiation. Native resources and callback registrations are released on the owning thread. Application access must already be granted; this backend does not provide the initial Windows consent UI.

macOS keeps IOBluetooth objects and their run loop on a dedicated native thread. Delegate callbacks copy events into a bounded thread-safe bridge before the worker updates connection state; callbacks are not assumed to run on that thread. It uses the cached exact MDR v2 SDP record and its RFCOMM channel. Missing cache entries return `NotFound`; the implementation does not pretend that its paired-device cache is a complete device scan. An application's Bluetooth privacy declarations, signing/runtime environment and OS permission behavior still require macOS validation. The application main run loop must remain available for native callback delivery; the backend does not run it for the caller.

The macOS RFCOMM backend is a bounded preview: a process may make at most 16 native open attempts, after which it returns `Unavailable` with restart guidance. IOBluetooth does not retain its delegate, and its documented detach/close APIs do not provide a cross-thread callback-drain guarantee. Each exposed delegate therefore remains as a small inert receiver until process exit, with its event route removed so it retains no session or queue. An unconfirmed in-flight native write also retains its buffer and opaque callback token. Removing spare buffer capacity bounds retained frame data to 16 MiB across all attempts, plus small fixed object overhead. A completed write does not require buffer retention. This limit must remain until native callback and buffer lifetime guarantees can be established and validated.

Both Linux and Windows use the platform-led MTU preparation path. c91 is exactly two big-endian bytes describing writable value bytes, so it must not have ATT overhead subtracted a second time. The alternative device-led c93 notification is exactly one status byte (`1` ready, `0` failure); its strict parser is included, but the backends do not wait unconditionally for a notification that may have occurred before host-side negotiation completed. No guessed MTU confirmation payload is written.

## Validation

`fake::connection` exposes a manual deterministic peer. `fake::FakeBackend` supplies a fixed catalog and delivers selected connections to a test controller, which can inject fragments, defer/fail write completion and exercise cancellation. It never accesses a Bluetooth adapter.

`cargo test -p bluetooth` checks channel backpressure, read progress during pending writes, cancellation, cleanup acknowledgement, old-connection isolation, strict GATT preparation values, and the Linux byte pump's fragmentation, short writes and partial failures. These tests use in-memory channels and streams. They prove neither service availability nor headphone/firmware compatibility. Native target checks prove Rust API compatibility, not OS runtime behavior.

Initial WF-1000XM6 firmware 2.0.5 validation on Debian 13.7 / BlueZ 5.82 passed over RFCOMM using the application probe: discovery, standard handshake, identity and battery reads, clean release, and a second connection. This does not validate setting writes, changing notifications, GATT or fault recovery. Windows and macOS native runtime behavior remains unverified.

Hardware acceptance must record the actual device, firmware, OS, service list, transport, write limit, query/notification behavior and release/reconnect results. Discovery, cached services, successful compilation and synthetic traffic must not be reported as a completed hardware handshake.
