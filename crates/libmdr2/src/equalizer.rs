//! Capability-driven table-1 equalizer messages.
//!
//! [`EqualizerKind`] selects the wire layout from an advertised function. Band
//! counts, preset lists and level ranges come from [`EqualizerCapability`].
//! Levels use the device's native unsigned steps; they are not decibels.
//!
//! [`EqualizerSetting`] encodes writes after checking a capability. A successful
//! encoding or link ACK does not confirm the device's resulting configuration.

#![doc = include_str!("../docs/equalizer.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A supported equalizer inquiry layout in MDR v2 table 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EqualizerKind {
    /// Presets and editable curves: inquiry `00`, function `50`.
    Preset,
    /// Preset selection without curve editing: inquiry `02`, function `52`.
    PresetNonCustomizable,
    /// Presets, editable curves and ULT mode: inquiry `03`, function `53`.
    PresetWithUlt,
    /// Presets and editable curves with status error codes: inquiry `04`, function `57`.
    PresetWithErrorCodes,
    /// A standalone editable curve without a preset: inquiry `31`, function `55`.
    Custom,
}

impl EqualizerKind {
    /// Returns the inquiry byte following the EQEBB opcode.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Preset => 0,
            Self::PresetNonCustomizable => 2,
            Self::PresetWithUlt => 3,
            Self::PresetWithErrorCodes => 4,
            Self::Custom => 0x31,
        }
    }

    /// Returns the table-aware function that advertises this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::Preset => 0x50,
                Self::PresetNonCustomizable => 0x52,
                Self::PresetWithUlt => 0x53,
                Self::PresetWithErrorCodes => 0x57,
                Self::Custom => 0x55,
            },
        }
    }

    /// Maps an advertised function to its layout without imposing precedence.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0x50 => Some(Self::Preset),
            0x52 => Some(Self::PresetNonCustomizable),
            0x53 => Some(Self::PresetWithUlt),
            0x57 => Some(Self::PresetWithErrorCodes),
            0x55 => Some(Self::Custom),
            _ => None,
        }
    }
}

impl TryFrom<u8> for EqualizerKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Preset),
            2 => Ok(Self::PresetNonCustomizable),
            3 => Ok(Self::PresetWithUlt),
            4 => Ok(Self::PresetWithErrorCodes),
            0x31 => Ok(Self::Custom),
            value => Err(CommandError::InvalidValue {
                field: "equalizer inquiry",
                value,
            }),
        }
    }
}

/// A preset identity, including identities not named by this library.
///
/// A constant does not imply that a device advertises the preset. Selection
/// requires membership in its capability's preset list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresetId(u8);

impl PresetId {
    /// Equalizer off (`00`).
    pub const OFF: Self = Self(0);
    /// Rock (`01`).
    pub const ROCK: Self = Self(1);
    /// Bright (`10`).
    pub const BRIGHT: Self = Self(0x10);
    /// Heavy (`30`).
    pub const HEAVY: Self = Self(0x30);
    /// Clear (`31`).
    pub const CLEAR: Self = Self(0x31);
    /// Hard (`32`).
    pub const HARD: Self = Self(0x32);
    /// Soft (`33`).
    pub const SOFT: Self = Self(0x33);
    /// Custom curve (`a0`).
    pub const CUSTOM: Self = Self(0xa0);
    /// First user curve (`a1`).
    pub const USER_1: Self = Self(0xa1);
    /// Second user curve (`a2`).
    pub const USER_2: Self = Self(0xa2);
    /// Third user curve (`a3`).
    pub const USER_3: Self = Self(0xa3);
    /// Fourth user curve (`a4`).
    pub const USER_4: Self = Self(0xa4);
    /// Fifth user curve (`a5`).
    pub const USER_5: Self = Self(0xa5);

    /// Preserves a preset byte without inferring device support.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original wire identity.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Whether this identity is a known editable custom/user slot (`a0..=a5`).
    pub const fn is_custom_slot(self) -> bool {
        matches!(self.0, 0xa0..=0xa5)
    }
}

