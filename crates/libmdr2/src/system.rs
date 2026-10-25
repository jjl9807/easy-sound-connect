//! Speak-to-Chat and local system controls selected by advertised functions.
//!
//! Availability, the configured switch and the active talking effect are
//! separate state. Encode changes against the capability from the same device
//! connection and reconcile the resulting state through notifications or reads.

#![doc = include_str!("../docs/system.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A supported table-1 system layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SystemKind {
    /// Call vibration, inquiry `00`, function `f0`.
    Vibrator,
    /// Automatic playback control when wearing/removing, inquiry `01`, function `f1`.
    PlaybackControlByWearing,
    /// Speak-to-Chat with voice focus, inquiry `02`, function `f2`.
    SpeakToChatType1,
    /// Automatic volume, inquiry `0a`, function `fa`.
    AutoVolume,
    /// Speak-to-Chat without a voice-focus field, inquiry `0c`, function `fc`.
    SpeakToChatType2,
    /// Head-gesture control switch, inquiry `0f`, function `ff`.
    HeadGesture,
}

impl SystemKind {
    /// Returns the inquiry byte shared by the layout's commands.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Vibrator => 0x00,
            Self::PlaybackControlByWearing => 0x01,
            Self::SpeakToChatType1 => 0x02,
            Self::AutoVolume => 0x0a,
            Self::SpeakToChatType2 => 0x0c,
            Self::HeadGesture => 0x0f,
        }
    }

    /// Returns the function identity advertising this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: 0xf0 | self.inquiry(),
        }
    }

    /// Selects a supported layout from a table-aware function identity.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0xf0 => Some(Self::Vibrator),
            0xf1 => Some(Self::PlaybackControlByWearing),
            0xf2 => Some(Self::SpeakToChatType1),
            0xfa => Some(Self::AutoVolume),
            0xfc => Some(Self::SpeakToChatType2),
            0xff => Some(Self::HeadGesture),
            _ => None,
        }
    }

    /// Whether this layout has a dynamic `f0` capability query.
    pub const fn has_capability_query(self) -> bool {
        matches!(self, Self::SpeakToChatType1 | Self::SpeakToChatType2)
    }

    /// Whether this layout has typed `fa/fb/fc/fd` extended parameters.
    pub const fn has_extended_parameters(self) -> bool {
        matches!(self, Self::SpeakToChatType1 | Self::SpeakToChatType2)
    }
}

impl TryFrom<u8> for SystemKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Vibrator),
            0x01 => Ok(Self::PlaybackControlByWearing),
            0x02 => Ok(Self::SpeakToChatType1),
            0x0a => Ok(Self::AutoVolume),
            0x0c => Ok(Self::SpeakToChatType2),
            0x0f => Ok(Self::HeadGesture),
            value => Err(CommandError::InvalidValue {
                field: "system inquiry",
                value,
            }),
        }
    }
}

/// Device-supplied preview support and values for the three automatic end modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeakToChatCapability {
    /// Whether the preview switch may be turned on.
    pub preview_supported: bool,
    /// Raw values for FAST, MID and SLOW, in that order.
    ///
    /// These values are metadata, not the bytes sent to select an end mode.
    /// No unit, nonzero lower bound or ordering is imposed by the decoder.
    pub mode_out_values: [u8; 3],
}

impl SpeakToChatCapability {
    /// Returns the advertised value for an automatic end mode.
    ///
    /// [`ModeOutTime::None`] has no corresponding capability value.
    pub const fn mode_out_value(self, mode: ModeOutTime) -> Option<u8> {
        match mode {
            ModeOutTime::Fast => Some(self.mode_out_values[0]),
            ModeOutTime::Mid => Some(self.mode_out_values[1]),
            ModeOutTime::Slow => Some(self.mode_out_values[2]),
            ModeOutTime::None => None,
        }
    }
}

/// A system capability associated with exactly one inquiry layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemCapability {
    kind: SystemKind,
    speak_to_chat: Option<SpeakToChatCapability>,
}

