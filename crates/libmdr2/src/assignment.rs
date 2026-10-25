//! Capability-driven physical-key and touch-action assignments.
//!
//! Read snapshots retain unknown identities. Writes edit one known local setting
//! in a decoded snapshot, preserving every unrelated value and its wire order.

#![doc = include_str!("../docs/assignment.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A table-1 assignment layout advertised by the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AssignmentKind {
    /// Standard assignments, inquiry `03`, function `f3`.
    Standard,
    /// Assignments carrying a limitation identity, inquiry `0e`, function `fe`.
    WithLimitations,
}

impl AssignmentKind {
    /// Returns the inquiry byte.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Standard => 3,
            Self::WithLimitations => 0x0e,
        }
    }
    /// Returns the table-aware advertised function identity.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::Standard => 0xf3,
                Self::WithLimitations => 0xfe,
            },
        }
    }
    /// Selects a known layout without inferring it from a model name.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0xf3 => Some(Self::Standard),
            0xfe => Some(Self::WithLimitations),
            _ => None,
        }
    }
}
impl TryFrom<u8> for AssignmentKind {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            3 => Ok(Self::Standard),
            0x0e => Ok(Self::WithLimitations),
            value => Err(CommandError::InvalidValue {
                field: "assignment inquiry",
                value,
            }),
        }
    }
}

