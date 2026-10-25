use libmdr2::frame::{DataType, Decoder, Frame, FrameError, Sequence};

const ACK_ZERO: &[u8] = &[0x3e, 1, 0, 0, 0, 0, 0, 1, 0x3c];
const ACK_ONE: &[u8] = &[0x3e, 1, 1, 0, 0, 0, 0, 2, 0x3c];
const PROTOCOL_QUERY: &[u8] = &[0x3e, 0x0c, 0, 0, 0, 0, 2, 0, 0, 0x0e, 0x3c];

fn decode(input: &[u8]) -> Vec<Result<Frame, FrameError>> {
    Decoder::new(4096).unwrap().feed(input).collect()
}

fn data(payload: &[u8]) -> Frame {
    Frame {
        data_type: DataType::DATA_MDR,
        sequence: Sequence::Zero,
        payload: payload.to_vec(),
    }
}

#[test]
fn independent_ack_and_query_vectors() {
    for (received, bytes) in [(Sequence::One, ACK_ZERO), (Sequence::Zero, ACK_ONE)] {
        let frame = Frame::acknowledgement(received);
        assert_eq!(frame.encode(0).unwrap(), bytes);
        assert_eq!(decode(bytes), vec![Ok(frame)]);
    }
    let query = data(&[0, 0]);
    assert_eq!(query.encode(2).unwrap(), PROTOCOL_QUERY);
    assert_eq!(decode(PROTOCOL_QUERY), vec![Ok(query)]);
}

#[test]
fn all_escapes_and_checksum_escape_match_wire_vectors() {
    // Header sums to 0x0f; payload sums to 0xb7, so checksum is 0xc6.
    let escaped = [
        0x3e, 0x0c, 0, 0, 0, 0, 3, 0x3d, 0x2c, 0x3d, 0x2d, 0x3d, 0x2e, 0xc6, 0x3c,
    ];
    let frame = data(&[0x3c, 0x3d, 0x3e]);
    assert_eq!(frame.encode(3).unwrap(), escaped);
    assert_eq!(decode(&escaped), vec![Ok(frame)]);
    for (payload, suffix) in [(0x2f, 0x2c), (0x30, 0x2d), (0x31, 0x2e)] {
        let bytes = [0x3e, 0x0c, 0, 0, 0, 0, 1, payload, 0x3d, suffix, 0x3c];
        assert_eq!(data(&[payload]).encode(1).unwrap(), bytes);
        assert_eq!(decode(&bytes), vec![Ok(data(&[payload]))]);
    }
}

#[test]
fn fragments_at_every_boundary_and_multiple_frames() {
    let input = [ACK_ONE, PROTOCOL_QUERY, ACK_ZERO].concat();
    let expected = vec![
        Ok(Frame::acknowledgement(Sequence::Zero)),
        Ok(data(&[0, 0])),
        Ok(Frame::acknowledgement(Sequence::One)),
    ];
    for split in 0..=input.len() {
        let mut decoder = Decoder::new(2).unwrap();
        let mut results: Vec<_> = decoder.feed(&input[..split]).collect();
        results.extend(decoder.feed(&input[split..]));
        assert_eq!(results, expected, "split at {split}");
        assert_eq!(decoder.finish(), Ok(()));
    }
    let escaped = data(&[0x3c, 0x3d, 0x3e]).encode(3).unwrap();
    let mut decoder = Decoder::new(3).unwrap();
    let results: Vec<_> = escaped
        .into_iter()
        .filter_map(|byte| decoder.push(byte))
        .collect();
    assert_eq!(results, vec![Ok(data(&[0x3c, 0x3d, 0x3e]))]);
}

#[test]
fn rejects_valid_checksum_with_false_length() {
    // A valid checksum must not hide the declared/actual payload length mismatch.
    let malformed = [0x3e, 0x0c, 0, 0, 0, 0, 5, 7, 0x18, 0x3c];
    assert_eq!(
        decode(&malformed),
        vec![Err(FrameError::LengthMismatch {
            declared: 5,
            actual: 1
        })]
    );
    let oversized_body = [0x3e, 0x0c, 0, 0, 0, 0, 0, 0x0c, 0xff, 0x3c];
    assert_eq!(
        decode(&oversized_body),
        vec![Err(FrameError::LengthMismatch {
            declared: 0,
            actual: 1
        })]
    );
}

