//! Table-1 audio settings selected by advertised capabilities.
//!
//! Availability, configured parameters and effect indicators are distinct.
//! This module encodes device settings and does not provide an audio engine.

#![doc = include_str!("../docs/audio.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A supported AUDIO inquiry in MDR v2 command table 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioKind {
    /// Sound-quality or connection-quality priority (`00`, function `e1`).
    ConnectionPriority,
    /// DSEE auto/off (`01`, function `e2`).
    Dsee,
    /// Connection priority with a second availability flag (`02`, function `e3`).
    ConnectionPriorityWithStatus,
    /// Background listening with a room size (`03`, function `e4`).
    Bgm,
    /// Cinema upmix switch (`04`, function `e5`).
    UpmixCinema,
    /// Classic/LE Audio priority selection (`05`, function `e7`).
    ClassicLePriority,
    /// Sound leakage reduction switch (`07`, function `e9`).
    SoundLeakageReduction,
    /// Assignment of local listening modes (`08`, function `ea`).
    ListeningOptions,
    /// Background listening with disabling reasons (`09`, function `eb`).
    BgmWithReasons,
    /// Local upmix selection (`0a`, function `ec`).
    UpmixSeries,
    /// DSEE auto/off with disabling reasons (`0b`, function `ed`).
    DseeWithReasons,
}

impl AudioKind {
    /// Returns the inquiry following the AUDIO opcode.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::ConnectionPriority => 0,
            Self::Dsee => 1,
            Self::ConnectionPriorityWithStatus => 2,
            Self::Bgm => 3,
            Self::UpmixCinema => 4,
            Self::ClassicLePriority => 5,
            Self::SoundLeakageReduction => 7,
            Self::ListeningOptions => 8,
            Self::BgmWithReasons => 9,
            Self::UpmixSeries => 10,
            Self::DseeWithReasons => 11,
        }
    }

    /// Returns the function identity advertising this layout.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::ConnectionPriority => 0xe1,
                Self::Dsee => 0xe2,
                Self::ConnectionPriorityWithStatus => 0xe3,
                Self::Bgm => 0xe4,
                Self::UpmixCinema => 0xe5,
                Self::ClassicLePriority => 0xe7,
                Self::SoundLeakageReduction => 0xe9,
                Self::ListeningOptions => 0xea,
                Self::BgmWithReasons => 0xeb,
                Self::UpmixSeries => 0xec,
                Self::DseeWithReasons => 0xed,
            },
        }
    }

    /// Maps a table-aware advertised function without choosing between variants.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_1 {
            return None;
        }
        match function.code {
            0xe1 => Some(Self::ConnectionPriority),
            0xe2 => Some(Self::Dsee),
            0xe3 => Some(Self::ConnectionPriorityWithStatus),
            0xe4 => Some(Self::Bgm),
            0xe5 => Some(Self::UpmixCinema),
            0xe7 => Some(Self::ClassicLePriority),
            0xe9 => Some(Self::SoundLeakageReduction),
            0xea => Some(Self::ListeningOptions),
            0xeb => Some(Self::BgmWithReasons),
            0xec => Some(Self::UpmixSeries),
            0xed => Some(Self::DseeWithReasons),
            _ => None,
        }
    }

    /// Whether the inquiry defines a dynamic capability query.
    pub const fn has_capability_query(self) -> bool {
        !matches!(
            self,
            Self::ConnectionPriority | Self::ConnectionPriorityWithStatus
        )
    }
}

impl TryFrom<u8> for AudioKind {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::ConnectionPriority),
            1 => Ok(Self::Dsee),
            2 => Ok(Self::ConnectionPriorityWithStatus),
            3 => Ok(Self::Bgm),
            4 => Ok(Self::UpmixCinema),
            5 => Ok(Self::ClassicLePriority),
            7 => Ok(Self::SoundLeakageReduction),
            8 => Ok(Self::ListeningOptions),
            9 => Ok(Self::BgmWithReasons),
            10 => Ok(Self::UpmixSeries),
            11 => Ok(Self::DseeWithReasons),
            value => Err(CommandError::InvalidValue {
                field: "audio inquiry",
                value,
            }),
        }
    }
}