macro_rules! identity {
    ($(#[$meta:meta])* $name:ident { $($(#[$item:meta])* $constant:ident = $value:expr;)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(u8);
        impl $name {
            $($(#[$item])* pub const $constant: Self = Self($value);)*
            /// Preserves a raw identity without granting permission to write it.
            pub const fn from_raw(value: u8) -> Self { Self(value) }
            /// Returns the original identity byte.
            pub const fn as_raw(self) -> u8 { self.0 }
        }
    };
}
identity! {
    /// A physical-key identity, including keys unknown to the library.
    AssignmentKey {
        /// Left-side control (`00`).
        LEFT = 0;
        /// Right-side control (`01`).
        RIGHT = 1;
        /// Custom button (`02`).
        CUSTOM = 2;
        /// C button (`03`).
        C = 3;
        /// NC/AMB button (`04`).
        NC_AMB = 4;
        /// NC/AMBIENT button (`05`).
        NC_AMBIENT = 5;
    }
}
identity! {
    /// The physical control type, including unknown identities.
    AssignmentKeyType {
        /// Touch sensor (`00`).
        TOUCH_SENSOR = 0;
        /// Physical button (`01`).
        BUTTON = 1;
        /// Face-tap input (`02`).
        FACE_TAP = 2;
    }
}
identity! {
    /// A preset identity. Unknown and service presets remain readable.
    AssignmentPreset {
        /// Ambient sound control (`00`).
        AMBIENT_SOUND = 0;
        /// Volume control (`10`).
        VOLUME = 0x10;
        /// Playback control (`20`).
        PLAYBACK = 0x20;
        /// Track control (`21`).
        TRACK = 0x21;
        /// Playback layout with voice-assistant restrictions (`22`).
        PLAYBACK_WITH_LIMITATION = 0x22;
        /// Ambient and microphone control (`45`).
        AMBIENT_MIC = 0x45;
        /// Ambient and listening-mode control (`47`).
        AMBIENT_LISTENING = 0x47;
        /// Game/chat balance (`70`).
        CHAT_MIX = 0x70;
        /// First custom preset (`71`).
        CUSTOM_1 = 0x71;
        /// Second custom preset (`72`).
        CUSTOM_2 = 0x72;
        /// No assigned function (`ff`).
        NONE = 0xff;
    }
}
impl AssignmentPreset {
    /// Whether this identity can be selected after its action map is validated.
    ///
    /// A local preset can still contain service actions. Its identity alone
    /// does not authorize activating the preset.
    pub const fn is_local(self) -> bool {
        matches!(
            self.0,
            0 | 0x10 | 0x20 | 0x21 | 0x22 | 0x45 | 0x47 | 0x70 | 0x71 | 0x72 | 0xff
        )
    }
}
identity! {
    /// A gesture or button-action identity, preserving unknown bytes.
    AssignmentAction {
        /// Single tap (`00`).
        SINGLE_TAP = 0;
        /// Double tap (`01`).
        DOUBLE_TAP = 1;
        /// Triple tap (`02`).
        TRIPLE_TAP = 2;
        /// Repeated taps (`03`).
        REPEAT_TAP = 3;
        /// Single tap and hold (`10`).
        SINGLE_TAP_HOLD = 0x10;
        /// Double tap and hold (`11`).
        DOUBLE_TAP_HOLD = 0x11;
        /// Long press to activate (`21`).
        LONG_PRESS_ACTIVATE = 0x21;
        /// Long press while active (`22`).
        LONG_PRESS_WHILE_ACTIVE = 0x22;
    }
}
impl AssignmentAction {
    /// Whether the action's meaning is implemented.
    pub const fn is_known(self) -> bool {
        matches!(self.0, 0..=3 | 0x10 | 0x11 | 0x21 | 0x22)
    }
}
identity! {
    /// An action's assigned function. Unknown and service identities are retained.
    AssignmentFunction {
        /// No function (`00`).
        NONE = 0;
        /// Cycle NC, ambient and off (`01`).
        NC_AMBIENT_OFF = 1;
        /// Cycle NC and ambient (`02`).
        NC_AMBIENT = 2;
        /// Cycle NC and off (`03`).
        NC_OFF = 3;
        /// Cycle ambient and off (`04`).
        AMBIENT_OFF = 4;
        /// Quick attention (`10`).
        QUICK_ATTENTION = 0x10;
        /// Play/pause (`20`).
        PLAY_PAUSE = 0x20;
        /// Next track (`21`).
        NEXT_TRACK = 0x21;
        /// Previous track (`22`).
        PREVIOUS_TRACK = 0x22;
        /// Volume up (`23`).
        VOLUME_UP = 0x23;
        /// Volume down (`24`).
        VOLUME_DOWN = 0x24;
        /// Ambient setting (`57`).
        AMBIENT_SETTING = 0x57;
        /// Standard/voice sound (`58`).
        STANDARD_VOICE_SOUND = 0x58;
        /// Local listening mode (`59`).
        LISTENING_MODE = 0x59;
        /// Microphone mute (`70`).
        MIC_MUTE = 0x70;
        /// Increase game balance (`71`).
        GAME_UP = 0x71;
        /// Increase chat balance (`72`).
        CHAT_UP = 0x72;
    }
}
impl AssignmentFunction {
    /// Whether a new binding to this device-local function is permitted.
    pub const fn is_local(self) -> bool {
        matches!(self.0, 0..=4 | 0x10 | 0x20..=0x24 | 0x57..=0x59 | 0x70..=0x72)
    }
}
identity! {
    /// A limitation identity carried by inquiry `0e`.
    AssignmentLimitation {
        /// No additional description (`00`).
        NONE = 0;
        /// Assistant presets are restricted during LE Audio (`01`).
        ASSISTANT_PRESET_WITH_LE_AUDIO = 1;
        /// Assistant actions under playback control are restricted during LE Audio (`02`).
        ASSISTANT_ACTION_WITH_LE_AUDIO = 2;
    }
}

/// One fixed or currently assigned action/function pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionBinding {
    /// Gesture or button action.
    pub action: AssignmentAction,
    /// Function assigned to that action.
    pub function: AssignmentFunction,
}
/// An action with an advertised default and selectable functions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomizableAction {
    /// Gesture or button action.
    pub action: AssignmentAction,
    /// Device-advertised default function.
    pub default_function: AssignmentFunction,
    /// Candidates in wire order, retaining unknown and service identities.
    pub functions: Vec<AssignmentFunction>,
}
/// A preset's fixed and customizable actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentPresetCapability {
    /// Preset identity.
    pub preset: AssignmentPreset,
    /// Fixed actions in wire order.
    pub fixed: Vec<ActionBinding>,
    /// Customizable actions in wire order.
    pub customizable: Vec<CustomizableAction>,
}
/// One physical key's capability, in the device's key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentKeyCapability {
    /// Physical-key identity.
    pub key: AssignmentKey,
    /// Physical input type.
    pub key_type: AssignmentKeyType,
    /// Advertised default preset, distinct from the current selection.
    pub default_preset: AssignmentPreset,
    /// Offered presets in wire order.
    pub presets: Vec<AssignmentPresetCapability>,
}
/// Complete nested assignment capabilities for one inquiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentCapability {
    kind: AssignmentKind,
    limitation: Option<AssignmentLimitation>,
    keys: Vec<AssignmentKeyCapability>,
}
impl AssignmentCapability {
    /// Decodes `SYSTEM_RET_CAPABILITY` (`f1`) with exact nested lengths.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0xf1)?;
        let mut cursor = Cursor::new(payload, 2);
        let limitation = if kind == AssignmentKind::WithLimitations {
            Some(AssignmentLimitation::from_raw(cursor.byte()?))
        } else {
            None
        };
        let count = cursor.count("assignment key count")?;
        let mut keys = Vec::new();
        for _ in 0..count {
            let key = AssignmentKey::from_raw(cursor.byte()?);
            let key_type = AssignmentKeyType::from_raw(cursor.byte()?);
            let default_preset = AssignmentPreset::from_raw(cursor.byte()?);
            let presets_count = cursor.count("assignment preset count")?;
            let mut presets = Vec::new();
            for _ in 0..presets_count {
                let preset = AssignmentPreset::from_raw(cursor.byte()?);
                let fixed_count = cursor.byte()?;
                let custom_count = cursor.byte()?;
                if fixed_count == 0 && custom_count == 0 {
                    return Err(invalid_cap("empty assignment preset"));
                }
                let fixed = cursor.bindings(fixed_count)?;
                let mut customizable = Vec::new();
                for _ in 0..custom_count {
                    let action = AssignmentAction::from_raw(cursor.byte()?);
                    let default_function = AssignmentFunction::from_raw(cursor.byte()?);
                    let functions_count = cursor.count("assignment function count")?;
                    let functions = cursor
                        .take(usize::from(functions_count))?
                        .iter()
                        .copied()
                        .map(AssignmentFunction::from_raw)
                        .collect();
                    customizable.push(CustomizableAction {
                        action,
                        default_function,
                        functions,
                    });
                }
                presets.push(AssignmentPresetCapability {
                    preset,
                    fixed,
                    customizable,
                });
            }
            keys.push(AssignmentKeyCapability {
                key,
                key_type,
                default_preset,
                presets,
            });
        }
        cursor.finish()?;
        Ok(Self {
            kind,
            limitation,
            keys,
        })
    }
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AssignmentKind {
        self.kind
    }
    /// Returns the optional limitation identity without interpreting unknown values.
    pub const fn limitation(&self) -> Option<AssignmentLimitation> {
        self.limitation
    }
    /// Returns physical controls in the order used by status and preset lists.
    pub fn keys(&self) -> &[AssignmentKeyCapability] {
        &self.keys
    }

    fn writable_key(
        &self,
        key: AssignmentKey,
    ) -> Result<(usize, &AssignmentKeyCapability), CommandError> {
        if self.limitation.is_some_and(|value| value.as_raw() > 2) {
            return Err(invalid_cap("unknown assignment limitation"));
        }
        let mut matching = self
            .keys
            .iter()
            .enumerate()
            .filter(|(_, item)| item.key == key);
        let result = matching
            .next()
            .ok_or_else(|| invalid_param("advertised assignment key"))?;
        if matching.next().is_some() {
            return Err(invalid_cap("ambiguous assignment key"));
        }
        if key.as_raw() > 5 || result.1.key_type.as_raw() > 2 {
            return Err(invalid_param("known assignment key type"));
        }
        Ok(result)
    }
}