#[test]
fn rejects_corrupt_checksum_and_invalid_sequence() {
    let mut bad_sum = PROTOCOL_QUERY.to_vec();
    bad_sum[9] = 0x0f;
    assert_eq!(
        decode(&bad_sum),
        vec![Err(FrameError::ChecksumMismatch {
            expected: 0x0e,
            actual: 0x0f
        })]
    );
    let mut bad_seq = PROTOCOL_QUERY.to_vec();
    bad_seq[2] = 2;
    assert_eq!(decode(&bad_seq), vec![Err(FrameError::InvalidSequence(2))]);
}

#[test]
fn malformed_frames_recover_at_the_next_start() {
    for (prefix, expected) in [
        (vec![0xff, 0x3e, 0x0c, 0], FrameError::UnexpectedStart),
        (vec![0x3e, 0x3d], FrameError::InvalidEscape(0x3e)),
        (vec![0x3e, 0x3d, 0x2f], FrameError::InvalidEscape(0x2f)),
        (vec![0x3e, 0x3d, 0x3c], FrameError::InvalidEscape(0x3c)),
        (vec![0x3e, 0x0c, 0x3c], FrameError::FrameTooShort(1)),
    ] {
        let input = [prefix.as_slice(), ACK_ONE].concat();
        assert_eq!(
            decode(&input),
            vec![Err(expected), Ok(Frame::acknowledgement(Sequence::Zero))]
        );
    }
    assert!(decode(&[0, 0x3c, 0x3d, 0xff]).is_empty());
}

#[test]
fn limit_is_checked_from_header_without_waiting_for_payload() {
    let mut decoder = Decoder::new(4).unwrap();
    let input = [0x3e, 0x0c, 0, 0xff, 0xff, 0xff, 0xff];
    assert_eq!(
        decoder.feed(&input).collect::<Vec<_>>(),
        vec![Err(FrameError::PayloadTooLarge {
            length: u32::MAX as usize,
            maximum: 4
        })]
    );
    assert_eq!(
        decoder.feed(ACK_ZERO).collect::<Vec<_>>(),
        vec![Ok(Frame::acknowledgement(Sequence::One))]
    );
    assert!(Decoder::new(usize::MAX).is_err());
}

#[test]
fn nonempty_ack_is_rejected_in_both_directions() {
    let bytes = [0x3e, 1, 0, 0, 0, 0, 1, 0, 2, 0x3c];
    assert_eq!(decode(&bytes), vec![Err(FrameError::InvalidAckPayload(1))]);
    let frame = Frame {
        data_type: DataType::ACK,
        sequence: Sequence::Zero,
        payload: vec![0],
    };
    let mut output = vec![0xaa];
    assert_eq!(
        frame.encode_into(&mut output, 1),
        Err(FrameError::InvalidAckPayload(1))
    );
    assert_eq!(output, [0xaa]);
    assert!(data(&[0, 0]).encode_into(&mut output, 1).is_err());
    assert_eq!(output, [0xaa]);
}

#[test]
fn end_of_stream_distinguishes_fragments_from_truncation() {
    let mut decoder = Decoder::new(2).unwrap();
    for length in 1..PROTOCOL_QUERY.len() {
        assert!(
            decoder
                .feed(&PROTOCOL_QUERY[..length])
                .collect::<Vec<_>>()
                .is_empty()
        );
        assert_eq!(decoder.finish(), Err(FrameError::TruncatedFrame));
        assert_eq!(decoder.finish(), Ok(()));
    }
    assert_eq!(
        decoder.feed(PROTOCOL_QUERY).collect::<Vec<_>>(),
        vec![Ok(data(&[0, 0]))]
    );
    assert_eq!(decoder.finish(), Ok(()));
}

#[test]
fn all_payload_bytes_and_escaped_length_round_trip() {
    for length in [0, 1, 60, 61, 62, 256, 4096] {
        let frame = Frame {
            data_type: DataType::from_raw(0xfe),
            sequence: Sequence::One,
            payload: (0..length).map(|i| i as u8).collect(),
        };
        let encoded = frame.encode(4096).unwrap();
        assert_eq!(decode(&encoded), vec![Ok(frame)]);
    }
}

#[test]
fn noisy_stream_remains_recoverable() {
    // Deterministic noise explores malformed headers, escape boundaries and recovery.
    let mut decoder = Decoder::new(32).unwrap();
    let mut state = 0x9e37_79b9u32;
    for _ in 0..512 {
        for _ in 0..127 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let _ = decoder.push(state as u8);
        }
        let results: Vec<_> = decoder.feed(ACK_ONE).collect();
        assert_eq!(
            results.last(),
            Some(&Ok(Frame::acknowledgement(Sequence::Zero)))
        );
    }
}
