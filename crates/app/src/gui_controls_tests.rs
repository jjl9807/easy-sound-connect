//! App synchronization against a synthetic peer; no native Bluetooth I/O.

use crate::gui_controls::refresh_controls;
use bluetooth::{Device, DeviceId, Driver, TransportKind, fake::FakeBackend};
use ezsc_core::{ConnectionState, DeviceHandle, DeviceManager, Freshness, SessionConfig};
use libmdr2::frame::{DataType, Decoder, Frame, Sequence};
use std::{collections::VecDeque, sync::Arc, time::Duration};

type Exchange = (Vec<u8>, Vec<u8>);

fn exchange(request: &[u8], response: &[u8]) -> Exchange {
    (request.to_vec(), response.to_vec())
}

fn handshake(eq: bool) -> Vec<Exchange> {
    vec![
        exchange(&[0, 0], &[1, 0, 3, 0, 0x30, 0x32, 0, 1]),
        exchange(&[2, 0], &[3, 0, 9, 0]),
        exchange(&[6, 0], if eq { &[7, 0, 1, 0x55, 0] } else { &[7, 0, 0] }),
        exchange(&[4, 1], b"\x05\x01\x04Demo"),
        exchange(&[4, 2], b"\x05\x02\x031.0"),
        exchange(&[4, 3], &[5, 3, 0xfe, 0xfd]),
    ]
}

fn eq_reads(levels: &[u8]) -> Vec<Exchange> {
    let count = levels.len() as u8;
    let mut parameters = vec![0x57, 0x31, count];
    parameters.extend_from_slice(levels);
    vec![
        exchange(&[0x50, 0x31, 1], &[0x51, 0x31, count, 13]),
        exchange(&[0x52, 0x31], &[0x53, 0x31, 0]),
        exchange(&[0x56, 0x31], &parameters),
    ]
}

fn band_reads(count: u8, response: &[u8]) -> Vec<Exchange> {
    vec![
        exchange(&[0x50, 0x31, 1], &[0x51, 0x31, count, 13]),
        exchange(&[0x5a, 0x31], response),
    ]
}

