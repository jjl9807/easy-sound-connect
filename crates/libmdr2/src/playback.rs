//! Playback reports, media metadata and capability-checked playback controls.

#![doc = include_str!("../docs/playback.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A table-1 PLAY capability, status or metadata wire layout.
///
/// Inquiry `01` is shared by two advertised profiles. Retain the selected
/// [`PlaybackProfile`] to determine which volume layouts are supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaybackKind {
    /// Playback and call-volume reporting, inquiry `01`.
    WithCallVolume,
    /// Playback and call volume with a separate control-availability flag, `02`.
    WithCallVolumeAndFunctionChange,
    /// Playback without call-volume reporting, inquiry `03`.
    WithFunctionChange,
}

impl PlaybackKind {
    /// Returns the inquiry byte for capability, status and metadata reads.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::WithCallVolume => 1,
            Self::WithCallVolumeAndFunctionChange => 2,
            Self::WithFunctionChange => 3,
        }
    }
}

impl TryFrom<u8> for PlaybackKind {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::WithCallVolume),
            2 => Ok(Self::WithCallVolumeAndFunctionChange),
            3 => Ok(Self::WithFunctionChange),
            value => Err(CommandError::InvalidValue {
                field: "playback inquiry",
                value,
            }),
        }
    }
}

/// An advertised playback function, including distinctions absent from replies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlaybackProfile {
    /// Function `a1`: ordinary music and call volume.
    WithCallVolume,
    /// Function `a2`: music and call volume with read-only mute reports.
    WithCallVolumeAndMute,
    /// Function `a3`: ordinary volumes and separate playback-control availability.
    WithCallVolumeAndFunctionChange,
    /// Function `a4`: ordinary music volume without call-volume support.
    WithFunctionChange,
}

impl PlaybackProfile {
    /// Returns the capability, status and metadata read layout.
    pub const fn kind(self) -> PlaybackKind {
        match self {
            Self::WithCallVolume | Self::WithCallVolumeAndMute => PlaybackKind::WithCallVolume,
            Self::WithCallVolumeAndFunctionChange => PlaybackKind::WithCallVolumeAndFunctionChange,
            Self::WithFunctionChange => PlaybackKind::WithFunctionChange,
        }
    }

    /// Returns the function identity required to select this profile.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::WithCallVolume => 0xa1,
                Self::WithCallVolumeAndMute => 0xa2,
                Self::WithCallVolumeAndFunctionChange => 0xa3,
                Self::WithFunctionChange => 0xa4,
            },
        }
    }

    /// Selects a profile from a table-aware advertised function.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0xa1 => Some(Self::WithCallVolume),
            0xa2 => Some(Self::WithCallVolumeAndMute),
            0xa3 => Some(Self::WithCallVolumeAndFunctionChange),
            0xa4 => Some(Self::WithFunctionChange),
            _ => None,
        }
    }

    /// Returns the supported music-volume layout.
    pub const fn music_volume_kind(self) -> VolumeKind {
        match self {
            Self::WithCallVolumeAndMute => VolumeKind::MusicWithMute,
            _ => VolumeKind::Music,
        }
    }

    /// Returns the call-volume layout, if the profile includes call volume.
    pub const fn call_volume_kind(self) -> Option<VolumeKind> {
        match self {
            Self::WithFunctionChange => None,
            Self::WithCallVolumeAndMute => Some(VolumeKind::CallWithMute),
            _ => Some(VolumeKind::Call),
        }
    }

    fn validate_capability(self, capability: &PlaybackCapability) -> Result<(), CommandError> {
        if self.kind() != capability.kind {
            return Err(CommandError::CapabilityMismatch {
                expected: self.kind().inquiry(),
                actual: capability.kind.inquiry(),
            });
        }
        Ok(())
    }

    fn control_inquiry(self) -> u8 {
        match self {
            Self::WithFunctionChange => 3,
            _ => 1,
        }
    }
}

