use crate::{Error, Result};
use libmdr2::{
    audio::{
        AudioKind, AudioParameterValue, AudioParameters, AudioSetting, DseeMode, PriorityMode,
    },
    system::{
        DetectionSensitivity, ModeOutTime, SpeakToChatSettings, SystemKind, SystemParameters,
    },
};

/// A configuration edit for the supported DSEE and connection-priority layouts.
/// Priority changes can interrupt the control link; only a subsequent matched
/// readback confirms the operation, and a disconnected command is never replayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioChange {
    /// Configures DSEE off/auto; this is independent of its active-effect indicator.
    Dsee(DseeMode),
    /// Selects a known priority advertised by the current capability.
    Priority(PriorityMode),
}

impl AudioChange {
    pub(crate) fn apply(self, current: &AudioParameters) -> Result<AudioSetting> {
        match (self, current.value()) {
            (Self::Dsee(mode), AudioParameterValue::Dsee(_)) => Ok(AudioSetting::Dsee(mode)),
            (Self::Priority(priority), AudioParameterValue::ConnectionPriority { .. }) => {
                Ok(AudioSetting::ConnectionPriority(priority))
            }
            _ => Err(Error::Unsupported(
                "audio edit does not match the advertised layout",
            )),
        }
    }
}

pub(crate) fn supports_audio(kind: AudioKind) -> bool {
    matches!(
        kind,
        AudioKind::Dsee
            | AudioKind::DseeWithReasons
            | AudioKind::ConnectionPriority
            | AudioKind::ConnectionPriorityWithStatus
            | AudioKind::ClassicLePriority
    )
}

pub(crate) fn audio_matches(expected: &AudioSetting, actual: &AudioParameters) -> bool {
    match (expected, actual.value()) {
        (AudioSetting::Dsee(expected), AudioParameterValue::Dsee(actual)) => expected == actual,
        (
            AudioSetting::ConnectionPriority(expected),
            AudioParameterValue::ConnectionPriority { priority, .. },
        ) => expected == priority,
        _ => false,
    }
}

/// A Speak-to-Chat edit to one field in its ordinary or extended parameter set.
/// Unedited fields come from the current preflight read. Extended edits never
/// change the configured enable/preview switches, and type 2 has no voice focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemChange {
    /// Changes the configured switch while retaining the latest preview value.
    Enabled(bool),
    /// Changes preview while retaining the configured switch; enabling preview
    /// requires its capability flag.
    Preview(bool),
    Sensitivity(DetectionSensitivity),
    /// Type-1 extended setting; type 2 rejects this edit.
    VoiceFocus(bool),
    /// Selects an end mode, not a duration from the capability's raw metadata.
    ModeOut(ModeOutTime),
}

impl SystemChange {
    pub(crate) fn is_extended(self) -> bool {
        matches!(
            self,
            Self::Sensitivity(_) | Self::VoiceFocus(_) | Self::ModeOut(_)
        )
    }

    pub(crate) fn apply_parameters(
        self,
        mut current: SystemParameters,
    ) -> Result<SystemParameters> {
        let (enabled, preview) = match &mut current {
            SystemParameters::SpeakToChatType1 { enabled, preview }
            | SystemParameters::SpeakToChatType2 { enabled, preview } => (enabled, preview),
            _ => {
                return Err(Error::Unsupported(
                    "system layout is outside the supported controls",
                ));
            }
        };
        match self {
            Self::Enabled(value) => *enabled = value,
            Self::Preview(value) => *preview = value,
            _ => {
                return Err(Error::Unsupported(
                    "extended edit requires Speak-to-Chat extended parameters",
                ));
            }
        }
        Ok(current)
    }

    pub(crate) fn apply_extended(
        self,
        mut current: SpeakToChatSettings,
    ) -> Result<SpeakToChatSettings> {
        match (&mut current, self) {
            (
                SpeakToChatSettings::Type1 { sensitivity, .. }
                | SpeakToChatSettings::Type2 { sensitivity, .. },
                Self::Sensitivity(value),
            ) => *sensitivity = value,
            (SpeakToChatSettings::Type1 { voice_focus, .. }, Self::VoiceFocus(value)) => {
                *voice_focus = value
            }
            (
                SpeakToChatSettings::Type1 { mode_out, .. }
                | SpeakToChatSettings::Type2 { mode_out, .. },
                Self::ModeOut(value),
            ) => *mode_out = value,
            (SpeakToChatSettings::Type2 { .. }, Self::VoiceFocus(_)) => {
                return Err(Error::Unsupported(
                    "Speak-to-Chat type 2 has no voice-focus field",
                ));
            }
            _ => {
                return Err(Error::Unsupported(
                    "system edit does not match the extended parameter layout",
                ));
            }
        }
        Ok(current)
    }
}

pub(crate) fn supports_system(kind: SystemKind) -> bool {
    matches!(
        kind,
        SystemKind::SpeakToChatType1 | SystemKind::SpeakToChatType2
    )
}