impl SystemCapability {
    /// Constructs the fixed descriptor for a discovered local toggle.
    ///
    /// These layouts have no dynamic capability query. Speak-to-Chat requires
    /// its actual capability response and is rejected here. The caller must
    /// first verify that the device advertises `kind.function_id()`.
    pub fn fixed(kind: SystemKind) -> Result<Self, CommandError> {
        if kind.has_capability_query() {
            return Err(CommandError::UnsupportedOperation {
                operation: "fixed system capability",
                inquiry: kind.inquiry(),
            });
        }
        Ok(Self {
            kind,
            speak_to_chat: None,
        })
    }

    /// Decodes an exact Speak-to-Chat `SYSTEM_RET_CAPABILITY` (`f1`) payload.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0xf1)?;
        require_speak_to_chat(kind, "system capability response")?;
        require_length(payload, 6)?;
        Ok(Self {
            kind,
            speak_to_chat: Some(SpeakToChatCapability {
                preview_supported: decode_bool(payload[2], 1, "Speak-to-Chat preview support")?,
                mode_out_values: [payload[3], payload[4], payload[5]],
            }),
        })
    }

    /// Returns the inquiry layout to which the descriptor belongs.
    pub const fn kind(self) -> SystemKind {
        self.kind
    }

    /// Returns dynamic Speak-to-Chat fields, absent for fixed local toggles.
    pub const fn speak_to_chat(&self) -> Option<&SpeakToChatCapability> {
        self.speak_to_chat.as_ref()
    }
}

/// Availability and, for Speak-to-Chat, whether the effect is currently active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemStatus {
    kind: SystemKind,
    /// Whether this control can currently be operated (`00` available).
    pub enabled: bool,
    /// Whether Speak-to-Chat is currently active (`01` active).
    ///
    /// Present only for Speak-to-Chat; this is independent of its configured switch.
    pub talking: Option<bool>,
}

impl SystemStatus {
    /// Returns the layout described by this status.
    pub const fn kind(self) -> SystemKind {
        self.kind
    }

    /// Decodes `SYSTEM_RET_STATUS` (`f3`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf3)
    }

    /// Decodes `SYSTEM_NTFY_STATUS` (`f5`) separately from query responses.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf5)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, if kind.has_capability_query() { 4 } else { 3 })?;
        Ok(Self {
            kind,
            enabled: decode_bool(payload[2], 0, "system availability")?,
            talking: if kind.has_capability_query() {
                Some(decode_bool(payload[3], 1, "Speak-to-Chat effect")?)
            } else {
                None
            },
        })
    }
}

/// Complete switch parameters for a supported system control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemParameters {
    /// Call vibration switch.
    Vibrator {
        /// Whether vibration is on.
        enabled: bool,
    },
    /// Automatic playback control by wearing/removing.
    PlaybackControlByWearing {
        /// Whether wearing-based playback control is on.
        enabled: bool,
    },
    /// Speak-to-Chat type 1 switches.
    SpeakToChatType1 {
        /// Whether Speak-to-Chat is on.
        enabled: bool,
        /// Whether preview mode is on.
        preview: bool,
    },
    /// Automatic-volume switch.
    AutoVolume {
        /// Whether automatic volume is on.
        enabled: bool,
    },
    /// Speak-to-Chat type 2 switches.
    SpeakToChatType2 {
        /// Whether Speak-to-Chat is on.
        enabled: bool,
        /// Whether preview mode is on.
        preview: bool,
    },
    /// Head-gesture control switch, independent of its training workflow.
    HeadGesture {
        /// Whether gesture control is on.
        enabled: bool,
    },
}

impl SystemParameters {
    /// Returns the inquiry represented by these fields.
    pub const fn kind(self) -> SystemKind {
        match self {
            Self::Vibrator { .. } => SystemKind::Vibrator,
            Self::PlaybackControlByWearing { .. } => SystemKind::PlaybackControlByWearing,
            Self::SpeakToChatType1 { .. } => SystemKind::SpeakToChatType1,
            Self::AutoVolume { .. } => SystemKind::AutoVolume,
            Self::SpeakToChatType2 { .. } => SystemKind::SpeakToChatType2,
            Self::HeadGesture { .. } => SystemKind::HeadGesture,
        }
    }

