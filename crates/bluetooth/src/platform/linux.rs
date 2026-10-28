//! Linux control transports using BlueZ's native D-Bus and RFCOMM interfaces.
//!
//! Enumeration reads paired devices and cached service UUIDs. It does not scan,
//! pair, power adapters on, or open control connections. RFCOMM uses a client
//! profile, so BlueZ performs SDP channel selection. GATT release stops this
//! client's notification/write sessions without disconnecting audio profiles.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use bluer::{Address, DeviceEvent, DeviceProperty, Uuid, rfcomm};
use futures_util::StreamExt;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, oneshot},
};

use crate::{
    Backend, BoxFuture, CancellationToken, Connection, ConnectionInfo, DEFAULT_CHANNEL_CAPACITY,
    Device, DeviceId, Driver, Error, ErrorKind, GATT_NOTIFY, GATT_SERVICE, GATT_WRITE,
    GATT_WRITE_LENGTH, RFCOMM_SERVICE, Result, TransportKind, driver_channel,
};

const OPEN_TIMEOUT: Duration = Duration::from_secs(25);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct LinuxBackend {
    session: bluer::Session,
    profile: Arc<Mutex<Option<Arc<ProfileHub>>>>,
    active: Arc<StdMutex<HashSet<DeviceId>>>,
}

impl LinuxBackend {
    pub async fn new() -> Result<Self> {
        let session = tokio::time::timeout(OPEN_TIMEOUT, bluer::Session::new())
            .await
            .map_err(|_| timeout("opening BlueZ D-Bus session"))?
            .map_err(native)?;
        Ok(Self {
            session,
            profile: Arc::new(Mutex::new(None)),
            active: Arc::new(StdMutex::new(HashSet::new())),
        })
    }

