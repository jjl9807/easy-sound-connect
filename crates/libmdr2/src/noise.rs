//! Capability-selected noise cancellation and ambient sound controls.
//!
//! All messages in this module use command table 1. Decode a device's current
//! parameters, modify the required fields, and encode the complete setting
//! against its matching capability. Changing the total effect does not reset
//! the stored mode, ambient level, or adaptive sensitivity.

#![doc = include_str!("../docs/noise-control.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A noise-control layout selected by a table-1 supported function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NoiseKind {
    /// Noise cancellation on/off, inquiry `01`, function `61`.
    NcOnly,
    /// NC mode selection and ambient on/off, inquiry `12`, function `63`.
    NcModesAndAmbientSwitch,
    /// NC on/off and ambient level, inquiry `13`, function `64`.
    NcAndAmbientLevel,
    /// NC mode selection and ambient level, inquiry `14`, function `65`.
    NcModesAndAmbientLevel,
    /// NC/ambient selection with automatic NC modes, inquiry `15`, function `68`.
    AutoNcAndAmbientLevel,
    /// NC/ambient selection with dual/single NC, inquiry `16`, function `6a`.
    DualSingleNcAndAmbientLevel,
    /// NC/ambient selection with dual NC, inquiry `17`, function `6b`.
    DualNcAndAmbientLevel,
    /// Dual NC/ambient with noise adaptation, inquiry `19`, function `6d`.
    AdaptiveNcAndAmbientLevel,
    /// Ambient level without NC, inquiry `22`, function `67`.
    AmbientLevel,
    /// The modes cycled by the NC/ambient button, inquiry `30`, function `69`.
    ButtonCycle,
}

impl NoiseKind {
    /// Returns the inquiry byte shared by this layout's message families.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::NcOnly => 0x01,
            Self::NcModesAndAmbientSwitch => 0x12,
            Self::NcAndAmbientLevel => 0x13,
            Self::NcModesAndAmbientLevel => 0x14,
            Self::AutoNcAndAmbientLevel => 0x15,
            Self::DualSingleNcAndAmbientLevel => 0x16,
            Self::DualNcAndAmbientLevel => 0x17,
            Self::AdaptiveNcAndAmbientLevel => 0x19,
            Self::AmbientLevel => 0x22,
            Self::ButtonCycle => 0x30,
        }
    }

    /// Returns the supported-function identity advertising this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::NcOnly => 0x61,
                Self::NcModesAndAmbientSwitch => 0x63,
                Self::NcAndAmbientLevel => 0x64,
                Self::NcModesAndAmbientLevel => 0x65,
                Self::AutoNcAndAmbientLevel => 0x68,
                Self::DualSingleNcAndAmbientLevel => 0x6a,
                Self::DualNcAndAmbientLevel => 0x6b,
                Self::AdaptiveNcAndAmbientLevel => 0x6d,
                Self::AmbientLevel => 0x67,
                Self::ButtonCycle => 0x69,
            },
        }
    }

    /// Maps a supported function to a known layout without assigning priority.
    ///
    /// Equal numeric function codes on other tables return `None`.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0x61 => Some(Self::NcOnly),
            0x63 => Some(Self::NcModesAndAmbientSwitch),
            0x64 => Some(Self::NcAndAmbientLevel),
            0x65 => Some(Self::NcModesAndAmbientLevel),
            0x68 => Some(Self::AutoNcAndAmbientLevel),
            0x6a => Some(Self::DualSingleNcAndAmbientLevel),
            0x6b => Some(Self::DualNcAndAmbientLevel),
            0x6d => Some(Self::AdaptiveNcAndAmbientLevel),
            0x67 => Some(Self::AmbientLevel),
            0x69 => Some(Self::ButtonCycle),
            _ => None,
        }
    }

    /// Whether the layout supports a `60` capability query.
    ///
    /// NC-only control has no dynamic capability fields. Use
    /// [`NoiseCapability::nc_only`] after discovering its supported function.
    pub const fn has_capability_query(self) -> bool {
        !matches!(self, Self::NcOnly)
    }

    fn parameter_length(self) -> usize {
        match self {
            Self::NcOnly => 5,
            Self::NcModesAndAmbientSwitch
            | Self::NcAndAmbientLevel
            | Self::NcModesAndAmbientLevel
            | Self::DualNcAndAmbientLevel => 7,
            Self::AutoNcAndAmbientLevel | Self::DualSingleNcAndAmbientLevel => 8,
            Self::AdaptiveNcAndAmbientLevel => 9,
            Self::AmbientLevel => 6,
            Self::ButtonCycle => 3,
        }
    }
}