/// A capability's preset identity and device-supplied label bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Original identity, including unknown values.
    pub id: PresetId,
    name: Vec<u8>,
}

impl Preset {
    /// Returns the exact label bytes; an empty label is valid.
    pub fn name_bytes(&self) -> &[u8] {
        &self.name
    }

    /// Interprets the label as UTF-8 without replacement, trimming or fallback.
    pub fn name_utf8(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.name)
    }
}

/// The decoded shape and selectable presets of one equalizer function.
///
/// Zero counts and zero level steps are retained as wire data. Curve writes
/// require a nonzero band count and a nonzero number of level steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EqualizerCapability {
    kind: EqualizerKind,
    band_count: u8,
    level_steps: u8,
    ult2_additional_step: Option<u8>,
    presets: Vec<Preset>,
}

impl EqualizerCapability {
    /// Decodes an exact table-1 `EQEBB_RET_CAPABILITY` (`51`) payload.
    ///
    /// Each preset label is limited to 128 bytes. Counts, record lengths and
    /// total length are checked; unknown preset IDs and invalid UTF-8 bytes are
    /// preserved. UTF-8 interpretation is available through [`Preset::name_utf8`].
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x51)?;
        let fixed = match kind {
            EqualizerKind::Custom => 4,
            EqualizerKind::PresetWithUlt => 6,
            _ => 5,
        };
        require_minimum(payload, fixed)?;
        let mut presets = Vec::new();
        let mut offset = fixed;
        if kind != EqualizerKind::Custom {
            let count = payload[fixed - 1];
            for _ in 0..count {
                require_minimum(payload, offset + 2)?;
                let name_length = payload[offset + 1];
                if name_length > 128 {
                    return Err(CommandError::InvalidValue {
                        field: "equalizer preset name length",
                        value: name_length,
                    });
                }
                let end = offset + 2 + usize::from(name_length);
                require_minimum(payload, end)?;
                presets.push(Preset {
                    id: PresetId::from_raw(payload[offset]),
                    name: payload[offset + 2..end].to_vec(),
                });
                offset = end;
            }
        }
        require_length(payload, offset)?;
        Ok(Self {
            kind,
            band_count: payload[2],
            level_steps: payload[3],
            ult2_additional_step: (kind == EqualizerKind::PresetWithUlt).then(|| payload[4]),
            presets,
        })
    }

    /// Returns the negotiated inquiry layout.
    pub const fn kind(&self) -> EqualizerKind {
        self.kind
    }

    /// Returns the advertised number of native band values.
    pub const fn band_count(&self) -> u8 {
        self.band_count
    }

    /// Returns the advertised number of steps; writable values are `0..level_steps`.
    pub const fn level_steps(&self) -> u8 {
        self.level_steps
    }

    /// Returns the ULT 2 display adjustment, present only for inquiry `03`.
    ///
    /// This value does not increase the range accepted for native curve writes.
    pub const fn ult2_additional_step(&self) -> Option<u8> {
        self.ult2_additional_step
    }

    /// Returns presets in wire order, preserving duplicate and unknown identities.
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// Whether the device advertised a selectable preset identity.
    pub fn supports_preset(&self, id: PresetId) -> bool {
        self.presets.iter().any(|preset| preset.id == id)
    }

    fn validate_levels(&self, levels: &[u8]) -> Result<(), CommandError> {
        if self.band_count == 0 {
            return Err(CommandError::InvalidCapability {
                field: "equalizer band count",
            });
        }
        if self.level_steps == 0 {
            return Err(CommandError::InvalidCapability {
                field: "equalizer level steps",
            });
        }
        require_length(levels, usize::from(self.band_count))?;
        for &value in levels {
            if value >= self.level_steps {
                return Err(CommandError::InvalidValue {
                    field: "equalizer band level",
                    value,
                });
            }
        }
        Ok(())
    }
}

