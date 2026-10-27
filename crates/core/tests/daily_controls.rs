//! Synthetic protocol peers for daily controls. These are independently
//! constructed wire cases, not device captures or hardware compatibility tests.

use bluetooth::{Device, DeviceId, Driver, TransportKind, fake::FakeBackend};
use ezsc_core::{
    AudioChange, ConnectionState, DeviceHandle, DeviceManager, Error, Freshness, OperationResult,
    SessionConfig, SystemChange, TimeoutStage,
    audio::{AudioKind, AudioParameterValue, AudioStatusDetails, DseeMode, PriorityMode},
    system::{DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemKind},
};
use libmdr2::frame::{DataType, Decoder, Frame, Sequence};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

struct Exchange {
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    before: Vec<Vec<u8>>,
    after: Vec<Vec<u8>>,
    disconnect: bool,
}

impl Exchange {
    fn read(request: &[u8], response: &[u8]) -> Self {
        Self {
            request: request.into(),
            response: Some(response.into()),
            before: Vec::new(),
            after: Vec::new(),
            disconnect: false,
        }
    }

    fn ack(request: &[u8]) -> Self {
        Self {
            response: None,
            ..Self::read(request, &[])
        }
    }

    fn notice(mut self, payload: &[u8]) -> Self {
        self.before.push(payload.into());
        self
    }

    fn after(mut self, payload: &[u8]) -> Self {
        self.after.push(payload.into());
        self
    }

    fn disconnect(mut self) -> Self {
        self.disconnect = true;
        self
    }
}

type Trace = Arc<Mutex<Vec<Vec<u8>>>>;

struct Harness {
    _manager: DeviceManager,
    handle: DeviceHandle,
    peer: tokio::task::JoinHandle<()>,
    trace: Trace,
}

impl Harness {
    async fn start(functions: &[u8], operations: Vec<Exchange>) -> Self {
        let id = DeviceId("daily-fixture".into());
        let (backend, mut openings) = FakeBackend::new(
            vec![Device {
                id: id.clone(),
                name: None,
                transports: vec![TransportKind::Gatt],
            }],
            64,
            7,
        )
        .unwrap();
        let manager = DeviceManager::new(Arc::new(backend));
        let handle = manager
            .connect(id, TransportKind::Gatt, SessionConfig::default())
            .await
            .unwrap();
        let driver = openings.recv().await.unwrap();
        let mut function_response = vec![7, 0, functions.len().try_into().unwrap()];
        for function in functions {
            function_response.extend([*function, 0]);
        }
        let mut script = vec![
            Exchange::read(&[0, 0], &[1, 0, 3, 0, 0x30, 0x32, 0, 1]),
            Exchange::read(&[2, 0], &[3, 0, 9, 0]),
            Exchange::read(&[6, 0], &function_response),
            Exchange::read(&[4, 1], b"\x05\x01\x07Fixture"),
            Exchange::read(&[4, 2], b"\x05\x02\x031.0"),
            Exchange::read(&[4, 3], &[5, 3, 0x01, 0x00]),
        ];
        script.extend(operations);
        let trace = Arc::new(Mutex::new(Vec::new()));
        let peer = tokio::spawn(serve(driver, script, trace.clone()));
        assert_eq!(
            handle.initialize().await.unwrap(),
            OperationResult::Synchronized
        );
        Self {
            _manager: manager,
            handle,
            peer,
            trace,
        }
    }

    async fn finish(self) {
        let _ = self.handle.close().await;
        self.peer.await.unwrap();
    }

    fn writes(&self, opcode: u8) -> usize {
        self.trace
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p[0] == opcode)
            .count()
    }
}

async fn deliver(driver: &Driver, frame: Frame) {
    for chunk in frame.encode(16384).unwrap().chunks(3) {
        driver.incoming.send(chunk.to_vec()).await.unwrap();
    }
}

