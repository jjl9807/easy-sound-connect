//! Windows native RFCOMM and GATT control links.
//!
//! Native objects live on a dedicated MTA thread with a current-thread runtime.
//! Device access must already be allowed by Windows; this backend never pairs a
//! device or substitutes a different service when the selected service fails.

use std::{
    collections::BTreeMap,
    future::Future,
    ops::Deref,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::oneshot;
use windows::{
    Devices::{
        Bluetooth::{
            BluetoothCacheMode, BluetoothConnectionStatus, BluetoothDevice, BluetoothError,
            BluetoothLEDevice,
            GenericAttributeProfile::{
                GattCharacteristic, GattCharacteristicProperties,
                GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
                GattDeviceService, GattSession, GattSessionStatus, GattValueChangedEventArgs,
                GattWriteOption,
            },
            Rfcomm::{RfcommDeviceService, RfcommServiceId},
        },
        Enumeration::DeviceInformation,
    },
    Foundation::{IClosable, TypedEventHandler},
    Networking::Sockets::StreamSocket,
    Storage::Streams::{DataReader, DataWriter, IBuffer, InputStreamOptions},
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{GUID, Interface},
};

use crate::{
    Backend, BoxFuture, CancellationToken, Connection, ConnectionInfo, DEFAULT_CHANNEL_CAPACITY,
    Device, DeviceId, Driver, Error, ErrorKind, Result, TransportKind, driver_channel,
};

const RFCOMM_UUID: GUID = GUID::from_u128(0x956c7b26_d49a_4ba8_b03f_b17d393cb6e2);
const GATT_UUID: GUID = GUID::from_u128(0x5b833e20_6bc7_4802_8e9a_723ceca4bd8f);
const WRITE_UUID: GUID = GUID::from_u128(0x5b833c60_6bc7_4802_8e9a_723ceca4bd8f);
const NOTIFY_UUID: GUID = GUID::from_u128(0x5b833c61_6bc7_4802_8e9a_723ceca4bd8f);
const LENGTH_UUID: GUID = GUID::from_u128(0x5b833c91_6bc7_4802_8e9a_723ceca4bd8f);
const OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// The native Windows Bluetooth adapter. No system I/O occurs in `new`.
#[derive(Clone, Default)]
pub struct WindowsBackend;

impl WindowsBackend {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

// WinRT operations continue independently of Rust futures. Always cancel an
// abandoned operation explicitly before releasing its native owner.
struct OnDrop<F: FnOnce()>(Option<F>);
impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

macro_rules! native_await {
    ($operation:expr, $cancel:expr) => {{
        let operation = $operation.map_err(native_error)?;
        let abandoned = operation.clone();
        let mut guard = OnDrop(Some(move || {
            let _ = abandoned.Cancel();
        }));
        let result = tokio::select! {
            biased;
            _ = $cancel.cancelled() => return Err(Error::cancelled()),
            result = operation.clone() => result.map_err(native_error),
        };
        guard.0 = None;
        let _ = operation.Close();
        result?
    }};
}

struct Apartment;
impl Apartment {
    fn enter() -> Result<Self> {
        // SAFETY: each worker owns its OS thread; this is balanced by Drop on
        // the same thread after all WinRT references and its runtime are gone.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(native_error)?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() }
    }
}

struct Native<T: Interface>(T);
impl<T: Interface> Deref for Native<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Interface> Drop for Native<T> {
    fn drop(&mut self) {
        if let Ok(closable) = self.0.cast::<IClosable>() {
            let _ = closable.Close();
        }
    }
}

fn native_error(error: windows::core::Error) -> Error {
    let kind = match error.code().0 as u32 {
        0x80070005 => ErrorKind::PermissionDenied,
        0x800704c7 => ErrorKind::Cancelled,
        0x80070490 => ErrorKind::NotFound,
        0x8007048f | 0x800704cd => ErrorKind::Disconnected,
        _ => ErrorKind::Io,
    };
    Error::new(kind, format!("Windows Bluetooth: {error}"))
}

fn status(status: GattCommunicationStatus) -> Result<()> {
    if status == GattCommunicationStatus::Success {
        return Ok(());
    }
    let kind = if status == GattCommunicationStatus::AccessDenied {
        ErrorKind::PermissionDenied
    } else if status == GattCommunicationStatus::Unreachable {
        ErrorKind::Disconnected
    } else {
        ErrorKind::Io
    };
    Err(Error::new(
        kind,
        format!("Windows GATT status {}", status.0),
    ))
}

fn spawn_worker<T, F, Fut>(cancel: CancellationToken, work: F) -> BoxFuture<'static, Result<T>>
where
    T: Send + 'static,
    F: FnOnce(oneshot::Sender<Result<T>>, CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + 'static,
{
    Box::pin(async move {
        let cancel = cancel.child_token();
        let guard = cancel.clone().drop_guard();
        let (sender, receiver) = oneshot::channel();
        let worker_cancel = cancel.clone();
        std::thread::Builder::new()
            .name("ezsc-windows-bluetooth".into())
            .spawn(move || {
                let _apartment = match Apartment::enter() {
                    Ok(apartment) => apartment,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        return;
                    }
                };
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = sender.send(Err(error.into()));
                        return;
                    }
                };
                runtime.block_on(work(sender, worker_cancel));
            })
            .map_err(Error::from)?;
        // Cancellation is forwarded to the worker, but the result does not
        // return until it has either published a link or released setup owners.
        let result = receiver.await.unwrap_or_else(|_| {
            Err(Error::new(
                ErrorKind::Io,
                "Windows Bluetooth worker stopped",
            ))
        });
        if result.is_ok() {
            guard.disarm();
        }
        result
    })
}

