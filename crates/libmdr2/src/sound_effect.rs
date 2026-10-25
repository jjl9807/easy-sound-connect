//! Local table-1 sound effects, customizable curves and ULT button assignments.
//!
//! [`SoundEffectKind`] identifies advertised layouts. Curves use native unsigned
//! steps and band metadata; ULT button edits preserve the current ordered list.

#![doc = include_str!("../docs/sound-effect.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    equalizer::{BandInformation, BandInformationType},
    types::{CommandTable, FunctionId},
};

/// An advertised sound-effect layout in MDR v2 table 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SoundEffectKind {
    /// Local effect selection (`30`), advertised by function `54`.
    Standard,
    /// Customizable effects (`33`) and their update messages (`34..=36`), function `59`.
    Customizable,
    /// ULT button cycle assignments (`40`), advertised by function `58`.
    UltButton,
}

impl SoundEffectKind {
    /// Returns the inquiry used for capability and parameter requests.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Standard => 0x30,
            Self::Customizable => 0x33,
            Self::UltButton => 0x40,
        }
    }

    /// Returns the table-aware function advertising this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::Standard => 0x54,
                Self::Customizable => 0x59,
                Self::UltButton => 0x58,
            },
        }
    }

    /// Maps a table-1 advertised function without imposing layout precedence.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0x54 => Some(Self::Standard),
            0x59 => Some(Self::Customizable),
            0x58 => Some(Self::UltButton),
            _ => None,
        }
    }

    /// Whether `GET_CAPABILITY` includes a display-language byte.
    pub const fn capability_uses_language(self) -> bool {
        matches!(self, Self::Standard)
    }

    /// Whether this layout defines `GET_STATUS`.
    pub const fn has_status_query(self) -> bool {
        matches!(self, Self::Standard)
    }

    /// Whether this layout defines `GET_EXTENDED_INFO` for band metadata.
    pub const fn has_band_info_query(self) -> bool {
        matches!(self, Self::Customizable)
    }
}

impl TryFrom<u8> for SoundEffectKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x30 => Ok(Self::Standard),
            0x33 => Ok(Self::Customizable),
            0x40 => Ok(Self::UltButton),
            value => Err(CommandError::InvalidValue {
                field: "sound effect inquiry",
                value,
            }),
        }
    }
}

/// A sound-effect identity, retaining unknown values on reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoundEffect(u8);

impl SoundEffect {
    /// Sound effect off (`00`).
    pub const OFF: Self = Self(0);
    /// ULT (`01`).
    pub const ULT: Self = Self(1);
    /// ULT 1 (`02`).
    pub const ULT_1: Self = Self(2);
    /// ULT 2 (`03`).
    pub const ULT_2: Self = Self(3);
    /// Custom (`04`).
    pub const CUSTOM: Self = Self(4);
    /// Flat (`05`).
    pub const FLAT: Self = Self(5);
    /// Live (`06`).
    pub const LIVE: Self = Self(6);
    /// No active effect while a mutually exclusive function is active (`ff`); read-only.
    pub const NONE: Self = Self(0xff);

    /// Preserves a wire identity without claiming that it is selectable.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original wire identity.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Whether this library defines a writable local effect identity.
    pub const fn is_writable(self) -> bool {
        self.0 <= 6
    }
}

/// An exclusive function identity reported with sound effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExclusiveFunction(u8);

impl ExclusiveFunction {
    /// Stamina mode (`00`).
    pub const STAMINA: Self = Self(0);

