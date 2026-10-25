use std::time::Duration;

use libmdr2::frame::{DataType, Frame, Sequence};
use libmdr2::session::{
    CloseReason, MAX_PENDING_ACKS, RetryPolicy, Session, SessionError, SessionEvent, WriteToken,
};
use libmdr2::types::CommandTable;

fn at(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

fn short_session(max_retries: u32) -> Session {
    Session::new(RetryPolicy::new(at(5), max_retries).unwrap())
}

fn data(sequence: Sequence, payload: &[u8]) -> Frame {
    Frame {
        data_type: DataType::DATA_MDR,
        sequence,
        payload: payload.to_vec(),
    }
}

fn ack(sequence: Sequence) -> Frame {
    Frame {
        data_type: DataType::ACK,
        sequence,
        payload: Vec::new(),
    }
}

fn transmitted(events: &[SessionEvent]) -> (WriteToken, Frame) {
    let writes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Transmit { token, frame } => Some((*token, frame.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 1, "expected exactly one write: {events:?}");
    writes.into_iter().next().unwrap()
}

fn acknowledged(events: &[SessionEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, SessionEvent::LinkAcknowledged { .. }))
}

fn received(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, SessionEvent::Received(_)))
        .count()
}

#[test]
fn policy_counts_retransmissions_and_rejects_zero_timeouts() {
    let policy = RetryPolicy::default();
    assert_eq!(policy.ack_timeout(), at(750));
    assert_eq!(policy.max_retries(), 10);
    assert_eq!(
        RetryPolicy::new(Duration::ZERO, 1),
        Err(SessionError::ZeroAckTimeout)
    );
    assert!(RetryPolicy::new(at(1), 0).is_ok());
}

#[test]
fn early_ack_waits_for_write_completion_and_then_advances_sequence() {
    let mut session = short_session(1);
    let events = session
        .send(CommandTable::TABLE_1, vec![0x00, 0x00], at(10))
        .unwrap();
    let (token, sent) = transmitted(&events);
    assert_eq!(sent.sequence, Sequence::Zero);
    assert!(
        session
            .receive(ack(Sequence::One), at(11))
            .unwrap()
            .is_empty()
    );
    assert!(session.is_busy());
    assert_eq!(session.next_deadline(), None);
    assert_eq!(
        session.send(CommandTable::TABLE_1, vec![0x02, 0x00], at(11)),
        Err(SessionError::Busy)
    );

    let events = session.complete_write(token, at(12)).unwrap();
    assert_eq!(
        events,
        vec![SessionEvent::LinkAcknowledged {
            table: CommandTable::TABLE_1,
            sequence: Sequence::Zero,
        }]
    );
    assert!(!session.is_busy());
    let (_, next) = transmitted(
        &session
            .send(CommandTable::TABLE_2, vec![0x06, 0x00], at(12))
            .unwrap(),
    );
    assert_eq!(next.sequence, Sequence::One);
    assert_eq!(next.data_type, DataType::DATA_MDR_NO2);
}

#[test]
fn early_ack_does_not_hide_a_failed_write() {
    let mut session = Session::default();
    let (token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x00, 0x00], at(0))
            .unwrap(),
    );
    session.receive(ack(Sequence::One), at(1)).unwrap();
    assert_eq!(
        session.fail_write(token, at(2)).unwrap(),
        vec![SessionEvent::Closed(CloseReason::WriteFailed)]
    );
    assert!(session.is_closed());
}