/// A music or call-volume parameter layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VolumeKind {
    /// Music volume (`20`).
    Music,
    /// Call volume (`21`).
    Call,
    /// Music volume with a mute field in reports (`30`).
    MusicWithMute,
    /// Call volume with a mute field in reports (`31`).
    CallWithMute,
}

impl VolumeKind {
    /// Returns the inquiry byte used by parameter requests and reports.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Music => 0x20,
            Self::Call => 0x21,
            Self::MusicWithMute => 0x30,
            Self::CallWithMute => 0x31,
        }
    }

    /// Whether response and notification bodies include a mute field.
    pub const fn reports_mute(self) -> bool {
        matches!(self, Self::MusicWithMute | Self::CallWithMute)
    }
}

impl TryFrom<u8> for VolumeKind {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x20 => Ok(Self::Music),
            0x21 => Ok(Self::Call),
            0x30 => Ok(Self::MusicWithMute),
            0x31 => Ok(Self::CallWithMute),
            value => Err(CommandError::InvalidValue {
                field: "volume inquiry",
                value,
            }),
        }
    }
}

/// Native volume maxima supplied by a PLAY capability response.
///
/// Maxima are inclusive. They are neither percentages nor decibel values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackCapability {
    kind: PlaybackKind,
    music_maximum: u8,
    call_maximum: Option<u8>,
}

impl PlaybackCapability {
    /// Decodes an exact table-1 `PLAY_RET_CAPABILITY` (`a1`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        header(table, payload, 0xa1)?;
        let kind = PlaybackKind::try_from(payload[1])?;
        let calls = kind != PlaybackKind::WithFunctionChange;
        require_length(payload, if calls { 4 } else { 3 })?;
        if payload[2] == 0 || (calls && payload[3] == 0) {
            return Err(CommandError::InvalidCapability {
                field: "nonzero playback volume maximum",
            });
        }
        Ok(Self {
            kind,
            music_maximum: payload[2],
            call_maximum: calls.then(|| payload[3]),
        })
    }

    /// Returns the reply's wire layout; it does not distinguish profiles `a1/a2`.
    pub const fn kind(self) -> PlaybackKind {
        self.kind
    }
    /// Returns the largest permitted native music-volume value.
    pub const fn music_maximum(self) -> u8 {
        self.music_maximum
    }
    /// Returns the largest call-volume value, if this layout includes calls.
    pub const fn call_maximum(self) -> Option<u8> {
        self.call_maximum
    }

    fn maximum(self, profile: PlaybackProfile, volume: VolumeKind) -> Result<u8, CommandError> {
        profile.validate_capability(&self)?;
        if volume == profile.music_volume_kind() {
            Ok(self.music_maximum)
        } else if Some(volume) == profile.call_volume_kind() {
            self.call_maximum.ok_or(CommandError::InvalidCapability {
                field: "call volume maximum",
            })
        } else {
            Err(CommandError::UnsupportedOperation {
                operation: "volume layout for playback profile",
                inquiry: volume.inquiry(),
            })
        }
    }
}

/// Device-reported playback state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackState {
    /// Playback state has not been settled (`00`).
    Unsettled,
    /// Playing (`01`).
    Playing,
    /// Paused (`02`).
    Paused,
    /// Stopped (`03`).
    Stopped,
}

impl TryFrom<u8> for PlaybackState {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unsettled),
            1 => Ok(Self::Playing),
            2 => Ok(Self::Paused),
            3 => Ok(Self::Stopped),
            value => Err(CommandError::InvalidValue {
                field: "playback state",
                value,
            }),
        }
    }
}

/// Device-reported music/call context, present in inquiry `01/02`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MusicCallState {
    /// Music context (`00`).
    Music,
    /// Call context (`01`).
    Call,
}

impl TryFrom<u8> for MusicCallState {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Music),
            1 => Ok(Self::Call),
            value => Err(CommandError::InvalidValue {
                field: "music/call state",
                value,
            }),
        }
    }
}

