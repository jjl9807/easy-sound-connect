//! Device-provided MDR v2 GATT preparation values.

use crate::{Error, ErrorKind, Result};

/// Decodes characteristic c91: exactly two bytes, big endian, specifying
/// writable value bytes. This is already a payload limit, not an ATT MTU.
pub fn parse_write_limit(bytes: &[u8]) -> Result<usize> {
    let [high, low] = bytes else {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "GATT writable length must contain exactly two bytes",
        ));
    };
    let value = usize::from(u16::from_be_bytes([*high, *low]));
    if value == 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "GATT writable length cannot be zero",
        ));
    }
    Ok(value)
}

/// Decodes characteristic c93's notification. The host never writes a
/// confirmation payload: 1 means ready, and 0 reports device-side failure.
pub fn parse_mtu_status(bytes: &[u8]) -> Result<()> {
    match bytes {
        [1] => Ok(()),
        [0] => Err(Error::new(
            ErrorKind::Io,
            "device reported GATT MTU negotiation failure",
        )),
        _ => Err(Error::new(
            ErrorKind::InvalidData,
            "invalid GATT MTU determination notification",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_payload_limit_is_big_endian_and_not_reduced_again() {
        assert_eq!(parse_write_limit(&[0, 20]).unwrap(), 20);
        assert_eq!(parse_write_limit(&[1, 244]).unwrap(), 500);
        assert_eq!(parse_write_limit(&[255, 255]).unwrap(), 65535);
        for malformed in [&[][..], &[20], &[0, 0], &[0, 20, 0]] {
            assert_eq!(
                parse_write_limit(malformed).unwrap_err().kind,
                ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn mtu_notification_distinguishes_ready_device_error_and_malformed_values() {
        parse_mtu_status(&[1]).unwrap();
        assert_eq!(parse_mtu_status(&[0]).unwrap_err().kind, ErrorKind::Io);
        for malformed in [&[][..], &[2], &[1, 0]] {
            assert_eq!(
                parse_mtu_status(malformed).unwrap_err().kind,
                ErrorKind::InvalidData
            );
        }
    }
}
