//! Local voice-prompt switches, volume and sound-effect settings.

#![doc = include_str!("../docs/voice-guidance.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A table-2 voice-guidance inquiry layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VoiceKind {
    /// Prompt switching with an additional capability header (`00`).
    OnOffWithMetadata,
    /// Prompt switching with a read-only language report (`01`).
    OnOffWithLanguageReport,
    /// Prompt switching whose writes must retain the current language (`02`).
    OnOffWithRetainedLanguage,
    /// Prompt switching without a capability query (`03`).
    OnOff,
    /// Prompt volume accompanying function `42` (`20`).
    Volume,
    /// A standalone prompt-volume function (`21`).
    FixedVolume,
    /// Battery-level announcements (`30`).
    BatteryAnnouncement,
    /// Power-on and power-off sounds (`31`).
    PowerSound,
    /// The ULT sound effect (`32`).
    UltBeep,
}

impl VoiceKind {
    /// Returns the wire inquiry byte.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::OnOffWithMetadata => 0,
            Self::OnOffWithLanguageReport => 1,
            Self::OnOffWithRetainedLanguage => 2,
            Self::OnOff => 3,
            Self::Volume => 0x20,
            Self::FixedVolume => 0x21,
            Self::BatteryAnnouncement => 0x30,
            Self::PowerSound => 0x31,
            Self::UltBeep => 0x32,
        }
    }

    /// Whether this layout has a `GET_CAPABILITY` command.
    pub const fn has_capability_query(self) -> bool {
        matches!(
            self,
            Self::OnOffWithMetadata
                | Self::OnOffWithLanguageReport
                | Self::OnOffWithRetainedLanguage
        )
    }

    /// Whether this layout has a local-setting availability query.
    pub const fn has_status_query(self) -> bool {
        !matches!(self, Self::Volume)
    }

    /// Encodes `GET_STATUS`, including the `ON_OFF` selector where required.
    ///
    /// Language-update status is outside this API's scope. Inquiry `20` has no
    /// status request; its companion prompt function supplies availability.
    pub fn status_request(self) -> Result<Vec<u8>, CommandError> {
        if !self.has_status_query() {
            return Err(unsupported("voice status query", self));
        }
        let mut payload = vec![0x42, self.inquiry()];
        if self.has_capability_query() {
            payload.push(0);
        }
        Ok(payload)
    }

    const fn is_volume(self) -> bool {
        matches!(self, Self::Volume | Self::FixedVolume)
    }
}

impl TryFrom<u8> for VoiceKind {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::OnOffWithMetadata),
            1 => Ok(Self::OnOffWithLanguageReport),
            2 => Ok(Self::OnOffWithRetainedLanguage),
            3 => Ok(Self::OnOff),
            0x20 => Ok(Self::Volume),
            0x21 => Ok(Self::FixedVolume),
            0x30 => Ok(Self::BatteryAnnouncement),
            0x31 => Ok(Self::PowerSound),
            0x32 => Ok(Self::UltBeep),
            value => Err(CommandError::InvalidValue {
                field: "voice inquiry",
                value,
            }),
        }
    }
}

/// An advertised voice function, preserving optional volume support.
///
/// Functions `41` and `42` share inquiry `01`; only `42` also permits volume
/// inquiry `20`. Retain the function identity after capability discovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VoiceProfile {
    /// Function `40`, inquiry `00`.
    OnOffWithMetadata,
    /// Function `41`, inquiry `01`, without volume control.
    OnOffWithLanguageReport,
    /// Function `42`, inquiries `01` and `20`.
    OnOffWithLanguageAndVolume,
    /// Function `43`, inquiry `21`.
    FixedVolume,
    /// Function `44`, inquiry `02`.
    OnOffWithRetainedLanguage,
    /// Function `45`, inquiry `03`.
    OnOff,
    /// Function `46`, inquiry `30`.
    BatteryAnnouncement,
    /// Function `47`, inquiry `31`.
    PowerSound,
    /// Function `48`, inquiry `32`.
    UltBeep,
}