    async fn enumerate(&self) -> Result<Vec<Device>> {
        let mut output = Vec::new();
        let mut powered = false;
        for adapter_name in self.session.adapter_names().await.map_err(native)? {
            let adapter = self.session.adapter(&adapter_name).map_err(native)?;
            if !adapter.is_powered().await.map_err(native)? {
                continue;
            }
            powered = true;
            for address in adapter.device_addresses().await.map_err(native)? {
                let device = adapter.device(address).map_err(native)?;
                if !device.is_paired().await.map_err(native)? {
                    continue;
                }
                let uuids = device.uuids().await.map_err(native)?.unwrap_or_default();
                let mut transports = Vec::new();
                if uuids.contains(&uuid(RFCOMM_SERVICE)) {
                    transports.push(TransportKind::Rfcomm);
                }
                if uuids.contains(&uuid(GATT_SERVICE)) {
                    transports.push(TransportKind::Gatt);
                }
                if !transports.is_empty() {
                    output.push(Device {
                        id: DeviceId(format!("bluez/{adapter_name}/{address}")),
                        name: device.name().await.map_err(native)?,
                        transports,
                    });
                }
            }
        }
        if !powered {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "no powered BlueZ adapter",
            ));
        }
        output.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(output)
    }

    async fn selected_device(&self, id: &DeviceId, kind: TransportKind) -> Result<bluer::Device> {
        let (adapter_name, address) = parse_id(id)?;
        let adapter = self.session.adapter(adapter_name).map_err(native)?;
        if !adapter.is_powered().await.map_err(native)? {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "selected Bluetooth adapter is powered off",
            ));
        }
        let device = adapter.device(address).map_err(native)?;
        if !device.is_paired().await.map_err(native)? {
            return Err(Error::new(
                ErrorKind::NotPaired,
                "selected device is not paired",
            ));
        }
        let service = match kind {
            TransportKind::Rfcomm => RFCOMM_SERVICE,
            TransportKind::Gatt => GATT_SERVICE,
        };
        if !device
            .uuids()
            .await
            .map_err(native)?
            .unwrap_or_default()
            .contains(&uuid(service))
        {
            return Err(Error::new(
                ErrorKind::NotFound,
                "selected v2 service is not reported by BlueZ",
            ));
        }
        Ok(device)
    }

    async fn acquire_profile(&self) -> Result<Arc<ProfileHub>> {
        let mut slot = self.profile.lock().await;
        if let Some(hub) = &*slot
            && !hub.cancel.is_cancelled()
        {
            return Ok(hub.clone());
        }
        // Registration is completed before cancellation is handled, so an
        // in-flight RegisterProfile call cannot leave an orphan registration.
        let handle = self
            .session
            .register_profile(rfcomm::Profile {
                uuid: uuid(RFCOMM_SERVICE),
                name: Some("Easy Sound Connect".into()),
                role: Some(rfcomm::Role::Client),
                require_authentication: Some(true),
                require_authorization: Some(false),
                auto_connect: Some(false),
                ..Default::default()
            })
            .await
            .map_err(native)?;
        let hub = ProfileHub::start(handle);
        *slot = Some(hub.clone());
        Ok(hub)
    }

    async fn release_profile(&self, hub: Arc<ProfileHub>) {
        // Registration belongs to this backend, while each stream belongs to a
        // device session. Keeping the shared registration avoids racing bluer's
        // asynchronous unregistration against an immediate reconnect. Backend
        // destruction drops the last hub and unregisters its profile.
        drop(hub);
    }

    async fn open_rfcomm(
        &self,
        id: DeviceId,
        device: bluer::Device,
        cancel: CancellationToken,
        lease: DeviceLease,
    ) -> Result<Connection> {
        let hub = self.acquire_profile().await?;
        let address = device.address();
        let (sender, receiver) = oneshot::channel();
        let inserted = {
            let mut pending = lock(&hub.pending);
            if let std::collections::hash_map::Entry::Vacant(entry) = pending.entry(address) {
                entry.insert(sender);
                true
            } else {
                false
            }
        };
        if !inserted {
            self.release_profile(hub).await;
            return Err(Error::new(
                ErrorKind::Busy,
                "RFCOMM connection already pending for this address",
            ));
        }
        let connected = bounded(&cancel, async {
            // NewConnection must be accepted while ConnectProfile is in flight.
            let service = uuid(RFCOMM_SERVICE);
            let connect = device.connect_profile(&service);
            let accept = async {
                receiver.await.map_err(|_| {
                    Error::new(ErrorKind::Disconnected, "BlueZ profile router stopped")
                })?
            };
            let (_, accepted) = tokio::try_join!(async { connect.await.map_err(native) }, accept)?;
            Ok(accepted)
        })
        .await;
        lock(&hub.pending).remove(&address);
        let accepted = match connected {
            Ok(accepted) => accepted,
            Err(error) => {
                let _ = disconnect_profile(&device).await;
                self.release_profile(hub).await;
                return Err(error);
            }
        };
        let (connection, driver) = driver_channel(
            ConnectionInfo {
                device: id,
                transport: TransportKind::Rfcomm,
                max_write_size: None,
            },
            DEFAULT_CHANNEL_CAPACITY,
        )?;
        let backend = self.clone();
        tokio::spawn(async move {
            let Accepted { stream, closed } = accepted;
            let (read, write) = tokio::io::split(stream);
            let result = run_stream(driver, read, write, None, cancel, closed).await;
            let (driver, result) = result;
            let cleanup = disconnect_profile(&device).await;
            backend.release_profile(hub).await;
            drop(lease);
            driver.finish(result.and(cleanup));
        });
        Ok(connection)
    }

    async fn open_gatt(
        &self,
        id: DeviceId,
        device: bluer::Device,
        cancel: CancellationToken,
        lease: DeviceLease,
    ) -> Result<Connection> {
        let prepared = bounded(&cancel, prepare_gatt(&device)).await?;
        let GattPrepared {
            reader,
            writer,
            limit,
        } = prepared;
        let events = bounded(&cancel, async { device.events().await.map_err(native) }).await?;
        let closed: BoxFuture<'static, ()> = Box::pin(async move {
            tokio::pin!(events);
            while let Some(event) = events.next().await {
                if matches!(
                    event,
                    DeviceEvent::PropertyChanged(
                        DeviceProperty::Connected(false) | DeviceProperty::ServicesResolved(false)
                    )
                ) {
                    return;
                }
            }
        });
        let (connection, driver) = driver_channel(
            ConnectionInfo {
                device: id,
                transport: TransportKind::Gatt,
                max_write_size: Some(limit),
            },
            DEFAULT_CHANNEL_CAPACITY,
        )?;
        tokio::spawn(async move {
            let (driver, result) =
                run_stream(driver, reader, writer, Some(limit), cancel, closed).await;
            // The acquired descriptors are closed by run_stream. Never call
            // Device.Disconnect here: that also tears down unrelated audio.
            drop(lease);
            driver.finish(result);
        });
        Ok(connection)
    }
}