macro_rules! identity {
    ($(#[$meta:meta])* $name:ident { $($(#[$doc:meta])* $constant:ident = $value:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(u8);
        impl $name {
            $($(#[$doc])* pub const $constant: Self = Self($value);)*
            /// Preserves an identity byte, including unrecognized values.
            pub const fn from_raw(value: u8) -> Self { Self(value) }
            /// Returns the original identity byte.
            pub const fn as_raw(self) -> u8 { self.0 }
        }
    };
}

identity! {
    /// Device-reported DSEE technology, independent of its configured auto/off mode.
    UpscalingType {
        /// DSEE HX (`00`).
        DSEE_HX = 0,
        /// DSEE (`01`).
        DSEE = 1,
        /// DSEE Extreme / HX AI (`02`).
        DSEE_EXTREME = 2,
        /// DSEE Ultimate (`03`).
        DSEE_ULTIMATE = 3,
    }
}
identity! {
    /// Connection priority identity. Dynamic capabilities can contain unknown values.
    PriorityMode {
        /// Prioritize sound quality (`00`).
        SOUND_QUALITY = 0,
        /// Prioritize connection quality (`01`).
        CONNECTION_QUALITY = 1,
        /// LE Audio low-latency priority, marked beta in the protocol (`02`).
        LOW_LATENCY = 2,
    }
}
identity! {
    /// A reason that an audio setting is unavailable; unknown reasons are retained.
    AudioErrorCode {
        /// A call is active (`00`).
        CALLING = 0,
        /// Demo mode (`01`).
        DEMO_MODE = 1,
        /// LE Audio is connected (`02`).
        LE_AUDIO_CONNECTED = 2,
        /// LE Audio music is playing (`03`).
        LE_AUDIO_MUSIC_PLAYING = 3,
        /// The control connection uses GATT (`04`).
        CONTROL_OVER_GATT = 4,
        /// The listening mode is not standard (`0b`).
        NONSTANDARD_LISTENING_MODE = 11,
        /// Other reason (`fe`).
        OTHER = 0xfe,
    }
}
identity! {
    /// Identity of a function mutually exclusive with an audio setting.
    AudioExclusiveFunction {
        /// Equalizer (`00`).
        EQUALIZER = 0,
        /// DSEE (`01`).
        DSEE = 1,
        /// Head tracker (`02`).
        HEAD_TRACKER = 2,
        /// LE Audio (`03`).
        LE_AUDIO = 3,
    }
}
identity! {
    /// Identity of a capability mutually exclusive with LDAC operation.
    LdacExclusiveFeature {
        /// GATT control connectivity (`00`).
        GATT_CONNECTABLE = 0,
    }
}
identity! {
    /// A device-local upmix selection. Unknown reported identities are retained.
    UpmixItem {
        /// No upmix (`00`).
        NONE = 0,
        /// Cinema (`01`).
        CINEMA = 1,
        /// Game (`02`).
        GAME = 2,
        /// Music (`03`).
        MUSIC = 3,
    }
}
identity! {
    /// A local listening mode available for assignment.
    ListeningOption {
        /// Standard listening (`00`).
        STANDARD = 0,
        /// Background listening (`01`).
        BGM = 1,
        /// Cinema upmix (`02`).
        CINEMA = 2,
        /// Game upmix (`03`).
        GAME = 3,
        /// Music upmix (`04`).
        MUSIC = 4,
    }
}
identity! {
    /// Preferred rendering effect reported by upmix availability.
    PreferredSoundEffect {
        /// Headphone upmix (`00`).
        HEADPHONE_UPMIX = 0,
        /// BRAVIA 3D Surround (`01`).
        BRAVIA_3D_SURROUND = 1,
    }
}

macro_rules! wire_enum {
    ($(#[$meta:meta])* $name:ident, $field:literal, { $($(#[$doc:meta])* $variant:ident = $value:literal),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($(#[$doc])* $variant = $value),* }
        impl $name {
            /// Returns the encoded byte.
            pub const fn as_u8(self) -> u8 { self as u8 }
        }
        impl TryFrom<u8> for $name {
            type Error = CommandError;
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)*
                    value => Err(CommandError::InvalidValue { field: $field, value }),
                }
            }
        }
    };
}
wire_enum! {
    /// The configured DSEE mode; this does not indicate whether processing is active.
    DseeMode, "DSEE mode", {
        /// Disabled (`00`).
        Off = 0,
        /// Automatic (`01`).
        Auto = 1,
    }
}
wire_enum! {
    /// Room-size setting for background listening.
    RoomSize, "background room size", {
        /// Small (`00`).
        Small = 0,
        /// Middle (`01`).
        Middle = 1,
        /// Large (`02`).
        Large = 2,
    }
}
wire_enum! {
    /// Stream transition reported only by the classic/LE priority notification.
    SwitchingStream, "switching stream", {
        /// No stream transition (`00`).
        None = 0,
        /// Transition to LE Audio (`01`).
        LeAudio = 1,
        /// Transition to Classic Audio (`02`).
        ClassicAudio = 2,
    }
}

/// An advertised listening-mode assignment and its inclusion requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListeningOptionCapability {
    /// Stable option identity.
    pub option: ListeningOption,
    /// Whether an assignment must include this option.
    pub mandatory: bool,
}