#[test]
fn retry_deadlines_start_after_each_completed_write_and_exhaust_exactly() {
    let mut session = short_session(2);
    let (first_token, original) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x04, 0x01], at(0))
            .unwrap(),
    );
    assert!(session.advance(at(100)).unwrap().is_empty());
    assert_eq!(session.next_deadline(), None);
    session.complete_write(first_token, at(100)).unwrap();
    assert_eq!(session.next_deadline(), Some(at(105)));
    assert!(session.advance(at(104)).unwrap().is_empty());

    let (retry_one, frame) = transmitted(&session.advance(at(105)).unwrap());
    assert_eq!(frame, original);
    assert_ne!(retry_one, first_token);
    assert!(session.advance(at(1000)).unwrap().is_empty());
    session.complete_write(retry_one, at(1000)).unwrap();
    let (retry_two, frame) = transmitted(&session.advance(at(1005)).unwrap());
    assert_eq!(frame, original);
    assert_ne!(retry_two, retry_one);
    session.complete_write(retry_two, at(1006)).unwrap();
    assert_eq!(
        session.advance(at(1011)).unwrap(),
        vec![SessionEvent::Closed(CloseReason::RetriesExhausted)]
    );
    assert_eq!(session.next_deadline(), None);
    assert!(!session.is_busy());
    assert!(session.is_closed());
}

#[test]
fn wrong_and_unsolicited_acks_do_not_advance_the_transmit_sequence() {
    let mut session = short_session(0);
    assert!(
        session
            .receive(ack(Sequence::One), at(0))
            .unwrap()
            .is_empty()
    );
    let (token, frame) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x00, 0x00], at(0))
            .unwrap(),
    );
    assert_eq!(frame.sequence, Sequence::Zero);
    session.complete_write(token, at(1)).unwrap();
    assert!(
        session
            .receive(ack(Sequence::Zero), at(2))
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.next_deadline(), Some(at(6)));
    assert!(acknowledged(
        &session.receive(ack(Sequence::One), at(3)).unwrap()
    ));
}

#[test]
fn duplicate_data_is_acknowledged_again_but_delivered_once() {
    let mut session = Session::default();
    let incoming = data(Sequence::Zero, &[0x23, 0x00, 85]);
    let events = session.receive(incoming.clone(), at(0)).unwrap();
    assert_eq!(received(&events), 1);
    let (token, reply) = transmitted(&events);
    assert_eq!(reply, ack(Sequence::One));
    session.complete_write(token, at(0)).unwrap();

    let events = session.receive(incoming, at(1)).unwrap();
    assert_eq!(received(&events), 0);
    let (_, reply) = transmitted(&events);
    assert_eq!(reply, ack(Sequence::One));
}

#[test]
fn valid_protocol_info_is_a_duplicate_exception_but_malformed_info_is_not() {
    let mut session = Session::default();
    // T1 response: opcode, inquiry, BE version, enabled T1 and T2 flags.
    let info = data(
        Sequence::Zero,
        &[0x01, 0x00, 0x03, 0x00, 0x30, 0x32, 0x00, 0x00],
    );
    for time in 0..2 {
        let events = session.receive(info.clone(), at(time)).unwrap();
        assert_eq!(received(&events), 1);
        let (token, _) = transmitted(&events);
        session.complete_write(token, at(time)).unwrap();
    }
    let malformed = data(Sequence::Zero, &[0x01, 0x00, 0x03]);
    let events = session.receive(malformed, at(2)).unwrap();
    assert_eq!(received(&events), 0);
    assert_eq!(transmitted(&events).1, ack(Sequence::One));
}

#[test]
fn notifications_are_delivered_while_data_is_writing_and_ack_writes_are_serialized() {
    let mut session = short_session(1);
    let (data_token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(0))
            .unwrap(),
    );
    let notification = data(Sequence::Zero, &[0x23, 0x00, 80]);
    let events = session.receive(notification.clone(), at(1)).unwrap();
    assert_eq!(events, vec![SessionEvent::Received(notification)]);
    let (ack_token, reply) = transmitted(&session.complete_write(data_token, at(2)).unwrap());
    assert_eq!(reply, ack(Sequence::One));

    assert!(acknowledged(
        &session.receive(ack(Sequence::One), at(3)).unwrap()
    ));
    assert!(
        session
            .send(CommandTable::TABLE_2, vec![0x06, 0x00], at(3))
            .unwrap()
            .is_empty()
    );
    let (_, next_data) = transmitted(&session.complete_write(ack_token, at(4)).unwrap());
    assert_eq!(next_data.data_type, DataType::DATA_MDR_NO2);
    assert_eq!(next_data.sequence, Sequence::One);
}

