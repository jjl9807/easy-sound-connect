//! macOS IOBluetooth RFCOMM transport.
//!
//! Discovery uses the paired-device SDP cache. A device must advertise the MDR
//! v2 service in that cache before it can be opened. GATT is not implemented by
//! this backend. The application bundle needs Bluetooth privacy authorization.
//! Keep the application main run loop running while awaiting native callbacks;
//! the backend never runs or blocks that loop on behalf of its caller.

use std::{
    cell::RefCell,
    ffi::c_void,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, mpsc as native_channel},
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
};
use objc2_core_foundation::{
    CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, kCFRunLoopDefaultMode,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use objc2_io_bluetooth::{
    BluetoothHCIPowerState, IOBluetoothDevice, IOBluetoothHostController, IOBluetoothRFCOMMChannel,
    IOBluetoothRFCOMMChannelDelegate, IOBluetoothSDPUUID,
};
use tokio::sync::oneshot;

use crate::{
    Backend, BoxFuture, CancellationToken, Connection, ConnectionInfo, DEFAULT_CHANNEL_CAPACITY,
    Device, DeviceId, Driver, Error, ErrorKind, Result, TransportKind, WriteRequest,
    driver_channel,
};

const SERVICE_UUID: [u8; 16] = 0x956c7b26_d49a_4ba8_b03f_b17d393cb6e2_u128.to_be_bytes();
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);
// Apple documents an unretained delegate, but no cross-thread callback-drain
// guarantee when detaching it. Bound process-lifetime callback tombstones until
// a native quiescence contract can be established on supported macOS versions.
const MAX_NATIVE_SESSION_LIFETIMES: usize = 16;
static NATIVE_SESSION_LIFETIMES: Mutex<usize> = Mutex::new(0);

#[derive(Clone, Default)]
pub struct MacOsBackend;
impl MacOsBackend {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl Backend for MacOsBackend {
    fn discover(&self, cancel: CancellationToken) -> BoxFuture<'_, Result<Vec<Device>>> {
        Box::pin(async move {
            let cancel = cancel.child_token();
            let _guard = cancel.clone().drop_guard();
            let (sender, receiver) = oneshot::channel();
            let worker_cancel = cancel.clone();
            std::thread::Builder::new()
                .name("ezsc-macos-discovery".into())
                .spawn(move || {
                    let result = autoreleasepool(|_| paired_devices(&worker_cancel));
                    let _ = sender.send(result);
                })
                .map_err(Error::from)?;
            // The worker responds only after its autorelease pool has drained.
            // Cancellation must not acknowledge cleanup before that point.
            let result = receiver.await.unwrap_or_else(|_| {
                Err(Error::new(ErrorKind::Io, "macOS discovery worker stopped"))
            });
            if cancel.is_cancelled() {
                Err(Error::cancelled())
            } else {
                result
            }
        })
    }

    fn connect(
        &self,
        id: DeviceId,
        transport: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Connection>> {
        Box::pin(async move {
            if transport != TransportKind::Rfcomm {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "macOS CoreBluetooth GATT transport is not implemented",
                ));
            }
            let cancel = cancel.child_token();
            let guard = cancel.clone().drop_guard();
            let (sender, mut receiver) = oneshot::channel();
            let worker_cancel = cancel.clone();
            std::thread::Builder::new()
                .name("ezsc-macos-rfcomm".into())
                .spawn(move || {
                    autoreleasepool(|_| run_connection(id, worker_cancel, sender));
                })
                .map_err(Error::from)?;
            let mut interruption = None;
            let received = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    interruption = Some(Error::cancelled());
                    receiver.await
                },
                result = tokio::time::timeout(OPEN_TIMEOUT, &mut receiver) => match result {
                    Ok(result) => result,
                    Err(_) => {
                        interruption = Some(Error::new(ErrorKind::Timeout, "macOS RFCOMM connection timed out"));
                        cancel.cancel();
                        // Wait for the owner to release native setup resources.
                        receiver.await
                    }
                },
            };
            let mut result = received
                .unwrap_or_else(|_| Err(Error::new(ErrorKind::Io, "macOS RFCOMM worker stopped")));
            if interruption.is_none() && cancel.is_cancelled() {
                interruption = Some(Error::cancelled());
            }
            if let Some(error) = interruption {
                if let Ok(connection) = &mut result {
                    let _ = connection.close().await;
                }
                return Err(error);
            }
            if result.is_ok() {
                guard.disarm();
            }
            result
        })
    }
}

