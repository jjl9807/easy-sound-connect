//! Table-2 wearing, automatic-volume and microphone-operation controls.
//!
//! Function identities include the command table. Select the layout from the
//! advertised function before querying it; inquiry numbers alone are not unique.

#![doc = include_str!("../docs/system2.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A supported table-2 system layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum System2Kind {
    /// Wearing-state reports, inquiry `00`, function `f0`.
    WearingStatus,
    /// Automatic volume optimizer, inquiry `03`, function `f3`.
    AutoVolumeOptimizer,
    /// Automatic volume with reported limitations, inquiry `04`, function `f4`.
    AutoVolumeWithLimitations,
    /// Microphone control through a headphone operation, inquiry `0b`, function `f9`.
    MicrophoneOperation,
}

impl System2Kind {
    /// Returns the inquiry byte.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::WearingStatus => 0,
            Self::AutoVolumeOptimizer => 3,
            Self::AutoVolumeWithLimitations => 4,
            Self::MicrophoneOperation => 0x0b,
        }
    }

    /// Returns the table-aware function advertising this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_2,
            code: match self {
                Self::WearingStatus => 0xf0,
                Self::AutoVolumeOptimizer => 0xf3,
                Self::AutoVolumeWithLimitations => 0xf4,
                Self::MicrophoneOperation => 0xf9,
            },
        }
    }

    /// Selects a layout from the advertised function, including its table.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_2 {
            return None;
        }
        match function.code {
            0xf0 => Some(Self::WearingStatus),
            0xf3 => Some(Self::AutoVolumeOptimizer),
            0xf4 => Some(Self::AutoVolumeWithLimitations),
            0xf9 => Some(Self::MicrophoneOperation),
            _ => None,
        }
    }

    /// Whether this layout has a dynamic `f0/f1` capability exchange.
    pub const fn has_capability_query(self) -> bool {
        matches!(self, Self::MicrophoneOperation)
    }

    /// Whether this layout has parameter reads and writes.
    pub const fn has_parameters(self) -> bool {
        !matches!(self, Self::WearingStatus)
    }
}

impl TryFrom<u8> for System2Kind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::WearingStatus),
            3 => Ok(Self::AutoVolumeOptimizer),
            4 => Ok(Self::AutoVolumeWithLimitations),
            0x0b => Ok(Self::MicrophoneOperation),
            value => Err(CommandError::InvalidValue {
                field: "table-2 system inquiry",
                value,
            }),
        }
    }
}

/// The physical control used to toggle microphone state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicrophoneKey {
    /// Touch-sensitive control (`00`).
    TouchSensor,
    /// Physical button (`01`).
    Button,
    /// Face-tap gesture (`02`).
    FaceTap,
}

/// Capability for an advertised writable table-2 system layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct System2Capability {
    kind: System2Kind,
    microphone_key: Option<MicrophoneKey>,
}

impl System2Capability {
    /// Constructs the fixed descriptor for an advertised automatic-volume function.
    ///
    /// Wearing status has no writable capability, and microphone operation requires
    /// the device's dynamic capability response.
    pub fn fixed(kind: System2Kind) -> Result<Self, CommandError> {
        if !matches!(
            kind,
            System2Kind::AutoVolumeOptimizer | System2Kind::AutoVolumeWithLimitations
        ) {
            return Err(unsupported("fixed capability", kind));
        }
        Ok(Self {
            kind,
            microphone_key: None,
        })
    }

    /// Decodes the microphone-operation capability (`f1 0b key`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0xf1)?;
        if !kind.has_capability_query() {
            return Err(unsupported("capability response", kind));
        }
        require_length(payload, 3)?;
        let microphone_key = match payload[2] {
            0 => MicrophoneKey::TouchSensor,
            1 => MicrophoneKey::Button,
            2 => MicrophoneKey::FaceTap,
            value => {
                return Err(CommandError::InvalidValue {
                    field: "microphone key",
                    value,
                });
            }
        };
        Ok(Self {
            kind,
            microphone_key: Some(microphone_key),
        })
    }

    /// Returns the inquiry layout.
    pub const fn kind(&self) -> System2Kind {
        self.kind
    }

    /// Returns the physical microphone control, when applicable.
    pub const fn microphone_key(&self) -> Option<MicrophoneKey> {
        self.microphone_key
    }
}

/// A reported wearing state, independent of automatic-pause settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WearingState {
    /// Normal wearing (`00`).
    Normal,
    /// Invalid wearing position (`01`).
    InvalidPosition,
    /// Left component not worn (`02`).
    LeftNotWorn,
    /// Right component not worn (`03`).
    RightNotWorn,
    /// Neither component worn (`04`).
    NeitherWorn,
}

/// Status content; availability and wearing state have distinct wire meanings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System2StatusValue {
    /// Current wearing state for inquiry `00`.
    Wearing(WearingState),
    /// Whether the function is currently available, not its configured switch.
    Available(bool),
}

/// A decoded table-2 system status response or notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct System2Status {
    kind: System2Kind,
    /// Wearing state or availability, selected by the inquiry layout.
    pub value: System2StatusValue,
}