impl Backend for LinuxBackend {
    fn discover(&self, cancel: CancellationToken) -> BoxFuture<'_, Result<Vec<Device>>> {
        Box::pin(async move { bounded(&cancel, self.enumerate()).await })
    }

    fn connect(
        &self,
        id: DeviceId,
        kind: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Connection>> {
        let backend = self.clone();
        Box::pin(async move {
            let lease = DeviceLease::acquire(backend.active.clone(), id.clone())?;
            let task_cancel = cancel.child_token();
            let mut guard = CancelSetup(Some(task_cancel.clone()));
            let (sender, receiver) = oneshot::channel();
            tokio::spawn(async move {
                let result = match bounded(&task_cancel, backend.selected_device(&id, kind)).await {
                    Ok(device) => match kind {
                        TransportKind::Rfcomm => {
                            backend.open_rfcomm(id, device, task_cancel, lease).await
                        }
                        TransportKind::Gatt => {
                            backend.open_gatt(id, device, task_cancel, lease).await
                        }
                    },
                    Err(error) => Err(error),
                };
                // If the caller stopped waiting, dropping a successful result
                // cancels its driver and releases the acquired resources.
                let _ = sender.send(result);
            });
            let result = receiver
                .await
                .map_err(|_| Error::new(ErrorKind::Disconnected, "connection setup task ended"))?;
            if result.is_ok() {
                guard.0 = None;
            }
            result
        })
    }
}

struct CancelSetup(Option<CancellationToken>);
impl Drop for CancelSetup {
    fn drop(&mut self) {
        if let Some(cancel) = &self.0 {
            cancel.cancel();
        }
    }
}

struct DeviceLease {
    active: Arc<StdMutex<HashSet<DeviceId>>>,
    id: DeviceId,
}
impl DeviceLease {
    fn acquire(active: Arc<StdMutex<HashSet<DeviceId>>>, id: DeviceId) -> Result<Self> {
        if !lock(&active).insert(id.clone()) {
            return Err(Error::new(
                ErrorKind::Busy,
                "device already has an active control connection",
            ));
        }
        Ok(Self { active, id })
    }
}
impl Drop for DeviceLease {
    fn drop(&mut self) {
        lock(&self.active).remove(&self.id);
    }
}

struct Accepted {
    stream: rfcomm::Stream,
    closed: BoxFuture<'static, ()>,
}
type Pending = Arc<StdMutex<HashMap<Address, oneshot::Sender<Result<Accepted>>>>>;

struct ProfileHub {
    pending: Pending,
    cancel: CancellationToken,
}

impl ProfileHub {
    fn start(mut handle: rfcomm::ProfileHandle) -> Arc<Self> {
        let pending: Pending = Arc::default();
        let cancel = CancellationToken::new();
        let hub = Arc::new(Self {
            pending: pending.clone(),
            cancel: cancel.clone(),
        });
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    request = handle.next() => {
                        let Some(request) = request else { break; };
                        let waiter = lock(&pending).remove(&request.device());
                        if let Some(waiter) = waiter {
                            let closed: BoxFuture<'static, ()> = Box::pin(request.closed());
                            let stream = request.accept().map_err(native);
                            let _ = waiter.send(stream.map(|stream| Accepted { stream, closed }));
                        } else {
                            request.reject(rfcomm::ReqError::Rejected);
                        }
                    }
                }
            }
            lock(&pending).clear();
            cancel.cancel();
            drop(handle);
        });
        hub
    }
}
impl Drop for ProfileHub {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct GattPrepared {
    reader: bluer::gatt::CharacteristicReader,
    writer: bluer::gatt::CharacteristicWriter,
    limit: usize,
}

async fn prepare_gatt(device: &bluer::Device) -> Result<GattPrepared> {
    if !device.is_connected().await.map_err(native)? {
        device.connect().await.map_err(native)?;
    }
    let mut selected = None;
    for service in device.services().await.map_err(native)? {
        if service.uuid().await.map_err(native)? == uuid(GATT_SERVICE)
            && selected.replace(service).is_some()
        {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "duplicate MDR v2 GATT services",
            ));
        }
    }
    let service = selected.ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "MDR v2 GATT service unavailable after resolution",
        )
    })?;
    let mut chars = HashMap::new();
    for characteristic in service.characteristics().await.map_err(native)? {
        let key = characteristic.uuid().await.map_err(native)?;
        if chars.insert(key, characteristic).is_some() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "duplicate GATT characteristic UUID",
            ));
        }
    }
    let take = |id: &str| {
        chars.get(&uuid(id)).cloned().ok_or_else(|| {
            Error::new(
                ErrorKind::NotFound,
                format!("required GATT characteristic {id} is missing"),
            )
        })
    };
    let write = take(GATT_WRITE)?;
    let notify = take(GATT_NOTIFY)?;
    let length = take(GATT_WRITE_LENGTH)?;
    let flags = write.flags().await.map_err(native)?;
    if !flags.write_without_response {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "MDR GATT characteristic lacks write-without-response support",
        ));
    }
    // AcquireWrite waits for BlueZ's host-side MTU exchange and returns the
    // writable payload limit. This is the platform-led preparation branch;
    // waiting for a later device-led c93 notification would race readiness.
    let writer = write.write_io().await.map_err(native)?;
    let advertised = crate::gatt::parse_write_limit(&length.read().await.map_err(native)?)?;
    // bluer already adjusts its MTU to writable value bytes. Do not subtract
    // ATT overhead from either this value or the device's c91 payload limit.
    let limit = advertised.min(writer.mtu());
    if limit == 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "BlueZ returned a zero writable payload limit",
        ));
    }
    let reader = notify.notify_io().await.map_err(native)?;
    Ok(GattPrepared {
        reader,
        writer,
        limit,
    })
}