/// Per-key availability, in capability key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentStatus {
    kind: AssignmentKind,
    enabled: Vec<bool>,
}
impl AssignmentStatus {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AssignmentKind {
        self.kind
    }
    /// Returns one availability flag per physical key.
    pub fn enabled(&self) -> &[bool] {
        &self.enabled
    }
    /// Decodes `f3`; availability uses `00` enabled and `01` disabled.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf3)
    }
    /// Decodes the separate `f5` status notification.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xf5)
    }
    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        let values = counted_body(payload)?;
        let enabled = values
            .iter()
            .map(|value| match value {
                0 => Ok(true),
                1 => Ok(false),
                value => Err(CommandError::InvalidValue {
                    field: "assignment availability",
                    value: *value,
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { kind, enabled })
    }
    /// Checks that the report contains exactly one entry per capability key.
    pub fn validate_capability(
        &self,
        capability: &AssignmentCapability,
    ) -> Result<(), CommandError> {
        same_kind(self.kind, capability.kind)?;
        same_key_count(self.enabled.len(), capability.keys.len())
    }
}

/// Current presets, in capability key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentParameters {
    kind: AssignmentKind,
    presets: Vec<AssignmentPreset>,
}
impl AssignmentParameters {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AssignmentKind {
        self.kind
    }
    /// Returns current presets, retaining unknown and service selections.
    pub fn presets(&self) -> &[AssignmentPreset] {
        &self.presets
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
        let presets = counted_body(payload)?
            .iter()
            .copied()
            .map(AssignmentPreset::from_raw)
            .collect();
        Ok(Self { kind, presets })
    }
    /// Checks the key count and inquiry without rejecting retained unknown presets.
    pub fn validate_capability(
        &self,
        capability: &AssignmentCapability,
    ) -> Result<(), CommandError> {
        same_kind(self.kind, capability.kind)?;
        same_key_count(self.presets.len(), capability.keys.len())
    }
    /// Encodes a complete `f8` preset list while changing only one physical key.
    ///
    /// The new preset must be advertised and local, and all functions activated
    /// by it must be local. If it has customizable actions, provide a decoded
    /// custom snapshot containing its complete action map. Unknown or service
    /// selections on other keys are retained byte-for-byte. The snapshot must
    /// belong to the same connection and capability revision.
    pub fn encode_preset_change(
        &self,
        capability: &AssignmentCapability,
        key: AssignmentKey,
        preset: AssignmentPreset,
        custom: Option<&AssignmentCustomParameters>,
    ) -> Result<Vec<u8>, CommandError> {
        self.validate_capability(capability)?;
        let (index, key_capability) = capability.writable_key(key)?;
        if !preset.is_local() {
            return Err(invalid_param("local assignment preset"));
        }
        let candidate = unique_preset(key_capability, preset)?;
        if self.presets[index] != preset {
            validate_local_activation(candidate, custom, self.kind)?;
        }
        let mut payload = vec![0xf8, self.kind.inquiry(), self.presets.len() as u8];
        payload.extend(self.presets.iter().enumerate().map(|(position, current)| {
            if position == index {
                preset.as_raw()
            } else {
                current.as_raw()
            }
        }));
        Ok(payload)
    }
}

