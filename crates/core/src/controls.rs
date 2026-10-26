use crate::{Error, Result};
use libmdr2::{
    equalizer::{
        EqualizerCapability, EqualizerKind, EqualizerParameters, EqualizerSetting, PresetId,
    },
    noise::{
        AdaptiveSensitivity, AmbientMode, ChangeStatus, NoiseMode, NoiseParameters, NoiseSettings,
        NoiseSwitch,
    },
};

/// A local edit applied to freshly read parameters, preserving unrelated fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseChange {
    /// Changes the total effect switch without resetting its stored settings.
    Effect(bool),
    /// Selects a supported NC/ambient mode and enables the total effect. Layouts
    /// without an unambiguous mode field reject unsupported selections.
    Mode(NoiseMode),
    /// Changes the stored level without enabling ambient sound or total effect.
    AmbientLevel(u8),
    /// Selects advertised Voice/Normal processing without changing the level.
    VoiceFocus(bool),
    /// Changes only the adaptation switch, preserving sensitivity and mode.
    Adaptive(bool),
    /// Changes sensitivity without enabling adaptation or switching effects.
    AdaptiveSensitivity(AdaptiveSensitivity),
}

pub(crate) enum EqualizerEdit {
    Preset(PresetId),
    Band {
        expected_preset: Option<PresetId>,
        index: usize,
        value: u8,
    },
}

impl EqualizerEdit {
    pub(crate) fn apply(
        self,
        current: &EqualizerParameters,
        capability: &EqualizerCapability,
    ) -> Result<EqualizerSetting> {
        let (preset, levels) = match self {
            Self::Preset(preset) => {
                if current.kind() == EqualizerKind::Custom {
                    return Err(Error::Unsupported(
                        "presets for standalone custom equalizer",
                    ));
                }
                (Some(preset), None)
            }
            Self::Band {
                expected_preset,
                index,
                value,
            } => {
                if current.preset() != expected_preset {
                    return Err(Error::Unavailable(
                        "equalizer preset changed; refresh before editing",
                    ));
                }
                current.validate_capability(capability)?;
                if current
                    .preset()
                    .is_some_and(|preset| !preset.is_custom_slot())
                {
                    return Err(Error::Unsupported(
                        "select an editable equalizer preset before changing a band",
                    ));
                }
                let mut levels = current.levels().to_vec();
                *levels.get_mut(index).ok_or(Error::Unavailable(
                    "equalizer band index is outside the current curve",
                ))? = value;
                (current.preset(), Some(levels))
            }
        };
        match (preset, current.ult_mode(), levels) {
            (Some(preset), Some(ult_mode), levels) => Ok(EqualizerSetting::PresetWithUlt {
                preset,
                ult_mode,
                levels,
            }),
            (Some(preset), None, levels) => Ok(EqualizerSetting::Preset { preset, levels }),
            (None, None, Some(levels)) => Ok(EqualizerSetting::Custom { levels }),
            _ => Err(Error::Unsupported(
                "equalizer edit for this parameter layout",
            )),
        }
    }
}

impl NoiseChange {
    pub(crate) fn apply(self, mut parameters: NoiseParameters) -> Result<NoiseParameters> {
        let NoiseParameters::Control {
            change,
            effect,
            settings,
        } = &mut parameters
        else {
            return Err(Error::Unsupported("button-cycle editing"));
        };
        *change = ChangeStatus::Changed;
        match self {
            Self::Effect(enabled) => {
                *effect = if enabled {
                    NoiseSwitch::On
                } else {
                    NoiseSwitch::Off
                }
            }
            Self::Mode(value) => {
                match settings {
                    NoiseSettings::AutoNcAndAmbientLevel { mode, .. }
                    | NoiseSettings::DualSingleNcAndAmbientLevel { mode, .. }
                    | NoiseSettings::DualNcAndAmbientLevel { mode, .. }
                    | NoiseSettings::AdaptiveNcAndAmbientLevel { mode, .. } => *mode = value,
                    NoiseSettings::NcOnly { nc } if value == NoiseMode::NoiseCancelling => {
                        *nc = NoiseSwitch::On
                    }
                    NoiseSettings::AmbientLevel { .. } if value == NoiseMode::AmbientSound => {}
                    _ => return Err(Error::Unsupported("mode selection for this noise layout")),
                }
                *effect = NoiseSwitch::On;
            }
            Self::AmbientLevel(value) => match settings {
                NoiseSettings::NcAndAmbientLevel { level, .. }
                | NoiseSettings::NcModesAndAmbientLevel { level, .. }
                | NoiseSettings::AutoNcAndAmbientLevel { level, .. }
                | NoiseSettings::DualSingleNcAndAmbientLevel { level, .. }
                | NoiseSettings::DualNcAndAmbientLevel { level, .. }
                | NoiseSettings::AdaptiveNcAndAmbientLevel { level, .. }
                | NoiseSettings::AmbientLevel { level, .. } => *level = value,
                _ => return Err(Error::Unsupported("ambient level for this noise layout")),
            },
            Self::VoiceFocus(enabled) => {
                let value = if enabled {
                    AmbientMode::Voice
                } else {
                    AmbientMode::Normal
                };
                match settings {
                    NoiseSettings::NcModesAndAmbientSwitch { ambient_mode, .. }
                    | NoiseSettings::NcAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::NcModesAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::AutoNcAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::DualSingleNcAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::DualNcAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::AdaptiveNcAndAmbientLevel { ambient_mode, .. }
                    | NoiseSettings::AmbientLevel { ambient_mode, .. } => *ambient_mode = value,
                    _ => return Err(Error::Unsupported("voice focus for this noise layout")),
                }
            }
            Self::Adaptive(enabled) => match settings {
                NoiseSettings::AdaptiveNcAndAmbientLevel { adaptive, .. } => {
                    *adaptive = if enabled {
                        NoiseSwitch::On
                    } else {
                        NoiseSwitch::Off
                    };
                }
                _ => return Err(Error::Unsupported("noise adaptation for this noise layout")),
            },
            Self::AdaptiveSensitivity(value) => match settings {
                NoiseSettings::AdaptiveNcAndAmbientLevel { sensitivity, .. } => {
                    *sensitivity = value
                }
                _ => {
                    return Err(Error::Unsupported(
                        "adaptive sensitivity for this noise layout",
                    ));
                }
            },
        }
        Ok(parameters)
    }
}

pub(crate) fn noise_matches(expected: NoiseParameters, actual: NoiseParameters) -> bool {
    match (expected, actual) {
        (
            NoiseParameters::Control {
                effect: a,
                settings: x,
                ..
            },
            NoiseParameters::Control {
                effect: b,
                settings: y,
                ..
            },
        ) => a == b && x == y,
        (a, b) => a == b,
    }
}

pub(crate) fn equalizer_matches(expected: &EqualizerSetting, actual: &EqualizerParameters) -> bool {
    match expected {
        EqualizerSetting::Preset { preset, levels } => {
            actual.preset() == Some(*preset)
                && levels
                    .as_ref()
                    .is_none_or(|levels| levels == actual.levels())
        }
        EqualizerSetting::PresetWithUlt {
            preset,
            ult_mode,
            levels,
        } => {
            actual.preset() == Some(*preset)
                && actual.ult_mode() == Some(*ult_mode)
                && levels
                    .as_ref()
                    .is_none_or(|levels| levels == actual.levels())
        }
        EqualizerSetting::Custom { levels } => levels == actual.levels(),
        _ => false,
    }
}