fn native_status(code: i32, operation: &str) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    let kind = match code as u32 {
        0xe00002c1 | 0xe00002e2 => ErrorKind::PermissionDenied,
        0xe00002c0 => ErrorKind::Unavailable,
        0xe00002d5 => ErrorKind::Busy,
        0xe00002d6 => ErrorKind::Timeout,
        0xe00002c7 => ErrorKind::Unsupported,
        0xe00002cd | 0xe00002d7 | 0xe00002d9 => ErrorKind::Disconnected,
        _ => ErrorKind::Io,
    };
    Err(Error::new(
        kind,
        format!(
            "macOS {operation} failed with IOReturn {:#010x}",
            code as u32
        ),
    ))
}

fn service_uuid() -> Result<Retained<IOBluetoothSDPUUID>> {
    // SAFETY: the UUID constructor copies these 16 bytes during the call.
    unsafe { IOBluetoothSDPUUID::uuidWithBytes_length(SERVICE_UUID.as_ptr().cast(), 16) }
        .ok_or_else(|| Error::new(ErrorKind::Io, "could not create MDR v2 SDP UUID"))
}

fn check_adapter() -> Result<()> {
    // SAFETY: the controller remains confined to this native worker.
    let controller =
        unsafe { IOBluetoothHostController::defaultController() }.ok_or_else(|| {
            Error::new(
                ErrorKind::Unavailable,
                "macOS Bluetooth controller is unavailable",
            )
        })?;
    if unsafe { controller.powerState() } != BluetoothHCIPowerState::ON {
        return Err(Error::new(
            ErrorKind::Unavailable,
            "macOS Bluetooth controller is powered off",
        ));
    }
    Ok(())
}

fn paired_devices(cancel: &CancellationToken) -> Result<Vec<Device>> {
    if cancel.is_cancelled() {
        return Err(Error::cancelled());
    }
    check_adapter()?;
    let uuid = service_uuid()?;
    let mut devices = Vec::new();
    // SAFETY: IOBluetooth owns the returned paired-device array. Runtime class
    // checks precede every use of its untyped elements.
    let Some(paired) = (unsafe { IOBluetoothDevice::pairedDevices() }) else {
        return Ok(devices);
    };
    for object in &*paired {
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let Some(device) = object.downcast_ref::<IOBluetoothDevice>() else {
            continue;
        };
        if unsafe { device.getServiceRecordForUUID(Some(&uuid)) }.is_none() {
            continue;
        }
        let Some(address) = (unsafe { device.addressString() }) else {
            continue;
        };
        let name = unsafe { device.name() }.to_string();
        devices.push(Device {
            id: DeviceId(format!("macos:{}", address)),
            name: (!name.is_empty()).then_some(name),
            transports: vec![TransportKind::Rfcomm],
        });
    }
    Ok(devices)
}

type CancelFuture = Pin<Box<dyn Future<Output = ()>>>;
struct PendingWrite {
    request: WriteRequest,
    offset: usize,
    in_flight: usize,
    // A real allocation stays valid until the callback or channel shutdown.
    cookie: Box<u8>,
}
struct State {
    id: DeviceId,
    ready: Option<oneshot::Sender<Result<Connection>>>,
    driver: Option<Driver>,
    channel: Option<Retained<IOBluetoothRFCOMMChannel>>,
    delegate_exposed: bool,
    pending: Option<PendingWrite>,
    mtu: usize,
    external_cancel: CancelFuture,
    connection_cancel: Option<CancelFuture>,
    result: Option<Result<()>>,
    wake: Option<Arc<RunLoopWake>>,
}

// Only the run-loop source signal and wake operations cross threads. Native
// IOBluetooth objects and State stay on the owning worker throughout their life.
// CoreFoundation explicitly permits signalling and waking from another thread.
struct RunLoopWake {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
}
unsafe impl Send for RunLoopWake {}
unsafe impl Sync for RunLoopWake {}
impl Wake for RunLoopWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.source.signal();
        self.run_loop.wake_up();
    }
}