    /// Returns the configured switch value, not current availability.
    pub const fn enabled(self) -> bool {
        match self {
            Self::Vibrator { enabled }
            | Self::PlaybackControlByWearing { enabled }
            | Self::SpeakToChatType1 { enabled, .. }
            | Self::AutoVolume { enabled }
            | Self::SpeakToChatType2 { enabled, .. }
            | Self::HeadGesture { enabled } => enabled,
        }
    }

    /// Returns the preview switch for Speak-to-Chat, absent for other layouts.
    pub const fn preview(self) -> Option<bool> {
        match self {
            Self::SpeakToChatType1 { preview, .. } | Self::SpeakToChatType2 { preview, .. } => {
                Some(preview)
            }
            _ => None,
        }
    }

    /// Decodes `SYSTEM_RET_PARAM` (`f7`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf7)
    }

    /// Decodes `SYSTEM_NTFY_PARAM` (`f9`).
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf9)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, if kind.has_capability_query() { 4 } else { 3 })?;
        let enabled = decode_bool(payload[2], 0, "system on/off")?;
        Ok(match kind {
            SystemKind::Vibrator => Self::Vibrator { enabled },
            SystemKind::PlaybackControlByWearing => Self::PlaybackControlByWearing { enabled },
            SystemKind::AutoVolume => Self::AutoVolume { enabled },
            SystemKind::HeadGesture => Self::HeadGesture { enabled },
            SystemKind::SpeakToChatType1 => Self::SpeakToChatType1 {
                enabled,
                preview: decode_bool(payload[3], 0, "Speak-to-Chat preview switch")?,
            },
            SystemKind::SpeakToChatType2 => Self::SpeakToChatType2 {
                enabled,
                preview: decode_bool(payload[3], 0, "Speak-to-Chat preview switch")?,
            },
        })
    }

    /// Encodes `SYSTEM_SET_PARAM` (`f8`) against the matching capability.
    ///
    /// Turning preview on requires advertised preview support. Both
    /// Speak-to-Chat switches are encoded; changing one must preserve the other.
    /// The caller separately checks availability and device support. The result
    /// is a table-1 payload, without a frame or dedicated response descriptor.
    pub fn encode_set(self, capability: &SystemCapability) -> Result<Vec<u8>, CommandError> {
        require_capability(self.kind(), capability)?;
        if self.preview() == Some(true)
            && !capability
                .speak_to_chat
                .is_some_and(|value| value.preview_supported)
        {
            return Err(CommandError::InvalidParameter {
                field: "unsupported Speak-to-Chat preview",
            });
        }
        let mut payload = vec![0xf8, self.kind().inquiry(), u8::from(!self.enabled())];
        if let Some(preview) = self.preview() {
            payload.push(u8::from(!preview));
        }
        Ok(payload)
    }
}

/// Sensitivity of Speak-to-Chat speech detection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetectionSensitivity {
    /// Automatic sensitivity (`00`).
    Auto,
    /// High sensitivity (`01`).
    High,
    /// Low sensitivity (`02`).
    Low,
}

impl DetectionSensitivity {
    /// Returns the wire value.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::High => 1,
            Self::Low => 2,
        }
    }
}

impl TryFrom<u8> for DetectionSensitivity {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Auto),
            1 => Ok(Self::High),
            2 => Ok(Self::Low),
            value => Err(CommandError::InvalidValue {
                field: "Speak-to-Chat sensitivity",
                value,
            }),
        }
    }
}

/// The selected automatic end mode, distinct from its device-supplied value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModeOutTime {
    /// FAST mode (`00`).
    Fast,
    /// MID mode (`01`).
    Mid,
    /// SLOW mode (`02`).
    Slow,
    /// No automatic end (`03`).
    None,
}