/// The selected ULT processing mode carried by inquiry `03`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UltMode {
    /// ULT processing off (`00`).
    Off,
    /// ULT 1 (`01`).
    Ult1,
    /// ULT 2 (`02`).
    Ult2,
}

impl UltMode {
    /// Returns the mode's wire byte.
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Ult1 => 1,
            Self::Ult2 => 2,
        }
    }
}

impl TryFrom<u8> for UltMode {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Off),
            1 => Ok(Self::Ult1),
            2 => Ok(Self::Ult2),
            value => Err(CommandError::InvalidValue {
                field: "equalizer ULT mode",
                value,
            }),
        }
    }
}

/// An equalizer disable-reason code, retaining unrecognized values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EqualizerErrorCode(u8);

impl EqualizerErrorCode {
    /// A call is active (`00`).
    pub const CALLING: Self = Self(0);
    /// Demo mode is active (`01`).
    pub const DEMO_MODE: Self = Self(1);
    /// The current listening mode restricts the equalizer (`02`).
    pub const LISTENING_MODE: Self = Self(2);
    /// Another reason (`fe`).
    pub const OTHER: Self = Self(0xfe);

    /// Preserves the original reason byte.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original reason byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Whether an equalizer function can currently be operated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EqualizerStatus {
    kind: EqualizerKind,
    /// `true` for wire `00` (available), `false` for `01` (unavailable).
    ///
    /// This is availability, independent of whether the active preset is OFF.
    pub enabled: bool,
    /// Reasons carried by inquiry `04`, preserving wire order and unknown codes.
    /// Other inquiry layouts have an empty list.
    pub errors: Vec<EqualizerErrorCode>,
}

impl EqualizerStatus {
    /// Decodes `EQEBB_RET_STATUS` (`53`) with exact length and value checks.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x53)
    }

    /// Decodes `EQEBB_NTFY_STATUS` (`55`), rejecting response opcodes.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x55)
    }

    /// Returns the inquiry layout.
    pub const fn kind(&self) -> EqualizerKind {
        self.kind
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_minimum(payload, 3)?;
        let enabled = match payload[2] {
            0 => true,
            1 => false,
            value => {
                return Err(CommandError::InvalidValue {
                    field: "equalizer availability",
                    value,
                });
            }
        };
        let errors = if kind == EqualizerKind::PresetWithErrorCodes {
            require_minimum(payload, 4)?;
            require_length(payload, 4 + usize::from(payload[3]))?;
            payload[4..]
                .iter()
                .copied()
                .map(EqualizerErrorCode::from_raw)
                .collect()
        } else {
            require_length(payload, 3)?;
            Vec::new()
        };
        Ok(Self {
            kind,
            enabled,
            errors,
        })
    }
}

/// A device-reported curve and, where present, its preset and ULT mode.
///
/// Reading does not require a capability and retains native band values, even
/// outside a previously reported range. [`Self::validate_capability`] performs
/// the optional cross-message shape and range checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EqualizerParameters {
    kind: EqualizerKind,
    preset: Option<PresetId>,
    ult_mode: Option<UltMode>,
    levels: Vec<u8>,
}