/// A preset-keyed extended action map; it carries no physical-key identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomPreset {
    /// Preset identifying this record.
    pub preset: AssignmentPreset,
    /// Action/function pairs in their original order.
    pub bindings: Vec<ActionBinding>,
}
/// Extended assignment records, keyed by preset identity rather than key index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssignmentCustomParameters {
    kind: AssignmentKind,
    records: Vec<CustomPreset>,
}
impl AssignmentCustomParameters {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AssignmentKind {
        self.kind
    }
    /// Returns records in wire order, preserving unknown and service bindings.
    pub fn records(&self) -> &[CustomPreset] {
        &self.records
    }
    /// Decodes `SYSTEM_RET_EXTENDED_PARAM` (`fb`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xfb)
    }
    /// Decodes `SYSTEM_NTFY_EXTENDED_PARAM` (`fd`).
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xfd)
    }
    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        let mut cursor = Cursor::new(payload, 2);
        let count = cursor.count("assignment custom record count")?;
        let mut records = Vec::new();
        for _ in 0..count {
            let preset = AssignmentPreset::from_raw(cursor.byte()?);
            let count = cursor.count("assignment custom action count")?;
            records.push(CustomPreset {
                preset,
                bindings: cursor.bindings(count)?,
            });
        }
        cursor.finish()?;
        Ok(Self { kind, records })
    }
    /// Encodes `fc`, replacing one customizable function and retaining all others.
    ///
    /// The current preset must identify exactly one physical key and exactly
    /// one extended record. The target action must be known, customizable and
    /// unambiguous; the replacement must be a local advertised function. Unknown
    /// and service values elsewhere are copied from the read snapshot unchanged.
    /// No records or actions are synthesized, reordered or removed.
    pub fn encode_function_change(
        &self,
        capability: &AssignmentCapability,
        current: &AssignmentParameters,
        key: AssignmentKey,
        action: AssignmentAction,
        function: AssignmentFunction,
    ) -> Result<Vec<u8>, CommandError> {
        same_kind(self.kind, capability.kind)?;
        current.validate_capability(capability)?;
        let (key_index, key_capability) = capability.writable_key(key)?;
        let preset = current.presets[key_index];
        if !preset.is_local() || !action.is_known() || !function.is_local() {
            return Err(invalid_param("local assignment action"));
        }
        if current
            .presets
            .iter()
            .filter(|value| **value == preset)
            .count()
            != 1
        {
            return Err(invalid_param("ambiguous active assignment preset"));
        }
        let candidate = unique_preset(key_capability, preset)?;
        if candidate
            .fixed
            .iter()
            .any(|binding| binding.action == action)
        {
            return Err(invalid_param("fixed assignment action"));
        }
        let mut choices = candidate
            .customizable
            .iter()
            .filter(|choice| choice.action == action);
        let choice = choices
            .next()
            .ok_or_else(|| invalid_param("customizable assignment action"))?;
        if choices.next().is_some() {
            return Err(invalid_cap("ambiguous customizable action"));
        }
        if !choice.functions.contains(&function) {
            return Err(invalid_param("advertised assignment function"));
        }
        let record_index = self.record_index(preset)?;
        let record = &self.records[record_index];
        let mut matches = record
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.action == action);
        let (action_index, _) = matches
            .next()
            .ok_or_else(|| invalid_param("existing assignment action"))?;
        if matches.next().is_some() {
            return Err(invalid_param("ambiguous current assignment action"));
        }
        let mut payload = vec![0xfc, self.kind.inquiry(), self.records.len() as u8];
        for (r, record) in self.records.iter().enumerate() {
            payload.extend([record.preset.as_raw(), record.bindings.len() as u8]);
            for (a, binding) in record.bindings.iter().enumerate() {
                let value = if r == record_index && a == action_index {
                    function
                } else {
                    binding.function
                };
                payload.extend([binding.action.as_raw(), value.as_raw()]);
            }
        }
        Ok(payload)
    }
    fn record_index(&self, preset: AssignmentPreset) -> Result<usize, CommandError> {
        let mut matches = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.preset == preset);
        let (index, _) = matches
            .next()
            .ok_or_else(|| invalid_param("existing assignment custom preset"))?;
        if matches.next().is_some() {
            return Err(invalid_param("ambiguous assignment custom preset"));
        }
        Ok(index)
    }
}