impl TryFrom<u8> for NoiseKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::NcOnly),
            0x12 => Ok(Self::NcModesAndAmbientSwitch),
            0x13 => Ok(Self::NcAndAmbientLevel),
            0x14 => Ok(Self::NcModesAndAmbientLevel),
            0x15 => Ok(Self::AutoNcAndAmbientLevel),
            0x16 => Ok(Self::DualSingleNcAndAmbientLevel),
            0x17 => Ok(Self::DualNcAndAmbientLevel),
            0x19 => Ok(Self::AdaptiveNcAndAmbientLevel),
            0x22 => Ok(Self::AmbientLevel),
            0x30 => Ok(Self::ButtonCycle),
            value => Err(CommandError::InvalidValue {
                field: "noise inquiry",
                value,
            }),
        }
    }
}

macro_rules! wire_enum {
    ($(#[$meta:meta])* $name:ident, $field:literal, { $($(#[$variant_meta:meta])* $variant:ident = $byte:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$variant_meta])* $variant = $byte),+ }

        impl $name {
            /// Returns the value encoded in the message payload.
            pub const fn as_u8(self) -> u8 { self as u8 }
        }

        impl TryFrom<u8> for $name {
            type Error = CommandError;

            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $($byte => Ok(Self::$variant),)+
                    value => Err(CommandError::InvalidValue { field: $field, value }),
                }
            }
        }
    };
}

wire_enum! {
    /// Whether a parameter change is intermediate or final.
    ChangeStatus, "noise change status", {
        /// Intermediate adjustment, such as a slider drag (`00`).
        Changing = 0,
        /// Completed adjustment (`01`).
        Changed = 1,
    }
}

wire_enum! {
    /// An NCASM switch; the wire convention is `00` off and `01` on.
    NoiseSwitch, "noise switch", {
        /// The effect or feature is off (`00`).
        Off = 0,
        /// The effect or feature is on (`01`).
        On = 1,
    }
}

wire_enum! {
    /// Current availability of a noise-control layout.
    NoiseAvailability, "noise availability", {
        /// Controls are available (`00`).
        Enabled = 0,
        /// Controls are temporarily unavailable (`01`).
        Disabled = 1,
    }
}

wire_enum! {
    /// The ambient sound processing mode.
    AmbientMode, "ambient mode", {
        /// Normal ambient sound (`00`).
        Normal = 0,
        /// Focus on voices (`01`).
        Voice = 1,
    }
}

wire_enum! {
    /// The selected effect when the total effect switch is on.
    NoiseMode, "noise mode", {
        /// Noise cancellation (`00`).
        NoiseCancelling = 0,
        /// Ambient sound (`01`).
        AmbientSound = 1,
    }
}

wire_enum! {
    /// NC processing values carried by layouts with an explicit NC field.
    NcValue, "NC value", {
        /// Noise cancellation is off (`00`).
        Off = 0,
        /// Single NC (`01`).
        Single = 1,
        /// Dual NC (`02`).
        Dual = 2,
        /// Automatic NC selection (`03`).
        Auto = 3,
        /// Automatic single NC (`04`).
        AutoSingle = 4,
        /// Automatic dual NC (`05`).
        AutoDual = 5,
    }
}

wire_enum! {
    /// Noise adaptation sensitivity in inquiry `19`.
    AdaptiveSensitivity, "noise-adaptive sensitivity", {
        /// Standard sensitivity (`00`).
        Standard = 0,
        /// High sensitivity (`01`).
        High = 1,
        /// Low sensitivity (`02`).
        Low = 2,
    }
}

wire_enum! {
    /// The physical control identified by a button-cycle capability.
    ButtonKey, "noise button key", {
        /// Left-side control (`00`).
        Left = 0,
        /// Right-side control (`01`).
        Right = 1,
        /// Custom button (`02`).
        Custom = 2,
        /// C button (`03`).
        C = 3,
        /// NC/AMB button (`04`).
        NcAmb = 4,
        /// NC/AMBIENT button (`05`).
        NcAmbient = 5,
    }
}

