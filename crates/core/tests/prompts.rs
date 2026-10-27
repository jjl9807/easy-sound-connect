//! Independently constructed protocol peers; no device capture or hardware claim.
use bluetooth::{Device, DeviceId, Driver, TransportKind, fake::FakeBackend};
use ezsc_core::{
    AudioChange, ConnectionState, DeviceHandle, DeviceManager, Error, OperationResult, PromptReply,
    SessionConfig, TimeoutStage,
    audio::{AudioKind, PriorityMode},
};
use libmdr2::frame::{DataType, Decoder, Frame, Sequence};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

struct Exchange {
    request: Vec<u8>,
    replies: Vec<Vec<u8>>,
    disconnect: bool,
}
impl Exchange {
    fn read(request: &[u8], response: &[u8]) -> Self {
        Self {
            request: request.into(),
            replies: vec![response.into()],
            disconnect: false,
        }
    }
    fn ack(request: &[u8]) -> Self {
        Self {
            request: request.into(),
            replies: vec![],
            disconnect: false,
        }
    }
    fn notice(mut self, notice: &[u8]) -> Self {
        self.replies.push(notice.into());
        self
    }
    fn disconnect(mut self) -> Self {
        self.disconnect = true;
        self
    }
}
type Trace = Arc<Mutex<Vec<Vec<u8>>>>;
struct Harness {
    manager: DeviceManager,
    handle: DeviceHandle,
    openings: mpsc::Receiver<Driver>,
    peer: tokio::task::JoinHandle<()>,
    inject: mpsc::Sender<Vec<u8>>,
    trace: Trace,
}
fn initialization(functions: &[u8], foreground: bool, notice: Option<&[u8]>) -> Vec<Exchange> {
    let mut supported = vec![7, 0, functions.len() as u8];
    for function in functions {
        supported.extend([*function, 0]);
    }
    let mut script = vec![
        Exchange::read(&[0, 0], &[1, 0, 3, 0, 0x30, 0x32, 0, 1]),
        Exchange::read(&[2, 0], &[3, 0, 9, 0]),
        Exchange::read(&[6, 0], &supported),
        Exchange::read(&[4, 1], b"\x05\x01\x07Fixture"),
        Exchange::read(&[4, 2], b"\x05\x02\x031.0"),
    ];
    if functions.contains(&0x90) {
        let mut enable = Exchange::ack(&[0x94, 0, 0]);
        if let Some(notice) = notice {
            enable = enable.notice(notice);
        }
        script.push(enable);
        if foreground {
            script.push(Exchange::ack(&[0x94, 4, 0]));
        }
    }
    script
}
impl Harness {
    async fn start(functions: &[u8], operations: Vec<Exchange>) -> Self {
        Self::configured(functions, SessionConfig::default(), None, operations).await
    }
    async fn configured(
        functions: &[u8],
        config: SessionConfig,
        notice: Option<&[u8]>,
        operations: Vec<Exchange>,
    ) -> Self {
        let id = DeviceId("prompt-fixture".into());
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
            .connect(id, TransportKind::Gatt, config.clone())
            .await
            .unwrap();
        let driver = openings.recv().await.unwrap();
        let mut script = initialization(functions, config.report_foreground, notice);
        script.extend(operations);
        let trace = Arc::new(Mutex::new(Vec::new()));
        let (inject, receiver) = mpsc::channel(8);
        let peer = tokio::spawn(serve(driver, script, trace.clone(), receiver));
        assert_eq!(
            handle.initialize().await.unwrap(),
            OperationResult::Synchronized
        );
        Self {
            manager,
            handle,
            openings,
            peer,
            inject,
            trace,
        }
    }
    async fn finish(self) {
        let _ = self.manager.shutdown().await;
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
    async fn notice(&self, bytes: &[u8]) -> ezsc_core::DevicePrompt {
        let previous = self.handle.snapshot().pending_prompt.map(|p| p.id);
        let mut updates = self.handle.subscribe();
        self.inject.send(bytes.into()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let snapshot = updates.borrow().clone();
                if let Some(prompt) = snapshot.pending_prompt
                    && Some(prompt.id) != previous
                {
                    return prompt;
                }
                updates.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
}
async fn deliver(driver: &Driver, frame: Frame) {
    for chunk in frame.encode(16384).unwrap().chunks(3) {
        driver.incoming.send(chunk.to_vec()).await.unwrap();
    }
}
async fn serve(
    mut driver: Driver,
    script: Vec<Exchange>,
    trace: Trace,
    mut notices: mpsc::Receiver<Vec<u8>>,
) {
    let mut steps: VecDeque<_> = script.into();
    let mut decoder = Decoder::new(16384).unwrap();
    let mut sequence = Sequence::Zero;
    loop {
        let write = tokio::select! {
            biased;
            _ = driver.cancel.cancelled() => break,
            Some(payload) = notices.recv() => {
                deliver(&driver, Frame { data_type:DataType::DATA_MDR, sequence, payload }).await;
                sequence = sequence.toggled();
                continue;
            },
            write = driver.writes.recv() => match write { Some(write)=>write,None=>break },
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
        trace.lock().unwrap().push(frame.payload.clone());
        let step = steps
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected write {:02x?}", frame.payload));
        assert_eq!(frame.payload, step.request);
        for payload in step.replies {
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
        deliver(&driver, Frame::acknowledgement(frame.sequence)).await;
        if step.disconnect {
            assert!(steps.is_empty());
            driver.finish(Err(bluetooth::Error::new(
                bluetooth::ErrorKind::Disconnected,
                "prompt fixture disconnect",
            )));
            return;
        }
    }
    assert!(steps.is_empty(), "{} requests not sent", steps.len());
    driver.finish(Ok(()));
}
fn priority_reads(value: u8) -> Vec<Exchange> {
    vec![
        Exchange::read(&[0xe0, 5], &[0xe1, 5, 3, 0, 1, 2, 0]),
        Exchange::read(&[0xe2, 5], &[0xe3, 5, 0, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, value]),
    ]
}
async fn set_sound(handle: &DeviceHandle) -> OperationResult {
    handle
        .set_audio(
            AudioKind::ClassicLePriority,
            AudioChange::Priority(PriorityMode::from_raw(0)),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn alert_delivery_and_foreground_are_capability_gated() {
    for (functions, foreground) in [(&[][..], true), (&[0x90][..], false), (&[0x90][..], true)] {
        let h = Harness::configured(
            functions,
            SessionConfig {
                report_foreground: foreground,
                ..Default::default()
            },
            None,
            vec![],
        )
        .await;
        assert_eq!(
            h.writes(0x94),
            if functions.is_empty() {
                0
            } else if foreground {
                2
            } else {
                1
            }
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn initialization_notice_remains_visible_without_preventing_ready() {
    let h = Harness::configured(
        &[0x90],
        SessionConfig::default(),
        Some(&[0x99, 0, 0xff, 1]),
        vec![],
    )
    .await;
    assert_eq!(h.handle.snapshot().connection, ConnectionState::Ready);
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert!(prompt.allows(PromptReply::Dismiss));
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Dismiss)
            .await
            .unwrap(),
        OperationResult::Dismissed
    );
    assert_eq!(h.writes(0x98), 0);
    h.finish().await;
}

#[tokio::test]
async fn default_control_waits_for_explicit_accept_or_reject_and_fresh_readback() {
    for (reply, wire, returned, expected) in [
        (PromptReply::Accept, 1, 0, OperationResult::Confirmed),
        (PromptReply::Reject, 0, 1, OperationResult::Cancelled),
        (PromptReply::Accept, 1, 1, OperationResult::Adjusted),
        (PromptReply::Reject, 0, 0, OperationResult::Adjusted),
    ] {
        let mut script = priority_reads(1);
        script.extend([
            Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x76, 1]),
            Exchange::ack(&[0x98, 0, 0x76, wire]),
            Exchange::read(&[0xe6, 5], &[0xe7, 5, returned]),
        ]);
        let h = Harness::start(&[0x90, 0xe7], script).await;
        assert_eq!(
            set_sound(&h.handle).await,
            OperationResult::AwaitingConfirmation
        );
        assert_eq!(h.writes(0x98), 0);
        let prompt = h.handle.snapshot().pending_prompt.unwrap();
        assert_eq!(
            h.handle.respond_prompt(prompt.id, reply).await.unwrap(),
            expected
        );
        assert_eq!(
            h.handle.respond_prompt(prompt.id, reply).await.unwrap_err(),
            Error::StalePrompt
        );
        assert_eq!(h.writes(0x98), 1);
        h.finish().await;
    }
}

#[tokio::test]
async fn no_alert_preserves_original_setting_verification() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(set_sound(&h.handle).await, OperationResult::Confirmed);
    assert_eq!(h.writes(0x98), 0);
    h.finish().await;
}

#[tokio::test]
async fn pending_prompt_blocks_settings_but_allows_reads() {
    let mut script = priority_reads(1);
    script.extend([Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x76, 1])]);
    script.extend(priority_reads(1));
    script.extend([
        Exchange::ack(&[0x98, 0, 0x76, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]),
    ]);
    let h = Harness::start(&[0x90, 0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    assert_eq!(
        h.handle
            .set_audio(
                AudioKind::ClassicLePriority,
                AudioChange::Priority(PriorityMode::from_raw(1))
            )
            .await
            .unwrap_err(),
        Error::PromptPending
    );
    assert_eq!(
        h.handle
            .refresh_audio(AudioKind::ClassicLePriority)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Reject)
            .await
            .unwrap(),
        OperationResult::Cancelled
    );
    h.finish().await;
}

#[tokio::test]
async fn unsupported_display_only_and_assistant_messages_can_only_be_dismissed() {
    for bytes in [
        vec![0x99, 0, 0xff, 1],
        vec![0x99, 0, 0x76, 0],
        vec![0x99, 2, 0, 0],
        vec![0x99, 6, 0, 1, 0xff, 1],
    ] {
        let h = Harness::start(&[], vec![]).await;
        let prompt = h.notice(&bytes).await;
        assert!(!prompt.allows(PromptReply::Accept));
        assert!(!prompt.allows(PromptReply::Reject));
        assert_eq!(
            h.handle
                .respond_prompt(prompt.id, PromptReply::Accept)
                .await
                .unwrap_err(),
            Error::InvalidPromptReply
        );
        assert_eq!(
            h.handle
                .respond_prompt(prompt.id, PromptReply::Dismiss)
                .await
                .unwrap(),
            OperationResult::Dismissed
        );
        assert_eq!(h.writes(0x98), 0);
        h.finish().await;
    }
}

#[tokio::test]
async fn positive_only_disallows_rejection_and_dismissal() {
    let h = Harness::start(&[], vec![Exchange::ack(&[0x98, 4, 0x76, 1])]).await;
    let prompt = h.notice(&[0x99, 4, 0x76, 2]).await;
    assert!(prompt.allows(PromptReply::Accept));
    assert!(!prompt.allows(PromptReply::Reject));
    assert!(!prompt.allows(PromptReply::Dismiss));
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Reject)
            .await
            .unwrap_err(),
        Error::InvalidPromptReply
    );
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Dismiss)
            .await
            .unwrap_err(),
        Error::InvalidPromptReply
    );
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn replacement_prompt_invalidates_old_identity_and_association() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x77, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let old = h.handle.snapshot().pending_prompt.unwrap();
    let current = h.notice(&[0x99, 0, 0x77, 1]).await;
    assert_ne!(old.id, current.id);
    assert_eq!(
        h.handle
            .respond_prompt(old.id, PromptReply::Accept)
            .await
            .unwrap_err(),
        Error::StalePrompt
    );
    assert_eq!(
        h.handle
            .respond_prompt(current.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn preflight_prompt_does_not_send_or_replay_the_original_setting() {
    let script = vec![
        Exchange::read(&[0xe0, 5], &[0xe1, 5, 3, 0, 1, 2, 0]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ];
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    assert_eq!(h.writes(0xe8), 0);
    h.finish().await;
}

#[tokio::test]
async fn prompt_during_reply_replaces_identity_without_confirming_old_setting() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]).notice(&[0x99, 0, 0x77, 1]),
        Exchange::ack(&[0x98, 0, 0x77, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let old = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(old.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::AwaitingConfirmation
    );
    let current = h.handle.snapshot().pending_prompt.unwrap();
    assert_ne!(old.id, current.id);
    assert_eq!(
        h.handle
            .respond_prompt(current.id, PromptReply::Reject)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn held_get_waits_for_explicit_reply_and_old_value_cannot_confirm_new_readback() {
    for fresh in [0, 1] {
        let mut script = priority_reads(1);
        script.extend([
            Exchange::ack(&[0xe8, 5, 0, 0]),
            Exchange::ack(&[0xe6, 5]).notice(&[0x99, 0, 0x76, 1]),
            // The old GET is answered only after explicit 98, and misleadingly reports desired 0.
            Exchange::ack(&[0x98, 0, 0x76, 1]).notice(&[0xe7, 5, 0]),
            Exchange::read(&[0xe6, 5], &[0xe7, 5, fresh]),
        ]);
        let h = Harness::start(&[0xe7], script).await;
        assert_eq!(
            set_sound(&h.handle).await,
            OperationResult::AwaitingConfirmation
        );
        let prompt = h.handle.snapshot().pending_prompt.unwrap();
        let expected = if fresh == 0 {
            OperationResult::Confirmed
        } else {
            OperationResult::Adjusted
        };
        assert_eq!(
            h.handle
                .respond_prompt(prompt.id, PromptReply::Accept)
                .await
                .unwrap(),
            expected
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn a_read_waiting_for_prompt_reply_can_also_release_its_matcher() {
    let script = vec![
        Exchange::ack(&[0xe0, 5]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]).notice(&[0xe1, 5, 3, 0, 1, 2, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ];
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        h.handle
            .refresh_audio(AudioKind::ClassicLePriority)
            .await
            .unwrap(),
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn missing_held_response_after_reply_is_bounded_and_marks_state_stale() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]),
        Exchange::ack(&[0xe6, 5]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]),
    ]);
    let h = Harness::configured(
        &[0xe7],
        SessionConfig {
            response_timeout: Duration::from_millis(30),
            ..Default::default()
        },
        None,
        script,
    )
    .await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap_err(),
        Error::Timeout(TimeoutStage::Response)
    );
    let _ = h.handle.close().await;
    assert!(h.handle.snapshot().pending_prompt.is_none());
    assert_eq!(
        h.handle.snapshot().audio[0].freshness,
        ezsc_core::Freshness::Stale
    );
    h.finish().await;
}

#[tokio::test]
async fn reply_disconnect_clears_prompt_without_claiming_confirmation() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]).disconnect(),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .is_err()
    );
    let _ = h.handle.close().await;
    assert!(h.handle.snapshot().pending_prompt.is_none());
    assert_eq!(
        h.handle.snapshot().audio[0].freshness,
        ezsc_core::Freshness::Stale
    );
    h.finish().await;
}