impl Backend for WindowsBackend {
    fn discover(&self, cancel: CancellationToken) -> BoxFuture<'_, Result<Vec<Device>>> {
        spawn_worker(cancel, |ready, cancel| async move {
            let result = tokio::time::timeout(OPEN_TIMEOUT, discover_paired(&cancel))
                .await
                .unwrap_or_else(|_| {
                    Err(Error::new(
                        ErrorKind::Timeout,
                        "Windows Bluetooth discovery timed out",
                    ))
                });
            let _ = ready.send(result);
        })
    }

    fn connect(
        &self,
        device: DeviceId,
        transport: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Connection>> {
        let caller_cancel = cancel.clone();
        let operation = spawn_worker(cancel, move |ready, cancel| async move {
            let result = tokio::time::timeout(OPEN_TIMEOUT, open(&device, transport, &cancel))
                .await
                .unwrap_or_else(|_| {
                    Err(Error::new(
                        ErrorKind::Timeout,
                        "Windows Bluetooth connection timed out",
                    ))
                });
            match result {
                Ok(opened) => {
                    let info = ConnectionInfo {
                        device,
                        transport,
                        max_write_size: opened.write_limit(),
                    };
                    match driver_channel(info, DEFAULT_CHANNEL_CAPACITY) {
                        Ok((connection, driver)) => {
                            opened.run(driver, cancel, connection, ready).await;
                        }
                        Err(error) => {
                            let _ = ready.send(Err(error));
                        }
                    }
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            }
        });
        Box::pin(async move {
            let mut result = operation.await;
            if caller_cancel.is_cancelled() {
                if let Ok(connection) = &mut result {
                    let _ = connection.close().await;
                }
                return Err(Error::cancelled());
            }
            result
        })
    }
}

fn address_id(address: u64) -> DeviceId {
    DeviceId(format!("windows:{address:012X}"))
}
fn address_from_id(id: &DeviceId) -> Result<u64> {
    let raw =
        id.0.strip_prefix("windows:")
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "not a Windows device identity"))?;
    if raw.len() != 12 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid Windows Bluetooth address identity",
        ));
    }
    u64::from_str_radix(raw, 16)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "invalid Windows device identity"))
}

