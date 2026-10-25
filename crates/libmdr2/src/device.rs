//! Read-only model, firmware and appearance information from MDR v2 table 1.
//!
//! These messages describe a product; they are not capability descriptors or
//! physical-device identities. Unknown appearance codes remain intact. Device
//! text is retained as raw bytes and can be interpreted as UTF-8 explicitly.

#![doc = include_str!("../docs/device-information.md")]

use std::str::Utf8Error;

use crate::command::{
    CommandError, require_length, require_minimum, require_opcode, require_table,
};
use crate::types::CommandTable;

/// Maximum model-name or firmware-version length, in bytes rather than characters.
pub const MAX_DEVICE_TEXT_LEN: usize = 128;

/// Implemented variants of `CONNECT_GET_DEVICE_INFO` and its response.
///
/// Querying one of these variants does not select a protocol compatibility path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DeviceInfoKind {
    /// Product/model name, inquiry `01`.
    ModelName,
    /// Firmware-version text, inquiry `02`.
    FirmwareVersion,
    /// Product-series and color bytes, inquiry `03`.
    SeriesAndColor,
}

impl DeviceInfoKind {
    /// Returns the inquiry byte following opcode `04` or `05`.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::ModelName => 0x01,
            Self::FirmwareVersion => 0x02,
            Self::SeriesAndColor => 0x03,
        }
    }

    fn decode(inquiry: u8) -> Result<Self, CommandError> {
        match inquiry {
            0x01 => Ok(Self::ModelName),
            0x02 => Ok(Self::FirmwareVersion),
            0x03 => Ok(Self::SeriesAndColor),
            value => Err(CommandError::InvalidValue {
                field: "device information inquiry",
                value,
            }),
        }
    }
}

/// A nonempty model-name or firmware-version field, preserved without conversion.
///
/// Decoding verifies the byte length but does not replace invalid text, trim
/// whitespace or NUL bytes, parse version components, or normalize a model name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceText(Vec<u8>);

impl DeviceText {
    /// Returns the exact reported bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Interprets the reported bytes as UTF-8 without replacement or allocation.
    ///
    /// Invalid text returns [`Utf8Error`] while the original bytes remain
    /// available through [`Self::as_bytes`]. A caller can choose its own display
    /// fallback without losing the underlying protocol data.
    pub fn as_utf8(&self) -> Result<&str, Utf8Error> {
        std::str::from_utf8(&self.0)
    }

    /// Takes ownership of the original reported bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

/// A raw product-series code that preserves unrecognized values.
///
/// Series labels describe the product family, not its control capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModelSeries(u8);

impl ModelSeries {
    /// No named series, code `00`.
    pub const NONE: Self = Self(0x00);
    /// EXTRA BASS, code `10`.
    pub const EXTRA_BASS: Self = Self(0x10);
    /// ULT POWER SOUND, code `11`.
    pub const ULT_POWER_SOUND: Self = Self(0x11);
    /// h.ear, code `20`.
    pub const HEAR: Self = Self(0x20);
    /// Premium series, code `30`.
    pub const PREMIUM: Self = Self(0x30);
    /// Sports series, code `40`.
    pub const SPORTS: Self = Self(0x40);
    /// Casual series, code `50`.
    pub const CASUAL: Self = Self(0x50);
    /// LinkBuds, code `60`.
    pub const LINK_BUDS: Self = Self(0x60);
    /// Neckband series, code `70`.
    pub const NECKBAND: Self = Self(0x70);
    /// Linkpod series, code `80`.
    pub const LINKPOD: Self = Self(0x80);
    /// Gaming series, code `90`.
    pub const GAMING: Self = Self(0x90);

    /// Preserves an original series code, including unknown values.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original series code.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// A raw product-color code; unfamiliar codes are never changed to a default.
///
/// Color is product metadata. It must not be used to infer controls or protocol
/// support. The caller maps these codes to display colors or product assets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModelColor(u8);

impl ModelColor {
    /// Preserves an original color code, including unknown values.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original color code.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Two independently preserved appearance identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeriesAndColor {
    /// Product-family byte.
    pub series: ModelSeries,
    /// Product-color byte.
    pub color: ModelColor,
}

/// A response to one implemented table-1 device-information query.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeviceInfo {
    /// The model name's original bytes.
    ModelName(DeviceText),
    /// The firmware version's original bytes.
    FirmwareVersion(DeviceText),
    /// Raw product-series and color identifiers.
    SeriesAndColor(SeriesAndColor),
}

impl DeviceInfo {
    /// Decodes a complete `05 inquiry ...` payload from command table 1.
    ///
    /// Model and firmware strings require `05 inquiry length bytes...`, with
    /// exactly `length` bytes and `1..=128` as the accepted byte-length range.
    /// Appearance requires exactly `05 03 series color`. Unsupported inquiry
    /// values, wrong tables, malformed headers, missing bytes and trailing bytes
    /// are rejected. Appearance codes and text bytes are preserved verbatim.
    ///
    /// # Examples
    ///
    /// ```
    /// use libmdr2::{device::DeviceInfo, types::CommandTable};
    ///
    /// let response = DeviceInfo::decode(CommandTable::TABLE_1, b"\x05\x01\x04Demo")?;
    /// let DeviceInfo::ModelName(name) = response else { unreachable!() };
    /// assert_eq!(name.as_utf8().unwrap(), "Demo");
    /// # Ok::<(), libmdr2::command::CommandError>(())
    /// ```
    pub fn decode(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(payload, 2)?;
        require_opcode(payload[0], 0x05)?;
        match DeviceInfoKind::decode(payload[1])? {
            DeviceInfoKind::SeriesAndColor => {
                require_length(payload, 4)?;
                Ok(Self::SeriesAndColor(SeriesAndColor {
                    series: ModelSeries::from_raw(payload[2]),
                    color: ModelColor::from_raw(payload[3]),
                }))
            }
            DeviceInfoKind::ModelName => decode_text(payload).map(Self::ModelName),
            DeviceInfoKind::FirmwareVersion => decode_text(payload).map(Self::FirmwareVersion),
        }
    }

    /// Returns the inquiry represented by this response, without inspecting text.
    pub const fn kind(&self) -> DeviceInfoKind {
        match self {
            Self::ModelName(_) => DeviceInfoKind::ModelName,
            Self::FirmwareVersion(_) => DeviceInfoKind::FirmwareVersion,
            Self::SeriesAndColor(_) => DeviceInfoKind::SeriesAndColor,
        }
    }
}

fn decode_text(payload: &[u8]) -> Result<DeviceText, CommandError> {
    require_minimum(payload, 3)?;
    let length = usize::from(payload[2]);
    if !(1..=MAX_DEVICE_TEXT_LEN).contains(&length) {
        return Err(CommandError::InvalidValue {
            field: "device text length",
            value: payload[2],
        });
    }
    require_length(payload, 3 + length)?;
    Ok(DeviceText(payload[3..].to_vec()))
}