/// Playback availability and device-reported state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackStatus {
    kind: PlaybackKind,
    /// The primary availability field (`00` enabled, `01` disabled).
    pub enabled: bool,
    /// Playback state, independent of availability.
    pub playback: PlaybackState,
    /// Music/call context, absent from inquiry `03`.
    pub music_call: Option<MusicCallState>,
    /// Additional control-availability field in inquiries `02/03`.
    pub control_enabled: Option<bool>,
}

impl PlaybackStatus {
    /// Returns the status layout.
    pub const fn kind(self) -> PlaybackKind {
        self.kind
    }
    /// Decodes `PLAY_RET_STATUS` (`a3`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa3)
    }
    /// Decodes `PLAY_NTFY_STATUS` (`a5`) separately from read replies.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa5)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        header(table, p, opcode)?;
        let kind = PlaybackKind::try_from(p[1])?;
        require_length(
            p,
            if kind == PlaybackKind::WithCallVolumeAndFunctionChange {
                6
            } else {
                5
            },
        )?;
        Ok(Self {
            kind,
            enabled: positive_zero(p[2], "playback availability")?,
            playback: p[3].try_into()?,
            music_call: if kind == PlaybackKind::WithFunctionChange {
                None
            } else {
                Some(p[4].try_into()?)
            },
            control_enabled: match kind {
                PlaybackKind::WithCallVolume => None,
                PlaybackKind::WithCallVolumeAndFunctionChange => {
                    Some(positive_zero(p[5], "playback control availability")?)
                }
                PlaybackKind::WithFunctionChange => {
                    Some(positive_zero(p[4], "playback control availability")?)
                }
            },
        })
    }
}

/// Availability of one metadata field, independent of its byte length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MetadataState {
    /// The field has not been settled (`00`).
    Unsettled,
    /// The device reports no metadata for this field (`01`).
    Nothing,
    /// Metadata is settled (`02`).
    Settled,
}

impl TryFrom<u8> for MetadataState {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Unsettled),
            1 => Ok(Self::Nothing),
            2 => Ok(Self::Settled),
            value => Err(CommandError::InvalidValue {
                field: "playback metadata state",
                value,
            }),
        }
    }
}

/// A metadata field with its original bytes and separate validity state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataText {
    /// The device's metadata state, retained even for an empty string.
    pub state: MetadataState,
    bytes: Vec<u8>,
}

impl MetadataText {
    /// Returns the exact name bytes, at most 128 bytes long.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Interprets the retained bytes as UTF-8 without replacement characters.
    pub fn as_utf8(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.bytes)
    }

    fn read(payload: &[u8], offset: &mut usize) -> Result<Self, CommandError> {
        require_minimum(payload, *offset + 2)?;
        let state = payload[*offset].try_into()?;
        let length = payload[*offset + 1];
        if length > 128 {
            return Err(CommandError::InvalidValue {
                field: "playback metadata byte length",
                value: length,
            });
        }
        let start = *offset + 2;
        let end = start + usize::from(length);
        require_minimum(payload, end)?;
        *offset = end;
        Ok(Self {
            state,
            bytes: payload[start..end].to_vec(),
        })
    }
}

/// Four metadata fields in track, album, artist and genre order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackMetadata {
    kind: PlaybackKind,
    /// Track title.
    pub track: MetadataText,
    /// Album name.
    pub album: MetadataText,
    /// Artist name.
    pub artist: MetadataText,
    /// Genre.
    pub genre: MetadataText,
}

impl PlaybackMetadata {
    /// Returns the metadata inquiry layout.
    pub const fn kind(&self) -> PlaybackKind {
        self.kind
    }
    /// Decodes metadata in a `PLAY_RET_PARAM` (`a7`) response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa7)
    }
    /// Decodes metadata in `PLAY_NTFY_PARAM` (`a9`).
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa9)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        header(table, p, opcode)?;
        let kind = PlaybackKind::try_from(p[1])?;
        let mut offset = 2;
        let result = Self {
            kind,
            track: MetadataText::read(p, &mut offset)?,
            album: MetadataText::read(p, &mut offset)?,
            artist: MetadataText::read(p, &mut offset)?,
            genre: MetadataText::read(p, &mut offset)?,
        };
        require_length(p, offset)?;
        Ok(result)
    }
}

