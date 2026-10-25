//! Drive an offline protocol-info exchange through the public API.

use std::{collections::VecDeque, error::Error, time::Duration};

use libmdr2::{
    command::{Request, Response},
    frame::Decoder,
    session::{Session, SessionEvent},
};

fn main() -> Result<(), Box<dyn Error>> {
    let request = Request::ProtocolInfo.encode()?;
    // Register the matcher before the first write. It is not a transaction ID.
    let expected = request.expected_response;
    let mut session = Session::default();
    let events = session.send(request.table, request.payload, Duration::ZERO)?;
    let [SessionEvent::Transmit { token, frame }] = events.as_slice() else {
        return Err("expected an initial write".into());
    };
    assert_eq!(
        frame.encode(4096)?,
        [0x3e, 0x0c, 0, 0, 0, 0, 2, 0, 0, 0x0e, 0x3c]
    );
    // A real transport calls this only after every byte has been written.
    assert!(session.complete_write(*token, Duration::ZERO)?.is_empty());

    // Synthetic exchange: ACK sequence 1, then protocol-info response sequence 0.
    // Header + response payload sums to 0x7a; the checksum is not a polynomial CRC.
    let wire = [
        0x3e, 1, 1, 0, 0, 0, 0, 2, 0x3c, 0x3e, 0x0c, 0, 0, 0, 0, 8, 1, 0, 3, 0, 0x30, 0x32, 0, 0,
        0x7a, 0x3c,
    ];
    let mut decoder = Decoder::new(4096)?;
    let now = Duration::from_millis(1);
    let mut confirmed_response = false;
    for chunk in wire.chunks(3) {
        for decoded in decoder.feed(chunk) {
            let mut effects = VecDeque::from(session.receive(decoded?, now)?);
            while let Some(effect) = effects.pop_front() {
                match effect {
                    SessionEvent::Transmit { token, frame } => {
                        assert_eq!(frame.encode(4096)?, [0x3e, 1, 1, 0, 0, 0, 0, 2, 0x3c]);
                        effects.extend(session.complete_write(token, now)?);
                    }
                    SessionEvent::LinkAcknowledged { .. } => {
                        println!("Request link-acknowledged; still awaiting its response.");
                    }
                    SessionEvent::Received(frame) => {
                        let table = frame.data_type.command_table().ok_or("missing table")?;
                        if expected.matches(table, &frame.payload) {
                            let Response::ProtocolInfo(info) =
                                expected.decode(table, &frame.payload)?
                            else {
                                return Err("unexpected response family".into());
                            };
                            println!(
                                "Protocol {}: T1={}, T2={}",
                                info.version, info.table_1_supported, info.table_2_supported
                            );
                            confirmed_response = true;
                        }
                    }
                    SessionEvent::Closed(reason) => {
                        return Err(format!("session closed: {reason:?}").into());
                    }
                    _ => return Err("unsupported session event".into()),
                }
            }
        }
    }
    decoder.finish()?;
    assert!(confirmed_response);
    Ok(())
}