async fn discover_paired(cancel: &CancellationToken) -> Result<Vec<Device>> {
    let mut devices = BTreeMap::<DeviceId, Device>::new();
    let mut inaccessible = None;
    for kind in [TransportKind::Rfcomm, TransportKind::Gatt] {
        let selector = match kind {
            TransportKind::Rfcomm => BluetoothDevice::GetDeviceSelectorFromPairingState(true),
            TransportKind::Gatt => BluetoothLEDevice::GetDeviceSelectorFromPairingState(true),
        }
        .map_err(native_error)?;
        let collection = native_await!(DeviceInformation::FindAllAsyncAqsFilter(&selector), cancel);
        for entry in collection {
            if cancel.is_cancelled() {
                return Err(Error::cancelled());
            }
            let probe = probe_device(&entry, kind, cancel).await;
            match probe {
                Ok(Some((address, name))) => {
                    let id = address_id(address);
                    let device = devices.entry(id.clone()).or_insert(Device {
                        id,
                        name,
                        transports: Vec::new(),
                    });
                    if !device.transports.contains(&kind) {
                        device.transports.push(kind);
                    }
                }
                Ok(None) => {}
                Err(error) if error.kind == ErrorKind::Cancelled => return Err(error),
                Err(error) => {
                    inaccessible.get_or_insert(error);
                }
            }
        }
    }
    if devices.is_empty()
        && let Some(error) = inaccessible
    {
        return Err(error);
    }
    Ok(devices.into_values().collect())
}

async fn probe_device(
    entry: &DeviceInformation,
    kind: TransportKind,
    cancel: &CancellationToken,
) -> Result<Option<(u64, Option<String>)>> {
    let id = entry.Id().map_err(native_error)?;
    let name = entry.Name().map_err(native_error)?.to_string();
    match kind {
        TransportKind::Rfcomm => {
            let device = Native(native_await!(BluetoothDevice::FromIdAsync(&id), cancel));
            let service_id = RfcommServiceId::FromUuid(RFCOMM_UUID).map_err(native_error)?;
            let result = native_await!(
                device.GetRfcommServicesForIdWithCacheModeAsync(
                    &service_id,
                    BluetoothCacheMode::Cached
                ),
                cancel
            );
            if result.Error().map_err(native_error)? != BluetoothError::Success {
                return Err(Error::new(
                    ErrorKind::Io,
                    "Windows RFCOMM service discovery failed",
                ));
            }
            let services = result.Services().map_err(native_error)?;
            let found = services.Size().map_err(native_error)? > 0;
            for service in services {
                let _ = service.Close();
            }
            Ok(found
                .then(|| device.BluetoothAddress())
                .transpose()
                .map_err(native_error)?
                .map(|address| (address, (!name.is_empty()).then_some(name))))
        }
        TransportKind::Gatt => {
            let device = Native(native_await!(BluetoothLEDevice::FromIdAsync(&id), cancel));
            let result = native_await!(
                device.GetGattServicesForUuidWithCacheModeAsync(
                    GATT_UUID,
                    BluetoothCacheMode::Cached
                ),
                cancel
            );
            status(result.Status().map_err(native_error)?)?;
            let services = result.Services().map_err(native_error)?;
            let found = services.Size().map_err(native_error)? > 0;
            for service in services {
                let _ = service.Close();
            }
            Ok(found
                .then(|| device.BluetoothAddress())
                .transpose()
                .map_err(native_error)?
                .map(|address| (address, (!name.is_empty()).then_some(name))))
        }
    }
}