impl System2Status {
    /// Returns the inquiry layout.
    pub const fn kind(self) -> System2Kind {
        self.kind
    }

    /// Decodes an `f3` read response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf3)
    }

    /// Decodes an `f5` notification.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf5)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, 3)?;
        let value = if kind == System2Kind::WearingStatus {
            System2StatusValue::Wearing(match payload[2] {
                0 => WearingState::Normal,
                1 => WearingState::InvalidPosition,
                2 => WearingState::LeftNotWorn,
                3 => WearingState::RightNotWorn,
                4 => WearingState::NeitherWorn,
                value => {
                    return Err(CommandError::InvalidValue {
                        field: "wearing state",
                        value,
                    });
                }
            })
        } else {
            System2StatusValue::Available(decode_switch(payload[2])?)
        };
        Ok(Self { kind, value })
    }
}

/// A read-only limitation identifier accompanying automatic-volume settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VolumeLimitation(u8);

impl VolumeLimitation {
    /// The device reports a safe-volume limitation (`00`).
    pub const SAFE_VOLUME: Self = Self(0);

    /// Retains a known or unknown device-supplied identifier.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original byte without discarding unknown values.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Settings reported by a table-2 system function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum System2Value {
    /// Automatic volume and its device-reported limitations.
    AutoVolume {
        /// Configured automatic-volume switch.
        enabled: bool,
        /// Read-only limitations; empty for the optimizer-only layout.
        limitations: Vec<VolumeLimitation>,
    },
    /// Microphone toggling through headphone operations and its audible feedback.
    MicrophoneOperation {
        /// Whether the headphone operation toggles the microphone.
        enabled: bool,
        /// Whether disabling the microphone produces the configured sound effect.
        mute_sound: bool,
    },
}

/// Decoded parameters with their inquiry layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct System2Parameters {
    kind: System2Kind,
    /// Reported settings and read-only metadata.
    pub value: System2Value,
}

impl System2Parameters {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> System2Kind {
        self.kind
    }

    /// Decodes an `f7` read response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf7)
    }

    /// Decodes an `f9` notification.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf9)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        if !kind.has_parameters() {
            return Err(unsupported("parameters", kind));
        }
        require_minimum(payload, 3)?;
        let enabled = decode_switch(payload[2])?;
        let value = match kind {
            System2Kind::AutoVolumeOptimizer => {
                require_length(payload, 3)?;
                System2Value::AutoVolume {
                    enabled,
                    limitations: Vec::new(),
                }
            }
            System2Kind::AutoVolumeWithLimitations => {
                require_minimum(payload, 4)?;
                require_length(payload, 4 + usize::from(payload[3]))?;
                System2Value::AutoVolume {
                    enabled,
                    limitations: payload[4..]
                        .iter()
                        .copied()
                        .map(VolumeLimitation::from_raw)
                        .collect(),
                }
            }
            System2Kind::MicrophoneOperation => {
                require_length(payload, 4)?;
                System2Value::MicrophoneOperation {
                    enabled,
                    mute_sound: decode_switch(payload[3])?,
                }
            }
            System2Kind::WearingStatus => unreachable!("parameter layout checked above"),
        };
        Ok(Self { kind, value })
    }
}

/// A complete writable setting; read-only limitations are never serialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System2Setting {
    /// Change the configured automatic-volume switch.
    AutoVolume {
        /// Desired switch state.
        enabled: bool,
    },
    /// Configure microphone operations and their sound together.
    MicrophoneOperation {
        /// Whether headphone operations toggle the microphone.
        enabled: bool,
        /// Whether microphone-off feedback is enabled.
        mute_sound: bool,
    },
}

impl System2Setting {
    /// Encodes `f8` against the capability from the current connection.
    ///
    /// Preserve the other microphone field from the latest parameters when
    /// changing one switch. Availability must be checked by the caller; an ACK
    /// confirms frame delivery, not that the setting took effect.
    pub fn encode_set(self, capability: &System2Capability) -> Result<Vec<u8>, CommandError> {
        match (self, capability.kind) {
            (
                Self::AutoVolume { enabled },
                System2Kind::AutoVolumeOptimizer | System2Kind::AutoVolumeWithLimitations,
            ) => Ok(vec![0xf8, capability.kind.inquiry(), u8::from(!enabled)]),
            (
                Self::MicrophoneOperation {
                    enabled,
                    mute_sound,
                },
                System2Kind::MicrophoneOperation,
            ) => Ok(vec![0xf8, 0x0b, u8::from(!enabled), u8::from(!mute_sound)]),
            _ => Err(CommandError::InvalidParameter {
                field: "setting does not match table-2 system capability",
            }),
        }
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<System2Kind, CommandError> {
    require_table(table, CommandTable::TABLE_2)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    System2Kind::try_from(payload[1])
}

fn decode_switch(value: u8) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue {
            field: "table-2 system switch",
            value,
        }),
    }
}

fn unsupported(operation: &'static str, kind: System2Kind) -> CommandError {
    CommandError::UnsupportedOperation {
        operation,
        inquiry: kind.inquiry(),
    }
}