async fn serve(mut driver: Driver, script: Vec<Exchange>, trace: Trace) {
    let mut steps: VecDeque<_> = script.into();
    let mut decoder = Decoder::new(16384).unwrap();
    let mut sequence = Sequence::Zero;
    loop {
        let write = tokio::select! {
            biased;
            _ = driver.cancel.cancelled() => break,
            write = driver.writes.recv() => match write { Some(write) => write, None => break },
        };
        let frames: Vec<_> = decoder
            .feed(&write.bytes)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        let _ = write.completion.send(Ok(()));
        if frame.data_type == DataType::ACK {
            continue;
        }
        assert_eq!(frame.data_type, DataType::DATA_MDR);
        trace.lock().unwrap().push(frame.payload.clone());
        let step = steps
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected write: {:02x?}", frame.payload));
        assert_eq!(frame.payload, step.request);
        for payload in step
            .before
            .into_iter()
            .chain(step.response)
            .chain(step.after)
        {
            deliver(
                &driver,
                Frame {
                    data_type: DataType::DATA_MDR,
                    sequence,
                    payload,
                },
            )
            .await;
            sequence = sequence.toggled();
        }
        // ACK last ensures trailing notifications are already queued when an
        // operation becomes complete; confirmation must retain its matched RET.
        deliver(&driver, Frame::acknowledgement(frame.sequence)).await;
        if step.disconnect {
            assert!(steps.is_empty());
            driver.finish(Err(bluetooth::Error::new(
                bluetooth::ErrorKind::Disconnected,
                "synthetic priority transition",
            )));
            return;
        }
    }
    assert!(
        steps.is_empty(),
        "{} expected requests were never sent",
        steps.len()
    );
    driver.finish(Ok(()));
}

fn audio_reads(inquiry: u8, capability: Option<&[u8]>, status: &[u8], value: u8) -> Vec<Exchange> {
    let mut steps = Vec::new();
    if let Some(body) = capability {
        steps.push(Exchange::read(
            &[0xe0, inquiry],
            &[&[0xe1, inquiry][..], body].concat(),
        ));
    }
    steps.extend([
        Exchange::read(&[0xe2, inquiry], &[&[0xe3, inquiry][..], status].concat()),
        Exchange::read(&[0xe6, inquiry], &[0xe7, inquiry, value]),
    ]);
    steps
}

fn system_reads(
    inquiry: u8,
    preview_supported: bool,
    body: &[u8],
    extended: bool,
) -> Vec<Exchange> {
    vec![
        // Deliberately nonstandard timing values ensure setters use selectors,
        // without substituting durations or inventing a positive lower bound.
        Exchange::read(
            &[0xf0, inquiry],
            &[0xf1, inquiry, u8::from(preview_supported), 0, 29, 7],
        ),
        Exchange::read(&[0xf2, inquiry], &[0xf3, inquiry, 0, 1]),
        Exchange::read(
            &[if extended { 0xfa } else { 0xf6 }, inquiry],
            &[&[if extended { 0xfb } else { 0xf7 }, inquiry][..], body].concat(),
        ),
    ]
}

