//! Typed MDR v2 queries, responses, and notifications.
//!
//! Queries carry an expected response descriptor. Notifications are decoded
//! separately from read responses. The caller selects a compatible handshake
//! variant and checks device support and availability before issuing commands.

#![doc = include_str!("../docs/connection.md")]

use crate::alert::{AlertMessage, LeAudioAlertStatus};
use crate::assignment::{
    AssignmentCapability, AssignmentCustomParameters, AssignmentKind, AssignmentParameters,
    AssignmentStatus,
};
use crate::connection::ConnectionNotice;
use crate::general::{GeneralCapability, GeneralKind, GeneralParameters, GeneralStatus};
use crate::peripheral::{
    PeripheralCapability, PeripheralKind, PeripheralParameters, PeripheralResultReport,
    PeripheralStatus,
};
use crate::playback::{
    PlaybackCapability, PlaybackKind, PlaybackMetadata, PlaybackStatus, PlaybackVolume, VolumeKind,
};
use crate::sound_effect::{
    SoundEffectBandInfo, SoundEffectCapability, SoundEffectKind, SoundEffectParameters,
    SoundEffectStatus,
};
use crate::system2::{System2Capability, System2Kind, System2Parameters, System2Status};
use crate::voice_guidance::{VoiceCapability, VoiceKind, VoiceParameters, VoiceStatus};
use std::{error::Error, fmt};

use crate::audio::{AudioCapability, AudioKind, AudioParameters, AudioStatus};
use crate::battery::{BatteryKind, BatteryStatus};
use crate::common::{CommonKind, CommonStatus};
use crate::device::{DeviceInfo, DeviceInfoKind};
use crate::equalizer::{
    EqualizerBandInfo, EqualizerCapability, EqualizerKind, EqualizerParameters, EqualizerStatus,
};
use crate::noise::{NoiseCapability, NoiseKind, NoiseParameters, NoiseStatus};
use crate::power::{PowerCapability, PowerKind, PowerParameters, PowerStatus};
use crate::system::{
    SpeakToChatSettings, SystemCapability, SystemKind, SystemParameters, SystemStatus,
};
use crate::types::{CommandTable, DisplayLanguage, FunctionId, ProtocolVersion};

/// The largest capability identifier accepted by this library, in bytes.
///
/// Both response variants accept an empty identifier and reject identifiers
/// longer than this bound without truncation.
pub const MAX_CAPABILITY_ID_LEN: usize = 128;

/// An error encoding or decoding a command payload.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    /// This query family is defined here only for command tables 1 and 2.
    UnsupportedTable(u8),
    /// A payload belongs to a different command table from the selected layout.
    WrongTable {
        /// The table required by the query.
        expected: CommandTable,
        /// The table attached to the received payload.
        actual: CommandTable,
    },
    /// A fixed or count-prefixed layout has a different total length.
    InvalidLength {
        /// The exact number of bytes required by the layout.
        expected: usize,
        /// The number of bytes provided.
        actual: usize,
    },
    /// The payload is too short even to read its length or variant fields.
    Truncated {
        /// The minimum number of bytes needed at this point in the payload.
        minimum: usize,
        /// The number of bytes provided.
        actual: usize,
    },
    /// The payload belongs to another command.
    UnexpectedOpcode {
        /// The expected command byte.
        expected: u8,
        /// The received command byte.
        actual: u8,
    },
    /// The payload belongs to another query variant.
    UnexpectedInquiry {
        /// The expected query type byte.
        expected: u8,
        /// The received query type byte.
        actual: u8,
    },
    /// A closed wire field contains an undefined value.
    InvalidValue {
        /// The field whose value is invalid.
        field: &'static str,
        /// The received byte.
        value: u8,
    },
    /// The advertised identifier exceeds the library's explicit bound.
    IdentifierTooLong {
        /// The advertised number of identifier bytes.
        length: usize,
        /// The largest accepted number of identifier bytes.
        maximum: usize,
    },
    /// No typed decoder is available for this opcode.
    UnsupportedOpcode(u8),
    /// The operation is not defined for the selected inquiry.
    UnsupportedOperation {
        /// The requested operation.
        operation: &'static str,
        /// The selected inquiry byte.
        inquiry: u8,
    },
    /// A capability descriptor belongs to a different parameter layout.
    CapabilityMismatch {
        /// The expected inquiry layout.
        expected: u8,
        /// The actual inquiry layout.
        actual: u8,
    },
    /// Capability fields do not form a usable descriptor.
    InvalidCapability {
        /// The invalid field or relationship.
        field: &'static str,
    },
    /// A parameter violates the selected capability's constraints.
    InvalidParameter {
        /// The invalid field or relationship.
        field: &'static str,
    },
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedTable(table) => write!(f, "unsupported command table {table}"),
            Self::WrongTable { expected, actual } => write!(
                f,
                "expected command table {}, received {}",
                expected.as_raw(),
                actual.as_raw()
            ),
            Self::InvalidLength { expected, actual } => {
                write!(f, "expected {expected} payload bytes, received {actual}")
            }
            Self::Truncated { minimum, actual } => {
                write!(
                    f,
                    "need at least {minimum} payload bytes, received {actual}"
                )
            }
            Self::UnexpectedOpcode { expected, actual } => write!(
                f,
                "expected opcode 0x{expected:02x}, received 0x{actual:02x}"
            ),
            Self::UnexpectedInquiry { expected, actual } => write!(
                f,
                "expected inquiry 0x{expected:02x}, received 0x{actual:02x}"
            ),
            Self::InvalidValue { field, value } => {
                write!(f, "invalid {field} value 0x{value:02x}")
            }
            Self::IdentifierTooLong { length, maximum } => {
                write!(
                    f,
                    "capability identifier has {length} bytes; maximum is {maximum}"
                )
            }
            Self::UnsupportedOpcode(opcode) => write!(f, "unsupported opcode {opcode:#04x}"),
            Self::UnsupportedOperation { operation, inquiry } => {
                write!(f, "{operation} is not supported for inquiry {inquiry:#04x}")
            }
            Self::CapabilityMismatch { expected, actual } => {
                write!(
                    f,
                    "expected capability inquiry {expected:#04x}, received {actual:#04x}"
                )
            }
            Self::InvalidCapability { field } => write!(f, "invalid capability: {field}"),
            Self::InvalidParameter { field } => write!(f, "invalid parameter: {field}"),
        }
    }
}

impl Error for CommandError {}

/// The capability query layout chosen by the caller's compatibility policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityQuery {
    /// The two-byte `02 00` request.
    Standard,
    /// The three-byte `02 01 app_type` request.
    ///
    /// The library preserves the caller's byte without assigning it a platform
    /// meaning. Selecting this variant does not prove that a device supports it.
    WithAppType(u8),
}

impl CapabilityQuery {
    fn response_variant(self) -> CapabilityInfoVariant {
        match self {
            Self::Standard => CapabilityInfoVariant::Standard,
            Self::WithAppType(_) => CapabilityInfoVariant::WithAppType,
        }
    }
}

/// The capability response layout, identified by its inquiry byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityInfoVariant {
    /// A response to the standard capability query (`03 00`).
    Standard,
    /// A response to an app-type query (`03 01`).
    ///
    /// The app-type request byte is not echoed in the response.
    WithAppType,
}