struct RfcommLink {
    socket: Native<StreamSocket>,
    _service: Native<RfcommDeviceService>,
    _device: Native<BluetoothDevice>,
}
struct GattLink {
    write: GattCharacteristic,
    notify: GattCharacteristic,
    session: Native<GattSession>,
    _service: Native<GattDeviceService>,
    device: Native<BluetoothLEDevice>,
    write_limit: usize,
    write_option: GattWriteOption,
}
enum Opened {
    Rfcomm(RfcommLink),
    Gatt(GattLink),
}
impl Opened {
    fn write_limit(&self) -> Option<usize> {
        match self {
            Self::Rfcomm(_) => None,
            Self::Gatt(link) => Some(link.write_limit),
        }
    }
    async fn run(
        self,
        mut driver: Driver,
        outer_cancel: CancellationToken,
        connection: Connection,
        ready: oneshot::Sender<Result<Connection>>,
    ) {
        let mut publication = Some((connection, ready));
        let result = match &self {
            Self::Rfcomm(link) => {
                run_rfcomm(link, &mut driver, &outer_cancel, &mut publication).await
            }
            Self::Gatt(link) => run_gatt(link, &mut driver, &outer_cancel, &mut publication).await,
        };
        // Release native owners before publishing cleanup completion.
        drop(self);
        if let Some((_, ready)) = publication {
            let _ = ready.send(Err(result.clone().err().unwrap_or_else(Error::cancelled)));
        }
        driver.finish(result);
    }
}

async fn open(id: &DeviceId, kind: TransportKind, cancel: &CancellationToken) -> Result<Opened> {
    let address = address_from_id(id)?;
    match kind {
        TransportKind::Rfcomm => {
            let device = Native(native_await!(
                BluetoothDevice::FromBluetoothAddressAsync(address),
                cancel
            ));
            if !device
                .DeviceInformation()
                .map_err(native_error)?
                .Pairing()
                .map_err(native_error)?
                .IsPaired()
                .map_err(native_error)?
            {
                return Err(Error::new(
                    ErrorKind::NotPaired,
                    "pair the device in Windows Bluetooth settings",
                ));
            }
            let service_id = RfcommServiceId::FromUuid(RFCOMM_UUID).map_err(native_error)?;
            let result = native_await!(
                device.GetRfcommServicesForIdWithCacheModeAsync(
                    &service_id,
                    BluetoothCacheMode::Uncached
                ),
                cancel
            );
            if result.Error().map_err(native_error)? != BluetoothError::Success {
                return Err(Error::new(ErrorKind::Io, "Windows RFCOMM discovery failed"));
            }
            let services = result.Services().map_err(native_error)?;
            if services.Size().map_err(native_error)? != 1 {
                for service in services {
                    let _ = service.Close();
                }
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "expected one MDR v2 RFCOMM service",
                ));
            }
            let service = Native(services.GetAt(0).map_err(native_error)?);
            let socket = Native(StreamSocket::new().map_err(native_error)?);
            native_await!(
                socket.ConnectAsync(
                    &service.ConnectionHostName().map_err(native_error)?,
                    &service.ConnectionServiceName().map_err(native_error)?
                ),
                cancel
            );
            Ok(Opened::Rfcomm(RfcommLink {
                _device: device,
                _service: service,
                socket,
            }))
        }
        TransportKind::Gatt => open_gatt(address, cancel).await.map(Opened::Gatt),
    }
}

type Publication = Option<(Connection, oneshot::Sender<Result<Connection>>)>;
fn announce(publication: &mut Publication) -> Result<()> {
    if let Some((connection, ready)) = publication.take() {
        ready.send(Ok(connection)).map_err(|_| Error::cancelled())?;
    }
    Ok(())
}

