//! Deterministic transport peer for integration tests, without device I/O.

use tokio::sync::mpsc;

use crate::{
    Backend, BoxFuture, CancellationToken, Connection, ConnectionInfo, Device, DeviceId, Driver,
    Error, ErrorKind, Result, TransportKind, driver_channel,
};

/// Returns the same bounded driver interface used by native backends. Tests may
/// inspect a full write, defer its completion, inject receive fragments while it
/// is outstanding, fail a partial write, and observe cancellation.
pub fn connection(info: ConnectionInfo, capacity: usize) -> Result<(Connection, Driver)> {
    driver_channel(info, capacity)
}

/// An explicit catalog with caller-controlled peers; no timing or responses are
/// synthesized. Each selected connection is delivered through `openings`.
pub struct FakeBackend {
    devices: Vec<Device>,
    openings: mpsc::Sender<Driver>,
    capacity: usize,
    gatt_write_size: usize,
}

impl FakeBackend {
    pub fn new(
        devices: Vec<Device>,
        capacity: usize,
        gatt_write_size: usize,
    ) -> Result<(Self, mpsc::Receiver<Driver>)> {
        if !(1..=crate::MAX_CHANNEL_CAPACITY).contains(&capacity) || gatt_write_size == 0 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "fake channel/write limits must be positive",
            ));
        }
        let (openings, receiver) = mpsc::channel(capacity);
        Ok((
            Self {
                devices,
                openings,
                capacity,
                gatt_write_size,
            },
            receiver,
        ))
    }
}

impl Backend for FakeBackend {
    fn discover(&self, cancel: CancellationToken) -> BoxFuture<'_, Result<Vec<Device>>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(Error::cancelled());
            }
            Ok(self.devices.clone())
        })
    }

    fn connect(
        &self,
        device: DeviceId,
        transport: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Connection>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(Error::cancelled());
            }
            let descriptor = self
                .devices
                .iter()
                .find(|entry| entry.id == device)
                .ok_or_else(|| Error::new(ErrorKind::NotFound, "fake device not found"))?;
            if !descriptor.transports.contains(&transport) {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "fake device does not expose selected service",
                ));
            }
            let (connection, driver) = driver_channel(
                ConnectionInfo {
                    device,
                    transport,
                    max_write_size: (transport == TransportKind::Gatt)
                        .then_some(self.gatt_write_size),
                },
                self.capacity,
            )?;
            let connection_cancel = driver.cancel.clone();
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(Error::cancelled()),
                sent = self.openings.send(driver) => {
                    sent.map_err(|_| Error::new(ErrorKind::Unavailable, "fake peer controller dropped"))?;
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = cancel.cancelled() => connection_cancel.cancel(),
                            _ = connection_cancel.cancelled() => {}
                        }
                    });
                    Ok(connection)
                }
            }
        })
    }
}