// Owns Driver until both I/O futures and native resources have been dropped.
async fn run_stream<R, W>(
    mut driver: Driver,
    mut reader: R,
    mut writer: W,
    fragment: Option<usize>,
    external: CancellationToken,
    closed: BoxFuture<'static, ()>,
) -> (Driver, Result<()>)
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let incoming = driver.incoming.clone();
    let cancel = driver.cancel.clone();
    let result = {
        let reading = async {
            let mut buffer = vec![0; 8192];
            loop {
                let count = reader.read(&mut buffer).await.map_err(Error::from)?;
                if count == 0 {
                    return Err(Error::new(
                        ErrorKind::Disconnected,
                        "Bluetooth read reached end of stream",
                    ));
                }
                incoming
                    .send(buffer[..count].to_vec())
                    .await
                    .map_err(|_| Error::cancelled())?;
            }
        };
        let writing = async {
            while let Some(request) = driver.writes.recv().await {
                let mut result = Ok(());
                for part in request
                    .bytes
                    .chunks(fragment.unwrap_or(request.bytes.len()))
                {
                    if let Err(error) = writer.write_all(part).await {
                        result = Err(Error::from(error));
                        break;
                    }
                }
                if result.is_ok() {
                    result = writer.flush().await.map_err(Error::from);
                }
                let _ = request.completion.send(result.clone());
                result?;
            }
            Ok(())
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Ok(()),
            _ = external.cancelled() => Ok(()),
            _ = closed => Err(Error::new(ErrorKind::Disconnected, "Bluetooth control service disconnected")),
            result = reading => result,
            result = writing => result,
        }
    };
    cancel.cancel();
    drop(incoming);
    drop(reader);
    drop(writer);
    (driver, result)
}

async fn disconnect_profile(device: &bluer::Device) -> Result<()> {
    match tokio::time::timeout(
        CLEANUP_TIMEOUT,
        device.disconnect_profile(&uuid(RFCOMM_SERVICE)),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error))
            if matches!(
                error.kind,
                bluer::ErrorKind::NotFound | bluer::ErrorKind::DoesNotExist
            ) =>
        {
            Ok(())
        }
        Ok(Err(error)) => Err(native(error)),
        Err(_) => Err(timeout("releasing RFCOMM control profile")),
    }
}

async fn bounded<T>(
    cancel: &CancellationToken,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Error::cancelled()),
        result = tokio::time::timeout(OPEN_TIMEOUT, future) => result.map_err(|_| timeout("Bluetooth setup/discovery"))?,
    }
}