    /// Preserves an exclusive function identity.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original wire identity.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Layout-specific sound-effect capability data.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SoundEffectCapabilityData {
    /// Selectable effects and functions that can exclude them.
    Standard {
        /// Effects in advertised order, including unknown identities.
        effects: Vec<SoundEffect>,
        /// Exclusive functions in advertised order.
        exclusive_functions: Vec<ExclusiveFunction>,
    },
    /// Customizable effect slots and their native level range.
    Customizable {
        /// The uninterpreted first capability data byte.
        capability_byte: u8,
        /// Number of native steps; writable values are `0..level_steps`.
        level_steps: u8,
        /// Effect slots in the order of the parameter response's curves.
        effects: Vec<SoundEffect>,
    },
    /// Bounds on the number of enabled entries in an ULT button cycle.
    UltButton {
        /// Minimum number of enabled entries.
        minimum: u8,
        /// Maximum number of enabled entries.
        maximum: u8,
    },
}

/// A decoded sound-effect capability response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEffectCapability {
    kind: SoundEffectKind,
    data: SoundEffectCapabilityData,
}

impl SoundEffectCapability {
    /// Decodes `EQEBB_RET_CAPABILITY` (`51`), checking all counts and total length.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = SoundEffectKind::try_from(header(table, payload, 0x51)?)?;
        let mut reader = Reader::new(payload, 2);
        let data = match kind {
            SoundEffectKind::Standard => SoundEffectCapabilityData::Standard {
                effects: reader.list()?.iter().copied().map(SoundEffect).collect(),
                exclusive_functions: reader
                    .list()?
                    .iter()
                    .copied()
                    .map(ExclusiveFunction)
                    .collect(),
            },
            SoundEffectKind::Customizable => SoundEffectCapabilityData::Customizable {
                capability_byte: reader.byte()?,
                level_steps: reader.byte()?,
                effects: reader.list()?.iter().copied().map(SoundEffect).collect(),
            },
            SoundEffectKind::UltButton => {
                let minimum = reader.byte()?;
                let maximum = reader.byte()?;
                if minimum > maximum {
                    return Err(CommandError::InvalidCapability {
                        field: "ULT button count bounds",
                    });
                }
                SoundEffectCapabilityData::UltButton { minimum, maximum }
            }
        };
        reader.finish()?;
        Ok(Self { kind, data })
    }

    /// Returns the advertised layout.
    pub const fn kind(&self) -> SoundEffectKind {
        self.kind
    }

    /// Returns the layout-specific data without discarding unknown values.
    pub const fn data(&self) -> &SoundEffectCapabilityData {
        &self.data
    }

    fn effects(&self) -> Option<&[SoundEffect]> {
        match &self.data {
            SoundEffectCapabilityData::Standard { effects, .. }
            | SoundEffectCapabilityData::Customizable { effects, .. } => Some(effects),
            SoundEffectCapabilityData::UltButton { .. } => None,
        }
    }

    fn selectable(&self, effect: SoundEffect) -> Result<(), CommandError> {
        if !effect.is_writable() {
            return Err(CommandError::InvalidParameter {
                field: "writable local sound effect",
            });
        }
        let effects = self.effects().ok_or(CommandError::InvalidCapability {
            field: "selectable sound effects",
        })?;
        if effects.iter().filter(|&&entry| entry == effect).count() != 1 {
            return Err(CommandError::InvalidParameter {
                field: "uniquely advertised sound effect",
            });
        }
        Ok(())
    }
}

/// One effect's availability or ULT button assignment state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundEffectState {
    /// Original effect identity.
    pub effect: SoundEffect,
    /// `true` corresponds to wire `00`; `false` to `01`.
    pub enabled: bool,
}

/// Current availability for standard sound-effect selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEffectStatus {
    /// Whether sound-effect controls are currently available.
    pub enabled: bool,
    /// Per-effect availability in wire order.
    pub effects: Vec<SoundEffectState>,
}