enum CallbackEvent {
    Opened(i32),
    Data(Vec<u8>),
    Closed,
    Written { cookie: usize, status: i32 },
    Failed(Error),
}
struct CallbackBridge {
    events: native_channel::SyncSender<CallbackEvent>,
    wake: Mutex<Option<Arc<RunLoopWake>>>,
    failure: Mutex<Option<Error>>,
}
impl CallbackBridge {
    fn send(&self, event: CallbackEvent) {
        if self.events.try_send(event).is_err()
            && let Ok(mut failure) = self.failure.lock()
        {
            failure.get_or_insert_with(|| {
                Error::new(
                    ErrorKind::Io,
                    "macOS native callback queue overflow or worker closed",
                )
            });
        }
        if let Ok(wake) = self.wake.lock()
            && let Some(wake) = &*wake
        {
            wake.wake_by_ref();
        }
    }
}

// Native callbacks may arrive on the main dispatch queue on some macOS
// versions. The delegate owns only a thread-safe event bridge; no callback
// borrows worker state or touches an IOBluetooth object.
define_class!(
    #[unsafe(super = NSObject)]
    #[name = "EZSCMdrRfcommDelegate"]
    #[ivars = Mutex<Option<Arc<CallbackBridge>>>]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl IOBluetoothRFCOMMChannelDelegate for Delegate {
        #[unsafe(method(rfcommChannelOpenComplete:status:))]
        unsafe fn opened(&self, _channel: Option<&IOBluetoothRFCOMMChannel>, status: i32) {
            self.post(CallbackEvent::Opened(status));
        }
        #[unsafe(method(rfcommChannelData:data:length:))]
        unsafe fn received(&self, _channel: Option<&IOBluetoothRFCOMMChannel>, data: *mut c_void, length: usize) {
            if length == 0 { return; }
            if data.is_null() || length > crate::MAX_WRITE_BYTES {
                self.post(CallbackEvent::Failed(Error::new(ErrorKind::InvalidData, "invalid RFCOMM receive buffer")));
                return;
            }
            // SAFETY: IOBluetooth guarantees callback buffer validity until
            // this callback returns. Copy before crossing threads.
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec();
            self.post(CallbackEvent::Data(bytes));
        }
        #[unsafe(method(rfcommChannelClosed:))]
        unsafe fn closed(&self, _channel: Option<&IOBluetoothRFCOMMChannel>) {
            self.post(CallbackEvent::Closed);
        }
        #[unsafe(method(rfcommChannelWriteComplete:refcon:status:))]
        unsafe fn written(&self, _channel: Option<&IOBluetoothRFCOMMChannel>, cookie: *mut c_void, status: i32) {
            // The opaque identity is compared on the owner thread, never
            // dereferenced or used to access native memory from this callback.
            self.post(CallbackEvent::Written { cookie: cookie as usize, status });
        }
    }
);
impl Delegate {
    fn new(bridge: Arc<CallbackBridge>) -> Retained<Self> {
        let object = Self::alloc().set_ivars(Mutex::new(Some(bridge)));
        // SAFETY: NSObject's initializer has no additional preconditions.
        unsafe { msg_send![super(object), init] }
    }
    fn post(&self, event: CallbackEvent) {
        // Copy the route while locked, then release the lock before calling the
        // bridge. An in-flight callback may retain its route safely; a callback
        // that starts after shutdown sees no route and does nothing.
        let route = self.ivars().lock().ok().and_then(|route| route.clone());
        if let Some(route) = route {
            route.send(event);
        }
    }
}