impl CapabilityInfoVariant {
    fn inquiry(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::WithAppType => 1,
        }
    }
}

/// A supported MDR v2 read query. Encoding a query does not check device support.
///
/// For capability-dependent reads, first select a variant from the advertised
/// functions. Model names alone must not determine the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Request {
    /// Read table-2 system capability.
    System2Capability(System2Kind),
    /// Read table-2 system status.
    System2Status(System2Kind),
    /// Read table-2 system parameters.
    System2Parameters(System2Kind),

    /// Read peripheral capability.
    PeripheralCapability(PeripheralKind),
    /// Read peripheral-control availability.
    PeripheralStatus(PeripheralKind),
    /// Read peripheral parameters.
    PeripheralParameters(PeripheralKind),
    /// Read voice-guidance capabilities.
    VoiceCapability(VoiceKind),
    /// Read voice-guidance availability.
    VoiceStatus(VoiceKind),
    /// Read voice-guidance parameters.
    VoiceParameters(VoiceKind),
    /// Read sound effect capability.
    SoundEffectCapability {
        /// Advertised inquiry layout.
        kind: SoundEffectKind,
        /// Device label language.
        language: DisplayLanguage,
    },
    /// Read sound-effect availability.
    SoundEffectStatus(SoundEffectKind),
    /// Read sound effect parameters.
    SoundEffectParameters(SoundEffectKind),
    /// Read sound-effect band metadata.
    SoundEffectBandInfo(SoundEffectKind),

    /// Read assignable keys, presets and action candidates.
    AssignmentCapability(AssignmentKind),
    /// Read per-key assignment availability.
    AssignmentStatus(AssignmentKind),
    /// Read selected presets in capability key order.
    AssignmentParameters(AssignmentKind),
    /// Read custom preset action/function mappings.
    AssignmentCustomParameters(AssignmentKind),

    /// Read the slot type, localized descriptions and list choices.
    GeneralCapability {
        /// Advertised inquiry layout.
        kind: GeneralKind,
        /// Device label language.
        language: DisplayLanguage,
    },
    /// Read current availability of a general-setting slot.
    GeneralStatus(GeneralKind),
    /// Read a general-setting slot value.
    GeneralParameters(GeneralKind),
    /// Read playback and volume capabilities.
    PlaybackCapability(PlaybackKind),
    /// Read playback state and control availability.
    PlaybackStatus(PlaybackKind),
    /// Read track, album, artist and genre fields.
    PlaybackMetadata(PlaybackKind),
    /// Read music or call volume and optional mute state.
    PlaybackVolume(VolumeKind),
    /// Read the LE Audio alert-notification flag.
    LeAudioAlertStatus,

    /// Read component connections, the audio codec, an effect or device modes.
    CommonStatus(CommonKind),
    /// Read local audio capabilities; fixed connection layouts have no query.
    AudioCapability(AudioKind),
    /// Read audio-setting availability and variant-specific state.
    AudioStatus(AudioKind),
    /// Read local audio parameters using the advertised layout.
    AudioParameters(AudioKind),
    /// Read power-setting capabilities; fixed switch layouts have no query.
    PowerCapability(PowerKind),
    /// Read power-setting availability, distinct from battery measurements.
    PowerStatus(PowerKind),
    /// Read automatic power-off, standby or charging parameters.
    PowerParameters(PowerKind),
    /// Read Speak-to-Chat capabilities; fixed system switches have no query.
    SystemCapability(SystemKind),
    /// Read system-setting availability and optional talking state.
    SystemStatus(SystemKind),
    /// Read a system toggle or Speak-to-Chat activation parameters.
    SystemParameters(SystemKind),
    /// Read Speak-to-Chat sensitivity and ending-time selection; other layouts have no extended parameters.
    SystemExtendedParameters(SystemKind),

    /// Read protocol version and command-table support flags from table 1.
    ProtocolInfo,
    /// Read capability counter and identifier from table 1.
    CapabilityInfo(CapabilityQuery),
    /// Read the function list for table 1 or table 2.
    SupportedFunctions(CommandTable),
    /// Read a model name, firmware version, or series/color pair from table 1.
    DeviceInfo(DeviceInfoKind),
    /// Read a battery status layout advertised by a table-1 function.
    BatteryStatus(BatteryKind),
    /// Read supported ambient modes, level ranges, or the assigned cycle key.
    ///
    /// [`NoiseKind::NcOnly`] has no dynamic query; use
    /// [`NoiseCapability::nc_only`] after discovering its function identity.
    NoiseCapability(NoiseKind),
    /// Read whether a noise-control variant is currently available.
    NoiseStatus(NoiseKind),
    /// Read the current noise-control parameters.
    NoiseParameters(NoiseKind),
    /// Read EQ bands, levels and preset labels in the requested display language.
    EqualizerCapability {
        /// The capability-selected EQ layout.
        kind: EqualizerKind,
        /// Language used for preset labels, not voice-guidance audio.
        language: DisplayLanguage,
    },
    /// Read EQ availability and, for supported layouts, disabling reasons.
    EqualizerStatus(EqualizerKind),
    /// Read the current EQ preset, ULT mode, or custom band levels.
    EqualizerParameters(EqualizerKind),
    /// Read band frequency/type descriptors, including optional Clear Bass.
    EqualizerBandInfo(EqualizerKind),
}

impl Request {
    /// Encode a query and the descriptor for its matching response.
    ///
    /// The caller must register the response descriptor before sending the
    /// payload. These bytes are a command payload, not an encoded wire frame.
    pub fn encode(self) -> Result<EncodedRequest, CommandError> {
        let (table, payload, kind) = match self {
            Self::System2Capability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "capability query",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_2,
                    vec![0xf0, kind.inquiry()],
                    ResponseKind::System2Capability(kind),
                )
            }
            Self::System2Status(kind) => (
                CommandTable::TABLE_2,
                vec![0xf2, kind.inquiry()],
                ResponseKind::System2Status(kind),
            ),
            Self::System2Parameters(kind) => {
                require_operation(kind.has_parameters(), "parameters query", kind.inquiry())?;
                (
                    CommandTable::TABLE_2,
                    vec![0xf6, kind.inquiry()],
                    ResponseKind::System2Parameters(kind),
                )
            }