impl SoundEffectStatus {
    /// Decodes a standard `EQEBB_RET_STATUS` (`53 30`) response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x53)
    }

    /// Decodes a standard `EQEBB_NTFY_STATUS` (`55 30`) notification.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x55)
    }

    /// Returns the only layout defining these status messages.
    pub const fn kind(&self) -> SoundEffectKind {
        SoundEffectKind::Standard
    }

    /// Checks layout and membership of every reported effect.
    pub fn validate_capability(&self, cap: &SoundEffectCapability) -> Result<(), CommandError> {
        same_kind(self.kind(), cap.kind)?;
        for entry in &self.effects {
            reported_effect(cap, entry.effect)?;
        }
        Ok(())
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        require_inquiry(header(table, payload, opcode)?, 0x30)?;
        let mut reader = Reader::new(payload, 2);
        let enabled = enabled(reader.byte()?)?;
        let effects = reader.states()?;
        reader.finish()?;
        Ok(Self { enabled, effects })
    }
}

/// Layout-specific sound-effect parameter data.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SoundEffectParameterData {
    /// Current effect for inquiry `30`.
    Standard(SoundEffect),
    /// Current effect, exclusivity and optional full curve collection for `33`.
    Customizable {
        /// Current effect, or `NONE` while an exclusive function is active.
        effect: SoundEffect,
        /// Exclusive functions; nonempty only when the effect is `NONE`.
        exclusive_functions: Vec<ExclusiveFunction>,
        /// Present in responses, absent in selection notifications.
        /// Curves follow the capability's effect order.
        curves: Option<Vec<Vec<u8>>>,
    },
    /// An updated curve in reset (`34`) or customization (`35`) notifications.
    Curve {
        /// Effect whose curve was updated.
        effect: SoundEffect,
        /// Native unsigned band values.
        levels: Vec<u8>,
    },
    /// An external curve/selection update (`36`).
    ExternalUpdate {
        /// Current effect, including `NONE`.
        effect: SoundEffect,
        /// Exclusive functions; nonempty only when the effect is `NONE`.
        exclusive_functions: Vec<ExclusiveFunction>,
        /// Native unsigned band values.
        levels: Vec<u8>,
    },
    /// Current ULT button entries in device order (`40`).
    UltButton(Vec<SoundEffectState>),
}

/// A response or notification with its exact inquiry identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEffectParameters {
    inquiry: u8,
    data: SoundEffectParameterData,
}