wire_enum! {
    /// The effects included in the NC/ambient button cycle.
    ButtonCycle, "noise button cycle", {
        /// NC, ambient sound, and off (`01`).
        NcAmbientOff = 1,
        /// NC and ambient sound (`02`).
        NcAmbient = 2,
        /// NC and off (`03`).
        NcOff = 3,
        /// Ambient sound and off (`04`).
        AmbientOff = 4,
    }
}

/// An inclusive ambient-level range and its regular increment.
///
/// The maximum is a valid terminal level even when the interval is not an
/// integer multiple of the increment. For example, `1..=6` with step `2`
/// permits `1`, `3`, `5`, and `6`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmbientLevelRange {
    minimum: u8,
    maximum: u8,
    step: u8,
}

impl AmbientLevelRange {
    /// Creates a range with `minimum < 255`, `maximum > 0`, nonzero step,
    /// and `minimum <= maximum`.
    pub fn new(minimum: u8, maximum: u8, step: u8) -> Result<Self, CommandError> {
        if minimum == u8::MAX || maximum == 0 || minimum > maximum || step == 0 {
            return Err(CommandError::InvalidCapability {
                field: "ambient level range",
            });
        }
        Ok(Self {
            minimum,
            maximum,
            step,
        })
    }

    /// Returns the lowest permitted level.
    pub const fn minimum(self) -> u8 {
        self.minimum
    }

    /// Returns the highest permitted level, including a partial terminal step.
    pub const fn maximum(self) -> u8 {
        self.maximum
    }

    /// Returns the increment between regular levels.
    pub const fn step(self) -> u8 {
        self.step
    }

    /// Whether a level is a regular step or the terminal maximum.
    pub fn contains(self, level: u8) -> bool {
        level >= self.minimum
            && level <= self.maximum
            && (level == self.maximum || (level - self.minimum).is_multiple_of(self.step))
    }
}

/// One advertised ambient mode and its optional level adjustment range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmbientCapability {
    /// The supported ambient processing mode.
    pub mode: AmbientMode,
    /// The level range; `None` means the ambient control is on/off only.
    pub levels: Option<AmbientLevelRange>,
}

/// The capability fields for one noise-control layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoiseCapability {
    kind: NoiseKind,
    ambient_modes: Vec<AmbientCapability>,
    button_key: Option<ButtonKey>,
}

impl NoiseCapability {
    /// Creates the fieldless NC-only capability after function `61` is discovered.
    ///
    /// This constructor does not query or establish device support.
    pub const fn nc_only() -> Self {
        Self {
            kind: NoiseKind::NcOnly,
            ambient_modes: Vec::new(),
            button_key: None,
        }
    }

    /// Returns the layout described by this capability.
    pub const fn kind(&self) -> NoiseKind {
        self.kind
    }

    /// Returns the advertised ambient modes in wire order.
    ///
    /// Empty lists and duplicates are preserved. A write must select exactly
    /// one entry; duplicate entries for the selected mode are ambiguous.
    pub fn ambient_modes(&self) -> &[AmbientCapability] {
        &self.ambient_modes
    }

    /// Returns the physical control for a button-cycle capability.
    pub const fn button_key(&self) -> Option<ButtonKey> {
        self.button_key
    }

    /// Decodes `NCASM_RET_CAPABILITY` (`61`) with exact length validation.
    ///
    /// The fieldless `61 01` response is accepted for payload decoding.
    /// The typed NC-only API uses a fixed descriptor and rejects capability queries.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = decode_header(table, payload, 0x61)?;
        let mut result = Self {
            kind,
            ambient_modes: Vec::new(),
            button_key: None,
        };
        match kind {
            NoiseKind::NcOnly => require_length(payload, 2)?,
            NoiseKind::ButtonCycle => {
                require_length(payload, 3)?;
                result.button_key = Some(ButtonKey::try_from(payload[2])?);
            }
            NoiseKind::NcModesAndAmbientSwitch => {
                require_minimum(payload, 3)?;
                require_length(payload, 3 + usize::from(payload[2]))?;
                for &mode in &payload[3..] {
                    result.ambient_modes.push(AmbientCapability {
                        mode: AmbientMode::try_from(mode)?,
                        levels: None,
                    });
                }
            }
            _ => {
                require_minimum(payload, 3)?;
                require_length(payload, 3 + 4 * usize::from(payload[2]))?;
                for &[mode, minimum, maximum, step] in payload[3..].as_chunks::<4>().0 {
                    result.ambient_modes.push(AmbientCapability {
                        mode: AmbientMode::try_from(mode)?,
                        levels: Some(AmbientLevelRange::new(minimum, maximum, step)?),
                    });
                }
            }
        }
        Ok(result)
    }

    fn validate_ambient(
        &self,
        mode: AmbientMode,
        level: Option<u8>,
        inactive_zero: bool,
    ) -> Result<(), CommandError> {
        let mut entries = self.ambient_modes.iter().filter(|entry| entry.mode == mode);
        let entry = entries.next().ok_or(CommandError::InvalidParameter {
            field: "unadvertised ambient mode",
        })?;
        if entries.next().is_some() {
            return Err(CommandError::InvalidCapability {
                field: "duplicate ambient mode",
            });
        }
        match (entry.levels, level) {
            (Some(range), Some(level))
                if range.contains(level) || (inactive_zero && level == 0) =>
            {
                Ok(())
            }
            (None, None) => Ok(()),
            (Some(_), Some(_)) => Err(CommandError::InvalidParameter {
                field: "ambient level",
            }),
            _ => Err(CommandError::InvalidCapability {
                field: "ambient level layout",
            }),
        }
    }
}

