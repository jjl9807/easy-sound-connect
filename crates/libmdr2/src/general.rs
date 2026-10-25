//! Device-described local settings selected by stable capability identities.

#![doc = include_str!("../docs/general.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// One of four table-1 general-setting slots.
///
/// A slot is a routing identity, not a feature name. Read its capability to
/// identify the setting and its Boolean or list layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeneralKind {
    /// First general-setting slot (`d1`).
    Setting1,
    /// Second general-setting slot (`d2`).
    Setting2,
    /// Third general-setting slot (`d3`).
    Setting3,
    /// Fourth general-setting slot (`d4`).
    Setting4,
}

impl GeneralKind {
    /// Returns the inquiry byte following the command opcode.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Setting1 => 0xd1,
            Self::Setting2 => 0xd2,
            Self::Setting3 => 0xd3,
            Self::Setting4 => 0xd4,
        }
    }

    /// Returns the table-aware function identity advertising this slot.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: self.inquiry(),
        }
    }

    /// Selects a slot only from a known table-1 function identity.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        Self::try_from(function.code).ok()
    }
}

impl TryFrom<u8> for GeneralKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0xd1 => Ok(Self::Setting1),
            0xd2 => Ok(Self::Setting2),
            0xd3 => Ok(Self::Setting3),
            0xd4 => Ok(Self::Setting4),
            value => Err(CommandError::InvalidValue {
                field: "general setting inquiry",
                value,
            }),
        }
    }
}

/// How device-provided title and summary bytes are intended to be displayed.
///
/// Unknown formats preserve their byte and labels but do not identify a
/// writable feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GeneralStringFormat(u8);

impl GeneralStringFormat {
    /// Device-provided display text (`00`).
    pub const RAW_NAME: Self = Self(0);
    /// Stable string identifiers to translate in the caller (`01`).
    pub const ENUM_NAME: Self = Self(1);

    /// Preserves a string-format identity, including unknown values.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original format byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// A setting or list-option description, preserving its original text bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralInfo {
    format: GeneralStringFormat,
    title: Vec<u8>,
    summary: Vec<u8>,
}

impl GeneralInfo {
    /// Returns the format applying to both the title and summary.
    pub const fn format(&self) -> GeneralStringFormat {
        self.format
    }

    /// Returns the unmodified title bytes.
    pub fn title_bytes(&self) -> &[u8] {
        &self.title
    }

    /// Interprets the title as UTF-8 without replacing invalid bytes.
    pub fn title_utf8(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.title)
    }

    /// Returns the unmodified summary bytes.
    pub fn summary_bytes(&self) -> &[u8] {
        &self.summary
    }

    /// Interprets the summary as UTF-8 without replacing invalid bytes.
    pub fn summary_utf8(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.summary)
    }
}

/// The wire representation of a general setting's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeneralSettingType {
    /// An on/off value (`00`).
    Boolean,
    /// An index into the advertised option list (`01`).
    List,
}

/// A recognized device-local feature that can be written through this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GeneralFeature {
    /// Sidetone, identified by `SIDETONE_SETTING`.
    Sidetone,
    /// Multipoint, identified by `MULTIPOINT_SETTING`.
    Multipoint,
    /// Touch-panel controls, identified by `TOUCH_PANEL_SETTING`.
    TouchPanel,
}

/// The complete descriptor for one general-setting slot.
///
/// Boolean and list descriptors are readable even when their semantic identity
/// is unknown. Only a recognized [`GeneralFeature`] authorizes a typed write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneralCapability {
    kind: GeneralKind,
    info: GeneralInfo,
    options: Option<Vec<GeneralInfo>>,
}