/// Fields belonging to an audio capability descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioCapabilityData {
    /// Selectable priorities and LDAC restrictions.
    Connection {
        /// Priority identities in device order.
        priorities: Vec<PriorityMode>,
        /// Restrictions advertised by inquiry `05`; empty for fixed layouts.
        ldac_exclusive: Vec<LdacExclusiveFeature>,
    },
    /// DSEE technology.
    Dsee(UpscalingType),
    /// Mutually exclusive features of BGM, cinema or sound leakage reduction.
    ExclusiveFunctions(Vec<AudioExclusiveFunction>),
    /// Assignment flexibility and inclusion requirements.
    ListeningOptions {
        /// The capability's flexibility flag (`00` fixed, `01` flexible).
        flexible: bool,
        /// Option identities, requirements and original ordering.
        options: Vec<ListeningOptionCapability>,
    },
    /// Upmix choices and mutually exclusive features.
    Upmix {
        /// Device-advertised upmix identities.
        items: Vec<UpmixItem>,
        /// Mutually exclusive function identities.
        exclusive: Vec<AudioExclusiveFunction>,
    },
}

/// A validated audio capability descriptor associated with one inquiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioCapability {
    kind: AudioKind,
    data: AudioCapabilityData,
}

impl AudioCapability {
    /// Constructs fixed capabilities for inquiries `00` and `02`.
    ///
    /// Use only after discovering the matching function. These layouts have no
    /// capability query and allow sound-quality or connection-quality priority.
    pub fn static_connection(kind: AudioKind) -> Result<Self, CommandError> {
        if kind.has_capability_query() {
            return Err(unsupported("static connection capability", kind));
        }
        Ok(Self {
            kind,
            data: AudioCapabilityData::Connection {
                priorities: vec![
                    PriorityMode::SOUND_QUALITY,
                    PriorityMode::CONNECTION_QUALITY,
                ],
                ldac_exclusive: Vec::new(),
            },
        })
    }

    /// Decodes an exact table-1 `AUDIO_RET_CAPABILITY` (`e1`) payload.
    pub fn decode_response(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, p, 0xe1)?;
        if !kind.has_capability_query() {
            return Err(unsupported("audio capability response", kind));
        }
        let data = match kind {
            AudioKind::Dsee | AudioKind::DseeWithReasons => {
                require_length(p, 3)?;
                AudioCapabilityData::Dsee(UpscalingType::from_raw(p[2]))
            }
            AudioKind::ClassicLePriority => {
                let (priorities, end) = counted_bytes(p, 2)?;
                if priorities.len() < 2 {
                    return Err(CommandError::InvalidCapability {
                        field: "connection priority count",
                    });
                }
                let (exclusive, end) = counted_bytes(p, end)?;
                require_length(p, end)?;
                AudioCapabilityData::Connection {
                    priorities: priorities
                        .iter()
                        .copied()
                        .map(PriorityMode::from_raw)
                        .collect(),
                    ldac_exclusive: exclusive
                        .iter()
                        .copied()
                        .map(LdacExclusiveFeature::from_raw)
                        .collect(),
                }
            }
            AudioKind::ListeningOptions => {
                require_minimum(p, 4)?;
                let flexible = binary(p[2], "listening assignment flexibility")? == 1;
                require_length(p, 4 + usize::from(p[3]) * 2)?;
                let options = p[4..]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| {
                        Ok(ListeningOptionCapability {
                            option: ListeningOption::from_raw(pair[0]),
                            mandatory: enabled(pair[1], "listening mandatory flag")?,
                        })
                    })
                    .collect::<Result<Vec<_>, CommandError>>()?;
                AudioCapabilityData::ListeningOptions { flexible, options }
            }
            AudioKind::UpmixSeries => {
                let (items, end) = counted_bytes(p, 2)?;
                let (exclusive, end) = counted_bytes(p, end)?;
                require_length(p, end)?;
                AudioCapabilityData::Upmix {
                    items: items.iter().copied().map(UpmixItem::from_raw).collect(),
                    exclusive: exclusive
                        .iter()
                        .copied()
                        .map(AudioExclusiveFunction::from_raw)
                        .collect(),
                }
            }
            _ => {
                let (items, end) = counted_bytes(p, 2)?;
                require_length(p, end)?;
                AudioCapabilityData::ExclusiveFunctions(
                    items
                        .iter()
                        .copied()
                        .map(AudioExclusiveFunction::from_raw)
                        .collect(),
                )
            }
        };
        Ok(Self { kind, data })
    }

    /// Returns the inquiry associated with this descriptor.
    pub const fn kind(&self) -> AudioKind {
        self.kind
    }

    /// Returns the descriptor's fields, retaining unknown identities and ordering.
    pub const fn data(&self) -> &AudioCapabilityData {
        &self.data
    }
}