async fn peer(mut driver: Driver, script: Vec<Exchange>) {
    let mut script: VecDeque<_> = script.into();
    let mut decoder = Decoder::new(16384).unwrap();
    let mut sequence = Sequence::Zero;
    loop {
        let write = tokio::select! {
            biased;
            _ = driver.cancel.cancelled() => break,
            write = driver.writes.recv() => match write { Some(write) => write, None => break },
        };
        let frames = decoder
            .feed(&write.bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(frames.len(), 1);
        let frame = frames.into_iter().next().unwrap();
        let _ = write.completion.send(Ok(()));
        if frame.data_type == DataType::ACK {
            continue;
        }
        let (request, response) = script
            .pop_front()
            .expect("no unsolicited or repeated query");
        assert_eq!(frame.data_type, DataType::DATA_MDR);
        assert_eq!(frame.payload, request);
        driver
            .incoming
            .send(
                Frame::acknowledgement(frame.sequence)
                    .encode(16384)
                    .unwrap(),
            )
            .await
            .unwrap();
        driver
            .incoming
            .send(
                Frame {
                    data_type: DataType::DATA_MDR,
                    sequence,
                    payload: response,
                }
                .encode(16384)
                .unwrap(),
            )
            .await
            .unwrap();
        sequence = sequence.toggled();
    }
    driver.finish(Ok(()));
    assert!(script.is_empty(), "every expected query must be issued");
}

struct Fixture {
    _manager: DeviceManager,
    handle: DeviceHandle,
    peer: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(script: Vec<Exchange>) -> Self {
        let device = DeviceId("automatic-eq-labels".into());
        let (backend, mut openings) = FakeBackend::new(
            vec![Device {
                id: device.clone(),
                name: None,
                transports: vec![TransportKind::Rfcomm],
            }],
            32,
            128,
        )
        .unwrap();
        let manager = DeviceManager::new(Arc::new(backend));
        let handle = manager
            .connect(device, TransportKind::Rfcomm, SessionConfig::default())
            .await
            .unwrap();
        let peer = tokio::spawn(peer(openings.recv().await.unwrap(), script));
        tokio::time::timeout(Duration::from_secs(2), handle.initialize())
            .await
            .unwrap()
            .unwrap();
        Self {
            _manager: manager,
            handle,
            peer,
        }
    }

    async fn refresh(&self) -> Result<(), String> {
        tokio::time::timeout(
            Duration::from_secs(2),
            refresh_controls(&self.handle, crate::preferences::Language::English),
        )
        .await
        .unwrap()
    }

    async fn close(self) {
        self.handle.close().await.unwrap();
        self.peer.await.unwrap();
    }
}

#[tokio::test]
async fn synchronization_loads_labels_once_and_reloads_after_a_band_count_change() {
    let mut script = handshake(true);
    script.extend(eq_reads(&[6, 7]));
    // The first complete synchronization automatically reads actual frequencies.
    script.extend(band_reads(2, &[0x5b, 0x31, 2, 1, 0, 31, 1, 0x03, 0xe8]));
    // A subsequent refresh of the same shape must reuse the metadata.
    script.extend(eq_reads(&[5, 6]));
    // Capability changes invalidate old labels and cause a new metadata query.
    script.extend(eq_reads(&[4, 5, 6]));
    script.extend(band_reads(
        3,
        &[0x5b, 0x31, 3, 1, 0, 63, 1, 0x03, 0xe8, 1, 0x1f, 0x40],
    ));
    let fixture = Fixture::new(script).await;
    assert!(fixture.handle.snapshot().equalizers[0].band_info.is_none());

    fixture.refresh().await.unwrap();
    let initial = fixture.handle.snapshot().equalizers[0].clone();
    let info = initial.band_info.as_ref().unwrap();
    assert_eq!(
        info.bands
            .iter()
            .map(|band| band.frequency_hz())
            .collect::<Vec<_>>(),
        [Some(31), Some(1000)]
    );
    assert_eq!(initial.parameters.as_ref().unwrap().levels(), [6, 7]);

    fixture.refresh().await.unwrap();
    assert_eq!(
        fixture.handle.snapshot().equalizers[0].band_info,
        initial.band_info
    );
    fixture.refresh().await.unwrap();
    let updated = fixture.handle.snapshot().equalizers[0].clone();
    assert_eq!(
        updated
            .band_info
            .unwrap()
            .bands
            .iter()
            .map(|band| band.frequency_hz())
            .collect::<Vec<_>>(),
        [Some(63), Some(1000), Some(8000)]
    );
    assert_eq!(updated.parameters.unwrap().levels(), [4, 5, 6]);
    fixture.close().await;
}

#[tokio::test]
async fn invalid_optional_labels_preserve_controls_and_are_retried_on_next_refresh() {
    let mut script = handshake(true);
    script.extend(eq_reads(&[6, 7]));
    // A valid response with the wrong band count must not label this curve.
    script.extend(band_reads(2, &[0x5b, 0x31, 1, 1, 0, 31]));
    script.extend(eq_reads(&[6, 7]));
    script.extend(band_reads(2, &[0x5b, 0x31, 2, 1, 0, 31, 1, 0x03, 0xe8]));
    let fixture = Fixture::new(script).await;

    assert!(fixture.refresh().await.is_err());
    let snapshot = fixture.handle.snapshot();
    assert_eq!(snapshot.connection, ConnectionState::Ready);
    assert_eq!(snapshot.equalizers[0].freshness, Freshness::Current);
    assert!(snapshot.equalizers[0].band_info.is_none());
    assert_eq!(
        snapshot.equalizers[0].parameters.as_ref().unwrap().levels(),
        [6, 7]
    );
    fixture.refresh().await.unwrap();
    assert_eq!(
        fixture.handle.snapshot().equalizers[0]
            .band_info
            .as_ref()
            .unwrap()
            .bands
            .len(),
        2
    );
    fixture.close().await;
}

#[tokio::test]
async fn synchronization_does_not_query_unadvertised_equalizers() {
    let fixture = Fixture::new(handshake(false)).await;
    assert!(fixture.handle.snapshot().equalizers.is_empty());
    fixture.refresh().await.unwrap();
    fixture.refresh().await.unwrap();
    fixture.close().await;
}