#[test]
fn an_ack_cancels_a_retry_waiting_behind_an_ack_write() {
    let mut session = short_session(1);
    let (token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(0))
            .unwrap(),
    );
    session.complete_write(token, at(0)).unwrap();
    let (ack_token, _) = transmitted(
        &session
            .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(1))
            .unwrap(),
    );
    assert!(session.advance(at(5)).unwrap().is_empty());
    assert_eq!(session.next_deadline(), None);
    assert!(acknowledged(
        &session.receive(ack(Sequence::One), at(6)).unwrap()
    ));
    assert!(session.complete_write(ack_token, at(7)).unwrap().is_empty());
    assert!(!session.is_busy());
}

#[test]
fn an_ack_before_the_first_actual_data_write_is_ignored() {
    let mut session = short_session(1);
    let (ack_token, _) = transmitted(
        &session
            .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
            .unwrap(),
    );
    assert!(
        session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(0))
            .unwrap()
            .is_empty()
    );
    assert!(
        session
            .receive(ack(Sequence::One), at(0))
            .unwrap()
            .is_empty()
    );
    let (data_token, outgoing) = transmitted(&session.complete_write(ack_token, at(1)).unwrap());
    assert_eq!(outgoing.sequence, Sequence::Zero);
    assert!(
        session
            .complete_write(data_token, at(2))
            .unwrap()
            .is_empty()
    );
    assert!(session.is_busy());
    assert_eq!(session.next_deadline(), Some(at(7)));
}

#[test]
fn stale_completion_does_not_complete_a_newer_write() {
    let mut session = short_session(1);
    let (first, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x00, 0x00], at(0))
            .unwrap(),
    );
    session.complete_write(first, at(0)).unwrap();
    let (retry, _) = transmitted(&session.advance(at(5)).unwrap());
    assert_eq!(
        session.complete_write(first, at(100)),
        Err(SessionError::UnknownWriteToken(first))
    );
    // A rejected callback did not advance the clock or complete the active write.
    session.complete_write(retry, at(6)).unwrap();
    assert_eq!(session.next_deadline(), Some(at(11)));
}

#[test]
fn malformed_and_unsupported_inputs_do_not_change_state() {
    let mut session = short_session(1);
    let mut invalid_ack = ack(Sequence::One);
    invalid_ack.payload.push(1);
    assert_eq!(
        session.receive(invalid_ack, at(100)),
        Err(SessionError::NonEmptyAckPayload)
    );
    assert_eq!(
        session.receive(data(Sequence::Zero, &[]), at(100)),
        Err(SessionError::EmptyPayload)
    );
    assert_eq!(
        session.send(CommandTable::TABLE_1, Vec::new(), at(100)),
        Err(SessionError::EmptyPayload)
    );
    for data_type in [
        DataType::SHOT_MDR,
        DataType::SHOT_MDR_NO2,
        DataType::from_raw(0xff),
    ] {
        let mut frame = data(Sequence::Zero, &[1]);
        frame.data_type = data_type;
        assert_eq!(
            session.receive(frame, at(100)),
            Err(SessionError::UnsupportedDataType(data_type))
        );
    }
    let unknown_table = CommandTable::from_raw(99);
    assert_eq!(
        session.send(unknown_table, vec![1], at(100)),
        Err(SessionError::UnsupportedTable(unknown_table))
    );
    // Invalid inputs neither advance time nor poison the RX duplicate tracker.
    let events = session
        .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
        .unwrap();
    assert_eq!(received(&events), 1);
}