/// A native music/call level and an optional read-only mute state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaybackVolume {
    kind: VolumeKind,
    /// Native level, decoded without requiring a capability response.
    pub level: u8,
    /// `Some(true)` means the reported mute switch is on (`00`).
    pub muted: Option<bool>,
}

impl PlaybackVolume {
    /// Returns the volume layout.
    pub const fn kind(self) -> VolumeKind {
        self.kind
    }
    /// Decodes a volume `PLAY_RET_PARAM` (`a7`) response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa7)
    }
    /// Decodes a volume `PLAY_NTFY_PARAM` (`a9`) notification.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0xa9)
    }
    /// Checks profile association and the capability's inclusive native maximum.
    pub fn validate_capability(
        self,
        profile: PlaybackProfile,
        capability: &PlaybackCapability,
    ) -> Result<(), CommandError> {
        check_level(self.level, capability.maximum(profile, self.kind)?)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        header(table, p, opcode)?;
        let kind = VolumeKind::try_from(p[1])?;
        require_length(p, if kind.reports_mute() { 4 } else { 3 })?;
        Ok(Self {
            kind,
            level: p[2],
            muted: if kind.reports_mute() {
                Some(positive_zero(p[3], "volume mute")?)
            } else {
                None
            },
        })
    }
}

/// A supported local playback operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackControl {
    /// Pause (`01`).
    Pause,
    /// Advance to the next track (`02`).
    NextTrack,
    /// Return to the previous track (`03`).
    PreviousTrack,
    /// Play (`07`).
    Play,
}

impl PlaybackControl {
    /// Encodes `PLAY_SET_STATUS` (`a4`) for a compatible advertised profile.
    ///
    /// Profiles `a1/a2/a3` use control inquiry `01`; profile `a4` uses `03`.
    /// A read inquiry must not be substituted for this operation inquiry.
    /// Check live availability separately. Do not repeat an unconfirmed track
    /// change as an application-level retry: a link ACK is not operation completion.
    pub fn encode_set(
        self,
        profile: PlaybackProfile,
        capability: &PlaybackCapability,
    ) -> Result<Vec<u8>, CommandError> {
        profile.validate_capability(capability)?;
        let action = match self {
            Self::Pause => 1,
            Self::NextTrack => 2,
            Self::PreviousTrack => 3,
            Self::Play => 7,
        };
        Ok(vec![0xa4, profile.control_inquiry(), 0, action])
    }
}

/// A volume change containing no writable mute flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeSetting {
    /// The volume layout supported by the selected profile.
    pub kind: VolumeKind,
    /// Native level in `0..=capability maximum`.
    pub level: u8,
}

impl VolumeSetting {
    /// Encodes `PLAY_SET_PARAM` (`a8`) after profile and range checks.
    ///
    /// All four supported volume writes are three bytes, including the layouts
    /// whose responses contain mute. Current mute state is never appended.
    /// The caller owns device/connection association and current availability.
    pub fn encode_set(
        self,
        profile: PlaybackProfile,
        capability: &PlaybackCapability,
    ) -> Result<Vec<u8>, CommandError> {
        check_level(self.level, capability.maximum(profile, self.kind)?)?;
        Ok(vec![0xa8, self.kind.inquiry(), self.level])
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<(), CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)
}

fn positive_zero(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn check_level(level: u8, maximum: u8) -> Result<(), CommandError> {
    if level <= maximum {
        Ok(())
    } else {
        Err(CommandError::InvalidParameter {
            field: "playback volume exceeds native maximum",
        })
    }
}
