//! Read-only table-1 battery status messages.
//!
//! Battery levels are `POWER_GET_STATUS` / `POWER_RET_STATUS` /
//! `POWER_NTFY_STATUS` messages, not parameter messages. The six layouts cover
//! single batteries, left/right batteries and the cradle, with optional reported
//! thresholds. Select a layout from the device's advertised function identity,
//! not its model name.
//!
//! A valid level is always a percentage, including zero. Charging status
//! [`ChargingStatus::Unknown`] says nothing about the level or whether an earbud
//! is connected. These payloads contain no connection flags. Missing or stale
//! readings and separately reported component connectivity belong to the caller.
//!
//! ```
//! use libmdr2::{battery::{BatteryReadings, BatteryStatus, ChargingStatus}, types::CommandTable};
//!
//! let status = BatteryStatus::decode_response(CommandTable::TABLE_1, &[0x23, 0, 81, 0])?;
//! let BatteryReadings::Single(reading) = status.batteries else { unreachable!() };
//! assert_eq!(reading.level.as_u8(), 81);
//! assert_eq!(reading.charging, ChargingStatus::NotCharging);
//! assert_eq!(reading.threshold, None);
//! # Ok::<(), libmdr2::command::CommandError>(())
//! ```

#![doc = include_str!("../docs/battery.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// The battery layout selected by an advertised table-1 function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BatteryKind {
    /// One battery, inquiry `00`, function `20`.
    Single,
    /// Left and right batteries, inquiry `01`, function `21`.
    LeftRight,
    /// Cradle/case battery, inquiry `02`, function `22`.
    Cradle,
    /// One battery with a threshold, inquiry `08`, function `28`.
    SingleWithThreshold,
    /// Left and right batteries with thresholds, inquiry `09`, function `29`.
    LeftRightWithThreshold,
    /// Cradle/case battery with a threshold, inquiry `0a`, function `2a`.
    CradleWithThreshold,
}

impl BatteryKind {
    /// Returns the POWER inquiry byte used after opcode `22`, `23` or `25`.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Single => 0x00,
            Self::LeftRight => 0x01,
            Self::Cradle => 0x02,
            Self::SingleWithThreshold => 0x08,
            Self::LeftRightWithThreshold => 0x09,
            Self::CradleWithThreshold => 0x0a,
        }
    }

    /// Returns the table-aware function identity that advertises this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::Single => 0x20,
                Self::LeftRight => 0x21,
                Self::Cradle => 0x22,
                Self::SingleWithThreshold => 0x28,
                Self::LeftRightWithThreshold => 0x29,
                Self::CradleWithThreshold => 0x2a,
            },
        }
    }

    /// Maps a supported function to its known battery layout.
    ///
    /// Equal numeric codes on table 2 are unrelated functions and return `None`.
    /// No precedence is imposed when a device advertises multiple layouts.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0x20 => Some(Self::Single),
            0x21 => Some(Self::LeftRight),
            0x22 => Some(Self::Cradle),
            0x28 => Some(Self::SingleWithThreshold),
            0x29 => Some(Self::LeftRightWithThreshold),
            0x2a => Some(Self::CradleWithThreshold),
            _ => None,
        }
    }

    /// Whether this layout carries a threshold for each reported battery.
    pub const fn has_threshold(self) -> bool {
        matches!(
            self,
            Self::SingleWithThreshold | Self::LeftRightWithThreshold | Self::CradleWithThreshold
        )
    }

    fn payload_length(self) -> usize {
        match self {
            Self::Single | Self::Cradle => 4,
            Self::LeftRight => 6,
            Self::SingleWithThreshold | Self::CradleWithThreshold => 5,
            Self::LeftRightWithThreshold => 8,
        }
    }
}

impl TryFrom<u8> for BatteryKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Single),
            0x01 => Ok(Self::LeftRight),
            0x02 => Ok(Self::Cradle),
            0x08 => Ok(Self::SingleWithThreshold),
            0x09 => Ok(Self::LeftRightWithThreshold),
            0x0a => Ok(Self::CradleWithThreshold),
            value => Err(CommandError::InvalidValue {
                field: "battery inquiry",
                value,
            }),
        }
    }
}

/// A percentage in the inclusive range `0..=100`.
///
/// There is no implicit missing-value sentinel. In particular, `ff` is rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percentage(u8);