impl SoundEffectParameters {
    /// Decodes `EQEBB_RET_PARAM` (`57`) for inquiries `30`, `33` or `40`.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x57)
    }

    /// Decodes `EQEBB_NTFY_PARAM` (`59`) for `30`, `33..=36` or `40`.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x59)
    }

    /// Returns the advertising layout, grouping `34..=36` under `Customizable`.
    pub const fn kind(&self) -> SoundEffectKind {
        match self.inquiry {
            0x30 => SoundEffectKind::Standard,
            0x40 => SoundEffectKind::UltButton,
            _ => SoundEffectKind::Customizable,
        }
    }

    /// Returns the exact inquiry, retaining reset/custom/external notification distinctions.
    pub const fn inquiry(&self) -> u8 {
        self.inquiry
    }

    /// Returns the decoded data.
    pub const fn data(&self) -> &SoundEffectParameterData {
        &self.data
    }

    /// Checks layout, effect membership, curve slot count and native level ranges.
    ///
    /// Unknown advertised identities remain valid reports. `NONE` is allowed for
    /// customizable selection and external updates. Band dimensions are checked
    /// separately with [`SoundEffectBandInfo::validate_parameters`].
    pub fn validate_capability(&self, cap: &SoundEffectCapability) -> Result<(), CommandError> {
        same_kind(self.kind(), cap.kind)?;
        match &self.data {
            SoundEffectParameterData::Standard(effect) => reported_effect(cap, *effect)?,
            SoundEffectParameterData::Customizable { effect, curves, .. } => {
                if *effect != SoundEffect::NONE {
                    reported_effect(cap, *effect)?;
                }
                if let Some(curves) = curves {
                    let count = cap.effects().map_or(0, <[SoundEffect]>::len);
                    if curves.len() != count {
                        return Err(CommandError::InvalidLength {
                            expected: count,
                            actual: curves.len(),
                        });
                    }
                    for levels in curves {
                        validate_levels(cap, levels)?;
                    }
                }
            }
            SoundEffectParameterData::Curve { effect, levels } => {
                reported_effect(cap, *effect)?;
                validate_levels(cap, levels)?;
            }
            SoundEffectParameterData::ExternalUpdate { effect, levels, .. } => {
                if *effect != SoundEffect::NONE {
                    reported_effect(cap, *effect)?;
                }
                validate_levels(cap, levels)?;
            }
            SoundEffectParameterData::UltButton(entries) => {
                validate_assignment_count(cap, entries)?
            }
        }
        Ok(())
    }

    /// Encodes a change to one existing ULT button entry, preserving the others byte-for-byte.
    ///
    /// The target must be a known local effect occurring exactly once. The
    /// resulting enabled count must satisfy the capability bounds. Unknown
    /// unrelated effects are retained; this API cannot add or reorder entries.
    pub fn encode_button_change(
        &self,
        cap: &SoundEffectCapability,
        effect: SoundEffect,
        enabled: bool,
    ) -> Result<Vec<u8>, CommandError> {
        same_kind(SoundEffectKind::UltButton, self.kind())?;
        same_kind(self.kind(), cap.kind)?;
        let SoundEffectParameterData::UltButton(entries) = &self.data else {
            unreachable!()
        };
        if !effect.is_writable()
            || entries
                .iter()
                .filter(|entry| entry.effect == effect)
                .count()
                != 1
        {
            return Err(CommandError::InvalidParameter {
                field: "unique local ULT button effect",
            });
        }
        let mut updated = entries.clone();
        for entry in &mut updated {
            if entry.effect == effect {
                entry.enabled = enabled;
            }
        }
        validate_assignment_count(cap, &updated)?;
        let mut payload = vec![0x58, 0x40, updated.len() as u8];
        for entry in updated {
            payload.extend([entry.effect.0, u8::from(!entry.enabled)]);
        }
        Ok(payload)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let inquiry = header(table, payload, opcode)?;
        let mut reader = Reader::new(payload, 2);
        let data = match inquiry {
            0x30 => SoundEffectParameterData::Standard(SoundEffect(reader.byte()?)),
            0x33 => {
                let effect = SoundEffect(reader.byte()?);
                let exclusive_functions = reader.exclusive(effect)?;
                let curves = if opcode == 0x57 {
                    let count = reader.byte()?;
                    if count == 0 {
                        return Err(CommandError::InvalidValue {
                            field: "sound effect curve count",
                            value: count,
                        });
                    }
                    let mut curves = Vec::new();
                    for _ in 0..count {
                        curves.push(reader.list()?.to_vec());
                    }
                    Some(curves)
                } else {
                    None
                };
                SoundEffectParameterData::Customizable {
                    effect,
                    exclusive_functions,
                    curves,
                }
            }
            0x34 | 0x35 if opcode == 0x59 => SoundEffectParameterData::Curve {
                effect: SoundEffect(reader.byte()?),
                levels: reader.list()?.to_vec(),
            },
            0x36 if opcode == 0x59 => {
                let effect = SoundEffect(reader.byte()?);
                let exclusive_functions = reader.exclusive(effect)?;
                let levels = reader.list()?.to_vec();
                SoundEffectParameterData::ExternalUpdate {
                    effect,
                    exclusive_functions,
                    levels,
                }
            }
            0x40 => SoundEffectParameterData::UltButton(reader.states()?),
            value => {
                return Err(CommandError::InvalidValue {
                    field: "sound effect parameter inquiry",
                    value,
                });
            }
        };
        reader.finish()?;
        Ok(Self { inquiry, data })
    }
}

/// Customizable sound-effect band metadata in native curve order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEffectBandInfo {
    bands: Vec<BandInformation>,
}