struct Worker {
    state: RefCell<State>,
    delegate: Retained<Delegate>,
    bridge: Arc<CallbackBridge>,
    events: native_channel::Receiver<CallbackEvent>,
}
impl Worker {
    fn handle(&self, event: CallbackEvent) -> Result<()> {
        match event {
            CallbackEvent::Opened(status) => {
                native_status(status, "RFCOMM open")?;
                let channel =
                    self.state.borrow().channel.clone().ok_or_else(|| {
                        Error::new(ErrorKind::Io, "RFCOMM open returned no channel")
                    })?;
                let mtu = unsafe { channel.getMTU() } as usize;
                if mtu == 0 {
                    return Err(Error::new(ErrorKind::InvalidData, "RFCOMM MTU is zero"));
                }
                let mut state = self.state.borrow_mut();
                if state.result.is_some() {
                    return Ok(());
                }
                if state.driver.is_some() {
                    return Err(Error::new(
                        ErrorKind::Io,
                        "duplicate RFCOMM open completion",
                    ));
                }
                let info = ConnectionInfo {
                    device: state.id.clone(),
                    transport: TransportKind::Rfcomm,
                    max_write_size: Some(mtu),
                };
                let (connection, driver) = driver_channel(info, DEFAULT_CHANNEL_CAPACITY)?;
                state.connection_cancel = Some(Box::pin(driver.cancel.clone().cancelled_owned()));
                state.driver = Some(driver);
                state.mtu = mtu;
                if let Some(ready) = state.ready.take() {
                    ready.send(Ok(connection)).map_err(|_| Error::cancelled())?;
                }
                Ok(())
            }
            CallbackEvent::Data(bytes) => {
                let state = self.state.borrow();
                state
                    .driver
                    .as_ref()
                    .ok_or_else(|| {
                        Error::new(ErrorKind::Io, "RFCOMM data arrived before open completion")
                    })
                    .and_then(|driver| {
                        driver.incoming.try_send(bytes).map_err(|_| {
                            Error::new(
                                ErrorKind::Io,
                                "RFCOMM receive queue overflow or receiver closed",
                            )
                        })
                    })
            }
            CallbackEvent::Closed => Err(Error::new(
                ErrorKind::Disconnected,
                "RFCOMM peer closed the channel",
            )),
            CallbackEvent::Failed(error) => Err(error),
            CallbackEvent::Written { cookie, status } => {
                native_status(status, "RFCOMM write")?;
                let mut state = self.state.borrow_mut();
                match state.pending.as_mut() {
                    Some(pending)
                        if (&*pending.cookie as *const u8) as usize == cookie
                            && pending.in_flight > 0 =>
                    {
                        pending.offset += pending.in_flight;
                        pending.in_flight = 0;
                        if pending.offset == pending.request.bytes.len() {
                            let pending =
                                state.pending.take().expect("pending write checked above");
                            let _ = pending.request.completion.send(Ok(()));
                        }
                        Ok(())
                    }
                    _ => Err(Error::new(
                        ErrorKind::Io,
                        "unexpected RFCOMM write completion",
                    )),
                }
            }
        }
    }
    fn stop(&self, result: Result<()>) {
        let mut state = self.state.borrow_mut();
        if state.result.is_none() {
            state.result = Some(result);
        }
        if let Some(wake) = &state.wake {
            wake.run_loop.stop();
        }
    }
    fn poll(&self) {
        let failure = self
            .bridge
            .failure
            .lock()
            .ok()
            .and_then(|mut failure| failure.take());
        if let Some(error) = failure {
            self.stop(Err(error));
            return;
        }
        for index in 0..DEFAULT_CHANNEL_CAPACITY {
            match self.events.try_recv() {
                Ok(event) => {
                    if let Err(error) = self.handle(event) {
                        self.stop(Err(error));
                        return;
                    }
                    if index + 1 == DEFAULT_CHANNEL_CAPACITY
                        && let Some(wake) = &self.state.borrow().wake
                    {
                        wake.wake_by_ref();
                    }
                }
                Err(native_channel::TryRecvError::Empty) => break,
                Err(native_channel::TryRecvError::Disconnected) => {
                    self.stop(Err(Error::new(
                        ErrorKind::Disconnected,
                        "macOS callback bridge stopped",
                    )));
                    return;
                }
            }
        }
        let Some(wake) = self.state.borrow().wake.clone() else {
            return;
        };
        let waker = Waker::from(wake);
        let mut context = Context::from_waker(&waker);
        let should_stop = {
            let mut state = self.state.borrow_mut();
            state.external_cancel.as_mut().poll(&mut context).is_ready()
                || state
                    .connection_cancel
                    .as_mut()
                    .is_some_and(|cancel| cancel.as_mut().poll(&mut context).is_ready())
        };
        if should_stop {
            self.stop(Ok(()));
            return;
        }
        let write = {
            let mut state = self.state.borrow_mut();
            if state.result.is_some() {
                return;
            }
            if state.pending.is_none()
                && let Some(driver) = &mut state.driver
            {
                match driver.writes.poll_recv(&mut context) {
                    Poll::Ready(Some(mut request)) => {
                        // The lifetime bound counts payload bytes, including on
                        // cancellation; do not retain a caller's spare capacity.
                        request.bytes = request.bytes.into_boxed_slice().into_vec();
                        state.pending = Some(PendingWrite {
                            request,
                            offset: 0,
                            in_flight: 0,
                            cookie: Box::new(0),
                        })
                    }
                    Poll::Ready(None) => {
                        drop(state);
                        self.stop(Ok(()));
                        return;
                    }
                    Poll::Pending => return,
                }
            }
            let mtu = state.mtu;
            let channel = state.channel.clone();
            state.pending.as_mut().and_then(|pending| {
                if pending.in_flight != 0 {
                    return None;
                }
                let length = (pending.request.bytes.len() - pending.offset).min(mtu);
                pending.in_flight = length;
                Some((
                    channel,
                    unsafe { pending.request.bytes.as_mut_ptr().add(pending.offset) },
                    length,
                    (&mut *pending.cookie as *mut u8).cast::<c_void>(),
                ))
            })
        };
        if let Some((Some(channel), bytes, length, cookie)) = write {
            // SAFETY: the pending frame and cookie remain owned by State until
            // completion or after closeChannel; no borrow crosses native calls.
            let result =
                unsafe { channel.writeAsync_length_refcon(bytes.cast(), length as u16, cookie) };
            if let Err(error) = native_status(result, "RFCOMM write submission") {
                self.stop(Err(error));
            }
        }
    }
}