impl VoiceProfile {
    /// Returns the primary inquiry for this advertised function.
    pub const fn kind(self) -> VoiceKind {
        match self {
            Self::OnOffWithMetadata => VoiceKind::OnOffWithMetadata,
            Self::OnOffWithLanguageReport | Self::OnOffWithLanguageAndVolume => {
                VoiceKind::OnOffWithLanguageReport
            }
            Self::FixedVolume => VoiceKind::FixedVolume,
            Self::OnOffWithRetainedLanguage => VoiceKind::OnOffWithRetainedLanguage,
            Self::OnOff => VoiceKind::OnOff,
            Self::BatteryAnnouncement => VoiceKind::BatteryAnnouncement,
            Self::PowerSound => VoiceKind::PowerSound,
            Self::UltBeep => VoiceKind::UltBeep,
        }
    }

    /// Returns the supported volume inquiry, if any.
    pub const fn volume_kind(self) -> Option<VoiceKind> {
        match self {
            Self::OnOffWithLanguageAndVolume => Some(VoiceKind::Volume),
            Self::FixedVolume => Some(VoiceKind::FixedVolume),
            _ => None,
        }
    }

    /// Returns the table-aware function identity.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_2,
            code: match self {
                Self::OnOffWithMetadata => 0x40,
                Self::OnOffWithLanguageReport => 0x41,
                Self::OnOffWithLanguageAndVolume => 0x42,
                Self::FixedVolume => 0x43,
                Self::OnOffWithRetainedLanguage => 0x44,
                Self::OnOff => 0x45,
                Self::BatteryAnnouncement => 0x46,
                Self::PowerSound => 0x47,
                Self::UltBeep => 0x48,
            },
        }
    }

    /// Selects a profile from a function advertised in table 2.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_2 {
            return None;
        }
        match function.code {
            0x40 => Some(Self::OnOffWithMetadata),
            0x41 => Some(Self::OnOffWithLanguageReport),
            0x42 => Some(Self::OnOffWithLanguageAndVolume),
            0x43 => Some(Self::FixedVolume),
            0x44 => Some(Self::OnOffWithRetainedLanguage),
            0x45 => Some(Self::OnOff),
            0x46 => Some(Self::BatteryAnnouncement),
            0x47 => Some(Self::PowerSound),
            0x48 => Some(Self::UltBeep),
            _ => None,
        }
    }
}

/// A read-only language identity; unknown identities retain their wire value.
///
/// These identities are specific to voice guidance. In particular, Chinese is
/// `f0`, and the language identifiers used by other command families may differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceLanguage(u8);

impl VoiceLanguage {
    /// Preserves a language identity without interpreting or selecting it.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }
    /// Returns the original wire identity.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// Prompt-switch support and read-only language identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceCapability {
    kind: VoiceKind,
    switch_supported: bool,
    languages: Vec<VoiceLanguage>,
    header_bytes: Option<[u8; 4]>,
}