async fn run_rfcomm(
    link: &RfcommLink,
    driver: &mut Driver,
    outer_cancel: &CancellationToken,
    publication: &mut Publication,
) -> Result<()> {
    let reader = Native(
        DataReader::CreateDataReader(&link.socket.InputStream().map_err(native_error)?)
            .map_err(native_error)?,
    );
    reader
        .SetInputStreamOptions(InputStreamOptions::Partial)
        .map_err(native_error)?;
    let writer = Native(
        DataWriter::CreateDataWriter(&link.socket.OutputStream().map_err(native_error)?)
            .map_err(native_error)?,
    );
    if outer_cancel.is_cancelled() {
        return Err(Error::cancelled());
    }
    announce(publication)?;
    let cancel = &driver.cancel;
    let incoming = &driver.incoming;
    let read = async {
        loop {
            let count = native_await!(reader.LoadAsync(4096), cancel);
            if count == 0 {
                return Err(Error::new(
                    ErrorKind::Disconnected,
                    "RFCOMM peer closed the stream",
                ));
            }
            let mut bytes = vec![0; count as usize];
            reader.ReadBytes(&mut bytes).map_err(native_error)?;
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                sent = incoming.send(bytes) => sent.map_err(|_| Error::cancelled())?,
            }
        }
    };
    let writes = &mut driver.writes;
    let write = async {
        while let Some(request) = writes.recv().await {
            let result = async {
                writer.WriteBytes(&request.bytes).map_err(native_error)?;
                let count = native_await!(writer.StoreAsync(), cancel);
                if count as usize != request.bytes.len() {
                    return Err(Error::new(
                        ErrorKind::Io,
                        "RFCOMM write completed only part of the frame",
                    ));
                }
                Ok(())
            }
            .await;
            let _ = request.completion.send(result.clone());
            result?;
        }
        Ok(())
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(()),
        _ = outer_cancel.cancelled() => Ok(()),
        result = read => result,
        result = write => result,
    }
}

async fn characteristic(
    service: &GattDeviceService,
    uuid: GUID,
    cancel: &CancellationToken,
) -> Result<GattCharacteristic> {
    let result = native_await!(
        service.GetCharacteristicsForUuidWithCacheModeAsync(uuid, BluetoothCacheMode::Uncached),
        cancel
    );
    status(result.Status().map_err(native_error)?)?;
    let items = result.Characteristics().map_err(native_error)?;
    if items.Size().map_err(native_error)? != 1 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            format!("expected one MDR v2 GATT characteristic {uuid:?}"),
        ));
    }
    items.GetAt(0).map_err(native_error)
}

fn read_buffer(buffer: &IBuffer) -> Result<Vec<u8>> {
    let reader = Native(DataReader::FromBuffer(buffer).map_err(native_error)?);
    let length = reader.UnconsumedBufferLength().map_err(native_error)? as usize;
    if length > crate::MAX_WRITE_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "native Bluetooth buffer is too large",
        ));
    }
    let mut bytes = vec![0; length];
    reader.ReadBytes(&mut bytes).map_err(native_error)?;
    Ok(bytes)
}
fn write_buffer(bytes: &[u8]) -> Result<IBuffer> {
    let writer = Native(DataWriter::new().map_err(native_error)?);
    writer.WriteBytes(bytes).map_err(native_error)?;
    writer.DetachBuffer().map_err(native_error)
}

