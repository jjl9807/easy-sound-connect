//! Independent synthetic peers exercise the asynchronous wire boundary. These
//! fixtures are not hardware captures and make no device compatibility claim.
use bluetooth::{
    Backend, BoxFuture, CancellationToken, Connection, ConnectionInfo, Device, DeviceId, Driver,
    TransportKind,
};
use ezsc_core::{
    BatteryComponent, ConnectionState, DeviceManager, Error, Freshness, NoiseChange,
    OperationResult, SessionConfig, TimeoutStage,
};
use libmdr2::{
    common::AudioCodec,
    equalizer::{EqualizerKind, EqualizerSetting, PresetId},
    frame::{DataType, Decoder, Frame, Sequence},
    noise::NoiseKind,
    types::DisplayLanguage,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct TestBackend {
    connections: Mutex<HashMap<DeviceId, VecDeque<Connection>>>,
}
impl TestBackend {
    fn insert(&self, id: &str) -> Driver {
        let id = DeviceId(id.into());
        let (connection, driver) = bluetooth::driver_channel(
            ConnectionInfo {
                device: id.clone(),
                transport: TransportKind::Gatt,
                max_write_size: Some(7),
            },
            64,
        )
        .unwrap();
        self.connections
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push_back(connection);
        driver
    }
}
impl Backend for TestBackend {
    fn discover(&self, _: CancellationToken) -> BoxFuture<'_, bluetooth::Result<Vec<Device>>> {
        Box::pin(async {
            Ok(self
                .connections
                .lock()
                .unwrap()
                .keys()
                .map(|id| Device {
                    id: id.clone(),
                    name: None,
                    transports: vec![TransportKind::Gatt],
                })
                .collect())
        })
    }
    fn connect(
        &self,
        id: DeviceId,
        _: TransportKind,
        _: CancellationToken,
    ) -> BoxFuture<'_, bluetooth::Result<Connection>> {
        Box::pin(async move {
            self.connections
                .lock()
                .unwrap()
                .get_mut(&id)
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| {
                    bluetooth::Error::new(bluetooth::ErrorKind::NotFound, "fixture exhausted")
                })
        })
    }
}
#[derive(Clone, Copy)]
enum WriteBehavior {
    Complete,
    Hold,
    Fail,
}
struct Exchange {
    table: DataType,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    notices: Vec<Vec<u8>>,
    write: WriteBehavior,
    acknowledge: bool,
    after_response: Vec<Vec<u8>>,
    ack_last: bool,
}
impl Exchange {
    fn new(request: &[u8], response: &[u8]) -> Self {
        Self {
            table: DataType::DATA_MDR,
            request: request.into(),
            response: Some(response.into()),
            notices: Vec::new(),
            write: WriteBehavior::Complete,
            acknowledge: true,
            after_response: Vec::new(),
            ack_last: false,
        }
    }
    fn ack(request: &[u8]) -> Self {
        let mut value = Self::new(request, &[]);
        value.response = None;
        value
    }
    fn t2(mut self) -> Self {
        self.table = DataType::DATA_MDR_NO2;
        self
    }
    fn notice(mut self, payload: &[u8]) -> Self {
        self.notices.push(payload.into());
        self
    }
    fn hold(mut self) -> Self {
        self.write = WriteBehavior::Hold;
        self
    }
    fn fail(mut self) -> Self {
        self.write = WriteBehavior::Fail;
        self
    }
    fn without_ack(mut self) -> Self {
        self.acknowledge = false;
        self
    }
    fn notification_after_response(mut self, payload: &[u8]) -> Self {
        self.after_response.push(payload.into());
        self.ack_last = true;
        self
    }
}
fn handshake(functions: &[u8], t2: bool) -> Vec<Exchange> {
    let mut list = vec![7, 0, functions.len() as u8];
    for function in functions {
        list.extend([*function, 0]);
    }
    let mut exchanges = vec![
        Exchange::new(&[0, 0], &[1, 0, 3, 0, 0x30, 0x32, 0, u8::from(!t2)]),
        Exchange::new(&[2, 0], &[3, 0, 9, 0]),
        Exchange::new(&[6, 0], &list),
    ];
    if t2 {
        exchanges.push(Exchange::new(&[6, 0], &[7, 0, 1, 0x20, 5]).t2());
    }
    exchanges.extend([
        Exchange::new(&[4, 1], b"\x05\x01\x04Demo"),
        Exchange::new(&[4, 2], b"\x05\x02\x031.0"),
        Exchange::new(&[4, 3], &[5, 3, 0xfe, 0xfd]),
    ]);
    exchanges
}
async fn send(driver: &Driver, frame: Frame) {
    // Byte splitting includes headers and escaped bytes.
    for chunk in frame.encode(16384).unwrap().chunks(2) {
        driver.incoming.send(chunk.to_vec()).await.unwrap();
    }
}
type Trace = Arc<Mutex<Vec<Vec<u8>>>>;
fn peer(mut driver: Driver, script: Vec<Exchange>) -> (tokio::task::JoinHandle<()>, Trace) {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let record = trace.clone();
    let task = tokio::spawn(async move {
        let mut script: VecDeque<_> = script.into();
        let mut decoder = Decoder::new(16384).unwrap();
        let mut sequence = Sequence::Zero;
        let mut held = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = driver.cancel.cancelled() => break,
                write = driver.writes.recv() => {
                    let Some(write) = write else { break; };
                    let frames: Vec<_> = decoder.feed(&write.bytes).collect::<std::result::Result<_, _>>().unwrap();
                    assert_eq!(frames.len(), 1);
                    let frame = frames.into_iter().next().unwrap();
                    if frame.data_type == DataType::ACK { let _ = write.completion.send(Ok(())); continue; }
                    record.lock().unwrap().push(frame.payload.clone());
                    let expected = script.pop_front().unwrap_or_else(|| panic!("unexpected data write: {:02x?}", frame.payload));
                    assert_eq!(frame.data_type, expected.table);
                    assert_eq!(frame.payload, expected.request);
                    if matches!(expected.write, WriteBehavior::Fail) {
                        let _ = write.completion.send(Err(bluetooth::Error::new(bluetooth::ErrorKind::Io, "synthetic partial write failure")));
                        continue;
                    }
                    match expected.write {
                        WriteBehavior::Complete => { let _ = write.completion.send(Ok(())); }
                        WriteBehavior::Hold => held.push(write.completion),
                        WriteBehavior::Fail => unreachable!(),
                    }
                    if expected.acknowledge && !expected.ack_last {
                        send(&driver, Frame::acknowledgement(frame.sequence)).await;
                    }
                    for payload in expected.notices {
                        send(&driver, Frame { data_type: expected.table, sequence, payload }).await;
                        sequence = sequence.toggled();
                    }
                    if let Some(payload) = expected.response {
                        send(&driver, Frame { data_type: expected.table, sequence, payload }).await;
                        sequence = sequence.toggled();
                    }
                    for payload in expected.after_response {
                        send(&driver, Frame { data_type: expected.table, sequence, payload }).await;
                        sequence = sequence.toggled();
                    }
                    if expected.acknowledge && expected.ack_last {
                        send(&driver, Frame::acknowledgement(frame.sequence)).await;
                    }
                }
            }
        }
        drop(held);
        driver.finish(Ok(()));
    });
    (task, trace)
}
async fn connect(manager: &DeviceManager, id: &str) -> ezsc_core::DeviceHandle {
    manager
        .connect(
            DeviceId(id.into()),
            TransportKind::Gatt,
            SessionConfig::default(),
        )
        .await
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn handshake_retains_table_identity_and_tws_zero_and_case_readings() {
    let backend = Arc::new(TestBackend::default());
    let driver = backend.insert("tws");
    let mut script = handshake(&[0x21, 0x22, 0xfa], true);
    script.extend([
        Exchange::new(&[0x22, 1], &[0x23, 1, 0, 0, 83, 1]).notice(&[0x25, 1, 14, 0, 19, 0]),
        Exchange::new(&[0x22, 2], &[0x23, 2, 95, 0]),
    ]);
    let (peer, trace) = peer(driver, script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "tws").await;
    assert_eq!(handle.snapshot().connection, ConnectionState::Connected);
    assert_eq!(
        handle.initialize().await.unwrap(),
        OperationResult::Synchronized
    );
    let state = handle.snapshot();
    assert_eq!(state.connection, ConnectionState::Ready);
    assert_eq!(state.model_name.unwrap().as_utf8().unwrap(), "Demo");
    let appearance = state.appearance.unwrap();
    assert_eq!(appearance.series.as_raw(), 0xfe);
    assert_eq!(appearance.color.as_raw(), 0xfd);
    assert!(trace.lock().unwrap().contains(&vec![4, 3]));
    assert_eq!(state.batteries.len(), 3);
    assert_eq!(
        state
            .batteries
            .iter()
            .find(|entry| entry.component == BatteryComponent::Left)
            .unwrap()
            .reading
            .unwrap()
            .level
            .as_u8(),
        0
    );
    assert_eq!(
        state
            .batteries
            .iter()
            .find(|entry| entry.component == BatteryComponent::Case)
            .unwrap()
            .reading
            .unwrap()
            .level
            .as_u8(),
        95
    );
    assert_eq!(state.functions.len(), 4);
    assert!(!state.functions[2].readable);
    assert!(!state.functions[3].readable); // T2 20 is not battery.
    handle.close().await.unwrap();
    assert!(
        handle
            .snapshot()
            .batteries
            .iter()
            .all(|entry| entry.freshness == Freshness::Stale)
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn malformed_appearance_keeps_color_unknown_without_changing_device_capabilities() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x20], false);
    *script.last_mut().unwrap() = Exchange::new(&[4, 3], &[5, 3, 0x01]);
    script.push(Exchange::new(&[0x22, 0], &[0x23, 0, 80, 0]));
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert_eq!(
        handle.initialize().await.unwrap(),
        OperationResult::Synchronized
    );
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.connection, ConnectionState::Ready);
    assert_eq!(snapshot.appearance, None);
    assert!(snapshot.last_error.is_some());
    assert_eq!(snapshot.batteries[0].reading.unwrap().level.as_u8(), 80);
    assert!(snapshot.functions[0].readable);
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn appearance_query_ack_does_not_supply_color_or_complete_initialization() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[], false);
    *script.last_mut().unwrap() = Exchange::ack(&[4, 3]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert_eq!(
        handle.initialize().await,
        Err(Error::Timeout(TimeoutStage::Response))
    );
    handle.close().await.unwrap();
    assert_eq!(handle.snapshot().connection, ConnectionState::Failed);
    assert_eq!(handle.snapshot().appearance, None);
    assert_eq!(trace.lock().unwrap().last(), Some(&vec![4, 3]));
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn notification_and_link_ack_cannot_complete_a_read() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x20], false);
    script.push(Exchange::ack(&[0x22, 0]).notice(&[0x25, 0, 55, 0]));
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert_eq!(
        handle.initialize().await,
        Err(Error::Timeout(TimeoutStage::Response))
    );
    handle.close().await.unwrap();
    assert_eq!(handle.snapshot().connection, ConnectionState::Failed);
    assert_eq!(
        handle.snapshot().batteries[0]
            .reading
            .unwrap()
            .level
            .as_u8(),
        55
    );
    assert_eq!(handle.snapshot().batteries[0].freshness, Freshness::Stale);
    assert_eq!(handle.refresh_battery().await, Err(Error::Disconnected));
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn early_ack_and_response_do_not_hide_an_unfinished_write() {
    let backend = Arc::new(TestBackend::default());
    let (peer, trace) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).hold()],
    );
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert_eq!(
        handle.initialize().await,
        Err(Error::Timeout(TimeoutStage::Write))
    );
    assert!(
        handle.snapshot().protocol.is_some(),
        "receives must continue while writing"
    );
    assert_eq!(
        trace.lock().unwrap().len(),
        1,
        "no second write may overlap the unfinished write"
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn partial_write_failure_is_terminal_and_reconnect_has_a_new_generation() {
    let backend = Arc::new(TestBackend::default());
    let (old_peer, _) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).fail()],
    );
    let (new_peer, _) = peer(backend.insert("a"), handshake(&[], false));
    let manager = DeviceManager::new(backend);
    let first = connect(&manager, "a").await;
    assert!(matches!(first.initialize().await, Err(Error::Bluetooth(_))));
    first.close().await.unwrap();
    old_peer.await.unwrap();
    let next = connect(&manager, "a").await;
    assert_ne!(first.snapshot().generation, next.snapshot().generation);
    assert!(next.snapshot().protocol.is_none());
    assert_eq!(
        next.snapshot().message_counters,
        ezsc_core::MessageCounters::default()
    );
    next.initialize().await.unwrap();
    first.cancel(); // Old handles cannot cancel the replacement connection.
    assert_eq!(next.snapshot().connection, ConnectionState::Ready);
    next.close().await.unwrap();
    new_peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn malformed_matching_body_never_completes_initialization() {
    let backend = Arc::new(TestBackend::default());
    let (peer, _) = peer(
        backend.insert("a"),
        vec![Exchange::new(&[0, 0], &[1, 0, 3])],
    );
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert!(matches!(handle.initialize().await, Err(Error::Protocol(_))));
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn one_stuck_device_does_not_block_another_and_shutdown_cancels_both() {
    let backend = Arc::new(TestBackend::default());
    let (peer_a, _) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).hold()],
    );
    let (peer_b, _) = peer(backend.insert("b"), handshake(&[], false));
    let manager = DeviceManager::new(backend);
    let a = connect(&manager, "a").await;
    let b = connect(&manager, "b").await;
    let task = tokio::spawn({
        let a = a.clone();
        async move { a.initialize().await }
    });
    b.initialize().await.unwrap();
    assert!(!task.is_finished());
    manager.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap(), Err(Error::Cancelled));
    peer_a.await.unwrap();
    peer_b.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn noise_edit_preserves_other_fields_and_requires_readback() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x6d], false);
    script.extend([
        Exchange::new(&[0x60, 0x19], &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1]),
        Exchange::new(&[0x62, 0x19], &[0x63, 0x19, 0]),
        Exchange::new(&[0x66, 0x19], &[0x67, 0x19, 1, 1, 1, 1, 17, 1, 2]),
        Exchange::ack(&[0x68, 0x19, 1, 0, 1, 1, 17, 1, 2])
            .notice(&[0x69, 0x19, 1, 0, 1, 1, 17, 1, 2]),
        // Device adjusted one field, so matching earlier notification cannot
        // make this operation report its requested state as confirmed.
        Exchange::new(&[0x66, 0x19], &[0x67, 0x19, 1, 0, 1, 1, 16, 1, 2]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle
            .set_noise(
                NoiseKind::AdaptiveNcAndAmbientLevel,
                NoiseChange::Effect(false)
            )
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    assert!(
        trace
            .lock()
            .unwrap()
            .contains(&vec![0x68, 0x19, 1, 0, 1, 1, 17, 1, 2])
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unavailable_noise_does_not_send_a_setting_and_session_remains_usable() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x61], false);
    script.extend([
        Exchange::new(&[0x62, 1], &[0x63, 1, 1]),
        Exchange::new(&[0x66, 1], &[0x67, 1, 1, 1, 1]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert!(matches!(
        handle
            .set_noise(NoiseKind::NcOnly, NoiseChange::Effect(false))
            .await,
        Err(Error::Unavailable(_))
    ));
    assert!(
        trace
            .lock()
            .unwrap()
            .iter()
            .all(|payload| payload[0] != 0x68)
    );
    assert_eq!(handle.snapshot().connection, ConnectionState::Ready);
    assert_eq!(
        handle.refresh_battery().await.unwrap(),
        OperationResult::Synchronized
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn equalizer_selects_custom_slot_before_writing_curve() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x50], false);
    script.extend([
        Exchange::new(&[0x50, 0, 1], &[0x51, 0, 2, 13, 2, 0, 0, 0xa0, 0]),
        Exchange::new(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0, 2, 6, 6]),
        Exchange::ack(&[0x58, 0, 0xa0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0xa0, 2, 6, 6]),
        Exchange::new(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::ack(&[0x58, 0, 0xa0, 2, 5, 7]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0xa0, 2, 5, 7]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let setting = EqualizerSetting::Preset {
        preset: PresetId::from_raw(0xa0),
        levels: Some(vec![5, 7]),
    };
    assert_eq!(
        handle
            .set_equalizer(EqualizerKind::Preset, setting)
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn bounded_queue_and_abandoned_request_cancel_without_waiting_for_io() {
    let backend = Arc::new(TestBackend::default());
    let (peer, trace) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).hold()],
    );
    let manager = DeviceManager::new(backend);
    let handle = manager
        .connect(
            DeviceId("a".into()),
            TransportKind::Gatt,
            SessionConfig {
                queue_capacity: 1,
                ..SessionConfig::default()
            },
        )
        .await
        .unwrap();
    let active = tokio::spawn({
        let handle = handle.clone();
        async move { handle.initialize().await }
    });
    while trace.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    let queued = tokio::spawn({
        let handle = handle.clone();
        async move { handle.refresh_battery().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(handle.refresh_battery().await, Err(Error::QueueFull));
    active.abort();
    assert_eq!(queued.await.unwrap(), Err(Error::Cancelled));
    handle.close().await.unwrap();
    peer.await.unwrap();
}

struct BlockingBackend {
    started: tokio::sync::Notify,
    cancelled: Arc<tokio::sync::Notify>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}
struct DropProbe(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Backend for BlockingBackend {
    fn discover(&self, token: CancellationToken) -> BoxFuture<'_, bluetooth::Result<Vec<Device>>> {
        let done = self.cancelled.clone();
        let native_cancel = token.clone();
        tokio::spawn(async move {
            token.cancelled().await;
            done.notify_one();
        });
        let guard = DropProbe(self.dropped.clone());
        Box::pin(async move {
            let _guard = guard;
            self.started.notify_one();
            native_cancel.cancelled().await;
            Err(bluetooth::Error::cancelled())
        })
    }
    fn connect(
        &self,
        _: DeviceId,
        _: TransportKind,
        token: CancellationToken,
    ) -> BoxFuture<'_, bluetooth::Result<Connection>> {
        let done = self.cancelled.clone();
        let native_cancel = token.clone();
        tokio::spawn(async move {
            token.cancelled().await;
            done.notify_one();
        });
        let guard = DropProbe(self.dropped.clone());
        Box::pin(async move {
            let _guard = guard;
            self.started.notify_one();
            native_cancel.cancelled().await;
            Err(bluetooth::Error::cancelled())
        })
    }
}
#[tokio::test]
async fn dropping_connect_and_discovery_futures_cancels_platform_work() {
    let backend = Arc::new(BlockingBackend {
        started: tokio::sync::Notify::new(),
        cancelled: Arc::new(tokio::sync::Notify::new()),
        dropped: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let manager = Arc::new(DeviceManager::new(backend.clone()));
    let connecting = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .connect(
                    DeviceId("a".into()),
                    TransportKind::Gatt,
                    SessionConfig::default(),
                )
                .await
        }
    });
    backend.started.notified().await;
    connecting.abort();
    let _ = connecting.await;
    tokio::time::timeout(Duration::from_secs(1), backend.cancelled.notified())
        .await
        .unwrap();
    let discovery = tokio::spawn({
        let manager = manager.clone();
        async move { manager.discover().await }
    });
    backend.started.notified().await;
    discovery.abort();
    let _ = discovery.await;
    tokio::time::timeout(Duration::from_secs(1), backend.cancelled.notified())
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn response_without_link_ack_exhausts_the_independent_ack_budget() {
    let backend = Arc::new(TestBackend::default());
    let (peer, trace) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).without_ack()],
    );
    let manager = DeviceManager::new(backend);
    let handle = manager
        .connect(
            DeviceId("a".into()),
            TransportKind::Gatt,
            SessionConfig {
                retry_policy: libmdr2::session::RetryPolicy::new(Duration::from_millis(20), 0)
                    .unwrap(),
                ..SessionConfig::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        handle.initialize().await,
        Err(Error::Timeout(TimeoutStage::Ack))
    );
    assert!(handle.snapshot().protocol.is_some());
    assert_eq!(trace.lock().unwrap().len(), 1);
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn queued_operation_expires_while_another_operation_is_still_writing() {
    let backend = Arc::new(TestBackend::default());
    let (peer, trace) = peer(
        backend.insert("a"),
        vec![handshake(&[], false).remove(0).hold()],
    );
    let manager = DeviceManager::new(backend);
    let handle = manager
        .connect(
            DeviceId("a".into()),
            TransportKind::Gatt,
            SessionConfig {
                queue_timeout: Duration::from_millis(20),
                ..SessionConfig::default()
            },
        )
        .await
        .unwrap();
    let active = tokio::spawn({
        let handle = handle.clone();
        async move { handle.initialize().await }
    });
    while trace.lock().unwrap().is_empty() {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        handle.refresh_battery().await,
        Err(Error::Timeout(TimeoutStage::Queue))
    );
    assert!(!active.is_finished());
    assert_eq!(trace.lock().unwrap().len(), 1);
    handle.close().await.unwrap();
    assert_eq!(active.await.unwrap(), Err(Error::Cancelled));
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn delayed_callbacks_and_bytes_from_old_connection_cannot_reach_replacement() {
    let backend = Arc::new(TestBackend::default());
    let mut old_driver = backend.insert("a");
    let stale_incoming = old_driver.incoming.clone();
    let (new_peer, _) = peer(backend.insert("a"), handshake(&[], false));
    let manager = DeviceManager::new(backend);
    let old = connect(&manager, "a").await;
    let active = tokio::spawn({
        let old = old.clone();
        async move { old.initialize().await }
    });
    let delayed = old_driver.writes.recv().await.unwrap();
    old.cancel();
    old_driver.finish(Ok(()));
    old.close().await.unwrap();
    assert!(active.await.unwrap().is_err());
    let current = connect(&manager, "a").await;
    current.initialize().await.unwrap();
    let before = current.snapshot();
    assert!(delayed.completion.send(Ok(())).is_err());
    assert!(stale_incoming.send(vec![0, 1, 2]).await.is_err());
    old.cancel();
    tokio::task::yield_now().await;
    assert_eq!(current.snapshot(), before);
    current.close().await.unwrap();
    new_peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn unknown_feature_layout_fails_only_that_feature_after_consuming_its_reply() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x6d], false);
    script.push(Exchange::new(&[0x60, 0x19], &[0x61, 0x19, 0xff]));
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert!(matches!(
        handle
            .refresh_noise(NoiseKind::AdaptiveNcAndAmbientLevel)
            .await,
        Err(Error::Protocol(_))
    ));
    assert_eq!(handle.snapshot().connection, ConnectionState::Ready);
    assert_eq!(
        handle.refresh_battery().await.unwrap(),
        OperationResult::Synchronized
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn disconnect_and_shutdown_wait_until_pending_backend_futures_are_dropped() {
    let backend = Arc::new(BlockingBackend {
        started: tokio::sync::Notify::new(),
        cancelled: Arc::new(tokio::sync::Notify::new()),
        dropped: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let manager = Arc::new(DeviceManager::new(backend.clone()));
    let launch = || {
        tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .connect(
                        DeviceId("a".into()),
                        TransportKind::Gatt,
                        SessionConfig::default(),
                    )
                    .await
            }
        })
    };
    let first = launch();
    backend.started.notified().await;
    manager.disconnect(&DeviceId("a".into())).await.unwrap();
    assert_eq!(backend.dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(matches!(first.await.unwrap(), Err(Error::Cancelled)));
    let second = launch();
    backend.started.notified().await; // Immediate reconnect has no stale reservation.
    let discovery = tokio::spawn({
        let manager = manager.clone();
        async move { manager.discover().await }
    });
    backend.started.notified().await;
    manager.shutdown().await.unwrap();
    assert_eq!(backend.dropped.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(matches!(second.await.unwrap(), Err(Error::Cancelled)));
    assert!(matches!(discovery.await.unwrap(), Err(Error::Cancelled)));
}

struct DetachedSetupBackend {
    started: Arc<tokio::sync::Notify>,
    cancel_seen: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    cleaned: Arc<std::sync::atomic::AtomicBool>,
    late_connection: Mutex<Option<Connection>>,
}
impl DetachedSetupBackend {
    fn new() -> Self {
        Self {
            started: Arc::default(),
            cancel_seen: Arc::default(),
            release: Arc::default(),
            cleaned: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            late_connection: Mutex::new(None),
        }
    }
}
impl Backend for DetachedSetupBackend {
    fn discover(&self, _: CancellationToken) -> BoxFuture<'_, bluetooth::Result<Vec<Device>>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn connect(
        &self,
        _: DeviceId,
        _: TransportKind,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, bluetooth::Result<Connection>> {
        let started = self.started.clone();
        let cancel_seen = self.cancel_seen.clone();
        let release = self.release.clone();
        let cleaned = self.cleaned.clone();
        let late_connection = self.late_connection.lock().unwrap().take();
        let (finish, receive) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            started.notify_one();
            cancel.cancelled().await;
            cancel_seen.notify_one();
            release.notified().await; // Represents asynchronous native teardown.
            cleaned.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = finish.send(late_connection.ok_or_else(bluetooth::Error::cancelled));
        });
        Box::pin(async move { receive.await.unwrap() })
    }
}

#[tokio::test]
async fn detached_native_setup_cleanup_is_awaited_after_disconnect_shutdown_and_caller_drop() {
    for mode in 0..3 {
        let backend = Arc::new(DetachedSetupBackend::new());
        let manager = Arc::new(DeviceManager::new(backend.clone()));
        let connect = tokio::spawn({
            let manager = manager.clone();
            async move {
                manager
                    .connect(
                        DeviceId("a".into()),
                        TransportKind::Gatt,
                        SessionConfig::default(),
                    )
                    .await
            }
        });
        backend.started.notified().await;
        let mut connect = Some(connect);
        if mode == 2 {
            let connect = connect.take().unwrap();
            connect.abort();
            let _ = connect.await;
            backend.cancel_seen.notified().await;
        }
        let cleanup = tokio::spawn({
            let manager = manager.clone();
            async move {
                if mode == 1 {
                    manager.shutdown().await
                } else {
                    manager.disconnect(&DeviceId("a".into())).await
                }
            }
        });
        if mode != 2 {
            backend.cancel_seen.notified().await;
        }
        tokio::task::yield_now().await;
        assert!(
            !cleanup.is_finished(),
            "cancellation is not native cleanup completion"
        );
        assert!(!backend.cleaned.load(std::sync::atomic::Ordering::SeqCst));
        if mode != 1 {
            assert!(matches!(
                manager
                    .connect(
                        DeviceId("a".into()),
                        TransportKind::Gatt,
                        SessionConfig::default()
                    )
                    .await,
                Err(Error::AlreadyConnected)
            ));
        }
        backend.release.notify_one();
        cleanup.await.unwrap().unwrap();
        assert!(backend.cleaned.load(std::sync::atomic::Ordering::SeqCst));
        if let Some(connect) = connect {
            assert!(matches!(connect.await.unwrap(), Err(Error::Cancelled)));
        }
        manager.shutdown().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn setup_cleanup_timeout_reports_failure_but_retains_connection_reservation() {
    let backend = Arc::new(DetachedSetupBackend::new());
    let manager = Arc::new(DeviceManager::new(backend.clone()));
    let connect = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .connect(
                    DeviceId("a".into()),
                    TransportKind::Gatt,
                    SessionConfig {
                        close_timeout: Duration::from_millis(20),
                        ..SessionConfig::default()
                    },
                )
                .await
        }
    });
    backend.started.notified().await;
    assert_eq!(
        manager.disconnect(&DeviceId("a".into())).await,
        Err(Error::Timeout(TimeoutStage::Close))
    );
    assert!(matches!(
        connect.await.unwrap(),
        Err(Error::Timeout(TimeoutStage::Close))
    ));
    assert!(!backend.cleaned.load(std::sync::atomic::Ordering::SeqCst));
    assert!(matches!(
        manager
            .connect(
                DeviceId("a".into()),
                TransportKind::Gatt,
                SessionConfig::default()
            )
            .await,
        Err(Error::AlreadyConnected)
    ));
    backend.release.notify_one();
    while !backend.cleaned.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        manager.shutdown().await,
        Err(Error::Timeout(TimeoutStage::Close))
    );
}

#[tokio::test(start_paused = true)]
async fn close_timeout_does_not_allow_reconnect_until_driver_cleanup_finishes() {
    let backend = Arc::new(TestBackend::default());
    let old_driver = backend.insert("a");
    let (new_peer, _) = peer(backend.insert("a"), handshake(&[], false));
    let manager = DeviceManager::new(backend);
    let old = manager
        .connect(
            DeviceId("a".into()),
            TransportKind::Gatt,
            SessionConfig {
                close_timeout: Duration::from_millis(20),
                ..SessionConfig::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(old.close().await, Err(Error::Timeout(TimeoutStage::Close)));
    assert!(matches!(
        manager
            .connect(
                DeviceId("a".into()),
                TransportKind::Gatt,
                SessionConfig::default()
            )
            .await,
        Err(Error::AlreadyConnected)
    ));
    old_driver.finish(Ok(()));
    tokio::task::yield_now().await;
    let new = connect(&manager, "a").await;
    new.initialize().await.unwrap();
    new.close().await.unwrap();
    new_peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn optional_initial_battery_layout_failure_preserves_other_endpoints() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x21, 0x22], false);
    script.extend([
        Exchange::new(&[0x22, 1], &[0x23, 1, 0xff]),
        Exchange::new(&[0x22, 2], &[0x23, 2, 95, 0]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let state = handle.snapshot();
    assert_eq!(state.connection, ConnectionState::Ready);
    assert!(state.last_error.is_some());
    assert!(
        state
            .batteries
            .iter()
            .filter(|b| b.component != BatteryComponent::Case)
            .all(|b| b.reading.is_none() && b.freshness == Freshness::Unknown)
    );
    assert_eq!(
        state
            .batteries
            .iter()
            .find(|b| b.component == BatteryComponent::Case)
            .unwrap()
            .reading
            .unwrap()
            .level
            .as_u8(),
        95
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn notification_after_noise_readback_before_ack_cannot_change_operation_result() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x61], false);
    script.extend([
        Exchange::new(&[0x62, 1], &[0x63, 1, 0]),
        Exchange::new(&[0x66, 1], &[0x67, 1, 1, 1, 1]),
        Exchange::ack(&[0x68, 1, 1, 0, 1]),
        Exchange::new(&[0x66, 1], &[0x67, 1, 1, 1, 1])
            .notification_after_response(&[0x69, 1, 1, 0, 1]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle
            .set_noise(NoiseKind::NcOnly, NoiseChange::Effect(false))
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    let state = handle.snapshot();
    assert!(matches!(
        state.noise[0].parameters,
        Some(libmdr2::noise::NoiseParameters::Control {
            effect: libmdr2::noise::NoiseSwitch::Off,
            ..
        })
    ));
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn notification_cannot_satisfy_equalizer_preset_readback() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x50], false);
    script.extend([
        Exchange::new(&[0x50, 0, 1], &[0x51, 0, 2, 13, 2, 0, 0, 0xa0, 0]),
        Exchange::new(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0, 2, 6, 6]),
        Exchange::ack(&[0x58, 0, 0xa0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0, 2, 6, 6])
            .notification_after_response(&[0x59, 0, 0xa0, 2, 6, 6]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert!(matches!(
        handle
            .set_equalizer(
                EqualizerKind::Preset,
                EqualizerSetting::Preset {
                    preset: PresetId::from_raw(0xa0),
                    levels: Some(vec![5, 7])
                }
            )
            .await,
        Err(Error::Unavailable(_))
    ));
    assert!(
        !trace
            .lock()
            .unwrap()
            .contains(&vec![0x58, 0, 0xa0, 2, 5, 7])
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn notification_after_equalizer_curve_readback_cannot_change_operation_result() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x50], false);
    script.extend([
        Exchange::new(&[0x50, 0, 1], &[0x51, 0, 2, 13, 2, 0, 0, 0xa0, 0]),
        Exchange::new(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0xa0, 2, 6, 6]),
        Exchange::ack(&[0x58, 0, 0xa0, 2, 5, 7]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0xa0, 2, 6, 6])
            .notification_after_response(&[0x59, 0, 0xa0, 2, 5, 7]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle
            .set_equalizer(
                EqualizerKind::Preset,
                EqualizerSetting::Preset {
                    preset: PresetId::from_raw(0xa0),
                    levels: Some(vec![5, 7])
                }
            )
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    assert_eq!(
        handle.snapshot().equalizers[0]
            .parameters
            .as_ref()
            .unwrap()
            .levels(),
        &[5, 7]
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn successful_native_setup_racing_cancel_is_closed_before_reservation_release() {
    let backend = Arc::new(DetachedSetupBackend::new());
    let (connection, driver) = bluetooth::driver_channel(
        ConnectionInfo {
            device: DeviceId("a".into()),
            transport: TransportKind::Gatt,
            max_write_size: None,
        },
        8,
    )
    .unwrap();
    *backend.late_connection.lock().unwrap() = Some(connection);
    let manager = Arc::new(DeviceManager::new(backend.clone()));
    let connecting = tokio::spawn({
        let manager = manager.clone();
        async move {
            manager
                .connect(
                    DeviceId("a".into()),
                    TransportKind::Gatt,
                    SessionConfig::default(),
                )
                .await
        }
    });
    backend.started.notified().await;
    let closing = tokio::spawn({
        let manager = manager.clone();
        async move { manager.disconnect(&DeviceId("a".into())).await }
    });
    backend.cancel_seen.notified().await;
    backend.release.notify_one();
    driver.cancel.cancelled().await;
    assert!(!closing.is_finished());
    assert!(matches!(
        manager
            .connect(
                DeviceId("a".into()),
                TransportKind::Gatt,
                SessionConfig::default()
            )
            .await,
        Err(Error::AlreadyConnected)
    ));
    driver.finish(Ok(()));
    closing.await.unwrap().unwrap();
    assert!(matches!(connecting.await.unwrap(), Err(Error::Cancelled)));
    manager.shutdown().await.unwrap();
}

fn eq_reads(inquiry: u8, capability: &[u8], parameters: &[u8]) -> [Exchange; 3] {
    let capability = [&[0x51, inquiry][..], capability].concat();
    let parameters = [&[0x57, inquiry][..], parameters].concat();
    let mut status = vec![0x53, inquiry, 0];
    if inquiry == 4 {
        status.push(0);
    }
    [
        Exchange::new(&[0x50, inquiry, 1], &capability),
        Exchange::new(&[0x52, inquiry], &status),
        Exchange::new(&[0x56, inquiry], &parameters),
    ]
}

#[tokio::test(start_paused = true)]
async fn display_language_refreshes_loaded_labels_without_changing_settings_or_identity() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x53, 0x55], false);
    let english = b"\x02\x0d\x02\x01\xa0\x06Custom";
    script.extend(eq_reads(3, english, &[0xa0, 2, 2, 4, 9]));
    let capability = [&[0x51, 3][..], english].concat();
    let bands = [0x5b, 3, 2, 1, 0, 31, 1, 0x27, 0x10];
    script.extend([
        Exchange::new(&[0x50, 3, 1], &capability),
        Exchange::new(&[0x5a, 3], &bands),
        Exchange::new(
            &[0x50, 3, 0x0c],
            &[&[0x51, 3, 2, 13, 2, 1, 0xa0, 9][..], "自定义".as_bytes()].concat(),
        ),
        Exchange::new(&[0x5a, 3], &bands),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    handle
        .refresh_equalizer(EqualizerKind::PresetWithUlt)
        .await
        .unwrap();
    handle
        .refresh_equalizer_band_info(EqualizerKind::PresetWithUlt)
        .await
        .unwrap();
    let before = handle.snapshot();
    let start = trace.lock().unwrap().len();
    assert_eq!(
        handle
            .set_display_language(DisplayLanguage::SIMPLIFIED_CHINESE)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    let after = handle.snapshot();
    assert_eq!(after.language, DisplayLanguage::SIMPLIFIED_CHINESE);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.device, before.device);
    assert_eq!(after.connection, ConnectionState::Ready);
    for (old, new) in before.equalizers.iter().zip(&after.equalizers) {
        assert_eq!(new.parameters, old.parameters);
        assert_eq!(new.status, old.status);
        assert_eq!(new.freshness, old.freshness);
        assert_eq!(new.band_info, old.band_info);
    }
    assert_eq!(
        after.equalizers[0].capability.as_ref().unwrap().presets()[0].id,
        PresetId::CUSTOM
    );
    assert_eq!(
        after.equalizers[0].capability.as_ref().unwrap().presets()[0]
            .name_utf8()
            .unwrap(),
        "自定义"
    );
    assert!(
        after.equalizers[1].capability.is_none(),
        "language selection does not opt into an unloaded layout"
    );
    assert_eq!(
        &trace.lock().unwrap()[start..],
        &[vec![0x50, 3, 0x0c], vec![0x5a, 3]]
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn display_language_waits_for_prior_operations_and_changes_subsequent_queries() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x55], false);
    script.extend(eq_reads(0x31, &[2, 13], &[2, 4, 9]));
    script.extend([
        Exchange::new(&[0x50, 0x31, 0x0c], &[0x51, 0x31, 2, 13]),
        Exchange::new(&[0x50, 0x31, 0x0c], &[0x51, 0x31, 2, 13]),
        Exchange::new(&[0x52, 0x31], &[0x53, 0x31, 0]),
        Exchange::new(&[0x56, 0x31], &[0x57, 0x31, 2, 4, 9]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let start = trace.lock().unwrap().len();
    let (refresh, language) = tokio::join!(biased;
        handle.refresh_equalizer(EqualizerKind::Custom),
        handle.set_display_language(DisplayLanguage::SIMPLIFIED_CHINESE),
    );
    assert_eq!(refresh.unwrap(), OperationResult::Synchronized);
    assert_eq!(language.unwrap(), OperationResult::Synchronized);
    handle
        .refresh_equalizer(EqualizerKind::Custom)
        .await
        .unwrap();
    assert_eq!(
        &trace.lock().unwrap()[start..],
        &[
            vec![0x50, 0x31, 1],
            vec![0x52, 0x31],
            vec![0x56, 0x31],
            vec![0x50, 0x31, 0x0c],
            vec![0x50, 0x31, 0x0c],
            vec![0x52, 0x31],
            vec![0x56, 0x31],
        ]
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn display_language_without_loaded_capabilities_requires_no_device_messages() {
    let backend = Arc::new(TestBackend::default());
    let (peer, trace) = peer(backend.insert("a"), handshake(&[0x50], false));
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    assert_eq!(
        handle
            .set_display_language(DisplayLanguage::SIMPLIFIED_CHINESE)
            .await,
        Err(Error::NotReady)
    );
    assert_eq!(handle.snapshot().language, DisplayLanguage::ENGLISH);
    handle.initialize().await.unwrap();
    let start = trace.lock().unwrap().len();
    handle
        .set_display_language(DisplayLanguage::SIMPLIFIED_CHINESE)
        .await
        .unwrap();
    assert_eq!(trace.lock().unwrap().len(), start);
    assert_eq!(
        handle.snapshot().language,
        DisplayLanguage::SIMPLIFIED_CHINESE
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn display_language_query_timeout_uses_normal_session_invalidation() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x55], false);
    script.extend(eq_reads(0x31, &[2, 13], &[2, 4, 9]));
    script.push(Exchange::ack(&[0x50, 0x31, 0x0c]));
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    handle
        .refresh_equalizer(EqualizerKind::Custom)
        .await
        .unwrap();
    let parameters = handle.snapshot().equalizers[0].parameters.clone();
    assert_eq!(
        handle
            .set_display_language(DisplayLanguage::SIMPLIFIED_CHINESE)
            .await,
        Err(Error::Timeout(TimeoutStage::Response))
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
    let snapshot = handle.snapshot();
    assert_eq!(snapshot.language, DisplayLanguage::SIMPLIFIED_CHINESE);
    assert_eq!(snapshot.connection, ConnectionState::Failed);
    assert_eq!(snapshot.equalizers[0].freshness, Freshness::Stale);
    assert_eq!(snapshot.equalizers[0].parameters, parameters);
    assert!(
        trace
            .lock()
            .unwrap()
            .iter()
            .all(|request| request[0] != 0x58)
    );
}

#[tokio::test(start_paused = true)]
async fn optional_band_metadata_preserves_order_and_unknown_labels_then_invalidates_on_capability_change()
 {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x55], false);
    script.extend([
        Exchange::new(&[0x50, 0x31, 1], &[0x51, 0x31, 3, 13]),
        Exchange::new(
            &[0x5a, 0x31],
            &[0x5b, 0x31, 3, 1, 0, 31, 0x10, 0, 1, 0xfe, 0x12, 0x34],
        ),
    ]);
    script.extend(eq_reads(0x31, &[2, 13], &[2, 6, 7]));
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    handle
        .refresh_equalizer_band_info(EqualizerKind::Custom)
        .await
        .unwrap();
    let state = handle.snapshot();
    let info = state.equalizers[0].band_info.as_ref().unwrap();
    assert_eq!(info.bands[0].frequency_hz(), Some(31));
    assert!(info.bands[1].is_clear_bass());
    assert_eq!(info.bands[2].information_type.as_raw(), 0xfe);
    assert_eq!(info.bands[2].value, 0x1234);
    assert!(
        state.equalizers[0].parameters.is_none(),
        "optional metadata request does not imply parameter sync"
    );
    handle
        .refresh_equalizer(EqualizerKind::Custom)
        .await
        .unwrap();
    assert!(handle.snapshot().equalizers[0].band_info.is_none());
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn mismatched_band_metadata_does_not_label_a_curve_or_close_the_session() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x55], false);
    script.extend([
        Exchange::new(&[0x50, 0x31, 1], &[0x51, 0x31, 3, 13]),
        Exchange::new(&[0x5a, 0x31], &[0x5b, 0x31, 1, 1, 0, 31]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert!(matches!(
        handle
            .refresh_equalizer_band_info(EqualizerKind::Custom)
            .await,
        Err(Error::Protocol(_))
    ));
    assert!(handle.snapshot().equalizers[0].band_info.is_none());
    assert_eq!(handle.snapshot().connection, ConnectionState::Ready);
    handle.refresh_battery().await.unwrap();
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn preset_intent_preserves_fresh_ult_without_resending_a_stale_curve() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x53], false);
    let capability = [2, 13, 4, 2, 0, 0, 0xa0, 0];
    script.extend(eq_reads(3, &capability, &[0, 0, 2, 6, 6]));
    script.extend(eq_reads(3, &capability, &[0, 2, 2, 9, 8]));
    script.extend([
        Exchange::ack(&[0x58, 3, 0xa0, 2, 0]),
        Exchange::new(&[0x56, 3], &[0x57, 3, 0xa0, 2, 2, 9, 8]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    handle
        .refresh_equalizer(EqualizerKind::PresetWithUlt)
        .await
        .unwrap();
    assert_eq!(
        handle
            .set_equalizer_preset(EqualizerKind::PresetWithUlt, PresetId::CUSTOM)
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    assert_eq!(
        handle.snapshot().equalizers[0]
            .parameters
            .as_ref()
            .unwrap()
            .ult_mode(),
        Some(ezsc_core::equalizer::UltMode::Ult2)
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn one_band_intent_merges_fresh_other_bands_and_ult() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x53], false);
    let capability = [3, 13, 4, 1, 0xa0, 0];
    script.extend(eq_reads(3, &capability, &[0xa0, 0, 3, 6, 6, 6]));
    script.extend(eq_reads(3, &capability, &[0xa0, 1, 3, 3, 4, 5]));
    script.extend([
        Exchange::ack(&[0x58, 3, 0xa0, 1, 3, 3, 7, 5]),
        Exchange::new(&[0x56, 3], &[0x57, 3, 0xa0, 1, 3, 3, 7, 5]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    handle
        .refresh_equalizer(EqualizerKind::PresetWithUlt)
        .await
        .unwrap();
    assert_eq!(
        handle
            .set_equalizer_band(EqualizerKind::PresetWithUlt, Some(PresetId::CUSTOM), 1, 7)
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn changed_active_preset_prevents_band_intent_from_modifying_another_slot() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x57], false);
    script.extend(eq_reads(4, &[2, 13, 2, 0xa0, 0, 0xa1, 0], &[0xa1, 2, 6, 6]));
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert!(matches!(
        handle
            .set_equalizer_band(
                EqualizerKind::PresetWithErrorCodes,
                Some(PresetId::CUSTOM),
                0,
                7
            )
            .await,
        Err(Error::Unavailable(_))
    ));
    assert!(
        trace
            .lock()
            .unwrap()
            .iter()
            .all(|payload| payload[0] != 0x58)
    );
    assert_eq!(handle.snapshot().connection, ConnectionState::Ready);
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn standalone_custom_band_intent_uses_no_preset_or_ult_fields() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x55], false);
    script.extend(eq_reads(0x31, &[2, 13], &[2, 4, 9]));
    script.extend([
        Exchange::ack(&[0x58, 0x31, 2, 4, 7]),
        Exchange::new(&[0x56, 0x31], &[0x57, 0x31, 2, 4, 7]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle
            .set_equalizer_band(EqualizerKind::Custom, None, 1, 7)
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn adaptation_edits_preserve_fresh_effect_mode_level_and_unedited_adaptation_fields() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x6d], false);
    for (parameters, setting) in [
        (vec![1, 0, 1, 1, 17, 0, 2], vec![1, 0, 1, 1, 17, 1, 2]),
        (vec![1, 1, 0, 0, 19, 0, 0], vec![1, 1, 0, 0, 19, 0, 1]),
    ] {
        let parameters = [&[0x67, 0x19][..], &parameters].concat();
        let write = [&[0x68, 0x19][..], &setting].concat();
        let readback = [&[0x67, 0x19][..], &setting].concat();
        script.extend([
            Exchange::new(&[0x60, 0x19], &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1]),
            Exchange::new(&[0x62, 0x19], &[0x63, 0x19, 0]),
            Exchange::new(&[0x66, 0x19], &parameters),
            Exchange::ack(&write),
            Exchange::new(&[0x66, 0x19], &readback),
        ]);
    }
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle
            .set_noise(
                NoiseKind::AdaptiveNcAndAmbientLevel,
                NoiseChange::Adaptive(true)
            )
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    assert_eq!(
        handle
            .set_noise(
                NoiseKind::AdaptiveNcAndAmbientLevel,
                NoiseChange::AdaptiveSensitivity(ezsc_core::noise::AdaptiveSensitivity::High)
            )
            .await
            .unwrap(),
        OperationResult::Confirmed
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn message_counters_separate_responses_and_notifications_before_after_and_without_gets() {
    let backend = Arc::new(TestBackend::default());
    let driver = backend.insert("a");
    let incoming = driver.incoming.clone();
    let mut script = handshake(&[0x61, 0x50], false);
    script.extend([
        Exchange::new(&[0x62, 1], &[0x63, 1, 0]).notice(&[0x65, 1, 0]),
        Exchange::new(&[0x66, 1], &[0x67, 1, 1, 1, 1])
            .notice(&[0x69, 1, 1, 0, 1])
            .notification_after_response(&[0x69, 1, 1, 1, 1]),
        Exchange::new(&[0x50, 0, 1], &[0x51, 0, 2, 13, 1, 0xa0, 0]),
        Exchange::new(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::new(&[0x56, 0], &[0x57, 0, 0xa0, 2, 6, 6]).notice(&[0x59, 0, 0xa0, 2, 5, 7]),
    ]);
    let (peer, _) = peer(driver, script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle.snapshot().message_counters,
        ezsc_core::MessageCounters {
            responses: 6,
            ..Default::default()
        }
    );
    handle.refresh_noise(NoiseKind::NcOnly).await.unwrap();
    assert_eq!(
        handle.snapshot().message_counters,
        ezsc_core::MessageCounters {
            responses: 8,
            notifications: 3,
            noise_parameters_notifications: 2,
            equalizer_parameters_notifications: 0,
            ..Default::default()
        }
    );
    handle
        .refresh_equalizer(EqualizerKind::Preset)
        .await
        .unwrap();
    assert_eq!(
        handle.snapshot().message_counters,
        ezsc_core::MessageCounters {
            responses: 11,
            notifications: 4,
            noise_parameters_notifications: 2,
            equalizer_parameters_notifications: 1,
            ..Default::default()
        }
    );

    // Fifteen delivered peer frames above leave the next reliable sequence at
    // one. Deliver an independent notification, its retransmission, an invalid
    // notification and an unmatched response without issuing another GET.
    let notification = Frame {
        data_type: DataType::DATA_MDR,
        sequence: Sequence::One,
        payload: vec![0x59, 0, 0xa0, 2, 7, 7],
    }
    .encode(16384)
    .unwrap();
    incoming.send(notification.clone()).await.unwrap();
    incoming.send(notification).await.unwrap();
    incoming
        .send(
            Frame {
                data_type: DataType::DATA_MDR,
                sequence: Sequence::Zero,
                payload: vec![0x59, 0, 0xa0, 2],
            }
            .encode(16384)
            .unwrap(),
        )
        .await
        .unwrap();
    incoming
        .send(
            Frame {
                data_type: DataType::DATA_MDR,
                sequence: Sequence::One,
                payload: vec![0x57, 0, 0xa0, 2, 6, 6],
            }
            .encode(16384)
            .unwrap(),
        )
        .await
        .unwrap();
    let mut updates = handle.subscribe();
    while handle.snapshot().unhandled_messages < 2 {
        updates.changed().await.unwrap();
    }
    assert_eq!(
        handle.snapshot().message_counters,
        ezsc_core::MessageCounters {
            responses: 11,
            notifications: 5,
            noise_parameters_notifications: 2,
            equalizer_parameters_notifications: 2,
            ..Default::default()
        }
    );
    drop(incoming);
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn codec_requires_its_table_aware_advertisement() {
    for table_two_codec in [false, true] {
        let backend = Arc::new(TestBackend::default());
        let mut script = handshake(&[0x11, 0x13, 0x18], table_two_codec);
        if table_two_codec {
            script[3].response = Some(vec![7, 0, 1, 0x12, 0]);
        }
        // Even a valid unsolicited codec report cannot create an unadvertised feature.
        script.last_mut().unwrap().notices.push(vec![0x15, 2, 0x10]);
        let (peer, trace) = peer(backend.insert("a"), script);
        let manager = DeviceManager::new(backend);
        let handle = connect(&manager, "a").await;
        handle.initialize().await.unwrap();
        assert!(handle.snapshot().audio_codec.is_none());
        assert!(
            handle
                .snapshot()
                .functions
                .iter()
                .all(|f| !f.readable && !f.writable)
        );
        assert_eq!(
            handle.refresh_audio_codec().await,
            Err(Error::Unsupported("audio codec report not advertised"))
        );
        assert!(!trace.lock().unwrap().iter().any(|p| p == &[0x12, 2]));
        handle.close().await.unwrap();
        peer.await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn codec_initial_read_refresh_and_live_notifications_preserve_identity() {
    let backend = Arc::new(TestBackend::default());
    let driver = backend.insert("a");
    let incoming = driver.incoming.clone();
    // Duplicate advertisements still produce only one initial GET.
    let mut script = handshake(&[0x12, 0x12], false);
    script.extend([
        Exchange::new(&[0x12, 2], &[0x13, 2, 0x10]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 0]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 1]).notification_after_response(&[0x15, 2, 0xa7]),
    ]);
    let (peer, trace) = peer(driver, script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let state = handle.snapshot();
    assert!(state.functions.iter().all(|f| f.readable && !f.writable));
    assert_eq!(
        state.audio_codec.as_ref().unwrap().codec,
        Some(AudioCodec::LDAC)
    );
    assert_eq!(state.audio_codec.unwrap().freshness, Freshness::Current);
    assert_eq!(trace.lock().unwrap().len(), 7);
    assert_eq!(
        handle.refresh_audio_codec().await.unwrap(),
        OperationResult::Synchronized
    );
    assert_eq!(
        handle.snapshot().audio_codec.unwrap().codec,
        Some(AudioCodec::UNSETTLED)
    );
    handle.refresh_audio_codec().await.unwrap();
    assert_eq!(
        handle
            .snapshot()
            .audio_codec
            .unwrap()
            .codec
            .unwrap()
            .as_raw(),
        0xa7
    );

    // Ten responses/notifications above leave the peer sequence at zero. A
    // notification outside a query must publish the new codec without a GET.
    let mut updates = handle.subscribe();
    incoming
        .send(
            Frame {
                data_type: DataType::DATA_MDR,
                sequence: Sequence::Zero,
                payload: vec![0x15, 2, 2],
            }
            .encode(16384)
            .unwrap(),
        )
        .await
        .unwrap();
    while updates
        .borrow_and_update()
        .audio_codec
        .as_ref()
        .unwrap()
        .codec
        != Some(AudioCodec::AAC)
    {
        updates.changed().await.unwrap();
    }
    assert_eq!(handle.snapshot().message_counters.notifications, 2);
    assert_eq!(handle.snapshot().unhandled_messages, 0);
    assert_eq!(trace.lock().unwrap().len(), 9);
    drop(incoming);
    handle.close().await.unwrap();
    assert_eq!(
        handle.snapshot().audio_codec.unwrap().freshness,
        Freshness::Stale
    );
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn codec_notification_and_ack_cannot_satisfy_a_refresh() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x12], false);
    script.extend([
        Exchange::new(&[0x12, 2], &[0x13, 2, 0x10]),
        Exchange::ack(&[0x12, 2]).notice(&[0x15, 2, 2]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    assert_eq!(
        handle.refresh_audio_codec().await,
        Err(Error::Timeout(TimeoutStage::Response))
    );
    handle.close().await.unwrap();
    let state = handle.snapshot();
    assert_eq!(state.connection, ConnectionState::Failed);
    assert_eq!(
        state.audio_codec.as_ref().unwrap().codec,
        Some(AudioCodec::AAC)
    );
    assert_eq!(state.audio_codec.unwrap().freshness, Freshness::Stale);
    assert_eq!(trace.lock().unwrap().len(), 8);
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn malformed_codec_reads_are_isolated_and_can_be_refreshed() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x12, 0x20], false);
    script.extend([
        Exchange::new(&[0x22, 0], &[0x23, 0, 80, 0]),
        Exchange::new(&[0x12, 2], &[0x13, 2]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 1]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 1, 0]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 0xff]),
    ]);
    let (peer, _) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let state = handle.snapshot();
    assert_eq!(state.connection, ConnectionState::Ready);
    assert!(matches!(state.last_error, Some(Error::Protocol(_))));
    assert_eq!(state.audio_codec.as_ref().unwrap().codec, None);
    assert_eq!(state.audio_codec.unwrap().freshness, Freshness::Unknown);
    assert_eq!(state.batteries[0].reading.unwrap().level.as_u8(), 80);
    handle.refresh_audio_codec().await.unwrap();
    assert_eq!(
        handle.snapshot().audio_codec.unwrap().codec,
        Some(AudioCodec::SBC)
    );
    assert!(matches!(
        handle.refresh_audio_codec().await,
        Err(Error::Protocol(_))
    ));
    assert_eq!(handle.snapshot().connection, ConnectionState::Ready);
    handle.refresh_audio_codec().await.unwrap();
    assert_eq!(
        handle.snapshot().audio_codec.unwrap().codec,
        Some(AudioCodec::OTHER)
    );
    handle.close().await.unwrap();
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn codec_reconnect_does_not_reuse_the_previous_generation() {
    let backend = Arc::new(TestBackend::default());
    let mut first = handshake(&[0x12], false);
    first.push(Exchange::new(&[0x12, 2], &[0x13, 2, 0x10]));
    let mut second = handshake(&[0x12], false);
    second.push(Exchange::new(&[0x12, 2], &[0x13, 2, 1]));
    let (first_peer, _) = peer(backend.insert("a"), first);
    let (second_peer, _) = peer(backend.insert("a"), second);
    let manager = DeviceManager::new(backend);
    let old = connect(&manager, "a").await;
    old.initialize().await.unwrap();
    old.close().await.unwrap();
    first_peer.await.unwrap();
    let new = connect(&manager, "a").await;
    assert_ne!(old.snapshot().generation, new.snapshot().generation);
    assert!(new.snapshot().audio_codec.is_none());
    new.initialize().await.unwrap();
    assert_eq!(
        new.snapshot().audio_codec.unwrap().codec,
        Some(AudioCodec::SBC)
    );
    let old_state = old.snapshot().audio_codec.unwrap();
    assert_eq!(old_state.codec, Some(AudioCodec::LDAC));
    assert_eq!(old_state.freshness, Freshness::Stale);
    new.close().await.unwrap();
    second_peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn independent_prompt_dismissal_refreshes_advertised_codec() {
    let backend = Arc::new(TestBackend::default());
    let mut script = handshake(&[0x12, 0x90], false);
    script.extend([
        Exchange::new(&[0x12, 2], &[0x13, 2, 1]),
        Exchange::ack(&[0x94, 0, 0]).notice(&[0x99, 0, 0xfe, 1]),
        Exchange::new(&[0x12, 2], &[0x13, 2, 0x10]),
    ]);
    let (peer, trace) = peer(backend.insert("a"), script);
    let manager = DeviceManager::new(backend);
    let handle = connect(&manager, "a").await;
    handle.initialize().await.unwrap();
    let prompt = handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        handle
            .respond_prompt(prompt.id, ezsc_core::PromptReply::Dismiss)
            .await
            .unwrap(),
        OperationResult::Dismissed
    );
    assert_eq!(
        handle.snapshot().audio_codec.unwrap().codec,
        Some(AudioCodec::LDAC)
    );
    assert!(!trace.lock().unwrap().iter().any(|p| p[0] == 0x98));
    assert_eq!(trace.lock().unwrap().len(), 9);
    handle.close().await.unwrap();
    peer.await.unwrap();
}
