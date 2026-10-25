//! Select a capability, encode a setting and validate a readback without Bluetooth.

use std::error::Error;

use libmdr2::{
    command::{Notification, Request, Response},
    noise::{ChangeStatus, NoiseAvailability, NoiseKind, NoiseParameters, NoiseSwitch},
    types::CommandTable,
};

fn main() -> Result<(), Box<dyn Error>> {
    let table = CommandTable::TABLE_1;
    // All device messages in this example are synthetic payloads.
    let functions_query = Request::SupportedFunctions(table).encode()?;
    let Response::SupportedFunctions(functions) = functions_query
        .expected_response
        .decode(table, &[0x07, 0, 1, 0x6d, 0])?
    else {
        return Err("expected supported functions".into());
    };
    // This device advertises one noise layout. Multiple layouts require caller policy.
    let kind = functions
        .functions
        .iter()
        .find_map(|function| NoiseKind::from_function(function.id))
        .ok_or("no implemented noise layout advertised")?;
    let capability_query = Request::NoiseCapability(kind).encode()?;
    let Response::NoiseCapability(capability) = capability_query
        .expected_response
        .decode(table, &[0x61, 0x19, 2, 0, 1, 20, 1, 1, 1, 20, 1])?
    else {
        return Err("expected noise capability".into());
    };
    let status_query = Request::NoiseStatus(kind).encode()?;
    let Response::NoiseStatus(status) = status_query
        .expected_response
        .decode(table, &[0x63, 0x19, 0])?
    else {
        return Err("expected noise availability".into());
    };
    if status.availability != NoiseAvailability::Enabled {
        return Err("noise control is temporarily unavailable".into());
    }

    let parameters_query = Request::NoiseParameters(kind).encode()?;
    let Response::NoiseParameters(mut desired) = parameters_query
        .expected_response
        .decode(table, &[0x67, 0x19, 1, 1, 1, 1, 20, 0, 2])?
    else {
        return Err("expected noise parameters".into());
    };
    // Switch the total effect off, retaining the ambient level and adaptation fields.
    if let NoiseParameters::Control { change, effect, .. } = &mut desired {
        *change = ChangeStatus::Changed;
        *effect = NoiseSwitch::Off;
    }
    let set_payload = desired.encode_set(&capability)?;
    assert_eq!(set_payload, [0x68, 0x19, 1, 0, 1, 1, 20, 0, 2]);
    // Submit with Session::send(table, set_payload, now), then drive its events.
    // A link ACK releases link delivery; it does not confirm this state change.

    let update = [0x69, 0x19, 1, 0, 1, 1, 20, 0, 2];
    let Notification::NoiseParameters(reported) = Notification::decode(table, &update)? else {
        return Err("expected parameter notification".into());
    };
    assert_eq!(reported, desired);

    // A new, serialized read obtains state through a response descriptor.
    // Notifications are not responses, even when their parameters match.
    let readback = Request::NoiseParameters(kind).encode()?;
    let expected = readback.expected_response;
    assert!(!expected.matches(table, &update));
    let Response::NoiseParameters(confirmed) =
        expected.decode(table, &[0x67, 0x19, 1, 0, 1, 1, 20, 0, 2])?
    else {
        return Err("expected readback".into());
    };
    assert_eq!(confirmed, desired);
    println!("Encoded setting: {set_payload:02x?}; matching state read back.");
    Ok(())
}