async fn open_gatt(address: u64, cancel: &CancellationToken) -> Result<GattLink> {
    let device = Native(native_await!(
        BluetoothLEDevice::FromBluetoothAddressAsync(address),
        cancel
    ));
    if !device
        .DeviceInformation()
        .map_err(native_error)?
        .Pairing()
        .map_err(native_error)?
        .IsPaired()
        .map_err(native_error)?
    {
        return Err(Error::new(
            ErrorKind::NotPaired,
            "pair the device in Windows Bluetooth settings",
        ));
    }
    let result = native_await!(
        device.GetGattServicesForUuidWithCacheModeAsync(GATT_UUID, BluetoothCacheMode::Uncached),
        cancel
    );
    status(result.Status().map_err(native_error)?)?;
    let services = result.Services().map_err(native_error)?;
    if services.Size().map_err(native_error)? != 1 {
        for service in services {
            let _ = service.Close();
        }
        return Err(Error::new(
            ErrorKind::Unsupported,
            "expected one MDR v2 GATT service",
        ));
    }
    let service = Native(services.GetAt(0).map_err(native_error)?);
    let session = Native(service.Session().map_err(native_error)?);
    let write = characteristic(&service, WRITE_UUID, cancel).await?;
    let notify = characteristic(&service, NOTIFY_UUID, cancel).await?;
    let properties = write.CharacteristicProperties().map_err(native_error)?;
    let write_option = if properties.contains(GattCharacteristicProperties::Write) {
        GattWriteOption::WriteWithResponse
    } else if properties.contains(GattCharacteristicProperties::WriteWithoutResponse) {
        GattWriteOption::WriteWithoutResponse
    } else {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "MDR characteristic cannot be written",
        ));
    };
    let notify_properties = notify.CharacteristicProperties().map_err(native_error)?;
    if !notify_properties.contains(GattCharacteristicProperties::Notify) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "MDR receive characteristic does not support notifications",
        ));
    }
    let length = characteristic(&service, LENGTH_UUID, cancel).await?;
    let write_limit = prepare_gatt(&session, &length, cancel).await?;
    Ok(GattLink {
        device,
        _service: service,
        session,
        write,
        notify,
        write_limit,
        write_option,
    })
}

async fn prepare_gatt(
    session: &GattSession,
    length: &GattCharacteristic,
    cancel: &CancellationToken,
) -> Result<usize> {
    // Uncached discovery has established the ATT session. Windows owns ATT MTU
    // negotiation; its active session exposes the current native MTU. This is
    // the host-managed path: do not wait for a device-led c93 notification that
    // may already have happened before this application opened its handles.
    if session.SessionStatus().map_err(native_error)? != GattSessionStatus::Active {
        return Err(Error::new(
            ErrorKind::Disconnected,
            "Windows ATT session is not active",
        ));
    }
    let native_mtu = session.MaxPduSize().map_err(native_error)? as usize;
    if native_mtu < 23 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "Windows ATT MTU is below the protocol minimum",
        ));
    }
    let result = native_await!(
        length.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached),
        cancel
    );
    status(result.Status().map_err(native_error)?)?;
    let vendor_limit =
        crate::gatt::parse_write_limit(&read_buffer(&result.Value().map_err(native_error)?)?)?;
    // c91 already counts characteristic-value bytes. Only the native ATT MTU
    // needs its opcode and handle removed to obtain a write payload bound.
    Ok(vendor_limit.min(native_mtu - 3))
}

async fn set_notifications(
    characteristic: &GattCharacteristic,
    enabled: bool,
    cancel: &CancellationToken,
) -> Result<()> {
    let value = if enabled {
        GattClientCharacteristicConfigurationDescriptorValue::Notify
    } else {
        GattClientCharacteristicConfigurationDescriptorValue::None
    };
    status(native_await!(
        characteristic.WriteClientCharacteristicConfigurationDescriptorAsync(value),
        cancel
    ))
}