impl ModeOutTime {
    /// Returns the selector byte, not the corresponding capability value.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Fast => 0,
            Self::Mid => 1,
            Self::Slow => 2,
            Self::None => 3,
        }
    }
}

impl TryFrom<u8> for ModeOutTime {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Fast),
            1 => Ok(Self::Mid),
            2 => Ok(Self::Slow),
            3 => Ok(Self::None),
            value => Err(CommandError::InvalidValue {
                field: "Speak-to-Chat end mode",
                value,
            }),
        }
    }
}

/// Variant-specific Speak-to-Chat extended settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SpeakToChatSettings {
    /// Type 1 includes a voice-focus switch.
    Type1 {
        /// Speech detection sensitivity.
        sensitivity: DetectionSensitivity,
        /// Whether voice focus is on.
        voice_focus: bool,
        /// Automatic end mode.
        mode_out: ModeOutTime,
    },
    /// Type 2 has no voice-focus field.
    Type2 {
        /// Speech detection sensitivity.
        sensitivity: DetectionSensitivity,
        /// Automatic end mode.
        mode_out: ModeOutTime,
    },
}

impl SpeakToChatSettings {
    /// Returns the exact Speak-to-Chat layout.
    pub const fn kind(self) -> SystemKind {
        match self {
            Self::Type1 { .. } => SystemKind::SpeakToChatType1,
            Self::Type2 { .. } => SystemKind::SpeakToChatType2,
        }
    }

    /// Decodes `SYSTEM_RET_EXT_PARAM` (`fb`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xfb)
    }

    /// Decodes `SYSTEM_NTFY_EXT_PARAM` (`fd`).
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xfd)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_speak_to_chat(kind, "system extended parameters")?;
        match kind {
            SystemKind::SpeakToChatType1 => {
                require_length(payload, 5)?;
                Ok(Self::Type1 {
                    sensitivity: payload[2].try_into()?,
                    voice_focus: decode_bool(payload[3], 0, "Speak-to-Chat voice focus")?,
                    mode_out: payload[4].try_into()?,
                })
            }
            SystemKind::SpeakToChatType2 => {
                require_length(payload, 4)?;
                Ok(Self::Type2 {
                    sensitivity: payload[2].try_into()?,
                    mode_out: payload[3].try_into()?,
                })
            }
            _ => unreachable!(),
        }
    }

    /// Encodes `SYSTEM_SET_EXT_PARAM` (`fc`) for the matching capability.
    ///
    /// The complete variant is encoded without clamping or resetting fields.
    /// Mode selectors remain `00..=03`, regardless of the raw timing metadata.
    /// A link ACK confirms frame delivery only. Check extended-parameter reports
    /// separately from ordinary switch parameters to confirm the resulting state.
    pub fn encode_set(self, capability: &SystemCapability) -> Result<Vec<u8>, CommandError> {
        require_capability(self.kind(), capability)?;
        Ok(match self {
            Self::Type1 {
                sensitivity,
                voice_focus,
                mode_out,
            } => vec![
                0xfc,
                0x02,
                sensitivity.as_u8(),
                u8::from(!voice_focus),
                mode_out.as_u8(),
            ],
            Self::Type2 {
                sensitivity,
                mode_out,
            } => vec![0xfc, 0x0c, sensitivity.as_u8(), mode_out.as_u8()],
        })
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<SystemKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    SystemKind::try_from(payload[1])
}

fn require_capability(kind: SystemKind, capability: &SystemCapability) -> Result<(), CommandError> {
    if kind == capability.kind {
        Ok(())
    } else {
        Err(CommandError::CapabilityMismatch {
            expected: kind.inquiry(),
            actual: capability.kind.inquiry(),
        })
    }
}

fn require_speak_to_chat(kind: SystemKind, operation: &'static str) -> Result<(), CommandError> {
    if kind.has_extended_parameters() {
        Ok(())
    } else {
        Err(CommandError::UnsupportedOperation {
            operation,
            inquiry: kind.inquiry(),
        })
    }
}

fn decode_bool(value: u8, true_value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 | 1 => Ok(value == true_value),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}