impl SoundEffectBandInfo {
    /// Decodes exact `5b 33 count (type value:u16be)*` records.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        require_inquiry(header(table, payload, 0x5b)?, 0x33)?;
        require_minimum(payload, 3)?;
        require_length(payload, 3 + usize::from(payload[2]) * 3)?;
        let bands = payload[3..]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|bytes| BandInformation {
                information_type: BandInformationType::from_raw(bytes[0]),
                value: u16::from_be_bytes([bytes[1], bytes[2]]),
            })
            .collect();
        Ok(Self { bands })
    }

    /// Returns the customizable effect layout.
    pub const fn kind(&self) -> SoundEffectKind {
        SoundEffectKind::Customizable
    }

    /// Returns metadata in the order of native curve values.
    pub fn bands(&self) -> &[BandInformation] {
        &self.bands
    }

    /// Checks every supplied curve's dimension against the band metadata.
    pub fn validate_parameters(&self, params: &SoundEffectParameters) -> Result<(), CommandError> {
        same_kind(self.kind(), params.kind())?;
        match &params.data {
            SoundEffectParameterData::Customizable {
                curves: Some(curves),
                ..
            } => {
                for levels in curves {
                    self.validate_length(levels)?;
                }
            }
            SoundEffectParameterData::Curve { levels, .. }
            | SoundEffectParameterData::ExternalUpdate { levels, .. } => {
                self.validate_length(levels)?
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_length(&self, levels: &[u8]) -> Result<(), CommandError> {
        if levels.len() != self.bands.len() {
            return Err(CommandError::InvalidLength {
                expected: self.bands.len(),
                actual: levels.len(),
            });
        }
        Ok(())
    }
}

/// A capability-checked local effect selection or curve reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SoundEffectSetting {
    /// Select an advertised standard or customizable effect.
    Select(SoundEffect),
    /// Restore an advertised customizable effect's curve (`34`).
    Reset(SoundEffect),
}

impl SoundEffectSetting {
    /// Encodes a `58` payload after checking layout and unique advertised membership.
    pub fn encode_set(self, cap: &SoundEffectCapability) -> Result<Vec<u8>, CommandError> {
        let (inquiry, effect) = match self {
            Self::Select(effect) => (cap.kind.inquiry(), effect),
            Self::Reset(effect) => {
                same_kind(SoundEffectKind::Customizable, cap.kind)?;
                (0x34, effect)
            }
        };
        cap.selectable(effect)?;
        Ok(vec![0x58, inquiry, effect.0])
    }
}

/// A complete customizable-effect curve write (`35`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundEffectCurveSetting {
    /// The advertised effect slot to update.
    pub effect: SoundEffect,
    /// Whether the device should issue an update notification (`00` when true).
    pub request_notification: bool,
    /// One native unsigned value per advertised band, without decibel conversion.
    pub levels: Vec<u8>,
}

impl SoundEffectCurveSetting {
    /// Encodes a curve using capability and band metadata from the same device.
    ///
    /// Empty metadata, unknown/unadvertised effects, incorrect band counts and
    /// levels outside `0..level_steps` are rejected. The uninterpreted capability
    /// byte is not used to infer a curve dimension.
    pub fn encode_set(
        &self,
        cap: &SoundEffectCapability,
        bands: &SoundEffectBandInfo,
    ) -> Result<Vec<u8>, CommandError> {
        same_kind(SoundEffectKind::Customizable, cap.kind)?;
        cap.selectable(self.effect)?;
        if bands.bands.is_empty() {
            return Err(CommandError::InvalidCapability {
                field: "nonempty sound effect bands",
            });
        }
        bands.validate_length(&self.levels)?;
        validate_levels(cap, &self.levels)?;
        let mut payload = vec![
            0x58,
            0x35,
            u8::from(!self.request_notification),
            self.effect.0,
            self.levels.len() as u8,
        ];
        payload.extend_from_slice(&self.levels);
        Ok(payload)
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<u8, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    Ok(payload[1])
}

fn require_inquiry(actual: u8, expected: u8) -> Result<(), CommandError> {
    if actual != expected {
        return Err(CommandError::UnexpectedInquiry { expected, actual });
    }
    Ok(())
}

fn same_kind(expected: SoundEffectKind, actual: SoundEffectKind) -> Result<(), CommandError> {
    if actual != expected {
        return Err(CommandError::CapabilityMismatch {
            expected: expected.inquiry(),
            actual: actual.inquiry(),
        });
    }
    Ok(())
}

fn enabled(value: u8) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue {
            field: "sound effect enabled state",
            value,
        }),
    }
}

