use bluetooth::{
    Backend, CancellationToken, ConnectionInfo, Device, DeviceId, Error, ErrorKind, TransportKind,
    driver_channel,
};

fn info() -> ConnectionInfo {
    ConnectionInfo {
        device: DeviceId("test/one".into()),
        transport: TransportKind::Gatt,
        max_write_size: Some(3),
    }
}

#[tokio::test]
async fn read_progresses_while_a_full_frame_write_is_unfinished() {
    let (mut connection, mut driver) = driver_channel(info(), 2).unwrap();
    let writer = connection.writer();
    let write = tokio::spawn(async move { writer.write(vec![1, 2, 3, 4, 5, 6, 7]).await });
    let request = driver.writes.recv().await.unwrap();
    assert_eq!(request.bytes, [1, 2, 3, 4, 5, 6, 7]);
    assert!(!write.is_finished());
    driver.incoming.send(vec![8, 9]).await.unwrap();
    driver.incoming.send(vec![10]).await.unwrap();
    assert_eq!(connection.recv().await.unwrap().unwrap(), [8, 9]);
    assert_eq!(connection.recv().await.unwrap().unwrap(), [10]);
    assert!(!write.is_finished());
    request.completion.send(Ok(())).unwrap();
    write.await.unwrap().unwrap();
    driver.finish(Ok(()));
    assert!(connection.recv().await.is_none());
    connection.close().await.unwrap();
}

#[tokio::test]
async fn dropping_an_enqueued_write_cancels_the_link() {
    let (mut connection, mut driver) = driver_channel(info(), 1).unwrap();
    let writer = connection.writer();
    let write = tokio::spawn(async move { writer.write(vec![1, 2, 3]).await });
    let request = driver.writes.recv().await.unwrap();
    write.abort();
    let _ = write.await;
    assert!(driver.cancel.is_cancelled());
    let error = connection.writer().write(vec![4]).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Cancelled);
    drop(request);
    driver.finish(Ok(()));
    connection.close().await.unwrap();
}

#[tokio::test]
async fn partial_write_failure_is_terminal_and_preserves_diagnostic() {
    let (mut connection, mut driver) = driver_channel(info(), 1).unwrap();
    let writer = connection.writer();
    let write = tokio::spawn(async move { writer.write(vec![1, 2, 3, 4]).await });
    let request = driver.writes.recv().await.unwrap();
    let failure = Error::new(ErrorKind::Io, "native write failed after first three bytes");
    request.completion.send(Err(failure.clone())).unwrap();
    assert_eq!(write.await.unwrap().unwrap_err(), failure);
    assert!(driver.cancel.is_cancelled());
    driver.finish(Err(failure.clone()));
    assert_eq!(connection.recv().await.unwrap().unwrap_err(), failure);
    assert!(connection.recv().await.is_none());
}

#[tokio::test]
async fn closing_waits_for_driver_cleanup_and_drop_cancels_even_with_writer_clones() {
    let (mut connection, driver) = driver_channel(info(), 1).unwrap();
    let retained_writer = connection.writer();
    let cancel = driver.cancel.clone();
    let closing = tokio::spawn(async move { connection.close().await });
    cancel.cancelled().await;
    assert!(!closing.is_finished());
    driver.finish(Ok(()));
    closing.await.unwrap().unwrap();
    assert_eq!(
        retained_writer.write(vec![1]).await.unwrap_err().kind,
        ErrorKind::Cancelled
    );

    let (connection, driver) = driver_channel(info(), 1).unwrap();
    let retained_writer = connection.writer();
    drop(connection);
    assert!(driver.cancel.is_cancelled());
    assert_eq!(
        retained_writer.write(vec![1]).await.unwrap_err().kind,
        ErrorKind::Cancelled
    );
}

#[tokio::test]
async fn bounded_receive_has_backpressure_without_dropping_fragments() {
    let (mut connection, driver) = driver_channel(info(), 1).unwrap();
    driver.incoming.send(vec![1]).await.unwrap();
    assert!(matches!(
        driver.incoming.try_send(vec![2]),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_))
    ));
    let sender = driver.incoming.clone();
    let pending = tokio::spawn(async move { sender.send(vec![2]).await });
    assert_eq!(connection.recv().await.unwrap().unwrap(), [1]);
    pending.await.unwrap().unwrap();
    assert_eq!(connection.recv().await.unwrap().unwrap(), [2]);
    driver.finish(Ok(()));
}

#[tokio::test]
async fn reconnect_owns_a_distinct_channel_even_when_old_callback_finishes() {
    let (old, mut old_driver) = driver_channel(info(), 1).unwrap();
    let writer = old.writer();
    let old_write = tokio::spawn(async move { writer.write(vec![1]).await });
    let old_request = old_driver.writes.recv().await.unwrap();
    drop(old);
    let (mut current, current_driver) = driver_channel(info(), 1).unwrap();
    current_driver.incoming.send(vec![2]).await.unwrap();
    let _ = old_request.completion.send(Ok(()));
    old_driver.finish(Ok(()));
    let _ = old_write.await;
    assert!(!current_driver.cancel.is_cancelled());
    assert_eq!(current.recv().await.unwrap().unwrap(), [2]);
    current_driver.finish(Ok(()));
}

#[tokio::test]
async fn abandoned_driver_unblocks_waiters_with_an_error() {
    let (mut connection, mut driver) = driver_channel(info(), 1).unwrap();
    let writer = connection.writer();
    let write = tokio::spawn(async move { writer.write(vec![1]).await });
    let request = driver.writes.recv().await.unwrap();
    drop(request);
    drop(driver);
    assert!(write.await.unwrap().is_err());
    assert_eq!(
        connection.recv().await.unwrap().unwrap_err().kind,
        ErrorKind::Disconnected
    );
    assert!(connection.close().await.is_err());
}

#[tokio::test]
async fn fake_backend_opens_only_advertised_transport_and_propagates_cancellation() {
    let device = Device {
        id: DeviceId("fake/headset".into()),
        name: None,
        transports: vec![TransportKind::Gatt],
    };
    let (backend, mut openings) =
        bluetooth::fake::FakeBackend::new(vec![device.clone()], 1, 20).unwrap();
    assert_eq!(
        backend
            .discover(CancellationToken::new())
            .await
            .unwrap()
            .as_slice(),
        std::slice::from_ref(&device)
    );
    let unsupported = backend
        .connect(
            device.id.clone(),
            TransportKind::Rfcomm,
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        unsupported,
        Err(Error {
            kind: ErrorKind::Unsupported,
            ..
        })
    ));
    assert!(openings.try_recv().is_err());
    let cancel = CancellationToken::new();
    let mut connection = backend
        .connect(device.id, TransportKind::Gatt, cancel.clone())
        .await
        .unwrap();
    let driver = openings.recv().await.unwrap();
    assert_eq!(connection.info().max_write_size, Some(20));
    cancel.cancel();
    driver.cancel.cancelled().await;
    driver.finish(Ok(()));
    connection.close().await.unwrap();
}