impl VoiceCapability {
    /// Decodes an exact table-2 `VOICE_GUIDANCE_RET_CAPABILITY` (`41`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x41)?;
        let (switch_offset, languages_offset, header_bytes) = match kind {
            VoiceKind::OnOffWithMetadata | VoiceKind::OnOffWithLanguageReport => {
                require_minimum(payload, 7)?;
                require_minimum(payload, 3 + usize::from(payload[2]))?;
                for value in &payload[3..6] {
                    enabled(*value, "voice capability flag")?;
                }
                (
                    6,
                    if kind == VoiceKind::OnOffWithMetadata {
                        None
                    } else {
                        Some(7)
                    },
                    Some([payload[2], payload[3], payload[4], payload[5]]),
                )
            }
            VoiceKind::OnOffWithRetainedLanguage => {
                require_minimum(payload, 4)?;
                (2, Some(3), None)
            }
            _ => return Err(unsupported("voice capability response", kind)),
        };
        let switch_supported = match payload[switch_offset] {
            0 => false,
            1 => true,
            value => {
                return Err(CommandError::InvalidValue {
                    field: "voice switch support",
                    value,
                });
            }
        };
        let languages = if let Some(offset) = languages_offset {
            require_minimum(payload, offset + 1)?;
            require_length(payload, offset + 1 + usize::from(payload[offset]))?;
            payload[offset + 1..]
                .iter()
                .copied()
                .map(VoiceLanguage)
                .collect()
        } else {
            require_length(payload, 7)?;
            Vec::new()
        };
        Ok(Self {
            kind,
            switch_supported,
            languages,
            header_bytes,
        })
    }

    /// Constructs a fixed capability descriptor for an advertised function without a capability query.
    ///
    /// The caller must select the kind from the device's advertised function.
    /// This does not establish current availability; query status where supported.
    pub fn fixed(kind: VoiceKind) -> Result<Self, CommandError> {
        if kind.has_capability_query() {
            return Err(unsupported("fixed voice capability", kind));
        }
        Ok(Self {
            kind,
            switch_supported: !kind.is_volume(),
            languages: Vec::new(),
            header_bytes: None,
        })
    }

    /// Returns the capability's inquiry layout.
    pub const fn kind(&self) -> VoiceKind {
        self.kind
    }
    /// Whether changing the local prompt switch is supported.
    pub const fn supports_switch(&self) -> bool {
        self.switch_supported
    }
    /// Returns the language list exactly as advertised, including duplicates.
    pub fn languages(&self) -> &[VoiceLanguage] {
        &self.languages
    }
    /// Returns the four additional header bytes for inquiries `00/01`.
    ///
    /// Their wire values are retained for inspection. They do not authorize
    /// language-package operations through this API.
    pub const fn header_bytes(&self) -> Option<[u8; 4]> {
        self.header_bytes
    }
}

/// Availability of the local prompt or sound setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceStatus {
    kind: VoiceKind,
    /// Whether the setting is currently available (`00` on the wire).
    pub enabled: bool,
}

impl VoiceStatus {
    /// Decodes `RET_STATUS` (`43`) for local-setting availability.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x43)
    }
    /// Decodes `NTFY_STATUS` (`45`) for local-setting availability.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x45)
    }
    /// Returns the inquiry layout.
    pub const fn kind(self) -> VoiceKind {
        self.kind
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        if !kind.has_status_query() {
            return Err(unsupported("voice status report", kind));
        }
        let offset = if kind.has_capability_query() {
            require_length(payload, 4)?;
            if payload[2] != 0 {
                return Err(CommandError::InvalidValue {
                    field: "voice local-setting status selector",
                    value: payload[2],
                });
            }
            3
        } else {
            require_length(payload, 3)?;
            2
        };
        Ok(Self {
            kind,
            enabled: enabled(payload[offset], "voice availability")?,
        })
    }
}

/// A current local voice setting, with any language field retained read-only.
///
/// Inquiry `01` returns a language in `RET_PARAM` but omits it in `NTFY_PARAM`.
/// An absent notification language is not a request to clear a cached language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceParameters {
    kind: VoiceKind,
    value: VoiceValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VoiceValue {
    Switch {
        enabled: bool,
        language: Option<VoiceLanguage>,
    },
    Volume(i8),
}