/// Current availability of a particular noise-control layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoiseStatus {
    kind: NoiseKind,
    /// Whether the device currently allows this control.
    pub availability: NoiseAvailability,
}

impl NoiseStatus {
    /// Returns the layout to which the availability applies.
    pub const fn kind(self) -> NoiseKind {
        self.kind
    }

    /// Decodes `NCASM_RET_STATUS` (`63`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x63)
    }

    /// Decodes `NCASM_NTFY_STATUS` (`65`).
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x65)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = decode_header(table, payload, opcode)?;
        require_length(payload, 3)?;
        Ok(Self {
            kind,
            availability: NoiseAvailability::try_from(payload[2])?,
        })
    }
}

/// Layout-specific control values following the change and total-effect fields.
///
/// Ambient levels remain unsigned bytes when decoded. Capability constraints
/// are applied by [`NoiseParameters::encode_set`], not by response decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoiseSettings {
    /// NC-only layout `01`.
    NcOnly {
        /// The NC switch independent of the total-effect field.
        nc: NoiseSwitch,
    },
    /// NC modes with ambient on/off, layout `12`.
    NcModesAndAmbientSwitch {
        /// NC processing value.
        nc: NcValue,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient on/off switch.
        ambient: NoiseSwitch,
    },
    /// NC on/off with ambient level, layout `13`.
    NcAndAmbientLevel {
        /// NC on/off switch.
        nc: NoiseSwitch,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level retained even when ambient sound is inactive.
        level: u8,
    },
    /// NC modes with ambient level, layout `14`.
    NcModesAndAmbientLevel {
        /// NC processing value.
        nc: NcValue,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
    },
    /// NC/ambient selection with automatic NC, layout `15`.
    AutoNcAndAmbientLevel {
        /// Selected effect when the total effect is on.
        mode: NoiseMode,
        /// NC processing value.
        nc: NcValue,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
    },
    /// NC/ambient selection with dual/single NC, layout `16`.
    DualSingleNcAndAmbientLevel {
        /// Selected effect when the total effect is on.
        mode: NoiseMode,
        /// NC processing value.
        nc: NcValue,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
    },
    /// NC/ambient selection with dual NC, layout `17`.
    DualNcAndAmbientLevel {
        /// Selected effect when the total effect is on.
        mode: NoiseMode,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
    },
    /// NC/ambient selection and noise adaptation, layout `19`.
    AdaptiveNcAndAmbientLevel {
        /// Selected effect when the total effect is on.
        mode: NoiseMode,
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
        /// Noise adaptation switch (`00` off, `01` on).
        adaptive: NoiseSwitch,
        /// Noise adaptation sensitivity, retained while adaptation is off.
        sensitivity: AdaptiveSensitivity,
    },
    /// Ambient-only level control, layout `22`.
    AmbientLevel {
        /// Ambient processing mode.
        ambient_mode: AmbientMode,
        /// Ambient level.
        level: u8,
    },
}

