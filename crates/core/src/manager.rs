use crate::{DeviceHandle, Error, Result, SessionConfig, TimeoutStage};
use bluetooth::{Backend, CancellationToken, Connection, Device, DeviceId, TransportKind};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    sync::{oneshot, watch},
    time::{Instant, timeout_at},
};

/// Owns device identities and lifetimes, with no lock held across platform I/O.
/// Setup supervisors retain native cleanup ownership even if callers disappear.
pub struct DeviceManager {
    inner: Arc<Inner>,
}
struct Inner {
    backend: Arc<dyn Backend>,
    entries: Mutex<HashMap<DeviceId, Entry>>,
    generation: AtomicU64,
    cancel: CancellationToken,
    pending: watch::Sender<usize>,
    cleanup_error: Mutex<Option<Error>>,
}
enum Entry {
    Connecting {
        generation: u64,
        cancel: CancellationToken,
        done: watch::Receiver<Option<Result<()>>>,
    },
    Connected(DeviceHandle),
}

impl DeviceManager {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                entries: Mutex::new(HashMap::new()),
                generation: AtomicU64::new(1),
                cancel: CancellationToken::new(),
                pending: watch::channel(0).0,
                cleanup_error: Mutex::new(None),
            }),
        }
    }

    /// Enumerates paired devices. Dropping this future requests cancellation;
    /// an owned supervisor awaits the backend's cleanup acknowledgement.
    pub async fn discover(&self) -> Result<Vec<Device>> {
        let mut setup = self.inner.begin_setup()?;
        let cancel = self.inner.cancel.child_token();
        let guard = CancelOnDrop(Some(cancel.clone()));
        let (reply, receive) = oneshot::channel();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let config = SessionConfig::default();
            let mut reply = Some(reply);
            let work = inner.backend.discover(cancel.clone());
            tokio::pin!(work);
            let reason = tokio::select! {
                result = &mut work => {
                    setup.report(Ok(()));
                    respond(&mut reply, if cancel.is_cancelled() { Err(Error::Cancelled) } else { result.map_err(Error::from) });
                    return;
                }
                _ = cancel.cancelled() => Error::Cancelled,
                _ = tokio::time::sleep(config.connect_timeout) => Error::Timeout(TimeoutStage::Discovery),
            };
            cancel.cancel();
            if timeout_at(Instant::now() + config.close_timeout, &mut work)
                .await
                .is_err()
            {
                setup.report(Err(Error::Timeout(TimeoutStage::Close)));
                respond(&mut reply, Err(Error::Timeout(TimeoutStage::Close)));
                let _ = work.await;
            }
            setup.report(Ok(()));
            respond(&mut reply, Err(reason));
        });
        let result = receive.await.unwrap_or(Err(Error::Disconnected));
        drop(guard);
        result
    }

    /// Returns before MDR initialization so views can subscribe before negotiation.
    pub async fn connect(
        &self,
        device: DeviceId,
        transport: TransportKind,
        config: SessionConfig,
    ) -> Result<DeviceHandle> {
        config.validate()?;
        let setup = self.inner.begin_setup()?;
        let generation = self
            .inner
            .generation
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| Error::InvalidConfiguration("connection generation exhausted"))?;
        let cancel = self.inner.cancel.child_token();
        {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = entries.get(&device) {
                match entry {
                    Entry::Connected(handle) if handle.cleanup_finished() => {}
                    _ => return Err(Error::AlreadyConnected),
                }
            }
            entries.insert(
                device.clone(),
                Entry::Connecting {
                    generation,
                    cancel: cancel.clone(),
                    done: setup.done.subscribe(),
                },
            );
        }
        let reservation = Reservation {
            inner: self.inner.clone(),
            device,
            generation,
            cancel: cancel.clone(),
            armed: true,
            setup,
        };
        let (reply, receive) = oneshot::channel();
        tokio::spawn(connect_supervisor(reservation, transport, config, reply));
        let mut guard = CancelOnDrop(Some(cancel));
        let result = receive.await.unwrap_or(Err(Error::Disconnected));
        // A returned handle keeps this cancellation token for its session.
        if result.is_ok() {
            guard.0 = None;
        }
        result
    }

    pub fn devices(&self) -> Vec<DeviceHandle> {
        self.inner
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter_map(|entry| match entry {
                Entry::Connected(handle) => Some(handle.clone()),
                Entry::Connecting { .. } => None,
            })
            .collect()
    }

    /// Waits for native cleanup, or reports the close deadline while retaining
    /// the identity reservation until the supervisor actually finishes cleanup.
    pub async fn disconnect(&self, device: &DeviceId) -> Result<()> {
        let (handle, pending) = {
            let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            match entries.get(device) {
                Some(Entry::Connected(handle)) => (Some(handle.clone()), None),
                Some(Entry::Connecting { cancel, done, .. }) => {
                    cancel.cancel();
                    (None, Some(done.clone()))
                }
                None => (None, None),
            }
        };
        if let Some(mut pending) = pending {
            loop {
                if let Some(result) = pending.borrow_and_update().clone() {
                    return result;
                }
                pending.changed().await.map_err(|_| Error::Disconnected)?;
            }
        }
        if let Some(handle) = handle {
            handle.close().await
        } else {
            Ok(())
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.inner.cancel.cancel();
        let mut pending = self.inner.pending.subscribe();
        while *pending.borrow_and_update() != 0 {
            pending.changed().await.map_err(|_| Error::Disconnected)?;
        }
        let mut failure = self
            .inner
            .cleanup_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut closing = tokio::task::JoinSet::new();
        for handle in self.devices() {
            closing.spawn(async move { handle.close().await });
        }
        while let Some(result) = closing.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(error) => {
                    failure
                        .get_or_insert(Error::Protocol(format!("session cleanup failed: {error}")));
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for DeviceManager {
    fn drop(&mut self) {
        self.inner.cancel.cancel();
    }
}

async fn connect_supervisor(
    mut reservation: Reservation,
    transport: TransportKind,
    config: SessionConfig,
    reply: oneshot::Sender<Result<DeviceHandle>>,
) {
    let mut reply = Some(reply);
    // Borrow a separate Arc so the future can remain alive while the reservation
    // publishes deadline errors or releases its identity after actual cleanup.
    let backend = reservation.inner.backend.clone();
    let work = backend.connect(
        reservation.device.clone(),
        transport,
        reservation.cancel.clone(),
    );
    tokio::pin!(work);
    let initial = tokio::select! {
        result = &mut work => Ok(result),
        _ = reservation.cancel.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(config.connect_timeout) => Err(Error::Timeout(TimeoutStage::Connect)),
    };
    let connection = match initial {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            let error = if reservation.cancel.is_cancelled() {
                Error::Cancelled
            } else {
                error.into()
            };
            reservation.finish();
            respond(&mut reply, Err(error));
            return;
        }
        Err(reason) => {
            reservation.cancel.cancel();
            let deadline = Instant::now() + config.close_timeout;
            let outcome = match timeout_at(deadline, &mut work).await {
                Ok(result) => result,
                Err(_) => {
                    reservation
                        .setup
                        .report(Err(Error::Timeout(TimeoutStage::Close)));
                    respond(&mut reply, Err(Error::Timeout(TimeoutStage::Close)));
                    work.await
                }
            };
            if let Ok(mut connection) = outcome {
                close_pending(
                    &mut connection,
                    deadline,
                    &mut reservation.setup,
                    &mut reply,
                )
                .await;
            }
            reservation.finish();
            respond(&mut reply, Err(reason));
            return;
        }
    };
    let valid_identity =
        connection.info().device == reservation.device && connection.info().transport == transport;
    if reservation.cancel.is_cancelled() || !valid_identity {
        let reason = if valid_identity {
            Error::Cancelled
        } else {
            Error::Protocol("backend returned a different connection identity".into())
        };
        let mut connection = connection;
        close_pending(
            &mut connection,
            Instant::now() + config.close_timeout,
            &mut reservation.setup,
            &mut reply,
        )
        .await;
        reservation.finish();
        respond(&mut reply, Err(reason));
        return;
    }
    let handle = match DeviceHandle::start(
        connection,
        reservation.generation,
        config,
        reservation.cancel.clone(),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            reservation.finish();
            respond(&mut reply, Err(error));
            return;
        }
    };
    reservation
        .inner
        .entries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(reservation.device.clone(), Entry::Connected(handle.clone()));
    reservation.armed = false;
    if reservation.cancel.is_cancelled() || reply.as_ref().is_none_or(oneshot::Sender::is_closed) {
        let cleanup = handle.close().await;
        reservation.setup.report(cleanup.clone());
        respond(&mut reply, cleanup.and(Err(Error::Cancelled)));
    } else {
        reservation.setup.report(Ok(()));
        if reply
            .take()
            .expect("unsent connect result")
            .send(Ok(handle.clone()))
            .is_err()
        {
            handle.cancel();
        }
    }
}

async fn close_pending(
    connection: &mut Connection,
    deadline: Instant,
    setup: &mut SetupGuard,
    reply: &mut Option<oneshot::Sender<Result<DeviceHandle>>>,
) {
    match timeout_at(deadline, connection.close()).await {
        Ok(result) => {
            if let Err(error) = result {
                setup.report(Err(error.into()));
            }
        }
        Err(_) => {
            setup.report(Err(Error::Timeout(TimeoutStage::Close)));
            respond(reply, Err(Error::Timeout(TimeoutStage::Close)));
            let _ = connection.close().await;
        }
    }
}
fn respond<T>(reply: &mut Option<oneshot::Sender<Result<T>>>, result: Result<T>) {
    if let Some(reply) = reply.take() {
        let _ = reply.send(result);
    }
}

impl Inner {
    fn begin_setup(self: &Arc<Self>) -> Result<SetupGuard> {
        let mut registered = false;
        self.pending.send_modify(|pending| {
            if !self.cancel.is_cancelled() {
                *pending += 1;
                registered = true;
            }
        });
        if !registered {
            return Err(Error::Cancelled);
        }
        Ok(SetupGuard {
            inner: self.clone(),
            done: watch::channel(None).0,
            reported: false,
        })
    }
}
struct CancelOnDrop(Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}
struct SetupGuard {
    inner: Arc<Inner>,
    done: watch::Sender<Option<Result<()>>>,
    reported: bool,
}
impl SetupGuard {
    fn report(&mut self, result: Result<()>) {
        if self.reported {
            return;
        }
        self.reported = true;
        if let Err(error) = &result {
            self.inner
                .cleanup_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert(error.clone());
        }
        self.done.send_replace(Some(result));
        self.inner.pending.send_modify(|pending| *pending -= 1);
    }
}
impl Drop for SetupGuard {
    fn drop(&mut self) {
        self.report(Ok(()));
    }
}
struct Reservation {
    inner: Arc<Inner>,
    device: DeviceId,
    generation: u64,
    cancel: CancellationToken,
    armed: bool,
    setup: SetupGuard,
}
impl Reservation {
    fn finish(&mut self) {
        self.release();
        self.setup.report(Ok(()));
    }
    fn release(&mut self) {
        if !self.armed {
            return;
        }
        self.cancel.cancel();
        let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(entries.get(&self.device), Some(Entry::Connecting { generation, .. }) if *generation == self.generation)
        {
            entries.remove(&self.device);
        }
        self.armed = false;
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.release();
    }
}