unsafe extern "C-unwind" fn perform_source(info: *mut c_void) {
    // SAFETY: the boxed worker stays at this address until the source has been
    // removed and invalidated. Only its owning run loop invokes this callback.
    let worker = unsafe { &*info.cast::<Worker>() };
    autoreleasepool(|_| worker.poll());
}

fn run_connection(
    id: DeviceId,
    cancel: CancellationToken,
    ready: oneshot::Sender<Result<Connection>>,
) {
    let state = RefCell::new(State {
        id,
        ready: Some(ready),
        driver: None,
        channel: None,
        delegate_exposed: false,
        pending: None,
        mtu: 0,
        external_cancel: Box::pin(cancel.cancelled_owned()),
        connection_cancel: None,
        result: None,
        wake: None,
    });
    let (events_tx, events) = native_channel::sync_channel(DEFAULT_CHANNEL_CAPACITY);
    let bridge = Arc::new(CallbackBridge {
        events: events_tx,
        wake: Mutex::new(None),
        failure: Mutex::new(None),
    });
    let delegate = Delegate::new(bridge.clone());
    let worker = Box::new(Worker {
        state,
        delegate,
        bridge,
        events,
    });
    let result = run_native(&worker);
    let (channel, mut pending, driver, ready, mut result, wake, delegate_exposed) = {
        let mut state = worker.state.borrow_mut();
        let result = state.result.take().unwrap_or(result);
        (
            state.channel.take(),
            state.pending.take(),
            state.driver.take(),
            state.ready.take(),
            result,
            state.wake.take(),
            state.delegate_exposed,
        )
    };
    // Detach callbacks, close only this control channel, then release the buffer
    // and native owners before acknowledging transport shutdown.
    if let Some(channel) = channel {
        // Stop new delivery and close this control channel. These operations
        // are not treated as proof that previously dispatched callbacks drained.
        let detached = native_status(
            unsafe { channel.setDelegate(None) },
            "RFCOMM delegate removal",
        );
        let closed = native_status(unsafe { channel.closeChannel() }, "RFCOMM close");
        let cleanup = detached.and(closed);
        if result.is_ok() {
            result = cleanup;
        }
    }
    if let Some(wake) = wake {
        wake.run_loop
            .remove_source(Some(&wake.source), unsafe { kCFRunLoopDefaultMode });
        wake.source.invalidate();
    }
    if let Ok(mut wake) = worker.bridge.wake.lock() {
        *wake = None;
    }
    if delegate_exposed {
        // IOBluetooth does not retain its delegate and does not document a
        // cross-thread callback join. Keep this tiny receiver valid for any
        // late message, even after successful detach/close. Disconnect its
        // bridge first so it retains neither a queue nor a session.
        if let Ok(mut route) = worker.delegate.ivars().lock() {
            *route = None;
        }
        std::mem::forget(worker.delegate.clone());
        if pending.as_ref().is_some_and(|frame| frame.in_flight > 0) {
            // Close does not document a buffer-drain barrier either. A pending
            // native write retains at most MAX_WRITE_BYTES, without spare Vec
            // capacity. The process-wide attempt limit bounds this retention.
            if let Some(mut frame) = pending.take() {
                let (unused, _) = oneshot::channel();
                let completion = std::mem::replace(&mut frame.request.completion, unused);
                let _ = completion.send(Err(result.clone().err().unwrap_or_else(Error::cancelled)));
                std::mem::forget(frame);
            }
        }
    }
    drop(worker);
    if let Some(pending) = pending {
        let _ = pending
            .request
            .completion
            .send(Err(result.clone().err().unwrap_or_else(Error::cancelled)));
    }
    if let Some(ready) = ready {
        let _ = ready.send(Err(result.clone().err().unwrap_or_else(Error::cancelled)));
    }
    if let Some(driver) = driver {
        driver.finish(result);
    }
}