impl NoiseSettings {
    /// Returns the layout represented by these fields.
    pub const fn kind(self) -> NoiseKind {
        match self {
            Self::NcOnly { .. } => NoiseKind::NcOnly,
            Self::NcModesAndAmbientSwitch { .. } => NoiseKind::NcModesAndAmbientSwitch,
            Self::NcAndAmbientLevel { .. } => NoiseKind::NcAndAmbientLevel,
            Self::NcModesAndAmbientLevel { .. } => NoiseKind::NcModesAndAmbientLevel,
            Self::AutoNcAndAmbientLevel { .. } => NoiseKind::AutoNcAndAmbientLevel,
            Self::DualSingleNcAndAmbientLevel { .. } => NoiseKind::DualSingleNcAndAmbientLevel,
            Self::DualNcAndAmbientLevel { .. } => NoiseKind::DualNcAndAmbientLevel,
            Self::AdaptiveNcAndAmbientLevel { .. } => NoiseKind::AdaptiveNcAndAmbientLevel,
            Self::AmbientLevel { .. } => NoiseKind::AmbientLevel,
        }
    }

    fn ambient(self) -> Option<(AmbientMode, Option<u8>)> {
        match self {
            Self::NcOnly { .. } => None,
            Self::NcModesAndAmbientSwitch { ambient_mode, .. } => Some((ambient_mode, None)),
            Self::NcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::NcModesAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::AutoNcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::DualSingleNcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::DualNcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::AdaptiveNcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | Self::AmbientLevel {
                ambient_mode,
                level,
            } => Some((ambient_mode, Some(level))),
        }
    }

    fn has_inactive_ambient_zero(self) -> bool {
        matches!(
            self,
            Self::NcAndAmbientLevel {
                nc: NoiseSwitch::On,
                level: 0,
                ..
            } | Self::NcModesAndAmbientLevel {
                nc: NcValue::Single | NcValue::Dual,
                level: 0,
                ..
            }
        )
    }

    fn append_to(self, payload: &mut Vec<u8>) {
        match self {
            Self::NcOnly { nc } => payload.push(nc.as_u8()),
            Self::NcModesAndAmbientSwitch {
                nc,
                ambient_mode,
                ambient,
            } => {
                payload.extend([nc.as_u8(), ambient_mode.as_u8(), ambient.as_u8()]);
            }
            Self::NcAndAmbientLevel {
                nc,
                ambient_mode,
                level,
            } => {
                payload.extend([nc.as_u8(), ambient_mode.as_u8(), level]);
            }
            Self::NcModesAndAmbientLevel {
                nc,
                ambient_mode,
                level,
            } => {
                payload.extend([nc.as_u8(), ambient_mode.as_u8(), level]);
            }
            Self::AutoNcAndAmbientLevel {
                mode,
                nc,
                ambient_mode,
                level,
            }
            | Self::DualSingleNcAndAmbientLevel {
                mode,
                nc,
                ambient_mode,
                level,
            } => {
                payload.extend([mode.as_u8(), nc.as_u8(), ambient_mode.as_u8(), level]);
            }
            Self::DualNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
            } => {
                payload.extend([mode.as_u8(), ambient_mode.as_u8(), level]);
            }
            Self::AdaptiveNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
                adaptive,
                sensitivity,
            } => {
                payload.extend([
                    mode.as_u8(),
                    ambient_mode.as_u8(),
                    level,
                    adaptive.as_u8(),
                    sensitivity.as_u8(),
                ]);
            }
            Self::AmbientLevel {
                ambient_mode,
                level,
            } => payload.extend([ambient_mode.as_u8(), level]),
        }
    }
}

/// A complete noise-control parameter payload, without framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoiseParameters {
    /// A noise/ambient setting with the shared change and total-effect fields.
    Control {
        /// Intermediate or completed adjustment.
        change: ChangeStatus,
        /// Total effect switch; turning this off preserves all other fields.
        effect: NoiseSwitch,
        /// Layout-specific control fields.
        settings: NoiseSettings,
    },
    /// The button-cycle layout `30`, which has no shared control header.
    ButtonCycle(ButtonCycle),
}

impl NoiseParameters {
    /// Returns the layout represented by this parameter set.
    pub const fn kind(self) -> NoiseKind {
        match self {
            Self::Control { settings, .. } => settings.kind(),
            Self::ButtonCycle(_) => NoiseKind::ButtonCycle,
        }
    }