fn reported_effect(cap: &SoundEffectCapability, effect: SoundEffect) -> Result<(), CommandError> {
    if !cap
        .effects()
        .is_some_and(|effects| effects.contains(&effect))
    {
        return Err(CommandError::InvalidParameter {
            field: "reported sound effect membership",
        });
    }
    Ok(())
}

fn validate_levels(cap: &SoundEffectCapability, levels: &[u8]) -> Result<(), CommandError> {
    let SoundEffectCapabilityData::Customizable { level_steps, .. } = cap.data else {
        return Err(CommandError::InvalidCapability {
            field: "customizable sound effect level range",
        });
    };
    if level_steps == 0 {
        return Err(CommandError::InvalidCapability {
            field: "nonzero sound effect level steps",
        });
    }
    if levels.iter().any(|&level| level >= level_steps) {
        return Err(CommandError::InvalidParameter {
            field: "sound effect native level",
        });
    }
    Ok(())
}

fn validate_assignment_count(
    cap: &SoundEffectCapability,
    entries: &[SoundEffectState],
) -> Result<(), CommandError> {
    let SoundEffectCapabilityData::UltButton { minimum, maximum } = cap.data else {
        return Err(CommandError::InvalidCapability {
            field: "ULT button count bounds",
        });
    };
    let count = entries.iter().filter(|entry| entry.enabled).count();
    if !(usize::from(minimum)..=usize::from(maximum)).contains(&count) {
        return Err(CommandError::InvalidParameter {
            field: "enabled ULT button count",
        });
    }
    Ok(())
}

struct Reader<'a> {
    payload: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(payload: &'a [u8], offset: usize) -> Self {
        Self { payload, offset }
    }

    fn byte(&mut self) -> Result<u8, CommandError> {
        require_minimum(self.payload, self.offset + 1)?;
        let value = self.payload[self.offset];
        self.offset += 1;
        Ok(value)
    }

    fn list(&mut self) -> Result<&'a [u8], CommandError> {
        let count = usize::from(self.byte()?);
        let end = self.offset + count;
        require_minimum(self.payload, end)?;
        let values = &self.payload[self.offset..end];
        self.offset = end;
        Ok(values)
    }

    fn exclusive(&mut self, effect: SoundEffect) -> Result<Vec<ExclusiveFunction>, CommandError> {
        let values = self.list()?;
        if effect != SoundEffect::NONE && !values.is_empty() {
            return Err(CommandError::InvalidParameter {
                field: "exclusive function without NONE effect",
            });
        }
        Ok(values.iter().copied().map(ExclusiveFunction).collect())
    }

    fn states(&mut self) -> Result<Vec<SoundEffectState>, CommandError> {
        let count = self.byte()?;
        require_minimum(self.payload, self.offset + usize::from(count) * 2)?;
        let mut states = Vec::new();
        for _ in 0..count {
            states.push(SoundEffectState {
                effect: SoundEffect(self.byte()?),
                enabled: enabled(self.byte()?)?,
            });
        }
        Ok(states)
    }

    fn finish(self) -> Result<(), CommandError> {
        require_length(self.payload, self.offset)
    }
}