fn validate_local_activation(
    candidate: &AssignmentPresetCapability,
    custom: Option<&AssignmentCustomParameters>,
    kind: AssignmentKind,
) -> Result<(), CommandError> {
    if candidate.fixed.iter().enumerate().any(|(index, binding)| {
        candidate.fixed[..index]
            .iter()
            .any(|other| other.action == binding.action)
            || !binding.action.is_known()
            || !binding.function.is_local()
    }) {
        return Err(invalid_param("local fixed assignment actions"));
    }
    if candidate.customizable.is_empty() {
        return Ok(());
    }
    let custom = custom.ok_or_else(|| invalid_param("current custom action map required"))?;
    same_kind(custom.kind, kind)?;
    let record = &custom.records[custom.record_index(candidate.preset)?];
    // A new selection activates the whole map; validate every field, not only
    // the function edited most recently.
    if record.bindings.len() != candidate.fixed.len() + candidate.customizable.len() {
        return Err(invalid_param("complete assignment action map"));
    }
    for (i, binding) in record.bindings.iter().enumerate() {
        if record.bindings[..i]
            .iter()
            .any(|other| other.action == binding.action)
            || !binding.action.is_known()
            || !binding.function.is_local()
        {
            return Err(invalid_param("unambiguous local assignment map"));
        }
        let fixed = candidate
            .fixed
            .iter()
            .filter(|item| item.action == binding.action)
            .collect::<Vec<_>>();
        let choices = candidate
            .customizable
            .iter()
            .filter(|item| item.action == binding.action)
            .collect::<Vec<_>>();
        match (fixed.as_slice(), choices.as_slice()) {
            ([fixed], []) if fixed.function == binding.function => {}
            ([], [choice]) if choice.functions.contains(&binding.function) => {}
            _ => return Err(invalid_param("advertised assignment action map")),
        }
    }
    Ok(())
}
fn unique_preset(
    key: &AssignmentKeyCapability,
    preset: AssignmentPreset,
) -> Result<&AssignmentPresetCapability, CommandError> {
    let mut matches = key
        .presets
        .iter()
        .filter(|candidate| candidate.preset == preset);
    let candidate = matches
        .next()
        .ok_or_else(|| invalid_param("advertised assignment preset"))?;
    if matches.next().is_some() {
        return Err(invalid_cap("ambiguous assignment preset"));
    }
    Ok(candidate)
}
fn same_key_count(actual: usize, expected: usize) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(invalid_param("assignment key count"))
    }
}
fn same_kind(actual: AssignmentKind, expected: AssignmentKind) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CommandError::CapabilityMismatch {
            expected: expected.inquiry(),
            actual: actual.inquiry(),
        })
    }
}
fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<AssignmentKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    AssignmentKind::try_from(payload[1])
}
fn counted_body(payload: &[u8]) -> Result<&[u8], CommandError> {
    require_minimum(payload, 3)?;
    if payload[2] == 0 {
        return Err(invalid_param("nonempty assignment list"));
    }
    require_length(payload, 3 + usize::from(payload[2]))?;
    Ok(&payload[3..])
}
fn invalid_cap(field: &'static str) -> CommandError {
    CommandError::InvalidCapability { field }
}
fn invalid_param(field: &'static str) -> CommandError {
    CommandError::InvalidParameter { field }
}
struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], offset: usize) -> Self {
        Self { bytes, offset }
    }
    fn byte(&mut self) -> Result<u8, CommandError> {
        Ok(self.take(1)?[0])
    }
    fn count(&mut self, field: &'static str) -> Result<u8, CommandError> {
        match self.byte()? {
            0 => Err(invalid_param(field)),
            count => Ok(count),
        }
    }
    fn take(&mut self, length: usize) -> Result<&'a [u8], CommandError> {
        let end = self.offset + length;
        require_minimum(self.bytes, end)?;
        let bytes = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(bytes)
    }
    fn bindings(&mut self, count: u8) -> Result<Vec<ActionBinding>, CommandError> {
        Ok(self
            .take(2 * usize::from(count))?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| ActionBinding {
                action: AssignmentAction::from_raw(pair[0]),
                function: AssignmentFunction::from_raw(pair[1]),
            })
            .collect())
    }
    fn finish(self) -> Result<(), CommandError> {
        require_length(self.bytes, self.offset)
    }
}
