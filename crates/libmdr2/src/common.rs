//! Device-reported connection, codec, processing and operating-mode information.
//!
//! These reports are read-only. A codec indicator does not configure the host
//! audio stack, and device modes do not enumerate paired audio sources.

#![doc = include_str!("../docs/common.md")]

use crate::{
    audio::UpscalingType,
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A table-1 COMMON status layout selected from advertised functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CommonKind {
    /// Left and right component connection states, inquiry `01`.
    LeftRightConnection,
    /// Device-reported audio codec identity, inquiry `02`.
    AudioCodec,
    /// Upscaling processing type and effect state, inquiry `03`.
    UpscalingEffect,
    /// A list of device operating-mode identities, inquiry `06`.
    DeviceModes,
}

impl CommonKind {
    /// Returns the inquiry byte used by status requests and reports.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::LeftRightConnection => 0x01,
            Self::AudioCodec => 0x02,
            Self::UpscalingEffect => 0x03,
            Self::DeviceModes => 0x06,
        }
    }

    /// Returns the function identity advertising this report.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::LeftRightConnection => 0x11,
                Self::AudioCodec => 0x12,
                Self::UpscalingEffect => 0x13,
                Self::DeviceModes => 0x18,
            },
        }
    }

    /// Selects a known layout from a table-aware function identity.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0x11 => Some(Self::LeftRightConnection),
            0x12 => Some(Self::AudioCodec),
            0x13 => Some(Self::UpscalingEffect),
            0x18 => Some(Self::DeviceModes),
            _ => None,
        }
    }
}

impl TryFrom<u8> for CommonKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::LeftRightConnection),
            0x02 => Ok(Self::AudioCodec),
            0x03 => Ok(Self::UpscalingEffect),
            0x06 => Ok(Self::DeviceModes),
            value => Err(CommandError::InvalidValue {
                field: "common inquiry",
                value,
            }),
        }
    }
}

/// The connection state of one earbud component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionState {
    /// The component is not connected (`00`).
    NotConnected,
    /// The component is connected (`01`).
    Connected,
}

impl TryFrom<u8> for ConnectionState {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::NotConnected),
            1 => Ok(Self::Connected),
            value => Err(CommandError::InvalidValue {
                field: "component connection",
                value,
            }),
        }
    }
}

/// An audio codec identity. Unknown identities retain their original byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioCodec(u8);

impl AudioCodec {
    /// No codec has been settled (`00`).
    pub const UNSETTLED: Self = Self(0x00);
    /// SBC (`01`).
    pub const SBC: Self = Self(0x01);
    /// AAC (`02`).
    pub const AAC: Self = Self(0x02);
    /// LDAC (`10`).
    pub const LDAC: Self = Self(0x10);
    /// aptX (`20`).
    pub const APT_X: Self = Self(0x20);
    /// aptX HD (`21`).
    pub const APT_X_HD: Self = Self(0x21);
    /// LC3 (`30`).
    pub const LC3: Self = Self(0x30);
    /// The device explicitly reports another codec (`ff`).
    pub const OTHER: Self = Self(0xff);

    /// Preserves a codec identity without inferring its format or capabilities.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original codec byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Device-reported validity of an upscaling effect.
///
/// This field is distinct from the configured DSEE auto/off setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UpscalingEffectState {
    /// Effect is off (`00`).
    Off,
    /// Effect is reported valid (`01`).
    Valid,
    /// Effect is reported invalid (`02`).
    Invalid,
}

impl TryFrom<u8> for UpscalingEffectState {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::Valid),
            2 => Ok(Self::Invalid),
            value => Err(CommandError::InvalidValue {
                field: "upscaling effect state",
                value,
            }),
        }
    }
}

/// An operating-mode identity, including modes unknown to the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceMode(u8);

impl DeviceMode {
    /// Normal operation (`00`).
    pub const NORMAL: Self = Self(0);
    /// Two-device connection mode (`01`).
    pub const TWO_DEVICES: Self = Self(1);
    /// Stereo pair (`02`).
    pub const STEREO_PAIR: Self = Self(2);
    /// USB dongle mode (`03`).
    pub const USB_DONGLE: Self = Self(3);
    /// Party Connect (`04`).
    pub const PARTY_CONNECT: Self = Self(4);
    /// Party Chain transmitter (`05`).
    pub const PARTY_CHAIN_TRANSMITTER: Self = Self(5);
    /// Party Chain receiver (`06`).
    pub const PARTY_CHAIN_RECEIVER: Self = Self(6);
    /// Auracast transmitter (`07`).
    pub const AURACAST_TRANSMITTER: Self = Self(7);
    /// Auracast receiver (`08`).
    pub const AURACAST_RECEIVER: Self = Self(8);

    /// Preserves a mode identity without providing a control operation for it.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original mode byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// A complete read-only COMMON status report.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommonStatus {
    /// Connection states, independent of battery levels.
    LeftRightConnection {
        /// Left component connection state.
        left: ConnectionState,
        /// Right component connection state.
        right: ConnectionState,
    },
    /// The codec identity reported by the device.
    AudioCodec(AudioCodec),
    /// Upscaling type and device-reported effect state.
    UpscalingEffect {
        /// Processing implementation identity, including unknown identities.
        kind: UpscalingType,
        /// The effect state, not the configured auto/off setting.
        state: UpscalingEffectState,
    },
    /// Modes in wire order, preserving duplicates and unknown identities.
    DeviceModes(Vec<DeviceMode>),
}

impl CommonStatus {
    /// Returns the report's inquiry layout.
    pub const fn kind(&self) -> CommonKind {
        match self {
            Self::LeftRightConnection { .. } => CommonKind::LeftRightConnection,
            Self::AudioCodec(_) => CommonKind::AudioCodec,
            Self::UpscalingEffect { .. } => CommonKind::UpscalingEffect,
            Self::DeviceModes(_) => CommonKind::DeviceModes,
        }
    }

    /// Decodes a table-1 `COMMON_RET_STATUS` (`13`) with an exact payload length.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x13)
    }

    /// Decodes a table-1 `COMMON_NTFY_STATUS` (`15`) independently of read replies.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x15)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(payload, 2)?;
        require_opcode(payload[0], opcode)?;
        match CommonKind::try_from(payload[1])? {
            CommonKind::LeftRightConnection => {
                require_length(payload, 4)?;
                Ok(Self::LeftRightConnection {
                    left: payload[2].try_into()?,
                    right: payload[3].try_into()?,
                })
            }
            CommonKind::AudioCodec => {
                require_length(payload, 3)?;
                Ok(Self::AudioCodec(AudioCodec::from_raw(payload[2])))
            }
            CommonKind::UpscalingEffect => {
                require_length(payload, 4)?;
                Ok(Self::UpscalingEffect {
                    kind: UpscalingType::from_raw(payload[2]),
                    state: payload[3].try_into()?,
                })
            }
            CommonKind::DeviceModes => {
                require_minimum(payload, 3)?;
                require_length(payload, 3 + usize::from(payload[2]))?;
                Ok(Self::DeviceModes(
                    payload[3..]
                        .iter()
                        .copied()
                        .map(DeviceMode::from_raw)
                        .collect(),
                ))
            }
        }
    }
}