fn parse_id(id: &DeviceId) -> Result<(&str, Address)> {
    let mut parts = id.0.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("bluez"), Some(adapter), Some(address), None) if !adapter.is_empty() => {
            let address = address
                .parse()
                .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid BlueZ device address"))?;
            Ok((adapter, address))
        }
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "device identity belongs to another backend",
        )),
    }
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("constant service UUID")
}
fn timeout(operation: &str) -> Error {
    Error::new(ErrorKind::Timeout, format!("timed out: {operation}"))
}
fn lock<T>(mutex: &StdMutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn native(error: bluer::Error) -> Error {
    use bluer::ErrorKind as BlueZ;
    let kind = match error.kind {
        BlueZ::NotAuthorized
        | BlueZ::NotPermitted
        | BlueZ::AuthenticationRejected
        | BlueZ::AuthenticationFailed => ErrorKind::PermissionDenied,
        BlueZ::NotAvailable | BlueZ::NotReady => ErrorKind::Unavailable,
        BlueZ::NotFound | BlueZ::DoesNotExist => ErrorKind::NotFound,
        BlueZ::NotSupported => ErrorKind::Unsupported,
        BlueZ::InProgress | BlueZ::AlreadyExists | BlueZ::AlreadyConnected => ErrorKind::Busy,
        BlueZ::AuthenticationCanceled => ErrorKind::Cancelled,
        BlueZ::AuthenticationTimeout => ErrorKind::Timeout,
        BlueZ::InvalidArguments
        | BlueZ::InvalidLength
        | BlueZ::InvalidOffset
        | BlueZ::InvalidAddress(_)
        | BlueZ::InvalidName(_) => ErrorKind::InvalidData,
        BlueZ::NotificationSessionStopped
        | BlueZ::ServicesUnresolved
        | BlueZ::ConnectionAttemptFailed => ErrorKind::Disconnected,
        _ => ErrorKind::Io,
    };
    Error::new(kind, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    #[derive(Default)]
    struct Written {
        bytes: Vec<u8>,
        calls: Vec<usize>,
    }

    struct Recorder {
        output: Arc<StdMutex<Written>>,
        short_write: usize,
        fail_after: Option<usize>,
    }

    impl AsyncWrite for Recorder {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut output = lock(&self.output);
            let remaining = self
                .fail_after
                .map_or(usize::MAX, |limit| limit.saturating_sub(output.bytes.len()));
            if remaining == 0 {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "injected partial write failure",
                )));
            }
            let count = bytes.len().min(self.short_write).min(remaining);
            output.calls.push(bytes.len());
            output.bytes.extend_from_slice(&bytes[..count]);
            Poll::Ready(Ok(count))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    fn stream_fixture(
        fail_after: Option<usize>,
    ) -> (Connection, tokio::io::DuplexStream, Arc<StdMutex<Written>>) {
        let (connection, driver) = driver_channel(
            ConnectionInfo {
                device: DeviceId("test/stream".into()),
                transport: TransportKind::Gatt,
                max_write_size: Some(3),
            },
            1,
        )
        .unwrap();
        let (remote, reader) = tokio::io::duplex(32);
        let output = Arc::new(StdMutex::new(Written::default()));
        let writer = Recorder {
            output: output.clone(),
            short_write: 2,
            fail_after,
        };
        tokio::spawn(async move {
            let (driver, result) = run_stream(
                driver,
                reader,
                writer,
                Some(3),
                CancellationToken::new(),
                Box::pin(std::future::pending()),
            )
            .await;
            driver.finish(result);
        });
        (connection, remote, output)
    }

    #[tokio::test]
    async fn native_pump_fragments_whole_frames_handles_short_writes_and_reads() {
        let (mut connection, mut remote, output) = stream_fixture(None);
        connection
            .writer()
            .write(vec![1, 2, 3, 4, 5, 6, 7])
            .await
            .unwrap();
        connection.writer().write(vec![8, 9]).await.unwrap();
        {
            let written = lock(&output);
            assert_eq!(written.bytes, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
            assert_eq!(written.calls, [3, 1, 3, 1, 1, 2]);
        }
        remote.write_all(&[10, 11, 12]).await.unwrap();
        assert_eq!(connection.recv().await.unwrap().unwrap(), [10, 11, 12]);
        connection.close().await.unwrap();
    }

    #[tokio::test]
    async fn native_pump_stops_on_partial_failure_without_writing_next_frame() {
        let (mut connection, _remote, output) = stream_fixture(Some(3));
        let failure = connection
            .writer()
            .write(vec![1, 2, 3, 4, 5])
            .await
            .unwrap_err();
        assert_eq!(failure.kind, ErrorKind::Disconnected);
        assert_eq!(lock(&output).bytes, [1, 2, 3]);
        assert!(connection.writer().write(vec![6, 7]).await.is_err());
        assert!(connection.close().await.is_err());
    }

    #[tokio::test]
    async fn native_pump_close_cancels_a_receive_blocked_on_backpressure() {
        let (mut connection, mut remote, _output) = stream_fixture(None);
        remote.write_all(&[1]).await.unwrap();
        tokio::task::yield_now().await;
        remote.write_all(&[2]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), connection.close())
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn device_handles_keep_adapter_identity_and_reject_foreign_handles() {
        let id = DeviceId("bluez/hci2/01:23:45:67:89:AB".into());
        let (adapter, address) = parse_id(&id).unwrap();
        assert_eq!(adapter, "hci2");
        assert_eq!(address.to_string(), "01:23:45:67:89:AB");
        for malformed in [
            "01:23:45:67:89:AB",
            "bluez//01:23:45:67:89:AB",
            "bluez/hci0/not-an-address",
            "bluez/hci0/01:23:45:67:89:AB/extra",
        ] {
            assert!(parse_id(&DeviceId(malformed.into())).is_err());
        }
    }
}