#[tokio::test]
async fn late_or_opposite_target_prompts_are_not_confirmation_of_a_completed_write() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(set_sound(&h.handle).await, OperationResult::Adjusted);
    let prompt = h.notice(&[0x99, 0, 0x76, 1]).await;
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;

    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0x77, 1]),
        Exchange::ack(&[0x98, 0, 0x77, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn dsee_warning_and_unrelated_priority_message_have_distinct_readback_semantics() {
    for (id, expected) in [
        (0x08, OperationResult::Confirmed),
        (0x76, OperationResult::Synchronized),
    ] {
        let script = vec![
            Exchange::read(&[0xe0, 1], &[0xe1, 1, 3]),
            Exchange::read(&[0xe2, 1], &[0xe3, 1, 0]),
            Exchange::read(&[0xe6, 1], &[0xe7, 1, 0]),
            Exchange::ack(&[0xe8, 1, 1]).notice(&[0x99, 0, id, 1]),
            Exchange::ack(&[0x98, 0, id, 1]),
            Exchange::read(&[0xe6, 1], &[0xe7, 1, 1]),
        ];
        let h = Harness::start(&[0xe2], script).await;
        assert_eq!(
            h.handle
                .set_audio(
                    AudioKind::Dsee,
                    AudioChange::Dsee(ezsc_core::audio::DseeMode::Auto)
                )
                .await
                .unwrap(),
            OperationResult::AwaitingConfirmation
        );
        let prompt = h.handle.snapshot().pending_prompt.unwrap();
        assert_eq!(
            h.handle
                .respond_prompt(prompt.id, PromptReply::Accept)
                .await
                .unwrap(),
            expected
        );
        h.finish().await;
    }
}

#[tokio::test]
async fn equalizer_preselection_prompt_does_not_claim_the_unsent_curve_was_written() {
    use ezsc_core::equalizer::{EqualizerKind, EqualizerSetting, PresetId};
    let script = vec![
        Exchange::read(&[0x50, 0, 1], &[0x51, 0, 2, 13, 2, 0, 0, 0xa0, 0]),
        Exchange::read(&[0x52, 0], &[0x53, 0, 0]),
        Exchange::read(&[0x56, 0], &[0x57, 0, 0, 2, 6, 6]),
        Exchange::ack(&[0x58, 0, 0xa0, 0]).notice(&[0x99, 0, 8, 1]),
        Exchange::ack(&[0x98, 0, 8, 1]),
        Exchange::read(&[0x56, 0], &[0x57, 0, 0xa0, 2, 6, 6]),
    ];
    let h = Harness::start(&[0x50], script).await;
    let setting = EqualizerSetting::Preset {
        preset: PresetId::from_raw(0xa0),
        levels: Some(vec![5, 7]),
    };
    assert_eq!(
        h.handle
            .set_equalizer(EqualizerKind::Preset, setting)
            .await
            .unwrap(),
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Adjusted
    );
    assert_eq!(h.writes(0x58), 1);
    h.finish().await;
}

#[tokio::test]
async fn dismissing_an_informational_prompt_refreshes_the_interrupted_write() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 0, 0xff, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Dismiss)
            .await
            .unwrap(),
        OperationResult::Dismissed
    );
    assert_eq!(h.writes(0x98), 0);
    assert_eq!(
        h.handle.snapshot().audio[0]
            .parameters
            .as_ref()
            .unwrap()
            .value(),
        &ezsc_core::audio::AudioParameterValue::ConnectionPriority {
            priority: PriorityMode::from_raw(0),
            switching_stream: None
        }
    );
    h.finish().await;
}

