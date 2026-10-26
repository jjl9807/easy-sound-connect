//! Ordered, cancellable Bluetooth control connections without MDR semantics.
//!
//! A write submits one complete encoded frame. Backends split it as necessary
//! and acknowledge completion only after all fragments have completed their
//! native writes. This is local I/O completion, never a device protocol ACK.

use std::{fmt, future::Future, pin::Pin};

use tokio::sync::{mpsc, oneshot, watch};
pub use tokio_util::sync::CancellationToken;

pub mod fake;
pub mod gatt;
pub mod platform;

/// The MDR v2 control service, discovered through SDP for RFCOMM.
pub const RFCOMM_SERVICE: &str = "956c7b26-d49a-4ba8-b03f-b17d393cb6e2";
pub const GATT_SERVICE: &str = "5b833e20-6bc7-4802-8e9a-723ceca4bd8f";
pub const GATT_WRITE: &str = "5b833c60-6bc7-4802-8e9a-723ceca4bd8f";
pub const GATT_NOTIFY: &str = "5b833c61-6bc7-4802-8e9a-723ceca4bd8f";
pub const GATT_WRITE_LENGTH: &str = "5b833c91-6bc7-4802-8e9a-723ceca4bd8f";
pub const GATT_MTU_CONFIRM: &str = "5b833c93-6bc7-4802-8e9a-723ceca4bd8f";

/// A local allocation limit, not an advertised device limit.
pub const MAX_WRITE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_CHANNEL_CAPACITY: usize = 32;
pub const MAX_CHANNEL_CAPACITY: usize = 1024;

pub type Result<T> = std::result::Result<T, Error>;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An opaque, backend-specific identity. It is not a model or capability ID.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct DeviceId(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TransportKind {
    Rfcomm,
    Gatt,
}

/// A paired device and the v2 services actually reported by its platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Device {
    pub id: DeviceId,
    pub name: Option<String>,
    pub transports: Vec<TransportKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionInfo {
    pub device: DeviceId,
    pub transport: TransportKind,
    /// Maximum native write fragment, after subtracting ATT overhead for GATT.
    /// `None` means the backend exposes a stream with no protocol fragment limit.
    pub max_write_size: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    Unavailable,
    PermissionDenied,
    NotPaired,
    NotFound,
    Unsupported,
    InvalidData,
    Busy,
    Disconnected,
    Cancelled,
    Timeout,
    Io,
}

/// Structured category plus native diagnostic context. No localized UI text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "control connection cancelled")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind as Io;
        let kind = match error.kind() {
            Io::PermissionDenied => ErrorKind::PermissionDenied,
            Io::TimedOut => ErrorKind::Timeout,
            Io::ConnectionAborted
            | Io::ConnectionReset
            | Io::BrokenPipe
            | Io::UnexpectedEof
            | Io::NotConnected => ErrorKind::Disconnected,
            _ => ErrorKind::Io,
        };
        Self::new(kind, error.to_string())
    }
}

/// Platform discovery never pairs devices or opens a control channel.
///
/// Cancellation tokens request cleanup. Keep polling a cancelled setup future
/// until it returns: an error acknowledges release of its native setup owners;
/// a successful connection must instead be explicitly closed. Dropping a future
/// requests cancellation but cannot synchronously await native cleanup. Callers
/// that offer bounded cancellation must retain a supervisor for unfinished work.
pub trait Backend: Send + Sync {
    fn discover(&self, cancel: CancellationToken) -> BoxFuture<'_, Result<Vec<Device>>>;
    /// Opens exactly the caller-selected v2 service. No legacy fallback.
    fn connect(
        &self,
        device: DeviceId,
        transport: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Connection>>;
}

/// Opens the platform adapter without discovering or connecting any devices.
pub async fn system_backend() -> Result<std::sync::Arc<dyn Backend>> {
    #[cfg(target_os = "linux")]
    {
        Ok(std::sync::Arc::new(
            platform::linux::LinuxBackend::new().await?,
        ))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(std::sync::Arc::new(
            platform::windows::WindowsBackend::new()?
        ))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(std::sync::Arc::new(platform::macos::MacOsBackend::new()?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        Err(Error::new(
            ErrorKind::Unsupported,
            "unsupported host operating system",
        ))
    }
}

/// One whole frame; a driver must serialize these and never interleave fragments.
pub struct WriteRequest {
    pub bytes: Vec<u8>,
    pub completion: oneshot::Sender<Result<()>>,
}

#[derive(Clone)]
pub struct Writer {
    requests: mpsc::Sender<WriteRequest>,
    cancel: CancellationToken,
}

impl Writer {
    /// Completion means every native fragment completed. Cancellation after
    /// enqueueing closes the link because a partially written frame is ambiguous.
    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_WRITE_BYTES {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid complete-frame write length",
            ));
        }
        let (completion, completed) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(Error::cancelled()),
            sent = self.requests.send(WriteRequest { bytes, completion }) => {
                sent.map_err(|_| Error::new(ErrorKind::Disconnected, "write driver ended"))?;
            }
        }
        let mut guard = CancelPendingWrite {
            cancel: self.cancel.clone(),
            armed: true,
        };
        let result = tokio::select! {
            biased;
            result = completed => result.unwrap_or_else(|_| Err(Error::new(ErrorKind::Disconnected, "write did not complete"))),
            _ = self.cancel.cancelled() => Err(Error::cancelled()),
        };
        guard.armed = false;
        if result.is_err() {
            self.cancel.cancel();
        }
        result
    }
}