impl Percentage {
    /// Returns the validated percentage.
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    fn decode(value: u8, field: &'static str) -> Result<Self, CommandError> {
        if value <= 100 {
            Ok(Self(value))
        } else {
            Err(CommandError::InvalidValue { field, value })
        }
    }
}

impl TryFrom<u8> for Percentage {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::decode(value, "percentage")
    }
}

/// The independently reported battery charging state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChargingStatus {
    /// Wire `00`: not charging.
    NotCharging,
    /// Wire `01`: charging.
    Charging,
    /// Wire `02`: charging state is unknown; the percentage is still valid.
    Unknown,
    /// Wire `03`: charged. The library does not force the percentage to 100.
    Charged,
}

impl ChargingStatus {
    /// Returns the wire charging-status code.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::NotCharging => 0,
            Self::Charging => 1,
            Self::Unknown => 2,
            Self::Charged => 3,
        }
    }
}

impl TryFrom<u8> for ChargingStatus {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::NotCharging),
            1 => Ok(Self::Charging),
            2 => Ok(Self::Unknown),
            3 => Ok(Self::Charged),
            value => Err(CommandError::InvalidValue {
                field: "battery charging status",
                value,
            }),
        }
    }
}

/// One battery's reported values, independent of component connectivity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatteryReading {
    /// Reported remaining percentage, including a valid zero.
    pub level: Percentage,
    /// Charging state, independent of the level.
    pub charging: ChargingStatus,
    /// Reported threshold, present only in a threshold layout.
    ///
    /// This value is reported with the battery status. The caller defines any
    /// alert policy based on it.
    pub threshold: Option<Percentage>,
}

/// Battery endpoints carried together by one status payload.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BatteryReadings {
    /// The headset's single battery.
    Single(BatteryReading),
    /// Separate left and right readings in wire order.
    LeftRight {
        /// Left component's reading.
        left: BatteryReading,
        /// Right component's reading.
        right: BatteryReading,
    },
    /// The cradle/charging-case battery.
    Cradle(BatteryReading),
}

/// A decoded battery status body and its exact inquiry layout.
///
/// Response and notification entry points validate distinct opcodes. A received
/// notification must not be treated as a response to a pending query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatteryStatus {
    /// The layout selected by the incoming inquiry byte.
    pub kind: BatteryKind,
    /// The component readings, without inferred online/offline state.
    pub batteries: BatteryReadings,
}

impl BatteryStatus {
    /// Decodes an exact table-1 `POWER_RET_STATUS` (`23`) battery payload.
    ///
    /// Rejects wrong tables, other POWER inquiries, invalid percentages/charging
    /// codes and missing or trailing bytes. No battery model is inferred.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x23)
    }

    /// Decodes an exact table-1 `POWER_NTFY_STATUS` (`25`) battery payload.
    ///
    /// The body layouts match responses, but this method accepts only opcode
    /// `25`. The caller tracks missing readings separately from reported zeroes.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x25)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(payload, 2)?;
        require_opcode(payload[0], opcode)?;
        let kind = BatteryKind::try_from(payload[1])?;
        require_length(payload, kind.payload_length())?;

        let reading = |offset: usize, threshold_offset: Option<usize>| {
            Ok::<_, CommandError>(BatteryReading {
                level: Percentage::decode(payload[offset], "battery level")?,
                charging: ChargingStatus::try_from(payload[offset + 1])?,
                threshold: threshold_offset
                    .map(|index| Percentage::decode(payload[index], "battery threshold"))
                    .transpose()?,
            })
        };
        let batteries = match kind {
            BatteryKind::Single | BatteryKind::SingleWithThreshold => {
                BatteryReadings::Single(reading(2, kind.has_threshold().then_some(4))?)
            }
            BatteryKind::Cradle | BatteryKind::CradleWithThreshold => {
                BatteryReadings::Cradle(reading(2, kind.has_threshold().then_some(4))?)
            }
            BatteryKind::LeftRight | BatteryKind::LeftRightWithThreshold => {
                BatteryReadings::LeftRight {
                    left: reading(2, kind.has_threshold().then_some(6))?,
                    right: reading(4, kind.has_threshold().then_some(7))?,
                }
            }
        };
        Ok(Self { kind, batteries })
    }
}