    /// Decodes `NCASM_RET_PARAM` (`67`), validating layout and closed fields.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x67)
    }

    /// Decodes `NCASM_NTFY_PARAM` (`69`) without treating it as a query response.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x69)
    }

    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = decode_header(table, p, opcode)?;
        require_length(p, kind.parameter_length())?;
        if kind == NoiseKind::ButtonCycle {
            return Ok(Self::ButtonCycle(ButtonCycle::try_from(p[2])?));
        }
        let settings = match kind {
            NoiseKind::NcOnly => NoiseSettings::NcOnly {
                nc: p[4].try_into()?,
            },
            NoiseKind::NcModesAndAmbientSwitch => NoiseSettings::NcModesAndAmbientSwitch {
                nc: p[4].try_into()?,
                ambient_mode: p[5].try_into()?,
                ambient: p[6].try_into()?,
            },
            NoiseKind::NcAndAmbientLevel => NoiseSettings::NcAndAmbientLevel {
                nc: p[4].try_into()?,
                ambient_mode: p[5].try_into()?,
                level: p[6],
            },
            NoiseKind::NcModesAndAmbientLevel => NoiseSettings::NcModesAndAmbientLevel {
                nc: p[4].try_into()?,
                ambient_mode: p[5].try_into()?,
                level: p[6],
            },
            NoiseKind::AutoNcAndAmbientLevel => NoiseSettings::AutoNcAndAmbientLevel {
                mode: p[4].try_into()?,
                nc: p[5].try_into()?,
                ambient_mode: p[6].try_into()?,
                level: p[7],
            },
            NoiseKind::DualSingleNcAndAmbientLevel => NoiseSettings::DualSingleNcAndAmbientLevel {
                mode: p[4].try_into()?,
                nc: p[5].try_into()?,
                ambient_mode: p[6].try_into()?,
                level: p[7],
            },
            NoiseKind::DualNcAndAmbientLevel => NoiseSettings::DualNcAndAmbientLevel {
                mode: p[4].try_into()?,
                ambient_mode: p[5].try_into()?,
                level: p[6],
            },
            NoiseKind::AdaptiveNcAndAmbientLevel => NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: p[4].try_into()?,
                ambient_mode: p[5].try_into()?,
                level: p[6],
                adaptive: p[7].try_into()?,
                sensitivity: p[8].try_into()?,
            },
            NoiseKind::AmbientLevel => NoiseSettings::AmbientLevel {
                ambient_mode: p[4].try_into()?,
                level: p[5],
            },
            NoiseKind::ButtonCycle => unreachable!(),
        };
        Ok(Self::Control {
            change: p[2].try_into()?,
            effect: p[3].try_into()?,
            settings,
        })
    }

    /// Encodes `NCASM_SET_PARAM` (`68`) using the matching capability.
    ///
    /// The selected ambient mode must have exactly one advertised entry. Every
    /// level is checked against that entry's minimum, maximum, and step. The
    /// inactive ambient value `0` is also permitted in layout `13` with NC on,
    /// and layout `14` with single or dual NC. These rules also apply when the
    /// total effect is off. No field is clamped or reset. A caller
    /// should modify a recently decoded complete parameter set to retain
    /// unrelated settings.
    ///
    /// The caller must separately check current availability and device support.
    /// The result is a table-1 command payload; it supplies neither a transport
    /// frame nor an expected response. A link ACK only acknowledges delivery.
    /// A notification reports state without a transaction identifier; use a
    /// subsequent parameter read when the application requires a fresh readback.
    pub fn encode_set(self, capability: &NoiseCapability) -> Result<Vec<u8>, CommandError> {
        let kind = self.kind();
        if kind != capability.kind {
            return Err(CommandError::CapabilityMismatch {
                expected: kind.inquiry(),
                actual: capability.kind.inquiry(),
            });
        }
        let mut payload = Vec::with_capacity(kind.parameter_length());
        payload.extend([0x68, kind.inquiry()]);
        match self {
            Self::ButtonCycle(cycle) => payload.push(cycle.as_u8()),
            Self::Control {
                change,
                effect,
                settings,
            } => {
                if let Some((mode, level)) = settings.ambient() {
                    capability.validate_ambient(
                        mode,
                        level,
                        settings.has_inactive_ambient_zero(),
                    )?;
                }
                payload.extend([change.as_u8(), effect.as_u8()]);
                settings.append_to(&mut payload);
            }
        }
        Ok(payload)
    }
}

fn decode_header(
    table: CommandTable,
    payload: &[u8],
    opcode: u8,
) -> Result<NoiseKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    NoiseKind::try_from(payload[1])
}