struct CancelPendingWrite {
    cancel: CancellationToken,
    armed: bool,
}
impl Drop for CancelPendingWrite {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
        }
    }
}

/// Unique receive/lifecycle owner; writer handles do not keep it alive.
pub struct Connection {
    info: ConnectionInfo,
    writer: Writer,
    incoming: mpsc::Receiver<Vec<u8>>,
    terminal: watch::Receiver<Option<Result<()>>>,
    reported_terminal: bool,
    cancel: CancellationToken,
}

impl Connection {
    pub fn info(&self) -> &ConnectionInfo {
        &self.info
    }
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// Receives raw ordered fragments, then at most one terminal error. No
    /// notification/read boundary is a protocol frame boundary.
    pub async fn recv(&mut self) -> Option<Result<Vec<u8>>> {
        if self.reported_terminal {
            return None;
        }
        if let Some(bytes) = self.incoming.recv().await {
            return Some(Ok(bytes));
        }
        self.reported_terminal = true;
        match self.terminal.borrow().clone() {
            Some(Err(error)) => Some(Err(error)),
            Some(Ok(())) => None,
            None => Some(Err(Error::new(
                ErrorKind::Disconnected,
                "receive driver ended",
            ))),
        }
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Requests cancellation and waits until the backend has released resources.
    /// Callers may apply their own shutdown deadline; dropping still cancels.
    pub async fn close(&mut self) -> Result<()> {
        self.cancel();
        loop {
            if let Some(result) = self.terminal.borrow().clone() {
                return result;
            }
            if self.terminal.changed().await.is_err() {
                return Err(Error::new(
                    ErrorKind::Disconnected,
                    "driver exited without cleanup result",
                ));
            }
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Driver endpoint for native backends and deterministic tests.
///
/// Keep receives independent from the write loop. Await bounded sends with
/// cancellation; never discard raw bytes to make room. Release all native
/// resources and sender clones before calling `finish`.
pub struct Driver {
    pub writes: mpsc::Receiver<WriteRequest>,
    pub incoming: mpsc::Sender<Vec<u8>>,
    pub cancel: CancellationToken,
    terminal: watch::Sender<Option<Result<()>>>,
    finished: bool,
}
impl Driver {
    pub fn finish(mut self, result: Result<()>) {
        self.terminal.send_replace(Some(result));
        self.finished = true;
        self.cancel.cancel();
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if !self.finished {
            self.terminal.send_replace(Some(Err(Error::new(
                ErrorKind::Disconnected,
                "backend driver dropped before orderly shutdown",
            ))));
        }
        self.cancel.cancel();
    }
}

/// Creates bounded channels; the caller must run and eventually finish `Driver`.
pub fn driver_channel(info: ConnectionInfo, capacity: usize) -> Result<(Connection, Driver)> {
    if !(1..=MAX_CHANNEL_CAPACITY).contains(&capacity) || info.max_write_size == Some(0) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "channel capacity must be 1..=1024 and write limits positive",
        ));
    }
    let (write_tx, write_rx) = mpsc::channel(capacity);
    let (incoming_tx, incoming_rx) = mpsc::channel(capacity);
    let (terminal_tx, terminal_rx) = watch::channel(None);
    let cancel = CancellationToken::new();
    Ok((
        Connection {
            info,
            writer: Writer {
                requests: write_tx,
                cancel: cancel.clone(),
            },
            incoming: incoming_rx,
            terminal: terminal_rx,
            reported_terminal: false,
            cancel: cancel.clone(),
        },
        Driver {
            writes: write_rx,
            incoming: incoming_tx,
            cancel,
            terminal: terminal_tx,
            finished: false,
        },
    ))
}