impl GeneralCapability {
    /// Decodes an exact table-1 `GENERAL_SETTING_RET_CAPABILITY` (`d1`).
    ///
    /// Each title and summary is at most 128 bytes. List descriptors contain
    /// `1..=64` options; their original order determines parameter indices.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0xd1)?;
        require_minimum(payload, 3)?;
        let setting_type = setting_type(payload[2])?;
        let mut offset = 3;
        let info = read_info(payload, &mut offset)?;
        let options = if setting_type == GeneralSettingType::List {
            require_minimum(payload, offset + 1)?;
            let count = payload[offset];
            offset += 1;
            if !(1..=64).contains(&count) {
                return Err(CommandError::InvalidValue {
                    field: "general option count",
                    value: count,
                });
            }
            let mut options = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                options.push(read_info(payload, &mut offset)?);
            }
            Some(options)
        } else {
            None
        };
        require_length(payload, offset)?;
        Ok(Self {
            kind,
            info,
            options,
        })
    }

    /// Returns the slot from which this descriptor was obtained.
    pub const fn kind(&self) -> GeneralKind {
        self.kind
    }

    /// Returns the advertised value layout.
    pub const fn setting_type(&self) -> GeneralSettingType {
        if self.options.is_some() {
            GeneralSettingType::List
        } else {
            GeneralSettingType::Boolean
        }
    }

    /// Returns the setting's title, summary and string format.
    pub const fn info(&self) -> &GeneralInfo {
        &self.info
    }

    /// Returns list options in index order; Boolean descriptors have none.
    pub fn options(&self) -> Option<&[GeneralInfo]> {
        self.options.as_deref()
    }

    /// Resolves a recognized local feature from an exact `ENUM_NAME` title.
    ///
    /// Slot numbers, summaries, translated text and `RAW_NAME` titles are never
    /// used as feature identities. Unknown identities remain read-only.
    pub fn feature(&self) -> Option<GeneralFeature> {
        if self.info.format != GeneralStringFormat::ENUM_NAME {
            return None;
        }
        match self.info.title.as_slice() {
            b"SIDETONE_SETTING" => Some(GeneralFeature::Sidetone),
            b"MULTIPOINT_SETTING" => Some(GeneralFeature::Multipoint),
            b"TOUCH_PANEL_SETTING" => Some(GeneralFeature::TouchPanel),
            _ => None,
        }
    }
}

/// Current availability of one general-setting slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneralStatus {
    kind: GeneralKind,
    /// Whether the setting is currently operable (`00` means enabled).
    pub enabled: bool,
}

impl GeneralStatus {
    /// Returns the slot to which this status applies.
    pub const fn kind(self) -> GeneralKind {
        self.kind
    }

    /// Decodes `GENERAL_SETTING_RET_STATUS` (`d3`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xd3)
    }

    /// Decodes `GENERAL_SETTING_NTFY_STATUS` (`d5`) separately from replies.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xd5)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, 3)?;
        Ok(Self {
            kind,
            enabled: positive_zero(payload[2], "general setting availability")?,
        })
    }
}

/// A Boolean setting or a zero-based index in a capability's option list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeneralValue {
    /// An on/off value; `true` encodes as `00`.
    Boolean(bool),
    /// An option index in `0..=63`, further bounded by the descriptor's list.
    List(u8),
}

/// Current or desired parameters associated with a general-setting slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneralParameters {
    kind: GeneralKind,
    /// The Boolean value or option index.
    pub value: GeneralValue,
}

impl GeneralParameters {
    /// Constructs parameters after checking the wire-level index bound.
    ///
    /// Writing additionally requires [`Self::encode_set`] and a matching,
    /// recognized capability descriptor.
    pub fn new(kind: GeneralKind, value: GeneralValue) -> Result<Self, CommandError> {
        validate_index(value)?;
        Ok(Self { kind, value })
    }

    /// Returns the slot to which these parameters apply.
    pub const fn kind(self) -> GeneralKind {
        self.kind
    }