impl VoiceParameters {
    /// Decodes an exact `RET_PARAM` (`47`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x47)
    }
    /// Decodes an exact `NTFY_PARAM` (`49`), including its inquiry-`01` short form.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x49)
    }
    /// Returns the inquiry layout.
    pub const fn kind(self) -> VoiceKind {
        self.kind
    }
    /// Returns the current switch value, if this is a switch layout.
    pub const fn enabled(self) -> Option<bool> {
        match self.value {
            VoiceValue::Switch { enabled, .. } => Some(enabled),
            VoiceValue::Volume(_) => None,
        }
    }
    /// Returns a reported language identity, if this packet includes one.
    pub const fn language(self) -> Option<VoiceLanguage> {
        match self.value {
            VoiceValue::Switch { language, .. } => language,
            VoiceValue::Volume(_) => None,
        }
    }
    /// Returns the signed native prompt-volume step, if this is a volume layout.
    pub const fn volume(self) -> Option<i8> {
        match self.value {
            VoiceValue::Volume(level) => Some(level),
            VoiceValue::Switch { .. } => None,
        }
    }

    /// Encodes a local on/off change using matching capability information.
    ///
    /// Inquiry `02` must resend its language field. This method retains the
    /// language from this report and verifies it is still advertised; it cannot
    /// select a replacement language. Use a current report and capability from
    /// the same device session. Other switch layouts send no language field.
    pub fn encode_switch(
        self,
        value: bool,
        capability: &VoiceCapability,
    ) -> Result<Vec<u8>, CommandError> {
        if self.kind != capability.kind {
            return Err(CommandError::CapabilityMismatch {
                expected: self.kind.inquiry(),
                actual: capability.kind.inquiry(),
            });
        }
        if self.kind.is_volume() || !capability.supports_switch() {
            return Err(unsupported("voice switch setting", self.kind));
        }
        let mut payload = vec![0x48, self.kind.inquiry(), on_off(value)];
        if self.kind == VoiceKind::OnOffWithRetainedLanguage {
            let language = self.language().ok_or(CommandError::InvalidParameter {
                field: "retained voice language",
            })?;
            if !capability.languages.contains(&language) {
                return Err(CommandError::InvalidParameter {
                    field: "retained voice language absent from capability",
                });
            }
            payload.push(language.as_raw());
        }
        Ok(payload)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        let has_language = kind == VoiceKind::OnOffWithRetainedLanguage
            || (kind == VoiceKind::OnOffWithLanguageReport && opcode == 0x47);
        require_length(payload, if has_language { 4 } else { 3 })?;
        let value = if kind.is_volume() {
            let level = payload[2] as i8;
            validate_volume(level)?;
            VoiceValue::Volume(level)
        } else {
            VoiceValue::Switch {
                enabled: enabled(payload[2], "voice switch")?,
                language: has_language.then(|| VoiceLanguage(payload[3])),
            }
        };
        Ok(Self { kind, value })
    }
}

/// A prompt-volume change, including an optional audible preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceVolumeSetting {
    /// Signed native volume step, from `-2` through `2` inclusive.
    pub level: i8,
    /// Whether the device should play a preview sound after applying the level.
    pub feedback: bool,
}

impl VoiceVolumeSetting {
    /// Encodes a volume write for advertised function `42` or `43`.
    ///
    /// Function `41` shares the prompt layout with `42` but does not authorize
    /// volume writes. The volume range is fixed by the protocol.
    pub fn encode_set(self, profile: VoiceProfile) -> Result<Vec<u8>, CommandError> {
        let kind = profile
            .volume_kind()
            .ok_or_else(|| unsupported("voice volume setting", profile.kind()))?;
        validate_volume(self.level)?;
        Ok(vec![
            0x48,
            kind.inquiry(),
            self.level as u8,
            on_off(self.feedback),
        ])
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<VoiceKind, CommandError> {
    require_table(table, CommandTable::TABLE_2)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    VoiceKind::try_from(payload[1])
}

fn enabled(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn on_off(value: bool) -> u8 {
    if value { 0 } else { 1 }
}

fn unsupported(operation: &'static str, kind: VoiceKind) -> CommandError {
    CommandError::UnsupportedOperation {
        operation,
        inquiry: kind.inquiry(),
    }
}

fn validate_volume(level: i8) -> Result<(), CommandError> {
    if !(-2..=2).contains(&level) {
        return Err(CommandError::InvalidValue {
            field: "voice volume step",
            value: level as u8,
        });
    }
    Ok(())
}
