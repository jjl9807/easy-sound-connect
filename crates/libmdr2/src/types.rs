//! Protocol identities, kept separate from product names and display labels.

use std::fmt;

use crate::command::{CommandError, require_length};

/// A Bluetooth address carried by MDR messages as six octets in ASCII notation.
///
/// Parsing accepts uppercase or lowercase hexadecimal with colon separators;
/// encoding uses uppercase. This is a protocol identity, not an OS device handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BluetoothAddress([u8; 6]);

impl BluetoothAddress {
    /// Constructs an address in display order, without reversing its octets.
    pub const fn from_octets(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    /// Returns the six octets in display order.
    pub const fn octets(self) -> [u8; 6] {
        self.0
    }

    /// Parses exactly 17 ASCII bytes in `XX:XX:XX:XX:XX:XX` form.
    pub fn parse_ascii(bytes: &[u8]) -> Result<Self, CommandError> {
        require_length(bytes, 17)?;
        let mut octets = [0; 6];
        for (i, octet) in octets.iter_mut().enumerate() {
            if i != 0 && bytes[i * 3 - 1] != b':' {
                return Err(CommandError::InvalidValue {
                    field: "Bluetooth address separator",
                    value: bytes[i * 3 - 1],
                });
            }
            *octet = (hex_digit(bytes[i * 3])? << 4) | hex_digit(bytes[i * 3 + 1])?;
        }
        Ok(Self(octets))
    }

    /// Encodes the address as exactly 17 uppercase ASCII bytes.
    pub const fn to_ascii(self) -> [u8; 17] {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut result = [b':'; 17];
        let mut i = 0;
        while i < 6 {
            result[i * 3] = HEX[(self.0[i] >> 4) as usize];
            result[i * 3 + 1] = HEX[(self.0[i] & 15) as usize];
            i += 1;
        }
        result
    }
}

impl fmt::Display for BluetoothAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{g:02X}")
    }
}

fn hex_digit(value: u8) -> Result<u8, CommandError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        value => Err(CommandError::InvalidValue {
            field: "Bluetooth address digit",
            value,
        }),
    }
}

/// A command-table identity. Unknown values are preserved for diagnostics.
///
/// Preserving a number does not mean that its commands are implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandTable(u8);

impl CommandTable {
    /// MDR v2 command table 1.
    pub const TABLE_1: Self = Self(1);
    /// MDR v2 command table 2.
    pub const TABLE_2: Self = Self(2);

    /// Preserves a raw table number without inferring compatibility.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original table number.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// A function's identity includes its table; codes alone are not unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionId {
    /// Command table in which the function was advertised.
    pub table: CommandTable,
    /// Original function code, including codes unknown to the library.
    pub code: u8,
}

/// An uninterpreted MDR v2 protocol-version field.
///
/// Numeric ordering does not establish compatible message layouts, and the
/// highest byte must not be used to infer the MDR protocol generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProtocolVersion(u32);

impl ProtocolVersion {
    /// Preserves a version decoded from the four-byte big-endian wire field.
    pub const fn from_raw(value: u32) -> Self {
        Self(value)
    }

    /// Returns the raw version field.
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ProtocolVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:08X}", self.0)
    }
}

/// Language used by capability queries that return device display labels.
///
/// This field selects capability labels; it does not select a voice-guidance language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DisplayLanguage(u8);

impl DisplayLanguage {
    /// English device labels.
    pub const ENGLISH: Self = Self(0x01);
    /// Simplified Chinese device labels.
    pub const SIMPLIFIED_CHINESE: Self = Self(0x0c);

    /// Preserves a language code, without claiming the device supports it.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the wire language code.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}