#[test]
fn the_clock_must_be_monotonic_and_backward_input_does_not_mutate_state() {
    let mut session = short_session(0);
    let (token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(10))
            .unwrap(),
    );
    assert_eq!(
        session.complete_write(token, at(9)),
        Err(SessionError::TimeWentBackwards {
            previous: at(10),
            received: at(9),
        })
    );
    session.complete_write(token, at(10)).unwrap();
    assert_eq!(session.next_deadline(), Some(at(15)));
}

#[test]
fn cancellation_abandons_all_work_and_rejects_late_inputs() {
    let mut session = Session::default();
    let (token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(0))
            .unwrap(),
    );
    session
        .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
        .unwrap();
    assert_eq!(
        session.cancel(at(1)).unwrap(),
        vec![SessionEvent::Closed(CloseReason::Cancelled)]
    );
    assert!(!session.is_busy());
    assert!(session.is_closed());
    assert_eq!(session.next_deadline(), None);
    assert_eq!(
        session.complete_write(token, at(2)),
        Err(SessionError::Closed)
    );
    assert_eq!(
        session.receive(ack(Sequence::One), at(2)),
        Err(SessionError::Closed)
    );
    assert_eq!(session.advance(at(2)), Err(SessionError::Closed));
    assert_eq!(
        session.send(CommandTable::TABLE_1, vec![1], at(2)),
        Err(SessionError::Closed)
    );
}

#[test]
fn ack_write_failure_is_terminal_too() {
    let mut session = Session::default();
    let (token, _) = transmitted(
        &session
            .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
            .unwrap(),
    );
    assert_eq!(
        session.fail_write(token, at(1)).unwrap(),
        vec![SessionEvent::Closed(CloseReason::WriteFailed)]
    );
    assert!(session.is_closed());
}

#[test]
fn ack_backlog_is_bounded_and_overflow_terminates_the_link() {
    let mut session = Session::default();
    session
        .send(CommandTable::TABLE_1, vec![0x20, 0x00], at(0))
        .unwrap();
    for index in 0..MAX_PENDING_ACKS {
        let sequence = if index % 2 == 0 {
            Sequence::Zero
        } else {
            Sequence::One
        };
        let events = session
            .receive(data(sequence, &[0x23, 0x00, 80]), at(0))
            .unwrap();
        assert_eq!(received(&events), 1);
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::Transmit { .. }))
        );
    }
    assert_eq!(
        session
            .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
            .unwrap(),
        vec![SessionEvent::Closed(CloseReason::AckQueueFull)]
    );
    assert!(session.is_closed());
}

#[test]
fn deadline_overflow_closes_without_panicking() {
    let mut session = Session::default();
    let (token, _) = transmitted(
        &session
            .send(CommandTable::TABLE_1, vec![0x20, 0x00], Duration::MAX)
            .unwrap(),
    );
    assert_eq!(
        session.complete_write(token, Duration::MAX).unwrap(),
        vec![SessionEvent::Closed(CloseReason::ClockOverflow)]
    );
}

#[test]
fn receive_sequence_is_shared_between_reliable_command_tables() {
    let mut session = Session::default();
    let (token, _) = transmitted(
        &session
            .receive(data(Sequence::Zero, &[0x23, 0x00, 80]), at(0))
            .unwrap(),
    );
    session.complete_write(token, at(0)).unwrap();
    let table_two = Frame {
        data_type: DataType::DATA_MDR_NO2,
        sequence: Sequence::One,
        payload: vec![0x23, 0x00, 80],
    };
    let events = session.receive(table_two.clone(), at(1)).unwrap();
    assert_eq!(received(&events), 1);
    let (token, reply) = transmitted(&events);
    assert_eq!(reply, ack(Sequence::Zero));
    session.complete_write(token, at(1)).unwrap();
    let events = session.receive(table_two, at(2)).unwrap();
    assert_eq!(received(&events), 0);
}