#[tokio::test]
async fn pending_prompt_then_a_read_waiting_for_reply_does_not_deadlock() {
    let script = vec![
        Exchange::ack(&[0xe0, 5]),
        Exchange::ack(&[0x98, 0, 0x76, 1]).notice(&[0xe1, 5, 3, 0, 1, 2, 0]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ];
    let h = Harness::start(&[0xe7], script).await;
    let prompt = h.notice(&[0x99, 0, 0x76, 1]).await;
    assert_eq!(
        h.handle
            .refresh_audio(AudioKind::ClassicLePriority)
            .await
            .unwrap(),
        OperationResult::AwaitingConfirmation
    );
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn new_prompt_while_draining_old_get_pauses_its_response_budget_again() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]),
        Exchange::ack(&[0xe6, 5]).notice(&[0x99, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x76, 1]),
        Exchange::ack(&[0x98, 0, 0x77, 0]).notice(&[0xe7, 5, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 1]),
    ]);
    let h = Harness::configured(
        &[0xe7],
        SessionConfig {
            response_timeout: Duration::from_millis(80),
            prompt_timeout: Duration::from_secs(2),
            ..Default::default()
        },
        None,
        script,
    )
    .await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let first = h.handle.snapshot().pending_prompt.unwrap();
    let handle = h.handle.clone();
    let reply =
        tokio::spawn(async move { handle.respond_prompt(first.id, PromptReply::Accept).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while h.writes(0x98) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second = h.notice(&[0x99, 0, 0x77, 1]).await;
    assert_eq!(
        reply.await.unwrap().unwrap(),
        OperationResult::AwaitingConfirmation
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(h.handle.snapshot().connection, ConnectionState::Ready);
    assert_eq!(
        h.handle
            .respond_prompt(second.id, PromptReply::Reject)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn prompt_expiry_closes_connection_and_invalidates_the_ticket() {
    let h = Harness::configured(
        &[],
        SessionConfig {
            prompt_timeout: Duration::from_millis(30),
            ..Default::default()
        },
        None,
        vec![],
    )
    .await;
    let prompt = h.notice(&[0x99, 0, 0x76, 1]).await;
    let mut updates = h.handle.subscribe();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if updates.borrow().connection == ConnectionState::Failed {
                break;
            }
            updates.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(
        h.handle.snapshot().last_error,
        Some(Error::Timeout(TimeoutStage::Prompt))
    );
    assert!(h.handle.snapshot().pending_prompt.is_none());
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap_err(),
        Error::StalePrompt
    );
    assert_eq!(h.writes(0x98), 0);
    h.finish().await;
}

#[tokio::test]
async fn reconnect_cannot_reuse_a_prompt_from_the_old_generation() {
    let mut h = Harness::start(&[], vec![]).await;
    let old = h.notice(&[0x99, 0, 0x76, 1]).await;
    let id = h.handle.snapshot().device;
    h.manager.disconnect(&id).await.unwrap();
    h.peer.await.unwrap();
    h.handle = h
        .manager
        .connect(id, TransportKind::Gatt, SessionConfig::default())
        .await
        .unwrap();
    let driver = h.openings.recv().await.unwrap();
    let mut script = initialization(&[], false, None);
    script.push(Exchange::ack(&[0x98, 0, 0x76, 0]));
    let (sender, receiver) = mpsc::channel(8);
    h.inject = sender;
    h.peer = tokio::spawn(serve(driver, script, h.trace.clone(), receiver));
    h.handle.initialize().await.unwrap();
    let current = h.notice(&[0x99, 0, 0x76, 1]).await;
    assert_ne!(old.id.generation, current.id.generation);
    assert_eq!(old.id.serial, current.id.serial);
    assert_eq!(
        h.handle
            .respond_prompt(old.id, PromptReply::Accept)
            .await
            .unwrap_err(),
        Error::StalePrompt
    );
    assert_eq!(
        h.handle
            .respond_prompt(current.id, PromptReply::Reject)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}

#[tokio::test]
async fn flexible_affected_items_do_not_infer_the_requested_setting() {
    let mut script = priority_reads(1);
    script.extend([
        Exchange::ack(&[0xe8, 5, 0, 0]).notice(&[0x99, 6, 0x11, 1, 0x18, 1]),
        Exchange::ack(&[0x98, 6, 0x11, 1]),
        Exchange::read(&[0xe6, 5], &[0xe7, 5, 0]),
    ]);
    let h = Harness::start(&[0xe7], script).await;
    assert_eq!(
        set_sound(&h.handle).await,
        OperationResult::AwaitingConfirmation
    );
    let prompt = h.handle.snapshot().pending_prompt.unwrap();
    assert_eq!(
        h.handle
            .respond_prompt(prompt.id, PromptReply::Accept)
            .await
            .unwrap(),
        OperationResult::Synchronized
    );
    h.finish().await;
}