impl EqualizerParameters {
    /// Decodes an exact `EQEBB_RET_PARAM` (`57`) payload.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x57)
    }

    /// Decodes an exact `EQEBB_NTFY_PARAM` (`59`) payload.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x59)
    }

    /// Returns the inquiry layout.
    pub const fn kind(&self) -> EqualizerKind {
        self.kind
    }

    /// Returns the original preset ID; standalone custom EQ has no preset.
    pub const fn preset(&self) -> Option<PresetId> {
        self.preset
    }

    /// Returns the ULT mode, present only for inquiry `03`.
    pub const fn ult_mode(&self) -> Option<UltMode> {
        self.ult_mode
    }

    /// Returns native unsigned levels in device band order.
    pub fn levels(&self) -> &[u8] {
        &self.levels
    }

    /// Checks inquiry identity, band count, level range and preset membership.
    ///
    /// This check does not establish that the capability is current or belongs
    /// to the same connection. The caller owns that association.
    pub fn validate_capability(
        &self,
        capability: &EqualizerCapability,
    ) -> Result<(), CommandError> {
        same_kind(self.kind, capability.kind)?;
        capability.validate_levels(&self.levels)?;
        if let Some(preset) = self.preset {
            require_preset(capability, preset)?;
        }
        Ok(())
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        let fixed = match kind {
            EqualizerKind::Custom => 3,
            EqualizerKind::PresetWithUlt => 5,
            _ => 4,
        };
        require_minimum(payload, fixed)?;
        require_length(payload, fixed + usize::from(payload[fixed - 1]))?;
        Ok(Self {
            kind,
            preset: (kind != EqualizerKind::Custom).then(|| PresetId::from_raw(payload[2])),
            ult_mode: if kind == EqualizerKind::PresetWithUlt {
                Some(UltMode::try_from(payload[3])?)
            } else {
                None
            },
            levels: payload[fixed..].to_vec(),
        })
    }
}

/// A band-information type, including types unknown to this library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BandInformationType(u8);

impl BandInformationType {
    /// No frequency information (`00`).
    pub const NONE: Self = Self(0);
    /// Value in hertz (`01`).
    pub const HERTZ: Self = Self(1);
    /// Value in kilohertz (`02`).
    pub const KILOHERTZ: Self = Self(2);
    /// A specific-information identity (`10`), including Clear Bass.
    pub const SPECIFIC: Self = Self(0x10);

    /// Preserves the wire type byte.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original type byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// One three-byte band-information record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BandInformation {
    /// Interpretation of the value; unknown types remain available as raw bytes.
    pub information_type: BandInformationType,
    /// Original unsigned big-endian value.
    pub value: u16,
}

impl BandInformation {
    /// Whether this record identifies Clear Bass (`10 00 01`).
    pub const fn is_clear_bass(self) -> bool {
        self.information_type.0 == 0x10 && self.value == 1
    }

    /// Returns the frequency in hertz for Hz/kHz records, otherwise `None`.
    pub const fn frequency_hz(self) -> Option<u32> {
        match self.information_type.0 {
            1 => Some(self.value as u32),
            2 => Some(self.value as u32 * 1000),
            _ => None,
        }
    }
}

/// The ordered band metadata returned by `EQEBB_RET_EXTENDED_INFO` (`5b`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EqualizerBandInfo {
    kind: EqualizerKind,
    /// Metadata in the same order as the equalizer's band values.
    pub bands: Vec<BandInformation>,
}

impl EqualizerBandInfo {
    /// Decodes exact count-delimited records, retaining unknown types and values.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x5b)?;
        require_minimum(payload, 3)?;
        require_length(payload, 3 + usize::from(payload[2]) * 3)?;
        let bands = payload[3..]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|record| BandInformation {
                information_type: BandInformationType::from_raw(record[0]),
                value: u16::from_be_bytes([record[1], record[2]]),
            })
            .collect();
        Ok(Self { kind, bands })
    }

    /// Returns the inquiry layout.
    pub const fn kind(&self) -> EqualizerKind {
        self.kind
    }

    /// Checks that the inquiry and band count match a capability.
    pub fn validate_capability(
        &self,
        capability: &EqualizerCapability,
    ) -> Result<(), CommandError> {
        same_kind(self.kind, capability.kind)?;
        if self.bands.len() != usize::from(capability.band_count) {
            return Err(CommandError::InvalidLength {
                expected: usize::from(capability.band_count),
                actual: self.bands.len(),
            });
        }
        Ok(())
    }
}