    /// Decodes `GENERAL_SETTING_RET_PARAM` (`d7`) without requiring cached capabilities.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xd7)
    }

    /// Decodes `GENERAL_SETTING_NTFY_PARAM` (`d9`) independently of read replies.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xd9)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, 4)?;
        let value = match setting_type(payload[2])? {
            GeneralSettingType::Boolean => {
                GeneralValue::Boolean(positive_zero(payload[3], "general Boolean value")?)
            }
            GeneralSettingType::List => GeneralValue::List(payload[3]),
        };
        Self::new(kind, value)
    }

    /// Checks slot, value layout and list membership against a descriptor.
    ///
    /// This check also works for unknown read-only feature identities. It does
    /// not establish availability, freshness or connection ownership.
    pub fn validate_capability(&self, capability: &GeneralCapability) -> Result<(), CommandError> {
        if self.kind != capability.kind {
            return Err(CommandError::CapabilityMismatch {
                expected: self.kind.inquiry(),
                actual: capability.kind.inquiry(),
            });
        }
        validate_index(self.value)?;
        match (self.value, capability.options.as_deref()) {
            (GeneralValue::Boolean(_), None) => Ok(()),
            (GeneralValue::List(index), Some(options)) if usize::from(index) < options.len() => {
                Ok(())
            }
            (GeneralValue::List(_), Some(_)) => Err(CommandError::InvalidParameter {
                field: "advertised general option index",
            }),
            _ => Err(CommandError::InvalidCapability {
                field: "general setting value layout",
            }),
        }
    }

    /// Encodes `GENERAL_SETTING_SET_PARAM` (`d8`) for a recognized local feature.
    ///
    /// Unknown features, service-specific identities and raw display labels do
    /// not authorize writes. A matching Boolean/list capability is required,
    /// with list indices bounded by its advertised entries. There is no clamping.
    ///
    /// Send the result on table 1 after checking current availability. These
    /// writes have no dedicated RET; notifications or a serialized readback
    /// reconcile device state. A link ACK confirms only frame delivery.
    pub fn encode_set(&self, capability: &GeneralCapability) -> Result<Vec<u8>, CommandError> {
        self.validate_capability(capability)?;
        if capability.feature().is_none() {
            return Err(CommandError::UnsupportedOperation {
                operation: "unrecognized general setting write",
                inquiry: self.kind.inquiry(),
            });
        }
        let (layout, value) = match self.value {
            GeneralValue::Boolean(enabled) => (0, u8::from(!enabled)),
            GeneralValue::List(index) => (1, index),
        };
        Ok(vec![0xd8, self.kind.inquiry(), layout, value])
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<GeneralKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    payload[1].try_into()
}

fn setting_type(value: u8) -> Result<GeneralSettingType, CommandError> {
    match value {
        0 => Ok(GeneralSettingType::Boolean),
        1 => Ok(GeneralSettingType::List),
        value => Err(CommandError::InvalidValue {
            field: "general setting type",
            value,
        }),
    }
}

fn positive_zero(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn validate_index(value: GeneralValue) -> Result<(), CommandError> {
    if let GeneralValue::List(index) = value
        && index > 63
    {
        return Err(CommandError::InvalidValue {
            field: "general option index",
            value: index,
        });
    }
    Ok(())
}

fn read_info(payload: &[u8], offset: &mut usize) -> Result<GeneralInfo, CommandError> {
    require_minimum(payload, *offset + 1)?;
    let format = GeneralStringFormat::from_raw(payload[*offset]);
    *offset += 1;
    let title = read_text(payload, offset)?;
    let summary = read_text(payload, offset)?;
    Ok(GeneralInfo {
        format,
        title,
        summary,
    })
}

fn read_text(payload: &[u8], offset: &mut usize) -> Result<Vec<u8>, CommandError> {
    require_minimum(payload, *offset + 1)?;
    let length = payload[*offset];
    *offset += 1;
    if length > 128 {
        return Err(CommandError::InvalidValue {
            field: "general text length",
            value: length,
        });
    }
    let end = *offset + usize::from(length);
    require_minimum(payload, end)?;
    let bytes = payload[*offset..end].to_vec();
    *offset = end;
    Ok(bytes)
}