            Self::PeripheralCapability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "capability query",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_2,
                    vec![0x30, kind.inquiry()],
                    ResponseKind::PeripheralCapability(kind),
                )
            }
            Self::PeripheralStatus(kind) => {
                require_operation(kind.has_status_query(), "status query", kind.inquiry())?;
                (
                    CommandTable::TABLE_2,
                    vec![0x32, kind.inquiry()],
                    ResponseKind::PeripheralStatus(kind),
                )
            }
            Self::PeripheralParameters(kind) => (
                CommandTable::TABLE_2,
                vec![0x36, kind.inquiry()],
                ResponseKind::PeripheralParameters(kind),
            ),
            Self::VoiceCapability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "capability query",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_2,
                    vec![0x40, kind.inquiry()],
                    ResponseKind::VoiceCapability(kind),
                )
            }
            Self::VoiceStatus(kind) => (
                CommandTable::TABLE_2,
                kind.status_request()?,
                ResponseKind::VoiceStatus(kind),
            ),
            Self::VoiceParameters(kind) => (
                CommandTable::TABLE_2,
                vec![0x46, kind.inquiry()],
                ResponseKind::VoiceParameters(kind),
            ),
            Self::SoundEffectCapability { kind, language } => (
                CommandTable::TABLE_1,
                if kind.capability_uses_language() {
                    vec![0x50, kind.inquiry(), language.as_raw()]
                } else {
                    vec![0x50, kind.inquiry()]
                },
                ResponseKind::SoundEffectCapability(kind),
            ),
            Self::SoundEffectStatus(kind) => {
                require_operation(kind.has_status_query(), "status query", kind.inquiry())?;
                (
                    CommandTable::TABLE_1,
                    vec![0x52, kind.inquiry()],
                    ResponseKind::SoundEffectStatus(kind),
                )
            }
            Self::SoundEffectParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0x56, kind.inquiry()],
                ResponseKind::SoundEffectParameters(kind),
            ),
            Self::SoundEffectBandInfo(kind) => {
                require_operation(
                    kind.has_band_info_query(),
                    "band information query",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_1,
                    vec![0x5a, kind.inquiry()],
                    ResponseKind::SoundEffectBandInfo(kind),
                )
            }

            Self::AssignmentCapability(kind) => (
                CommandTable::TABLE_1,
                vec![0xf0, kind.inquiry()],
                ResponseKind::AssignmentCapability(kind),
            ),
            Self::AssignmentStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0xf2, kind.inquiry()],
                ResponseKind::AssignmentStatus(kind),
            ),
            Self::AssignmentParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0xf6, kind.inquiry()],
                ResponseKind::AssignmentParameters(kind),
            ),
            Self::AssignmentCustomParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0xfa, kind.inquiry()],
                ResponseKind::AssignmentCustomParameters(kind),
            ),

            Self::GeneralCapability { kind, language } => (
                CommandTable::TABLE_1,
                vec![0xd0, kind.inquiry(), language.as_raw()],
                ResponseKind::GeneralCapability(kind),
            ),
            Self::GeneralStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0xd2, kind.inquiry()],
                ResponseKind::GeneralStatus(kind),
            ),
            Self::GeneralParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0xd6, kind.inquiry()],
                ResponseKind::GeneralParameters(kind),
            ),
            Self::PlaybackCapability(kind) => (
                CommandTable::TABLE_1,
                vec![0xa0, kind.inquiry()],
                ResponseKind::PlaybackCapability(kind),
            ),
            Self::PlaybackStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0xa2, kind.inquiry()],
                ResponseKind::PlaybackStatus(kind),
            ),
            Self::PlaybackMetadata(kind) => (
                CommandTable::TABLE_1,
                vec![0xa6, kind.inquiry()],
                ResponseKind::PlaybackMetadata(kind),
            ),
            Self::PlaybackVolume(kind) => (
                CommandTable::TABLE_1,
                vec![0xa6, kind.inquiry()],
                ResponseKind::PlaybackVolume(kind),
            ),
            Self::LeAudioAlertStatus => (
                CommandTable::TABLE_1,
                vec![0x92, 5],
                ResponseKind::LeAudioAlertStatus,
            ),

            Self::CommonStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0x12, kind.inquiry()],
                ResponseKind::CommonStatus(kind),
            ),
            Self::AudioCapability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "AudioCapability",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_1,
                    vec![0xe0, kind.inquiry()],
                    ResponseKind::AudioCapability(kind),
                )
            }
            Self::AudioStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0xe2, kind.inquiry()],
                ResponseKind::AudioStatus(kind),
            ),
            Self::AudioParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0xe6, kind.inquiry()],
                ResponseKind::AudioParameters(kind),
            ),
            Self::PowerCapability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "PowerCapability",
                    kind.inquiry(),
                )?;
                (
                    kind.table(),
                    vec![0x20, kind.inquiry()],
                    ResponseKind::PowerCapability(kind),
                )
            }
            Self::PowerStatus(kind) => (
                kind.table(),
                vec![0x22, kind.inquiry()],
                ResponseKind::PowerStatus(kind),
            ),
            Self::PowerParameters(kind) => (
                kind.table(),
                vec![0x26, kind.inquiry()],
                ResponseKind::PowerParameters(kind),
            ),
            Self::SystemCapability(kind) => {
                require_operation(
                    kind.has_capability_query(),
                    "SystemCapability",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_1,
                    vec![0xf0, kind.inquiry()],
                    ResponseKind::SystemCapability(kind),
                )
            }
            Self::SystemStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0xf2, kind.inquiry()],
                ResponseKind::SystemStatus(kind),
            ),
            Self::SystemParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0xf6, kind.inquiry()],
                ResponseKind::SystemParameters(kind),
            ),
            Self::SystemExtendedParameters(kind) => {
                require_operation(
                    kind.has_extended_parameters(),
                    "SystemExtendedParameters",
                    kind.inquiry(),
                )?;
                (
                    CommandTable::TABLE_1,
                    vec![0xfa, kind.inquiry()],
                    ResponseKind::SystemExtendedParameters(kind),
                )
            }

            Self::ProtocolInfo => (
                CommandTable::TABLE_1,
                vec![0x00, 0x00],
                ResponseKind::ProtocolInfo,
            ),
            Self::CapabilityInfo(query) => {
                let payload = match query {
                    CapabilityQuery::Standard => vec![0x02, 0x00],
                    CapabilityQuery::WithAppType(app_type) => vec![0x02, 0x01, app_type],
                };
                (
                    CommandTable::TABLE_1,
                    payload,
                    ResponseKind::CapabilityInfo(query.response_variant()),
                )
            }
            Self::SupportedFunctions(table) => {
                validate_supported_table(table)?;
                (table, vec![0x06, 0x00], ResponseKind::SupportedFunctions)
            }
            Self::DeviceInfo(kind) => (
                CommandTable::TABLE_1,
                vec![0x04, kind.inquiry()],
                ResponseKind::DeviceInfo(kind),
            ),
            Self::BatteryStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0x22, kind.inquiry()],
                ResponseKind::BatteryStatus(kind),
            ),
            Self::NoiseCapability(kind) => {
                if !kind.has_capability_query() {
                    return Err(CommandError::UnsupportedOperation {
                        operation: "noise capability query",
                        inquiry: kind.inquiry(),
                    });
                }
                (
                    CommandTable::TABLE_1,
                    vec![0x60, kind.inquiry()],
                    ResponseKind::NoiseCapability(kind),
                )
            }
            Self::NoiseStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0x62, kind.inquiry()],
                ResponseKind::NoiseStatus(kind),
            ),
            Self::NoiseParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0x66, kind.inquiry()],
                ResponseKind::NoiseParameters(kind),
            ),
            Self::EqualizerCapability { kind, language } => (
                CommandTable::TABLE_1,
                vec![0x50, kind.inquiry(), language.as_raw()],
                ResponseKind::EqualizerCapability(kind),
            ),
            Self::EqualizerStatus(kind) => (
                CommandTable::TABLE_1,
                vec![0x52, kind.inquiry()],
                ResponseKind::EqualizerStatus(kind),
            ),
            Self::EqualizerParameters(kind) => (
                CommandTable::TABLE_1,
                vec![0x56, kind.inquiry()],
                ResponseKind::EqualizerParameters(kind),
            ),
            Self::EqualizerBandInfo(kind) => (
                CommandTable::TABLE_1,
                vec![0x5a, kind.inquiry()],
                ResponseKind::EqualizerBandInfo(kind),
            ),
        };
        Ok(EncodedRequest {
            table,
            payload,
            expected_response: ExpectedResponse { table, kind },
        })
    }
}