/// An equalizer write intent checked against a negotiated capability.
///
/// `None` levels select a preset using a zero band count. `Some` levels replace
/// an entire curve and must target an advertised custom/user slot (`a0..=a5`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EqualizerSetting {
    /// A preset selection or curve update for inquiry `00`, `02` or `04`.
    Preset {
        /// An advertised preset identity.
        preset: PresetId,
        /// Complete native curve, or `None` to select without replacing it.
        levels: Option<Vec<u8>>,
    },
    /// A preset selection or curve update including the ULT mode (inquiry `03`).
    PresetWithUlt {
        /// An advertised preset identity.
        preset: PresetId,
        /// ULT mode included in the same command.
        ult_mode: UltMode,
        /// Complete native curve, or `None` to select without replacing it.
        levels: Option<Vec<u8>>,
    },
    /// A complete standalone custom curve (inquiry `31`).
    Custom {
        /// Native levels in device band order.
        levels: Vec<u8>,
    },
}

impl EqualizerSetting {
    /// Encodes `EQEBB_SET_PARAM` (`58`) for table 1 after capability checks.
    ///
    /// Rejects mismatched layouts, unadvertised presets, edits of factory or
    /// unknown preset slots, curve edits on noncustomizable EQ, wrong band
    /// counts and levels outside `0..level_steps`. No values are clamped.
    /// This method does not check current status, perform I/O or create an
    /// expected response: reconcile the result using parameter reports.
    pub fn encode_set(&self, capability: &EqualizerCapability) -> Result<Vec<u8>, CommandError> {
        let mut payload = vec![0x58, capability.kind.inquiry()];
        let (preset, levels) = match self {
            Self::Preset { preset, levels } => {
                if !matches!(
                    capability.kind,
                    EqualizerKind::Preset
                        | EqualizerKind::PresetNonCustomizable
                        | EqualizerKind::PresetWithErrorCodes
                ) {
                    return Err(incompatible_setting(capability.kind));
                }
                payload.push(preset.as_raw());
                (Some(*preset), levels.as_deref())
            }
            Self::PresetWithUlt {
                preset,
                ult_mode,
                levels,
            } => {
                if capability.kind != EqualizerKind::PresetWithUlt {
                    return Err(incompatible_setting(capability.kind));
                }
                payload.extend_from_slice(&[preset.as_raw(), ult_mode.as_u8()]);
                (Some(*preset), levels.as_deref())
            }
            Self::Custom { levels } => {
                if capability.kind != EqualizerKind::Custom {
                    return Err(incompatible_setting(capability.kind));
                }
                (None, Some(levels.as_slice()))
            }
        };
        if let Some(preset) = preset {
            require_preset(capability, preset)?;
            if levels.is_some() {
                if capability.kind == EqualizerKind::PresetNonCustomizable {
                    return Err(CommandError::UnsupportedOperation {
                        operation: "equalizer curve editing",
                        inquiry: capability.kind.inquiry(),
                    });
                }
                if !preset.is_custom_slot() {
                    return Err(CommandError::InvalidValue {
                        field: "editable equalizer preset",
                        value: preset.as_raw(),
                    });
                }
            }
        }
        match levels {
            Some(levels) => {
                capability.validate_levels(levels)?;
                payload.push(capability.band_count);
                payload.extend_from_slice(levels);
            }
            None => payload.push(0),
        }
        Ok(payload)
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<EqualizerKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    EqualizerKind::try_from(payload[1])
}

fn same_kind(actual: EqualizerKind, expected: EqualizerKind) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CommandError::CapabilityMismatch {
            expected: expected.inquiry(),
            actual: actual.inquiry(),
        })
    }
}

fn require_preset(capability: &EqualizerCapability, preset: PresetId) -> Result<(), CommandError> {
    if capability.supports_preset(preset) {
        Ok(())
    } else {
        Err(CommandError::InvalidValue {
            field: "advertised equalizer preset",
            value: preset.as_raw(),
        })
    }
}

fn incompatible_setting(kind: EqualizerKind) -> CommandError {
    CommandError::UnsupportedOperation {
        operation: "equalizer setting layout",
        inquiry: kind.inquiry(),
    }
}
