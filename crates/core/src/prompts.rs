use libmdr2::{
    alert::{AlertContents, AlertDecision, AlertMessage},
    audio::{AudioKind, AudioParameters, AudioSetting},
    command::{Request, Response},
    equalizer::{EqualizerKind, EqualizerParameters, EqualizerSetting},
    noise::{NoiseKind, NoiseParameters},
    system::{SpeakToChatSettings, SystemKind, SystemParameters},
};

use crate::{
    Error, OperationResult, Result,
    controls::{audio_matches, equalizer_matches, noise_matches},
};

/// Identifies one received prompt within a connection generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PromptId {
    pub generation: u64,
    pub serial: u64,
}

/// An explicit user action on a device prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptReply {
    Accept,
    Reject,
    /// Closes an informational or unsupported prompt without transmitting a reply.
    Dismiss,
}

/// A received message and its identity. Reply validation uses the actor's copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePrompt {
    pub id: PromptId,
    pub message: AlertMessage,
}

impl DevicePrompt {
    /// Whether this action can be performed within the supported protocol scope.
    pub fn allows(&self, reply: PromptReply) -> bool {
        match reply {
            PromptReply::Accept => self.message.encode_reply(AlertDecision::Accept).is_ok(),
            PromptReply::Reject => self.message.encode_reply(AlertDecision::Reject).is_ok(),
            PromptReply::Dismiss => {
                !self.allows(PromptReply::Accept) && !self.allows(PromptReply::Reject)
            }
        }
    }
}

/// A single transmitted setting, with the preflight value used to verify rejection.
/// This is retained only when a prompt arrives during that setting's wire exchange.
#[derive(Clone)]
pub(crate) enum SettingReadback {
    Noise(NoiseKind, NoiseParameters, NoiseParameters),
    Equalizer(EqualizerKind, EqualizerSetting, EqualizerParameters),
    Audio(AudioKind, AudioSetting, AudioParameters),
    System(SystemKind, SystemParameters, SystemParameters),
    Extended(SystemKind, SpeakToChatSettings, SpeakToChatSettings),
}

impl SettingReadback {
    /// Association is limited to supported messages naming this setting domain.
    /// It is not a protocol transaction identifier.
    pub fn relates_to(&self, message: &AlertMessage) -> bool {
        if message.encode_reply(AlertDecision::Accept).is_err() {
            return false;
        }
        match message.contents() {
            AlertContents::Fixed { message, .. } => match self {
                Self::Audio(
                    AudioKind::ConnectionPriority
                    | AudioKind::ConnectionPriorityWithStatus
                    | AudioKind::ClassicLePriority,
                    AudioSetting::ConnectionPriority(priority),
                    ..,
                ) => match message.as_raw() {
                    0x00 => true,
                    0x74 | 0x76 => priority.as_raw() == 0,
                    0x75 | 0x77 => priority.as_raw() == 1,
                    _ => false,
                },
                Self::Audio(AudioKind::Dsee | AudioKind::DseeWithReasons, ..) => {
                    matches!(message.as_raw(), 0x08 | 0x2b)
                }
                Self::Equalizer(..) => matches!(message.as_raw(), 0x08 | 0x2b | 0x5c | 0x90),
                _ => false,
            },
            // Flexible items name affected features, not necessarily the requested
            // setting. They remain explicitly answerable but are synchronized
            // without inferring an association with an active write.
            _ => false,
        }
    }

    pub fn query(&self) -> Request {
        match self {
            Self::Noise(kind, ..) => Request::NoiseParameters(*kind),
            Self::Equalizer(kind, ..) => Request::EqualizerParameters(*kind),
            Self::Audio(kind, ..) => Request::AudioParameters(*kind),
            Self::System(kind, ..) => Request::SystemParameters(*kind),
            Self::Extended(kind, ..) => Request::SystemExtendedParameters(*kind),
        }
    }

    pub fn verify(&self, response: &Response, reply: PromptReply) -> Result<OperationResult> {
        let (desired, baseline) = match (self, response) {
            (Self::Noise(kind, desired, baseline), Response::NoiseParameters(actual))
                if actual.kind() == *kind =>
            {
                (noise_matches(*desired, *actual), baseline == actual)
            }
            (Self::Equalizer(kind, desired, baseline), Response::EqualizerParameters(actual))
                if actual.kind() == *kind =>
            {
                (equalizer_matches(desired, actual), baseline == actual)
            }
            (Self::Audio(kind, desired, baseline), Response::AudioParameters(actual))
                if actual.kind() == *kind =>
            {
                (audio_matches(desired, actual), baseline == actual)
            }
            (Self::System(kind, desired, baseline), Response::SystemParameters(actual))
                if actual.kind() == *kind =>
            {
                (desired == actual, baseline == actual)
            }
            (
                Self::Extended(kind, desired, baseline),
                Response::SystemExtendedParameters(actual),
            ) if actual.kind() == *kind => (desired == actual, baseline == actual),
            _ => return Err(Error::NotReady),
        };
        Ok(match reply {
            PromptReply::Accept if desired => OperationResult::Confirmed,
            PromptReply::Reject if baseline => OperationResult::Cancelled,
            _ => OperationResult::Adjusted,
        })
    }
}