/// An encoded command payload and its response-routing information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedRequest {
    /// The command table used to choose the outgoing frame's data type.
    pub table: CommandTable,
    /// Command bytes, without framing, sequence, or checksum.
    pub payload: Vec<u8>,
    /// The expected response, independent of link-level acknowledgements.
    pub expected_response: ExpectedResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResponseKind {
    System2Capability(System2Kind),
    System2Status(System2Kind),
    System2Parameters(System2Kind),

    PeripheralCapability(PeripheralKind),
    PeripheralStatus(PeripheralKind),
    PeripheralParameters(PeripheralKind),
    VoiceCapability(VoiceKind),
    VoiceStatus(VoiceKind),
    VoiceParameters(VoiceKind),
    SoundEffectCapability(SoundEffectKind),
    SoundEffectStatus(SoundEffectKind),
    SoundEffectParameters(SoundEffectKind),
    SoundEffectBandInfo(SoundEffectKind),

    AssignmentCapability(AssignmentKind),
    AssignmentStatus(AssignmentKind),
    AssignmentParameters(AssignmentKind),
    AssignmentCustomParameters(AssignmentKind),

    GeneralCapability(GeneralKind),
    GeneralStatus(GeneralKind),
    GeneralParameters(GeneralKind),
    PlaybackCapability(PlaybackKind),
    PlaybackStatus(PlaybackKind),
    PlaybackMetadata(PlaybackKind),
    PlaybackVolume(VolumeKind),
    LeAudioAlertStatus,

    CommonStatus(CommonKind),
    AudioCapability(AudioKind),
    AudioStatus(AudioKind),
    AudioParameters(AudioKind),
    PowerCapability(PowerKind),
    PowerStatus(PowerKind),
    PowerParameters(PowerKind),
    SystemCapability(SystemKind),
    SystemStatus(SystemKind),
    SystemParameters(SystemKind),
    SystemExtendedParameters(SystemKind),

    ProtocolInfo,
    CapabilityInfo(CapabilityInfoVariant),
    SupportedFunctions,
    DeviceInfo(DeviceInfoKind),
    BatteryStatus(BatteryKind),
    NoiseCapability(NoiseKind),
    NoiseStatus(NoiseKind),
    NoiseParameters(NoiseKind),
    EqualizerCapability(EqualizerKind),
    EqualizerStatus(EqualizerKind),
    EqualizerParameters(EqualizerKind),
    EqualizerBandInfo(EqualizerKind),
}

/// A typed response descriptor produced together with a query.
///
/// It identifies a command family and variant, not a unique transaction. The
/// protocol has no general request ID; callers must handle late responses and
/// must not run indistinguishable requests concurrently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedResponse {
    table: CommandTable,
    kind: ResponseKind,
}

impl ExpectedResponse {
    /// Return the expected command table.
    pub fn table(self) -> CommandTable {
        self.table
    }

    /// Return the expected response opcode.
    pub fn opcode(self) -> u8 {
        match self.kind {
            ResponseKind::System2Capability(_) => 0xf1,
            ResponseKind::System2Status(_) => 0xf3,
            ResponseKind::System2Parameters(_) => 0xf7,

            ResponseKind::PeripheralCapability(_) => 0x31,
            ResponseKind::PeripheralStatus(_) => 0x33,
            ResponseKind::PeripheralParameters(_) => 0x37,
            ResponseKind::VoiceCapability(_) => 0x41,
            ResponseKind::VoiceStatus(_) => 0x43,
            ResponseKind::VoiceParameters(_) => 0x47,
            ResponseKind::SoundEffectCapability(_) => 0x51,
            ResponseKind::SoundEffectStatus(_) => 0x53,
            ResponseKind::SoundEffectParameters(_) => 0x57,
            ResponseKind::SoundEffectBandInfo(_) => 0x5b,

            ResponseKind::AssignmentCapability(_) => 0xf1,
            ResponseKind::AssignmentStatus(_) => 0xf3,
            ResponseKind::AssignmentParameters(_) => 0xf7,
            ResponseKind::AssignmentCustomParameters(_) => 0xfb,

            ResponseKind::GeneralCapability(_) => 0xd1,
            ResponseKind::GeneralStatus(_) => 0xd3,
            ResponseKind::GeneralParameters(_) => 0xd7,
            ResponseKind::PlaybackCapability(_) => 0xa1,
            ResponseKind::PlaybackStatus(_) => 0xa3,
            ResponseKind::PlaybackMetadata(_) => 0xa7,
            ResponseKind::PlaybackVolume(_) => 0xa7,
            ResponseKind::LeAudioAlertStatus => 0x93,

            ResponseKind::CommonStatus(_) => 0x13,
            ResponseKind::AudioCapability(_) => 0xe1,
            ResponseKind::AudioStatus(_) => 0xe3,
            ResponseKind::AudioParameters(_) => 0xe7,
            ResponseKind::PowerCapability(_) => 0x21,
            ResponseKind::PowerStatus(_) => 0x23,
            ResponseKind::PowerParameters(_) => 0x27,
            ResponseKind::SystemCapability(_) => 0xf1,
            ResponseKind::SystemStatus(_) => 0xf3,
            ResponseKind::SystemParameters(_) => 0xf7,
            ResponseKind::SystemExtendedParameters(_) => 0xfb,

            ResponseKind::ProtocolInfo => 0x01,
            ResponseKind::CapabilityInfo(_) => 0x03,
            ResponseKind::SupportedFunctions => 0x07,
            ResponseKind::DeviceInfo(_) => 0x05,
            ResponseKind::BatteryStatus(_) => 0x23,
            ResponseKind::NoiseCapability(_) => 0x61,
            ResponseKind::NoiseStatus(_) => 0x63,
            ResponseKind::NoiseParameters(_) => 0x67,
            ResponseKind::EqualizerCapability(_) => 0x51,
            ResponseKind::EqualizerStatus(_) => 0x53,
            ResponseKind::EqualizerParameters(_) => 0x57,
            ResponseKind::EqualizerBandInfo(_) => 0x5b,
        }
    }