async fn run_gatt(
    link: &GattLink,
    driver: &mut Driver,
    outer_cancel: &CancellationToken,
    publication: &mut Publication,
) -> Result<()> {
    let failure = Arc::new(Mutex::new(None::<Error>));
    let callback_failure = failure.clone();
    let callback_cancel = driver.cancel.clone();
    let callback_incoming = driver.incoming.clone();
    let callback =
        TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(move |_, args| {
            let result = args
                .as_ref()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "empty GATT notification"))
                .and_then(|args| args.CharacteristicValue().map_err(native_error))
                .and_then(|buffer| read_buffer(&buffer))
                .and_then(|bytes| {
                    callback_incoming.try_send(bytes).map_err(|_| {
                        Error::new(
                            ErrorKind::Io,
                            "GATT receive queue overflow or receiver closed",
                        )
                    })
                });
            if let Err(error) = result {
                if let Ok(mut stored) = callback_failure.lock() {
                    stored.get_or_insert(error);
                }
                callback_cancel.cancel();
            }
            Ok(())
        });
    let token = link.notify.ValueChanged(&callback).map_err(native_error)?;
    let notify = link.notify.clone();
    let _unregister = OnDrop(Some(move || {
        let _ = notify.RemoveValueChanged(token);
    }));
    let cancel = &driver.cancel;
    let disconnect_failure = failure.clone();
    let disconnect_cancel = driver.cancel.clone();
    let disconnected = TypedEventHandler::<BluetoothLEDevice, windows::core::IInspectable>::new(
        move |device, _| {
            if let Some(device) = device.as_ref()
                && device.ConnectionStatus()? == BluetoothConnectionStatus::Disconnected
            {
                if let Ok(mut stored) = disconnect_failure.lock() {
                    stored.get_or_insert_with(|| {
                        Error::new(ErrorKind::Disconnected, "Windows GATT peer disconnected")
                    });
                }
                disconnect_cancel.cancel();
            }
            Ok(())
        },
    );
    let disconnect_token = link
        .device
        .ConnectionStatusChanged(&disconnected)
        .map_err(native_error)?;
    let device = link.device.0.clone();
    let _remove_disconnect = OnDrop(Some(move || {
        let _ = device.RemoveConnectionStatusChanged(disconnect_token);
    }));
    let setup = tokio::select! {
        biased;
        _ = outer_cancel.cancelled() => Err(Error::cancelled()),
        result = tokio::time::timeout(OPEN_TIMEOUT, set_notifications(&link.notify, true, cancel)) => result.unwrap_or_else(|_| Err(Error::new(ErrorKind::Timeout, "Windows GATT subscription timed out"))),
    };
    let writes = &mut driver.writes;
    let write = async {
        while let Some(request) = writes.recv().await {
            let result = async {
                let mut remaining = request.bytes.as_slice();
                while !remaining.is_empty() {
                    let mtu = link.session.MaxPduSize().map_err(native_error)? as usize;
                    if mtu < 23 {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            "Windows ATT MTU fell below the protocol minimum",
                        ));
                    }
                    let count = remaining.len().min(link.write_limit).min(mtu - 3);
                    let (chunk, next) = remaining.split_at(count);
                    let result = native_await!(
                        link.write.WriteValueWithResultAndOptionAsync(
                            &write_buffer(chunk)?,
                            link.write_option
                        ),
                        cancel
                    );
                    status(result.Status().map_err(native_error)?)?;
                    remaining = next;
                }
                Ok(())
            }
            .await;
            let _ = request.completion.send(result.clone());
            result?;
        }
        Ok(())
    };
    let result = match setup {
        Err(error) => Err(error),
        Ok(()) => match link.device.ConnectionStatus().map_err(native_error) {
            Err(error) => Err(error),
            Ok(connection_status) if connection_status != BluetoothConnectionStatus::Connected => {
                Err(Error::new(
                    ErrorKind::Disconnected,
                    "Windows GATT peer disconnected during setup",
                ))
            }
            Ok(_) => match announce(publication) {
                Err(error) => Err(error),
                Ok(()) => tokio::select! {
                    biased;
                    _ = cancel.cancelled() => Ok(()),
                    _ = outer_cancel.cancelled() => Ok(()),
                    result = write => result,
                },
            },
        },
    };
    // Best-effort bounded CCCD cleanup; closing the service follows regardless.
    if let Ok(operation) = link
        .notify
        .WriteClientCharacteristicConfigurationDescriptorAsync(
            GattClientCharacteristicConfigurationDescriptorValue::None,
        )
    {
        let abandoned = operation.clone();
        let _cancel = OnDrop(Some(move || {
            let _ = abandoned.Cancel();
        }));
        let _ = tokio::time::timeout(Duration::from_secs(2), operation).await;
    }
    let stored = failure
        .lock()
        .map_err(|_| Error::new(ErrorKind::Io, "GATT callback state poisoned"))?
        .take();
    if let Some(error) = stored {
        Err(error)
    } else {
        result
    }
}