#[tokio::test(start_paused = true)]
async fn all_five_audio_layouts_use_exact_queries_and_write_shapes() {
    let cases = [
        (
            AudioKind::ConnectionPriority,
            None,
            vec![0],
            0,
            AudioChange::Priority(PriorityMode::CONNECTION_QUALITY),
            vec![0xe8, 0, 1],
            1,
        ),
        (
            AudioKind::ConnectionPriorityWithStatus,
            None,
            vec![0, 1],
            1,
            AudioChange::Priority(PriorityMode::SOUND_QUALITY),
            vec![0xe8, 2, 0],
            0,
        ),
        (
            AudioKind::ClassicLePriority,
            Some(vec![3, 0, 1, 2, 1, 0]),
            vec![0, 0],
            0,
            AudioChange::Priority(PriorityMode::LOW_LATENCY),
            vec![0xe8, 5, 2, 0],
            2,
        ),
        (
            AudioKind::Dsee,
            Some(vec![3]),
            vec![0],
            0,
            AudioChange::Dsee(DseeMode::Auto),
            vec![0xe8, 1, 1],
            1,
        ),
        (
            AudioKind::DseeWithReasons,
            Some(vec![2]),
            vec![0, 0],
            1,
            AudioChange::Dsee(DseeMode::Off),
            vec![0xe8, 11, 0],
            0,
        ),
    ];
    let mut script = Vec::new();
    for (kind, cap, status, before, _, setting, after) in &cases {
        script.extend(audio_reads(kind.inquiry(), cap.as_deref(), status, *before));
        let mut write = Exchange::ack(setting);
        if *kind == AudioKind::ClassicLePriority {
            write = write.notice(&[0xe9, 5, 2, 1]);
        }
        script.extend([
            write,
            Exchange::read(&[0xe6, kind.inquiry()], &[0xe7, kind.inquiry(), *after]),
        ]);
    }
    let fixture = Harness::start(&[0xe1, 0xe3, 0xe7, 0xe2, 0xed], script).await;
    assert_eq!(fixture.handle.snapshot().audio.len(), 5);
    for (kind, _, _, _, change, _, _) in cases {
        assert_eq!(
            fixture.handle.set_audio(kind, change).await.unwrap(),
            OperationResult::Confirmed
        );
        let snapshot = fixture.handle.snapshot();
        let entry = snapshot
            .audio
            .iter()
            .find(|entry| entry.kind == kind)
            .unwrap();
        assert_eq!(entry.freshness, Freshness::Current);
        if kind == AudioKind::ConnectionPriorityWithStatus {
            assert_eq!(
                entry.status.as_ref().unwrap().details,
                AudioStatusDetails::Connection {
                    secondary_enabled: false
                }
            );
        }
    }
    assert_eq!(
        fixture.writes(0xe0),
        3,
        "fixed priorities must not send capability queries"
    );
    assert_eq!(fixture.writes(0xe8), 5);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn unknown_or_unadvertised_priority_is_never_sent() {
    let mut script = Vec::new();
    for _ in 0..2 {
        script.extend(audio_reads(5, Some(&[3, 0, 1, 0x80, 0]), &[0, 0], 0x80));
    }
    let fixture = Harness::start(&[0xe7], script).await;
    for priority in [PriorityMode::from_raw(0x80), PriorityMode::LOW_LATENCY] {
        assert!(
            fixture
                .handle
                .set_audio(
                    AudioKind::ClassicLePriority,
                    AudioChange::Priority(priority)
                )
                .await
                .is_err()
        );
        assert_eq!(fixture.handle.snapshot().connection, ConnectionState::Ready);
    }
    assert!(
        matches!(fixture.handle.snapshot().audio[0].parameters.as_ref().unwrap().value(), AudioParameterValue::ConnectionPriority { priority, .. } if priority.as_raw() == 0x80)
    );
    assert_eq!(fixture.writes(0xe8), 0);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn latest_audio_availability_notification_prevents_setting() {
    let mut script = audio_reads(11, Some(&[0x7f]), &[0, 0], 0);
    script
        .last_mut()
        .unwrap()
        .after
        .push(vec![0xe5, 11, 1, 1, 0xee]);
    let fixture = Harness::start(&[0xed], script).await;
    assert!(matches!(
        fixture
            .handle
            .set_audio(
                AudioKind::DseeWithReasons,
                AudioChange::Dsee(DseeMode::Auto)
            )
            .await,
        Err(Error::Unavailable(_))
    ));
    let snapshot = fixture.handle.snapshot();
    let status = snapshot.audio[0].status.as_ref().unwrap();
    assert!(!status.enabled);
    assert!(
        matches!(&status.details, AudioStatusDetails::Reasons(reasons) if reasons[0].as_raw() == 0xee)
    );
    assert_eq!(fixture.writes(0xe8), 0);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn audio_confirmation_uses_matched_readback_not_a_later_matching_notification() {
    let mut script = audio_reads(5, Some(&[2, 0, 1, 0]), &[0, 0], 1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0xe9, 5, 0, 2]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]).after(&[0xe9, 5, 0, 0]),
    ]);
    let fixture = Harness::start(&[0xe7], script).await;
    assert_eq!(
        fixture
            .handle
            .set_audio(
                AudioKind::ClassicLePriority,
                AudioChange::Priority(PriorityMode::SOUND_QUALITY)
            )
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    assert!(matches!(
        fixture.handle.snapshot().audio[0]
            .parameters
            .as_ref()
            .unwrap()
            .value(),
        AudioParameterValue::ConnectionPriority {
            priority: PriorityMode::SOUND_QUALITY,
            ..
        }
    ));
    assert_eq!(
        fixture
            .handle
            .snapshot()
            .message_counters
            .audio_parameters_notifications,
        2
    );
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn audio_notification_without_get_response_does_not_confirm_setting() {
    let mut script = audio_reads(1, Some(&[1]), &[0], 0);
    script.extend([
        Exchange::ack(&[0xe8, 1, 1]).notice(&[0xe9, 1, 1]),
        Exchange::ack(&[0xe6, 1]).notice(&[0xe9, 1, 1]),
    ]);
    let fixture = Harness::start(&[0xe2], script).await;
    assert_eq!(
        fixture
            .handle
            .set_audio(AudioKind::Dsee, AudioChange::Dsee(DseeMode::Auto))
            .await,
        Err(Error::Timeout(TimeoutStage::Response))
    );
    assert_eq!(
        fixture.handle.snapshot().audio[0].freshness,
        Freshness::Stale
    );
    assert_eq!(fixture.writes(0xe8), 1);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn both_speak_to_chat_switches_preserve_fresh_preview_and_keep_extended_bank() {
    for (function, kind, ext) in [
        (0xf2, SystemKind::SpeakToChatType1, vec![2, 0, 3]),
        (0xfc, SystemKind::SpeakToChatType2, vec![2, 3]),
    ] {
        let inquiry = kind.inquiry();
        let mut script = system_reads(inquiry, true, &[1, 1], false);
        script.push(Exchange::read(
            &[0xfa, inquiry],
            &[&[0xfb, inquiry][..], &ext].concat(),
        ));
        // Preview changed externally since the old GUI snapshot was captured.
        script.extend(system_reads(inquiry, true, &[1, 0], false));
        script.extend([
            Exchange::ack(&[0xf8, inquiry, 0, 0]),
            Exchange::read(&[0xf6, inquiry], &[0xf7, inquiry, 0, 0]),
        ]);
        let fixture = Harness::start(&[function], script).await;
        fixture.handle.refresh_system(kind).await.unwrap();
        let old_extended = fixture.handle.snapshot().system[0].extended;
        assert_eq!(
            fixture
                .handle
                .set_system(kind, SystemChange::Enabled(true))
                .await
                .unwrap(),
            OperationResult::Confirmed
        );
        let snapshot = fixture.handle.snapshot();
        let state = &snapshot.system[0];
        assert_eq!(state.parameters.unwrap().preview(), Some(true));
        assert_eq!(state.extended, old_extended);
        assert_eq!(state.extended_freshness, Freshness::Current);
        assert_eq!(fixture.writes(0xfc), 0);
        fixture.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn extended_edits_preserve_other_fields_and_send_selectors_in_both_layouts() {
    let cases = [
        (
            0xf2,
            SystemKind::SpeakToChatType1,
            vec![0, 0, 3],
            SystemChange::Sensitivity(DetectionSensitivity::High),
            vec![0xfc, 2, 1, 0, 3],
        ),
        (
            0xf2,
            SystemKind::SpeakToChatType1,
            vec![2, 0, 2],
            SystemChange::VoiceFocus(false),
            vec![0xfc, 2, 2, 1, 2],
        ),
        (
            0xfc,
            SystemKind::SpeakToChatType2,
            vec![2, 3],
            SystemChange::ModeOut(ModeOutTime::Mid),
            vec![0xfc, 12, 2, 1],
        ),
    ];
    for (function, kind, before, change, setting) in cases {
        let inquiry = kind.inquiry();
        let mut script = system_reads(inquiry, true, &before, true);
        let response = [&[0xfb, inquiry][..], &setting[2..]].concat();
        script.extend([
            Exchange::ack(&setting),
            Exchange::read(&[0xfa, inquiry], &response),
        ]);
        let fixture = Harness::start(&[function], script).await;
        assert_eq!(
            fixture.handle.set_system(kind, change).await.unwrap(),
            OperationResult::Confirmed
        );
        let snapshot = fixture.handle.snapshot();
        let state = &snapshot.system[0];
        assert_eq!(state.extended_freshness, Freshness::Current);
        assert_eq!(
            state.freshness,
            Freshness::Unknown,
            "extended readback cannot mark the unread normal bank current"
        );
        assert!(state.parameters.is_none());
        assert_eq!(
            state
                .capability
                .unwrap()
                .speak_to_chat()
                .unwrap()
                .mode_out_values,
            [0, 29, 7]
        );
        assert_eq!(fixture.writes(0xf8), 0);
        fixture.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn unsupported_preview_and_type2_voice_focus_never_reset_unrelated_fields() {
    let kind = SystemKind::SpeakToChatType2;
    let mut script = system_reads(12, false, &[1, 0], false);
    script.extend(system_reads(12, true, &[0, 3], true));
    let fixture = Harness::start(&[0xfc], script).await;
    assert!(
        fixture
            .handle
            .set_system(kind, SystemChange::Enabled(true))
            .await
            .is_err(),
        "preserving unsupported preview must reject instead of silently clearing it"
    );
    assert!(matches!(
        fixture
            .handle
            .set_system(kind, SystemChange::VoiceFocus(true))
            .await,
        Err(Error::Unsupported(_))
    ));
    assert_eq!(fixture.writes(0xf8), 0);
    assert_eq!(fixture.writes(0xfc), 0);
    assert_eq!(fixture.handle.snapshot().connection, ConnectionState::Ready);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn preview_patch_preserves_enable_and_compares_the_entire_normal_bank() {
    let mut script = system_reads(12, true, &[0, 0], false);
    script.extend([
        Exchange::ack(&[0xf8, 12, 0, 1]),
        // Preview reached its requested value, but the untouched enabled flag
        // changed too. A target-field-only comparison would falsely confirm.
        Exchange::read(&[0xf6, 12], &[0xf7, 12, 1, 1]),
    ]);
    let fixture = Harness::start(&[0xfc], script).await;
    assert_eq!(
        fixture
            .handle
            .set_system(SystemKind::SpeakToChatType2, SystemChange::Preview(false))
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    let snapshot = fixture.handle.snapshot();
    assert_eq!(snapshot.system[0].freshness, Freshness::Current);
    assert_eq!(snapshot.system[0].extended_freshness, Freshness::Unknown);
    assert!(snapshot.system[0].extended.is_none());
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn latest_system_unavailability_prevents_extended_write() {
    let mut script = system_reads(2, true, &[0, 1, 2], true);
    script.last_mut().unwrap().after.push(vec![0xf5, 2, 1, 1]);
    let fixture = Harness::start(&[0xf2], script).await;
    assert!(matches!(
        fixture
            .handle
            .set_system(
                SystemKind::SpeakToChatType1,
                SystemChange::ModeOut(ModeOutTime::None)
            )
            .await,
        Err(Error::Unavailable(_))
    ));
    assert_eq!(fixture.writes(0xfc), 0);
    assert_eq!(fixture.handle.snapshot().connection, ConnectionState::Ready);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn system_notifications_project_separate_banks_but_do_not_replace_readback_evidence() {
    let mut script = system_reads(2, true, &[1, 1], false);
    script.push(Exchange::read(&[0xfa, 2], &[0xfb, 2, 0, 1, 3]));
    script.extend(system_reads(2, true, &[0, 1, 3], true));
    script.extend([
        Exchange::ack(&[0xfc, 2, 1, 1, 3])
            .notice(&[0xf9, 2, 0, 1])
            .notice(&[0xf5, 2, 0, 0]),
        // Sensitivity matches, but the unedited voice-focus flag differs. The
        // later notification must update the UI without changing this result.
        Exchange::read(&[0xfa, 2], &[0xfb, 2, 1, 0, 3]).after(&[0xfd, 2, 1, 1, 3]),
    ]);
    let fixture = Harness::start(&[0xf2], script).await;
    fixture
        .handle
        .refresh_system(SystemKind::SpeakToChatType1)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .handle
            .set_system(
                SystemKind::SpeakToChatType1,
                SystemChange::Sensitivity(DetectionSensitivity::High)
            )
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    let snapshot = fixture.handle.snapshot();
    let state = &snapshot.system[0];
    assert!(state.parameters.unwrap().enabled());
    assert_eq!(state.status.unwrap().talking, Some(false));
    assert_eq!(
        state.extended,
        Some(SpeakToChatSettings::Type1 {
            sensitivity: DetectionSensitivity::High,
            voice_focus: false,
            mode_out: ModeOutTime::None
        })
    );
    assert_eq!(snapshot.message_counters.system_parameters_notifications, 1);
    assert_eq!(snapshot.message_counters.system_extended_notifications, 1);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn connection_drop_after_priority_write_is_unconfirmed_stales_all_banks_and_never_replays() {
    let mut script = system_reads(12, true, &[1, 1], false);
    script.push(Exchange::read(&[0xfa, 12], &[0xfb, 12, 0, 2]));
    script.extend(audio_reads(5, Some(&[3, 0, 1, 2, 0]), &[0, 0], 0));
    script.push(Exchange::ack(&[0xe8, 5, 2, 0]).disconnect());
    let fixture = Harness::start(&[0xe7, 0xfc], script).await;
    fixture
        .handle
        .refresh_system(SystemKind::SpeakToChatType2)
        .await
        .unwrap();
    assert!(
        fixture
            .handle
            .set_audio(
                AudioKind::ClassicLePriority,
                AudioChange::Priority(PriorityMode::LOW_LATENCY)
            )
            .await
            .is_err()
    );
    let _ = fixture.handle.close().await;
    let snapshot = fixture.handle.snapshot();
    assert_eq!(snapshot.connection, ConnectionState::Failed);
    assert_eq!(snapshot.audio[0].freshness, Freshness::Stale);
    assert_eq!(snapshot.system[0].freshness, Freshness::Stale);
    assert_eq!(snapshot.system[0].extended_freshness, Freshness::Stale);
    assert_eq!(fixture.writes(0xe8), 1);
    fixture.finish().await;
}

#[tokio::test(start_paused = true)]
async fn only_advertised_daily_layouts_are_exposed_and_other_operations_do_not_send() {
    let fixture = Harness::start(&[0xe2, 0xe4, 0xf2, 0xfa], Vec::new()).await;
    let snapshot = fixture.handle.snapshot();
    assert_eq!(
        snapshot.audio.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [AudioKind::Dsee]
    );
    assert_eq!(
        snapshot.system.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [SystemKind::SpeakToChatType1]
    );
    for code in [0xe4, 0xfa] {
        let function = snapshot
            .functions
            .iter()
            .find(|entry| entry.advertised.id.code == code)
            .unwrap();
        assert!(!function.readable && !function.writable);
    }
    assert!(matches!(
        fixture.handle.refresh_audio(AudioKind::Bgm).await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        fixture
            .handle
            .refresh_audio(AudioKind::ClassicLePriority)
            .await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        fixture.handle.refresh_system(SystemKind::AutoVolume).await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        fixture
            .handle
            .refresh_system(SystemKind::SpeakToChatType2)
            .await,
        Err(Error::Unsupported(_))
    ));
    assert_eq!(fixture.trace.lock().unwrap().len(), 6);
    fixture.finish().await;
}