fn run_native(worker: &Worker) -> Result<()> {
    check_adapter()?;
    let id = worker.state.borrow().id.clone();
    let address =
        id.0.strip_prefix("macos:")
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "not a macOS Bluetooth identity"))?;
    if address.len() != 17
        || !address.bytes().enumerate().all(|(index, b)| {
            if index % 3 == 2 {
                b == b'-' || b == b':'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid macOS Bluetooth address",
        ));
    }
    let address = NSString::from_str(address);
    // SAFETY: calls and every native object are confined to this worker thread.
    let device = unsafe { IOBluetoothDevice::deviceWithAddressString(Some(&address)) }
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "macOS Bluetooth device not found"))?;
    if !unsafe { device.isPaired() } {
        return Err(Error::new(
            ErrorKind::NotPaired,
            "pair the device in macOS Bluetooth settings",
        ));
    }
    let uuid = service_uuid()?;
    let service = unsafe { device.getServiceRecordForUUID(Some(&uuid)) }.ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "MDR v2 RFCOMM service absent from macOS SDP cache",
        )
    })?;
    let mut channel_id = 0;
    native_status(
        unsafe { service.getRFCOMMChannelID(&mut channel_id) },
        "SDP RFCOMM channel lookup",
    )?;
    if channel_id == 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "SDP returned RFCOMM channel zero",
        ));
    }
    let run_loop = CFRunLoop::current()
        .ok_or_else(|| Error::new(ErrorKind::Unavailable, "macOS worker has no run loop"))?;
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: (worker as *const Worker).cast_mut().cast(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(perform_source),
    };
    let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }.ok_or_else(|| {
        Error::new(
            ErrorKind::Unavailable,
            "could not create macOS Bluetooth run-loop source",
        )
    })?;
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
    let wake = Arc::new(RunLoopWake { run_loop, source });
    worker.state.borrow_mut().wake = Some(wake.clone());
    *worker
        .bridge
        .wake
        .lock()
        .map_err(|_| Error::new(ErrorKind::Io, "macOS wake bridge poisoned"))? = Some(wake.clone());
    // Register cancellation before starting the native asynchronous open.
    worker.poll();
    if worker.state.borrow().result.is_some() {
        return Ok(());
    }
    {
        let mut lifetimes = NATIVE_SESSION_LIFETIMES
            .lock()
            .map_err(|_| Error::new(ErrorKind::Io, "macOS native lifetime counter poisoned"))?;
        if *lifetimes >= MAX_NATIVE_SESSION_LIFETIMES {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "macOS RFCOMM preview reached its 16-session native lifetime limit; restart the application before reconnecting",
            ));
        }
        *lifetimes += 1;
    }
    worker.state.borrow_mut().delegate_exposed = true;
    let mut channel = None;
    native_status(
        unsafe {
            device.openRFCOMMChannelAsync_withChannelID_delegate(
                Some(&mut channel),
                channel_id,
                Some(&*worker.delegate as &AnyObject),
            )
        },
        "RFCOMM open submission",
    )?;
    let channel = channel
        .ok_or_else(|| Error::new(ErrorKind::Io, "RFCOMM open returned no native channel"))?;
    worker.state.borrow_mut().channel = Some(channel);
    wake.wake_by_ref();
    while worker.state.borrow().result.is_none() {
        // Drain native autoreleases after each handled source. The long bound
        // is only a run-loop safety deadline; writes and cancellation wake the
        // loop through the source, without periodic polling.
        autoreleasepool(|_| {
            CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 86_400.0, true)
        });
    }
    Ok(())
}