    /// Return the expected inquiry byte.
    pub fn inquiry(self) -> u8 {
        match self.kind {
            ResponseKind::System2Capability(kind) => kind.inquiry(),
            ResponseKind::System2Status(kind) => kind.inquiry(),
            ResponseKind::System2Parameters(kind) => kind.inquiry(),

            ResponseKind::PeripheralCapability(kind) => kind.inquiry(),
            ResponseKind::PeripheralStatus(kind) => kind.inquiry(),
            ResponseKind::PeripheralParameters(kind) => kind.inquiry(),
            ResponseKind::VoiceCapability(kind) => kind.inquiry(),
            ResponseKind::VoiceStatus(kind) => kind.inquiry(),
            ResponseKind::VoiceParameters(kind) => kind.inquiry(),
            ResponseKind::SoundEffectCapability(kind) => kind.inquiry(),
            ResponseKind::SoundEffectStatus(kind) => kind.inquiry(),
            ResponseKind::SoundEffectParameters(kind) => kind.inquiry(),
            ResponseKind::SoundEffectBandInfo(kind) => kind.inquiry(),

            ResponseKind::AssignmentCapability(kind) => kind.inquiry(),
            ResponseKind::AssignmentStatus(kind) => kind.inquiry(),
            ResponseKind::AssignmentParameters(kind) => kind.inquiry(),
            ResponseKind::AssignmentCustomParameters(kind) => kind.inquiry(),

            ResponseKind::GeneralCapability(kind) => kind.inquiry(),
            ResponseKind::GeneralStatus(kind) => kind.inquiry(),
            ResponseKind::GeneralParameters(kind) => kind.inquiry(),
            ResponseKind::PlaybackCapability(kind) => kind.inquiry(),
            ResponseKind::PlaybackStatus(kind) => kind.inquiry(),
            ResponseKind::PlaybackMetadata(kind) => kind.inquiry(),
            ResponseKind::PlaybackVolume(kind) => kind.inquiry(),
            ResponseKind::LeAudioAlertStatus => 5,

            ResponseKind::CommonStatus(kind) => kind.inquiry(),
            ResponseKind::AudioCapability(kind) => kind.inquiry(),
            ResponseKind::AudioStatus(kind) => kind.inquiry(),
            ResponseKind::AudioParameters(kind) => kind.inquiry(),
            ResponseKind::PowerCapability(kind) => kind.inquiry(),
            ResponseKind::PowerStatus(kind) => kind.inquiry(),
            ResponseKind::PowerParameters(kind) => kind.inquiry(),
            ResponseKind::SystemCapability(kind) => kind.inquiry(),
            ResponseKind::SystemStatus(kind) => kind.inquiry(),
            ResponseKind::SystemParameters(kind) => kind.inquiry(),
            ResponseKind::SystemExtendedParameters(kind) => kind.inquiry(),

            ResponseKind::CapabilityInfo(variant) => variant.inquiry(),
            ResponseKind::ProtocolInfo | ResponseKind::SupportedFunctions => 0,
            ResponseKind::DeviceInfo(kind) => kind.inquiry(),
            ResponseKind::BatteryStatus(kind) => kind.inquiry(),
            ResponseKind::NoiseCapability(kind)
            | ResponseKind::NoiseStatus(kind)
            | ResponseKind::NoiseParameters(kind) => kind.inquiry(),
            ResponseKind::EqualizerCapability(kind)
            | ResponseKind::EqualizerStatus(kind)
            | ResponseKind::EqualizerParameters(kind)
            | ResponseKind::EqualizerBandInfo(kind) => kind.inquiry(),
        }
    }

    /// Check only the table, opcode, and inquiry used to route a response.
    ///
    /// A `true` result does not mean the body is valid or that an operation has
    /// succeeded. Use [`Self::decode`] to validate and decode the whole payload.
    pub fn matches(self, table: CommandTable, payload: &[u8]) -> bool {
        table == self.table
            && payload.first() == Some(&self.opcode())
            && payload.get(1) == Some(&self.inquiry())
    }

    /// Validate a response against this descriptor and decode its complete body.
    pub fn decode(self, table: CommandTable, payload: &[u8]) -> Result<Response, CommandError> {
        require_table(table, self.table)?;
        validate_header(payload, self.opcode(), self.inquiry())?;
        match self.kind {
            ResponseKind::System2Capability(_) => {
                System2Capability::decode_response(table, payload).map(Response::System2Capability)
            }
            ResponseKind::System2Status(_) => {
                System2Status::decode_response(table, payload).map(Response::System2Status)
            }
            ResponseKind::System2Parameters(_) => {
                System2Parameters::decode_response(table, payload).map(Response::System2Parameters)
            }

            ResponseKind::PeripheralCapability(_) => {
                PeripheralCapability::decode_response(table, payload)
                    .map(Response::PeripheralCapability)
            }
            ResponseKind::PeripheralStatus(_) => {
                PeripheralStatus::decode_response(table, payload).map(Response::PeripheralStatus)
            }
            ResponseKind::PeripheralParameters(_) => {
                PeripheralParameters::decode_response(table, payload)
                    .map(Response::PeripheralParameters)
            }
            ResponseKind::VoiceCapability(_) => {
                VoiceCapability::decode_response(table, payload).map(Response::VoiceCapability)
            }
            ResponseKind::VoiceStatus(_) => {
                VoiceStatus::decode_response(table, payload).map(Response::VoiceStatus)
            }
            ResponseKind::VoiceParameters(_) => {
                VoiceParameters::decode_response(table, payload).map(Response::VoiceParameters)
            }
            ResponseKind::SoundEffectCapability(_) => {
                SoundEffectCapability::decode_response(table, payload)
                    .map(Response::SoundEffectCapability)
            }
            ResponseKind::SoundEffectStatus(_) => {
                SoundEffectStatus::decode_response(table, payload).map(Response::SoundEffectStatus)
            }
            ResponseKind::SoundEffectParameters(_) => {
                SoundEffectParameters::decode_response(table, payload)
                    .map(Response::SoundEffectParameters)
            }
            ResponseKind::SoundEffectBandInfo(_) => {
                SoundEffectBandInfo::decode_response(table, payload)
                    .map(Response::SoundEffectBandInfo)
            }

            ResponseKind::AssignmentCapability(_) => {
                AssignmentCapability::decode_response(table, payload)
                    .map(Response::AssignmentCapability)
            }
            ResponseKind::AssignmentStatus(_) => {
                AssignmentStatus::decode_response(table, payload).map(Response::AssignmentStatus)
            }
            ResponseKind::AssignmentParameters(_) => {
                AssignmentParameters::decode_response(table, payload)
                    .map(Response::AssignmentParameters)
            }
            ResponseKind::AssignmentCustomParameters(_) => {
                AssignmentCustomParameters::decode_response(table, payload)
                    .map(Response::AssignmentCustomParameters)
            }

            ResponseKind::GeneralCapability(_) => {
                GeneralCapability::decode_response(table, payload).map(Response::GeneralCapability)
            }
            ResponseKind::GeneralStatus(_) => {
                GeneralStatus::decode_response(table, payload).map(Response::GeneralStatus)
            }
            ResponseKind::GeneralParameters(_) => {
                GeneralParameters::decode_response(table, payload).map(Response::GeneralParameters)
            }
            ResponseKind::PlaybackCapability(_) => {
                PlaybackCapability::decode_response(table, payload)
                    .map(Response::PlaybackCapability)
            }
            ResponseKind::PlaybackStatus(_) => {
                PlaybackStatus::decode_response(table, payload).map(Response::PlaybackStatus)
            }
            ResponseKind::PlaybackMetadata(_) => {
                PlaybackMetadata::decode_response(table, payload).map(Response::PlaybackMetadata)
            }
            ResponseKind::PlaybackVolume(_) => {
                PlaybackVolume::decode_response(table, payload).map(Response::PlaybackVolume)
            }
            ResponseKind::LeAudioAlertStatus => LeAudioAlertStatus::decode_response(table, payload)
                .map(Response::LeAudioAlertStatus),

            ResponseKind::CommonStatus(_) => {
                CommonStatus::decode_response(table, payload).map(Response::CommonStatus)
            }
            ResponseKind::AudioCapability(_) => {
                AudioCapability::decode_response(table, payload).map(Response::AudioCapability)
            }
            ResponseKind::AudioStatus(_) => {
                AudioStatus::decode_response(table, payload).map(Response::AudioStatus)
            }
            ResponseKind::AudioParameters(_) => {
                AudioParameters::decode_response(table, payload).map(Response::AudioParameters)
            }
            ResponseKind::PowerCapability(_) => {
                PowerCapability::decode_response(table, payload).map(Response::PowerCapability)
            }
            ResponseKind::PowerStatus(_) => {
                PowerStatus::decode_response(table, payload).map(Response::PowerStatus)
            }
            ResponseKind::PowerParameters(_) => {
                PowerParameters::decode_response(table, payload).map(Response::PowerParameters)
            }
            ResponseKind::SystemCapability(_) => {
                SystemCapability::decode_response(table, payload).map(Response::SystemCapability)
            }
            ResponseKind::SystemStatus(_) => {
                SystemStatus::decode_response(table, payload).map(Response::SystemStatus)
            }
            ResponseKind::SystemParameters(_) => {
                SystemParameters::decode_response(table, payload).map(Response::SystemParameters)
            }
            ResponseKind::SystemExtendedParameters(_) => {
                SpeakToChatSettings::decode_response(table, payload)
                    .map(Response::SystemExtendedParameters)
            }

            ResponseKind::ProtocolInfo => {
                ProtocolInfo::decode(table, payload).map(Response::ProtocolInfo)
            }
            ResponseKind::CapabilityInfo(_) => {
                CapabilityInfo::decode(table, payload).map(Response::CapabilityInfo)
            }
            ResponseKind::SupportedFunctions => {
                SupportedFunctions::decode(table, payload).map(Response::SupportedFunctions)
            }
            ResponseKind::DeviceInfo(_) => {
                DeviceInfo::decode(table, payload).map(Response::DeviceInfo)
            }
            ResponseKind::BatteryStatus(_) => {
                BatteryStatus::decode_response(table, payload).map(Response::BatteryStatus)
            }
            ResponseKind::NoiseCapability(_) => {
                NoiseCapability::decode_response(table, payload).map(Response::NoiseCapability)
            }
            ResponseKind::NoiseStatus(_) => {
                NoiseStatus::decode_response(table, payload).map(Response::NoiseStatus)
            }
            ResponseKind::NoiseParameters(_) => {
                NoiseParameters::decode_response(table, payload).map(Response::NoiseParameters)
            }
            ResponseKind::EqualizerCapability(_) => {
                EqualizerCapability::decode_response(table, payload)
                    .map(Response::EqualizerCapability)
            }
            ResponseKind::EqualizerStatus(_) => {
                EqualizerStatus::decode_response(table, payload).map(Response::EqualizerStatus)
            }
            ResponseKind::EqualizerParameters(_) => {
                EqualizerParameters::decode_response(table, payload)
                    .map(Response::EqualizerParameters)
            }
            ResponseKind::EqualizerBandInfo(_) => {
                EqualizerBandInfo::decode_response(table, payload).map(Response::EqualizerBandInfo)
            }
        }
    }
}