/// Additional fields carried by an audio availability message.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioStatusDetails {
    /// No additional fields.
    None,
    /// A second inverted Boolean in connection layouts `02` and `05`.
    Connection {
        /// Offset 3 decoded as `00` enabled, `01` disabled; no codec identity is implied.
        secondary_enabled: bool,
    },
    /// Device-supplied disabling reasons, including unknown values.
    Reasons(Vec<AudioErrorCode>),
    /// Assignment status carried by inquiry `08`.
    ListeningOptions {
        /// Whether listening modes are assigned (`00` yes, `01` no).
        assigned: bool,
    },
    /// The preferred effect carried by inquiry `0a`.
    Upmix(PreferredSoundEffect),
}

/// Current availability; distinct from the configured on/off parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioStatus {
    kind: AudioKind,
    /// Whether the setting is currently operable (`00` available, `01` unavailable).
    pub enabled: bool,
    /// Additional fields determined by the inquiry.
    pub details: AudioStatusDetails,
}

impl AudioStatus {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AudioKind {
        self.kind
    }
    /// Decodes `AUDIO_RET_STATUS` (`e3`).
    pub fn decode_response(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0xe3)
    }
    /// Decodes `AUDIO_NTFY_STATUS` (`e5`).
    pub fn decode_notification(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0xe5)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, p, opcode)?;
        require_minimum(p, 3)?;
        let available = enabled(p[2], "audio availability")?;
        let details = match kind {
            AudioKind::ConnectionPriorityWithStatus | AudioKind::ClassicLePriority => {
                require_length(p, 4)?;
                AudioStatusDetails::Connection {
                    secondary_enabled: enabled(p[3], "secondary audio availability")?,
                }
            }
            AudioKind::BgmWithReasons | AudioKind::DseeWithReasons => {
                let (reasons, end) = counted_bytes(p, 3)?;
                require_length(p, end)?;
                AudioStatusDetails::Reasons(
                    reasons
                        .iter()
                        .copied()
                        .map(AudioErrorCode::from_raw)
                        .collect(),
                )
            }
            AudioKind::ListeningOptions => {
                require_length(p, 4)?;
                AudioStatusDetails::ListeningOptions {
                    assigned: enabled(p[3], "listening assignment status")?,
                }
            }
            AudioKind::UpmixSeries => {
                require_length(p, 4)?;
                AudioStatusDetails::Upmix(PreferredSoundEffect::from_raw(p[3]))
            }
            _ => {
                require_length(p, 3)?;
                AudioStatusDetails::None
            }
        };
        Ok(Self {
            kind,
            enabled: available,
            details,
        })
    }
}