/// A fully decoded response to one of this module's queries.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Response {
    /// Reported table-2 system capability.
    System2Capability(System2Capability),
    /// Reported table-2 system status.
    System2Status(System2Status),
    /// Reported table-2 system parameters.
    System2Parameters(System2Parameters),

    /// Reported peripheral capability.
    PeripheralCapability(PeripheralCapability),
    /// Peripheral-control availability.
    PeripheralStatus(PeripheralStatus),
    /// Reported peripheral parameters.
    PeripheralParameters(PeripheralParameters),
    /// Voice-guidance capabilities.
    VoiceCapability(VoiceCapability),
    /// Voice-guidance availability.
    VoiceStatus(VoiceStatus),
    /// Voice-guidance parameters.
    VoiceParameters(VoiceParameters),
    /// Reported sound effect capability.
    SoundEffectCapability(SoundEffectCapability),
    /// Sound-effect availability.
    SoundEffectStatus(SoundEffectStatus),
    /// Reported sound effect parameters.
    SoundEffectParameters(SoundEffectParameters),
    /// Sound-effect band metadata.
    SoundEffectBandInfo(SoundEffectBandInfo),

    /// Reported assignable keys, presets and action candidates.
    AssignmentCapability(AssignmentCapability),
    /// Reported per-key assignment availability.
    AssignmentStatus(AssignmentStatus),
    /// Reported selected presets in capability key order.
    AssignmentParameters(AssignmentParameters),
    /// Reported custom preset action/function mappings.
    AssignmentCustomParameters(AssignmentCustomParameters),

    /// General-setting slot type, localized descriptions and list choices.
    GeneralCapability(GeneralCapability),
    /// Reported current availability of a general-setting slot.
    GeneralStatus(GeneralStatus),
    /// A general-setting slot value.
    GeneralParameters(GeneralParameters),
    /// Reported playback and volume capabilities.
    PlaybackCapability(PlaybackCapability),
    /// Reported playback state and control availability.
    PlaybackStatus(PlaybackStatus),
    /// Reported track, album, artist and genre fields.
    PlaybackMetadata(PlaybackMetadata),
    /// Reported music or call volume and optional mute state.
    PlaybackVolume(PlaybackVolume),
    /// The LE Audio alert-notification flag.
    LeAudioAlertStatus(LeAudioAlertStatus),

    /// Read-only component connections, audio codec, upscaling effect or device modes.
    CommonStatus(CommonStatus),
    /// Supported audio choices, exclusivity metadata or upscaling identity.
    AudioCapability(AudioCapability),
    /// Current audio-setting availability, reasons and variant-specific state.
    AudioStatus(AudioStatus),
    /// Current local audio parameters in the selected inquiry layout.
    AudioParameters(AudioParameters),
    /// Power-setting choices or thresholds, identified by table and inquiry.
    PowerCapability(PowerCapability),
    /// Power-setting availability and optional charging-notice state.
    PowerStatus(PowerStatus),
    /// Automatic power-off, standby or charging parameters.
    PowerParameters(PowerParameters),
    /// Speak-to-Chat preview support and ending-time metadata.
    SystemCapability(SystemCapability),
    /// System-setting availability and optional talking state.
    SystemStatus(SystemStatus),
    /// System activation and switch parameters.
    SystemParameters(SystemParameters),
    /// Speak-to-Chat sensitivity, optional voice focus and ending-time selection.
    SystemExtendedParameters(SpeakToChatSettings),

    /// Protocol version and command-table support.
    ProtocolInfo(ProtocolInfo),
    /// Capability counter and opaque identifier.
    CapabilityInfo(CapabilityInfo),
    /// Ordered function and priority entries for one command table.
    SupportedFunctions(SupportedFunctions),
    /// Model name, firmware version or product appearance.
    DeviceInfo(DeviceInfo),
    /// A battery read response, distinct from a battery notification.
    BatteryStatus(BatteryStatus),
    /// Supported noise-control modes, level ranges or assigned cycle key.
    NoiseCapability(NoiseCapability),
    /// Current noise-control availability.
    NoiseStatus(NoiseStatus),
    /// Current noise-control parameters.
    NoiseParameters(NoiseParameters),
    /// Supported EQ bands, levels, presets and optional ULT data.
    EqualizerCapability(EqualizerCapability),
    /// Current EQ availability and disabling reasons.
    EqualizerStatus(EqualizerStatus),
    /// Current EQ parameters.
    EqualizerParameters(EqualizerParameters),
    /// Band frequency/type descriptors.
    EqualizerBandInfo(EqualizerBandInfo),
}

/// An unsolicited message, kept distinct from a response to a read query.
///
/// Unknown opcodes or layouts return an error. The caller still owns the original
/// payload and can record it; this module does not reinterpret unknown messages.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Notification {
    /// Table-2 wearing state or availability changed.
    System2Status(System2Status),
    /// Table-2 local system parameters changed.
    System2Parameters(System2Parameters),

    /// Peripheral availability changed.
    PeripheralStatus(PeripheralStatus),
    /// Peripheral state changed.
    PeripheralParameters(PeripheralParameters),
    /// Peripheral operation result or progress report.
    PeripheralResult(PeripheralResultReport),
    /// Voice guidance availability changed.
    VoiceStatus(VoiceStatus),
    /// Voice guidance parameters changed.
    VoiceParameters(VoiceParameters),
    /// Sound effect availability changed.
    SoundEffectStatus(SoundEffectStatus),
    /// Sound effect parameters changed.
    SoundEffectParameters(SoundEffectParameters),

    /// A device reconnection or GATT-disconnect request; no transport action is automatic.
    ConnectionNotice(ConnectionNotice),

    /// Per-key assignment availability changes.
    AssignmentStatus(AssignmentStatus),
    /// Selected assignment presets in key order.
    AssignmentParameters(AssignmentParameters),
    /// Custom preset mapping changes.
    AssignmentCustomParameters(AssignmentCustomParameters),

    /// A general-setting availability update.
    GeneralStatus(GeneralStatus),
    /// A general-setting value update.
    GeneralParameters(GeneralParameters),
    /// Playback state and availability changes.
    PlaybackStatus(PlaybackStatus),
    /// Track metadata changes.
    PlaybackMetadata(PlaybackMetadata),
    /// Volume and optional mute-state changes.
    PlaybackVolume(PlaybackVolume),
    /// A device prompt requiring explicit caller handling.
    AlertMessage(AlertMessage),
    /// An LE Audio notice-flag update.
    LeAudioAlertStatus(LeAudioAlertStatus),

    /// An unsolicited `CommonStatus` update (opcode `15`).
    CommonStatus(CommonStatus),
    /// An unsolicited `AudioStatus` update (opcode `e5`).
    AudioStatus(AudioStatus),
    /// An unsolicited `AudioParameters` update (opcode `e9`).
    AudioParameters(AudioParameters),
    /// An unsolicited `PowerStatus` update (opcode `25`).
    PowerStatus(PowerStatus),
    /// An unsolicited `PowerParameters` update (opcode `29`).
    PowerParameters(PowerParameters),
    /// An unsolicited `SystemStatus` update (opcode `f5`).
    SystemStatus(SystemStatus),
    /// An unsolicited `SystemParameters` update (opcode `f9`).
    SystemParameters(SystemParameters),
    /// An unsolicited `SystemExtendedParameters` update (opcode `fd`).
    SystemExtendedParameters(SpeakToChatSettings),

    /// A table-1 `POWER_NTFY_STATUS` battery update (opcode `25`).
    BatteryStatus(BatteryStatus),
    /// A `65` noise-control availability update.
    NoiseStatus(NoiseStatus),
    /// A `69` noise-control parameter update.
    NoiseParameters(NoiseParameters),
    /// A `55` EQ availability update.
    EqualizerStatus(EqualizerStatus),
    /// A `59` EQ parameter update.
    EqualizerParameters(EqualizerParameters),
}

impl Notification {
    /// Decodes supported notification families by command table, opcode and inquiry.
    ///
    /// Read responses are not accepted here. Notifications do not satisfy an
    /// [`ExpectedResponse`], even when their bodies use the same layout.
    pub fn decode(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        validate_supported_table(table)?;
        require_minimum(payload, 2)?;
        match (table.as_raw(), payload[0]) {
            (2, 0xf5) => {
                System2Status::decode_notification(table, payload).map(Self::System2Status)
            }
            (2, 0xf9) => {
                System2Parameters::decode_notification(table, payload).map(Self::System2Parameters)
            }

            (2, 0x35) => {
                PeripheralStatus::decode_notification(table, payload).map(Self::PeripheralStatus)
            }
            (2, 0x39) => PeripheralParameters::decode_notification(table, payload)
                .map(Self::PeripheralParameters),
            (2, 0x3d) => PeripheralResultReport::decode_notification(table, payload)
                .map(Self::PeripheralResult),
            (2, 0x45) => VoiceStatus::decode_notification(table, payload).map(Self::VoiceStatus),
            (2, 0x49) => {
                VoiceParameters::decode_notification(table, payload).map(Self::VoiceParameters)
            }
            (1, 0x55) if payload[1] == 0x30 => {
                SoundEffectStatus::decode_notification(table, payload).map(Self::SoundEffectStatus)
            }
            (1, 0x59) if matches!(payload[1], 0x30 | 0x33..=0x36 | 0x40) => {
                SoundEffectParameters::decode_notification(table, payload)
                    .map(Self::SoundEffectParameters)
            }

            (1, 0x19) => {
                ConnectionNotice::decode_notification(table, payload).map(Self::ConnectionNotice)
            }

            (1, 0xf5) if AssignmentKind::try_from(payload[1]).is_ok() => {
                AssignmentStatus::decode_notification(table, payload).map(Self::AssignmentStatus)
            }
            (1, 0xf9) if AssignmentKind::try_from(payload[1]).is_ok() => {
                AssignmentParameters::decode_notification(table, payload)
                    .map(Self::AssignmentParameters)
            }
            (1, 0xfd) if AssignmentKind::try_from(payload[1]).is_ok() => {
                AssignmentCustomParameters::decode_notification(table, payload)
                    .map(Self::AssignmentCustomParameters)
            }

            (1, 0xd5) => {
                GeneralStatus::decode_notification(table, payload).map(Self::GeneralStatus)
            }
            (1, 0xd9) => {
                GeneralParameters::decode_notification(table, payload).map(Self::GeneralParameters)
            }
            (1, 0xa5) => {
                PlaybackStatus::decode_notification(table, payload).map(Self::PlaybackStatus)
            }
            (1, 0xa9) if VolumeKind::try_from(payload[1]).is_ok() => {
                PlaybackVolume::decode_notification(table, payload).map(Self::PlaybackVolume)
            }
            (1, 0xa9) => {
                PlaybackMetadata::decode_notification(table, payload).map(Self::PlaybackMetadata)
            }
            (1, 0x99) => AlertMessage::decode_notification(table, payload).map(Self::AlertMessage),
            (1, 0x95) => LeAudioAlertStatus::decode_notification(table, payload)
                .map(Self::LeAudioAlertStatus),

            (1, 0x25) if BatteryKind::try_from(payload[1]).is_ok() => {
                BatteryStatus::decode_notification(table, payload).map(Self::BatteryStatus)
            }
            (1, 0x15) => CommonStatus::decode_notification(table, payload).map(Self::CommonStatus),
            (1, 0xe5) => AudioStatus::decode_notification(table, payload).map(Self::AudioStatus),
            (1, 0xe9) => {
                AudioParameters::decode_notification(table, payload).map(Self::AudioParameters)
            }
            (1 | 2, 0x25) => {
                PowerStatus::decode_notification(table, payload).map(Self::PowerStatus)
            }
            (1 | 2, 0x29) => {
                PowerParameters::decode_notification(table, payload).map(Self::PowerParameters)
            }
            (1, 0xf5) => SystemStatus::decode_notification(table, payload).map(Self::SystemStatus),
            (1, 0xf9) => {
                SystemParameters::decode_notification(table, payload).map(Self::SystemParameters)
            }
            (1, 0xfd) => SpeakToChatSettings::decode_notification(table, payload)
                .map(Self::SystemExtendedParameters),
            (1, 0x65) => NoiseStatus::decode_notification(table, payload).map(Self::NoiseStatus),
            (1, 0x69) => {
                NoiseParameters::decode_notification(table, payload).map(Self::NoiseParameters)
            }
            (1, 0x55) => {
                EqualizerStatus::decode_notification(table, payload).map(Self::EqualizerStatus)
            }
            (1, 0x59) => EqualizerParameters::decode_notification(table, payload)
                .map(Self::EqualizerParameters),
            (_, opcode) => Err(CommandError::UnsupportedOpcode(opcode)),
        }
    }
}