/// Device-reported fields of an audio parameter payload.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioParameterValue {
    /// Connection priority, with a transition field only on inquiry `05` notifications.
    ConnectionPriority {
        /// Configured priority identity.
        priority: PriorityMode,
        /// Present on `e9 05`; absent on `e7 05` and inquiries `00`/`02`.
        switching_stream: Option<SwitchingStream>,
    },
    /// DSEE auto/off.
    Dsee(DseeMode),
    /// Background listening, preserving room size when disabled.
    Bgm {
        /// `00` on, `01` off.
        enabled: bool,
        /// Configured room size.
        room_size: RoomSize,
    },
    /// Cinema upmix or sound leakage reduction (`00` on, `01` off).
    Switch(bool),
    /// Complete assignment in device order.
    ListeningOptions(Vec<ListeningOption>),
    /// Selected local upmix identity.
    Upmix(UpmixItem),
}

/// An exact response or notification parameter layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioParameters {
    kind: AudioKind,
    value: AudioParameterValue,
}

impl AudioParameters {
    /// Returns the inquiry layout.
    pub const fn kind(&self) -> AudioKind {
        self.kind
    }
    /// Returns the decoded fields without requiring a capability cache.
    pub const fn value(&self) -> &AudioParameterValue {
        &self.value
    }
    /// Decodes `AUDIO_RET_PARAM` (`e7`).
    pub fn decode_response(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0xe7)
    }
    /// Decodes `AUDIO_NTFY_PARAM` (`e9`), including the extra `05` transition byte.
    pub fn decode_notification(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0xe9)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, p, opcode)?;
        require_minimum(p, 3)?;
        let value = match kind {
            AudioKind::ConnectionPriority
            | AudioKind::ConnectionPriorityWithStatus
            | AudioKind::ClassicLePriority => {
                let transition = kind == AudioKind::ClassicLePriority && opcode == 0xe9;
                require_length(p, if transition { 4 } else { 3 })?;
                AudioParameterValue::ConnectionPriority {
                    priority: PriorityMode::from_raw(p[2]),
                    switching_stream: if transition {
                        Some(p[3].try_into()?)
                    } else {
                        None
                    },
                }
            }
            AudioKind::Dsee | AudioKind::DseeWithReasons => {
                require_length(p, 3)?;
                AudioParameterValue::Dsee(p[2].try_into()?)
            }
            AudioKind::Bgm | AudioKind::BgmWithReasons => {
                require_length(p, 4)?;
                AudioParameterValue::Bgm {
                    enabled: enabled(p[2], "background setting")?,
                    room_size: p[3].try_into()?,
                }
            }
            AudioKind::UpmixCinema | AudioKind::SoundLeakageReduction => {
                require_length(p, 3)?;
                AudioParameterValue::Switch(enabled(p[2], "audio on/off setting")?)
            }
            AudioKind::ListeningOptions => {
                let (items, end) = counted_bytes(p, 2)?;
                require_length(p, end)?;
                AudioParameterValue::ListeningOptions(
                    items
                        .iter()
                        .copied()
                        .map(ListeningOption::from_raw)
                        .collect(),
                )
            }
            AudioKind::UpmixSeries => {
                require_length(p, 3)?;
                AudioParameterValue::Upmix(UpmixItem::from_raw(p[2]))
            }
        };
        Ok(Self { kind, value })
    }
}

/// A setting encoded against a matching audio capability.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AudioSetting {
    /// Select a known, advertised priority. Inquiry `05` includes control byte `00`.
    ConnectionPriority(PriorityMode),
    /// Configure DSEE auto/off.
    Dsee(DseeMode),
    /// Configure background listening without clearing its stored room size.
    Bgm {
        /// Whether background listening should be enabled.
        enabled: bool,
        /// Room size retained while disabled.
        room_size: RoomSize,
    },
    /// Configure cinema upmix.
    UpmixCinema(bool),
    /// Configure sound leakage reduction.
    SoundLeakageReduction(bool),
    /// Replace the complete local listening-mode assignment.
    ListeningOptions(Vec<ListeningOption>),
    /// Select a known, advertised local upmix item.
    Upmix(UpmixItem),
}