/// Protocol information from the fixed eight-byte table-1 response.
///
/// The version is retained verbatim. Its numerical value is not a decision
/// about protocol generation, handshake layout, or device compatibility.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolInfo {
    /// The four-byte big-endian protocol version.
    pub version: ProtocolVersion,
    /// Whether the device enables the table-1 function list.
    pub table_1_supported: bool,
    /// Whether the device enables the table-2 function list.
    pub table_2_supported: bool,
}

impl ProtocolInfo {
    /// Decode `01 00 version:u32BE table_1:u8 table_2:u8` on table 1.
    ///
    /// Both support flags use `0 = enabled` and `1 = disabled`; other values
    /// are rejected. Missing or trailing payload bytes are rejected.
    pub fn decode(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        validate_header(payload, 0x01, 0x00)?;
        require_length(payload, 8)?;
        Ok(Self {
            version: ProtocolVersion::from_raw(u32::from_be_bytes([
                payload[2], payload[3], payload[4], payload[5],
            ])),
            table_1_supported: decode_enable(payload[6], "table 1 support")?,
            table_2_supported: decode_enable(payload[7], "table 2 support")?,
        })
    }
}

/// Capability cache identity reported by a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityInfo {
    /// The capability-response layout selected by the inquiry byte.
    pub variant: CapabilityInfoVariant,
    /// The unsigned capability counter.
    pub counter: u8,
    /// The identifier's exact bytes, including a valid empty identifier.
    ///
    /// No character set, MAC-address shape, or physical-device uniqueness is
    /// assumed. This identifier is distinct from other device identifiers.
    pub unique_id: Vec<u8>,
}

impl CapabilityInfo {
    /// Decode a standard or app-type capability response on table 1.
    ///
    /// Both layouts are `03 inquiry counter id_len id_bytes...`. The total
    /// length must equal `4 + id_len`. This library accepts lengths 0 through
    /// [`MAX_CAPABILITY_ID_LEN`] for both variants and preserves the bytes.
    pub fn decode(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(payload, 4)?;
        require_opcode(payload[0], 0x03)?;
        let variant = match payload[1] {
            0 => CapabilityInfoVariant::Standard,
            1 => CapabilityInfoVariant::WithAppType,
            value => {
                return Err(CommandError::InvalidValue {
                    field: "capability inquiry",
                    value,
                });
            }
        };
        let length = usize::from(payload[3]);
        if length > MAX_CAPABILITY_ID_LEN {
            return Err(CommandError::IdentifierTooLong {
                length,
                maximum: MAX_CAPABILITY_ID_LEN,
            });
        }
        require_length(payload, 4 + length)?;
        Ok(Self {
            variant,
            counter: payload[2],
            unique_id: payload[4..].to_vec(),
        })
    }
}

/// An advertised function, including its table and original priority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportedFunction {
    /// A stable identity that does not collapse equal codes in different tables.
    pub id: FunctionId,
    /// The device's unsigned priority byte; not a client-side sort position.
    pub priority: u8,
}

/// An ordered function list for one command table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportedFunctions {
    /// The frame's command table, validated as table 1 or table 2.
    pub table: CommandTable,
    /// Entries in wire order, including unknown codes and repeated entries.
    ///
    /// Advertising an entry does not establish that this library implements it or
    /// that the function is currently available or within application scope.
    pub functions: Vec<SupportedFunction>,
}

impl SupportedFunctions {
    /// Decode `07 00 count (code priority)*count` on table 1 or table 2.
    ///
    /// The count is an unsigned byte. The payload must contain exactly that
    /// many complete entries. Unknown codes, priorities, and order are retained.
    pub fn decode(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        validate_supported_table(table)?;
        validate_header(payload, 0x07, 0x00)?;
        require_minimum(payload, 3)?;
        let count = usize::from(payload[2]);
        require_length(payload, 3 + 2 * count)?;
        let functions = payload[3..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|entry| SupportedFunction {
                id: FunctionId {
                    table,
                    code: entry[0],
                },
                priority: entry[1],
            })
            .collect();
        Ok(Self { table, functions })
    }
}

fn validate_supported_table(table: CommandTable) -> Result<(), CommandError> {
    if table == CommandTable::TABLE_1 || table == CommandTable::TABLE_2 {
        Ok(())
    } else {
        Err(CommandError::UnsupportedTable(table.as_raw()))
    }
}

pub(crate) fn require_table(
    actual: CommandTable,
    expected: CommandTable,
) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CommandError::WrongTable { expected, actual })
    }
}

pub(crate) fn require_minimum(payload: &[u8], minimum: usize) -> Result<(), CommandError> {
    if payload.len() >= minimum {
        Ok(())
    } else {
        Err(CommandError::Truncated {
            minimum,
            actual: payload.len(),
        })
    }
}

pub(crate) fn require_length(payload: &[u8], expected: usize) -> Result<(), CommandError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(CommandError::InvalidLength {
            expected,
            actual: payload.len(),
        })
    }
}

pub(crate) fn require_opcode(actual: u8, expected: u8) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CommandError::UnexpectedOpcode { expected, actual })
    }
}

fn validate_header(payload: &[u8], opcode: u8, inquiry: u8) -> Result<(), CommandError> {
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    if payload[1] != inquiry {
        return Err(CommandError::UnexpectedInquiry {
            expected: inquiry,
            actual: payload[1],
        });
    }
    Ok(())
}

fn decode_enable(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn require_operation(
    supported: bool,
    operation: &'static str,
    inquiry: u8,
) -> Result<(), CommandError> {
    if supported {
        Ok(())
    } else {
        Err(CommandError::UnsupportedOperation { operation, inquiry })
    }
}