impl AudioSetting {
    /// Encodes a table-1 `AUDIO_SET_PARAM` (`e8`) payload.
    ///
    /// Checks the descriptor layout and advertised choices. Listening assignments
    /// must contain distinct advertised items and every mandatory item. Unknown
    /// identities remain readable but cannot be selected by this encoder.
    /// Current availability and conflicting functions require caller checks.
    /// An ACK confirms frame delivery only; observe parameters or perform a readback.
    pub fn encode_set(&self, capability: &AudioCapability) -> Result<Vec<u8>, CommandError> {
        let kind = capability.kind;
        let mut payload = vec![0xe8, kind.inquiry()];
        match (self, kind, &capability.data) {
            (
                Self::ConnectionPriority(priority),
                AudioKind::ConnectionPriority
                | AudioKind::ConnectionPriorityWithStatus
                | AudioKind::ClassicLePriority,
                AudioCapabilityData::Connection { priorities, .. },
            ) => {
                if priority.as_raw() > 2 || !priorities.contains(priority) {
                    return Err(CommandError::InvalidValue {
                        field: "advertised connection priority",
                        value: priority.as_raw(),
                    });
                }
                payload.push(priority.as_raw());
                if kind == AudioKind::ClassicLePriority {
                    payload.push(0);
                }
            }
            (
                Self::Dsee(mode),
                AudioKind::Dsee | AudioKind::DseeWithReasons,
                AudioCapabilityData::Dsee(_),
            ) => payload.push(mode.as_u8()),
            (Self::Bgm { enabled, room_size }, AudioKind::Bgm | AudioKind::BgmWithReasons, _) => {
                payload.extend([u8::from(!enabled), room_size.as_u8()])
            }
            (Self::UpmixCinema(value), AudioKind::UpmixCinema, _)
            | (Self::SoundLeakageReduction(value), AudioKind::SoundLeakageReduction, _) => {
                payload.push(u8::from(!value))
            }
            (
                Self::ListeningOptions(items),
                AudioKind::ListeningOptions,
                AudioCapabilityData::ListeningOptions { options, .. },
            ) => {
                let count =
                    u8::try_from(items.len()).map_err(|_| CommandError::InvalidParameter {
                        field: "listening assignment count",
                    })?;
                for (index, item) in items.iter().enumerate() {
                    if item.as_raw() > 4 || !options.iter().any(|option| option.option == *item) {
                        return Err(CommandError::InvalidValue {
                            field: "advertised listening option",
                            value: item.as_raw(),
                        });
                    }
                    if items[..index].contains(item) {
                        return Err(CommandError::InvalidParameter {
                            field: "duplicate listening assignment",
                        });
                    }
                    if options
                        .iter()
                        .filter(|option| option.option == *item)
                        .count()
                        != 1
                    {
                        return Err(CommandError::InvalidCapability {
                            field: "ambiguous listening option",
                        });
                    }
                }
                if options
                    .iter()
                    .any(|option| option.mandatory && !items.contains(&option.option))
                {
                    return Err(CommandError::InvalidParameter {
                        field: "missing mandatory listening option",
                    });
                }
                payload.push(count);
                payload.extend(items.iter().map(|item| item.as_raw()));
            }
            (
                Self::Upmix(item),
                AudioKind::UpmixSeries,
                AudioCapabilityData::Upmix { items, .. },
            ) => {
                if item.as_raw() > 3 || !items.contains(item) {
                    return Err(CommandError::InvalidValue {
                        field: "advertised upmix item",
                        value: item.as_raw(),
                    });
                }
                payload.push(item.as_raw());
            }
            _ => return Err(unsupported("audio setting layout", kind)),
        }
        Ok(payload)
    }
}

fn header(table: CommandTable, p: &[u8], opcode: u8) -> Result<AudioKind, CommandError> {
    require_table(table, CommandTable::TABLE_1)?;
    require_minimum(p, 2)?;
    require_opcode(p[0], opcode)?;
    p[1].try_into()
}

fn counted_bytes(p: &[u8], offset: usize) -> Result<(&[u8], usize), CommandError> {
    require_minimum(p, offset + 1)?;
    let end = offset + 1 + usize::from(p[offset]);
    require_minimum(p, end)?;
    Ok((&p[offset + 1..end], end))
}

fn binary(value: u8, field: &'static str) -> Result<u8, CommandError> {
    match value {
        0 | 1 => Ok(value),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn enabled(value: u8, field: &'static str) -> Result<bool, CommandError> {
    binary(value, field).map(|value| value == 0)
}

fn unsupported(operation: &'static str, kind: AudioKind) -> CommandError {
    CommandError::UnsupportedOperation {
        operation,
        inquiry: kind.inquiry(),
    }
}
