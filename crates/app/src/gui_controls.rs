//! Capability-driven sound controls. Values always come from core snapshots;
//! buttons submit explicit changes and never optimistically rewrite live state.

use ezsc_core::{
    AudioChange, ConnectionState, DeviceHandle, EqualizerState, Freshness, NoiseChange, NoiseState,
    SystemChange,
    audio::AudioKind,
    equalizer::{BandInformation, EqualizerKind, Preset, PresetId},
    noise::{
        AdaptiveSensitivity, AmbientLevelRange, AmbientMode, NoiseAvailability, NoiseKind,
        NoiseMode, NoiseParameters, NoiseSettings, NoiseSwitch,
    },
    system::SystemKind,
};
use gpui::{Context, Div, div, prelude::*, rgb};

use crate::gui_daily_controls::{audio_change_ready, system_change_ready};
use crate::{Desktop, Event, button};

pub(super) async fn refresh_controls(handle: &DeviceHandle) -> Result<(), String> {
    let snapshot = handle.snapshot();
    if snapshot.connection != ConnectionState::Ready {
        return Err("The control connection is not ready.".into());
    }
    let mut failures = Vec::new();
    for entry in snapshot.noise {
        if entry.kind == NoiseKind::ButtonCycle {
            continue;
        }
        if let Err(error) = handle.refresh_noise(entry.kind).await {
            failures.push(format!("Noise control: {error}"));
            if handle.snapshot().connection != ConnectionState::Ready {
                return Err(failures.join("; "));
            }
        }
    }
    for entry in snapshot.equalizers {
        if let Err(error) = handle.refresh_equalizer(entry.kind).await {
            failures.push(format!("Equalizer: {error}"));
            if handle.snapshot().connection != ConnectionState::Ready {
                return Err(failures.join("; "));
            }
        }
    }
    for entry in snapshot.audio {
        if let Err(error) = handle.refresh_audio(entry.kind).await {
            failures.push(format!("Audio control: {error}"));
            if handle.snapshot().connection != ConnectionState::Ready {
                return Err(failures.join("; "));
            }
        }
    }
    for entry in snapshot.system {
        if let Err(error) = handle.refresh_system(entry.kind).await {
            failures.push(format!("Speak-to-Chat: {error}"));
            if handle.snapshot().connection != ConnectionState::Ready {
                return Err(failures.join("; "));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ControlAction {
    Noise(NoiseKind, NoiseChange),
    Audio(AudioKind, AudioChange),
    System(SystemKind, SystemChange),
    Preset(EqualizerKind, PresetId),
    Band {
        kind: EqualizerKind,
        preset: Option<PresetId>,
        index: usize,
        value: u8,
    },
    BandInfo(EqualizerKind),
}

impl Desktop {
    pub(super) fn submit_control(&mut self, action: ControlAction, label: String) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if self.settings_blocked() || snapshot.connection != ConnectionState::Ready {
            return;
        }
        let allowed = match action {
            ControlAction::Noise(kind, _) => snapshot
                .noise
                .iter()
                .find(|entry| entry.kind == kind)
                .is_some_and(noise_ready),
            ControlAction::Audio(kind, change) => snapshot
                .audio
                .iter()
                .find(|entry| entry.kind == kind)
                .is_some_and(|entry| audio_change_ready(entry, change)),
            ControlAction::System(kind, change) => snapshot
                .system
                .iter()
                .find(|entry| entry.kind == kind)
                .is_some_and(|entry| system_change_ready(entry, change)),
            ControlAction::Preset(kind, _)
            | ControlAction::Band { kind, .. }
            | ControlAction::BandInfo(kind) => snapshot
                .equalizers
                .iter()
                .find(|entry| entry.kind == kind)
                .is_some_and(equalizer_ready),
        };
        if !allowed {
            return;
        }
        let Some(handle) = self.handle.clone() else {
            return;
        };
        self.busy = true;
        self.message = format!("Pending: {label}. Waiting for the headset's readback…");
        let sender = self.events.clone();
        let epoch = self.epoch;
        self.runtime.spawn(async move {
            let result = match action {
                ControlAction::Noise(kind, change) => handle.set_noise(kind, change).await,
                ControlAction::Audio(kind, change) => handle.set_audio(kind, change).await,
                ControlAction::System(kind, change) => handle.set_system(kind, change).await,
                ControlAction::Preset(kind, preset) => {
                    handle.set_equalizer_preset(kind, preset).await
                }
                ControlAction::Band {
                    kind,
                    preset,
                    index,
                    value,
                } => handle.set_equalizer_band(kind, preset, index, value).await,
                ControlAction::BandInfo(kind) => handle.refresh_equalizer_band_info(kind).await,
            }
            .map_err(|error| error.to_string());
            let _ = sender.send(Event::Operation(epoch, label, result)).await;
        });
    }

    pub(super) fn render_sound_controls(&self, cx: &mut Context<Self>) -> Div {
        let mut cards = div().flex().flex_col().flex_shrink_0().gap_4().pb_6();
        let Some(snapshot) = &self.snapshot else {
            return cards;
        };
        let session_ready = snapshot.connection == ConnectionState::Ready;
        let editable = session_ready && !self.settings_blocked();
        for (index, entry) in snapshot.noise.iter().enumerate() {
            if entry.kind == NoiseKind::ButtonCycle {
                continue;
            }
            let enabled = editable && noise_ready(entry);
            let mut card = card("Noise control").child(availability(
                session_ready,
                self.busy,
                entry.freshness,
                entry
                    .status
                    .is_some_and(|status| status.availability == NoiseAvailability::Enabled),
            ));
            let Some(NoiseParameters::Control {
                effect, settings, ..
            }) = entry.parameters
            else {
                cards =
                    cards.child(card.child(note("Noise-control values have not been read yet.")));
                continue;
            };
            let presentation = NoisePresentation::from_settings(settings);
            let effect_on = effect == NoiseSwitch::On;
            let kind = entry.kind;
            let mut modes = div().flex().flex_wrap().gap_2();
            if let Some(mode) = presentation.mode {
                for (choice, text) in [
                    (NoiseMode::NoiseCancelling, "Noise cancelling"),
                    (NoiseMode::AmbientSound, "Ambient sound"),
                ] {
                    modes = modes.child(
                        action_button(
                            (
                                "noise-mode",
                                index * 2 + usize::from(choice == NoiseMode::AmbientSound),
                            ),
                            text,
                            enabled,
                            effect_on && mode == choice,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if enabled {
                                view.submit_control(
                                    ControlAction::Noise(kind, NoiseChange::Mode(choice)),
                                    text.into(),
                                );
                                cx.notify();
                            }
                        })),
                    );
                }
            } else if matches!(settings, NoiseSettings::NcOnly { .. }) {
                modes = modes.child(
                    action_button(
                        ("nc-only", index),
                        "Noise cancelling",
                        enabled,
                        effect_on && presentation.nc_on,
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if enabled {
                            view.submit_control(
                                ControlAction::Noise(
                                    kind,
                                    NoiseChange::Mode(NoiseMode::NoiseCancelling),
                                ),
                                "Noise cancelling".into(),
                            );
                            cx.notify();
                        }
                    })),
                );
            } else {
                modes = modes.child(
                    action_button(("effect-on", index), "Effect on", enabled, effect_on).on_click(
                        cx.listener(move |view, _, _, cx| {
                            if enabled {
                                view.submit_control(
                                    ControlAction::Noise(kind, NoiseChange::Effect(true)),
                                    "Effect on".into(),
                                );
                                cx.notify();
                            }
                        }),
                    ),
                );
            }
            modes = modes.child(
                action_button(("effect-off", index), "Off", enabled, !effect_on).on_click(
                    cx.listener(move |view, _, _, cx| {
                        if enabled {
                            view.submit_control(
                                ControlAction::Noise(kind, NoiseChange::Effect(false)),
                                "Effect off".into(),
                            );
                            cx.notify();
                        }
                    }),
                ),
            );
            card = card.child(modes);
            if let Some(ambient_mode) = presentation.ambient_mode {
                if let Some(level) = presentation.level {
                    let range = unique_ambient(entry, ambient_mode)
                        .and_then(|capability| capability.levels);
                    let lower = range.and_then(|range| next_level(range, level, false));
                    let higher = range.and_then(|range| next_level(range, level, true));
                    let mut level_row = div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(format!("Ambient sound level: {level}"));
                    for (direction, label, candidate) in [(0, "−", lower), (1, "+", higher)] {
                        let can_step = enabled && candidate.is_some();
                        level_row = level_row.child(
                            action_button(
                                ("ambient-step", index * 2 + direction),
                                label,
                                can_step,
                                false,
                            )
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    if can_step && let Some(value) = candidate {
                                        view.submit_control(
                                            ControlAction::Noise(
                                                kind,
                                                NoiseChange::AmbientLevel(value),
                                            ),
                                            format!("Ambient sound level {value}"),
                                        );
                                        cx.notify();
                                    }
                                },
                            )),
                        );
                    }
                    card = card.child(level_row);
                    if let Some(range) = range {
                        card = card.child(note(format!(
                            "Device range {}–{}, increment {}.",
                            range.minimum(),
                            range.maximum(),
                            range.step()
                        )));
                    }
                }
                let target = if ambient_mode == AmbientMode::Voice {
                    AmbientMode::Normal
                } else {
                    AmbientMode::Voice
                };
                let can_focus = enabled
                    && unique_ambient(entry, target).is_some_and(|capability| {
                        match (capability.levels, presentation.level) {
                            (Some(range), Some(level)) => range.contains(level),
                            (None, None) => true,
                            _ => false,
                        }
                    });
                if unique_ambient(entry, AmbientMode::Voice).is_some() {
                    let focus = ambient_mode == AmbientMode::Voice;
                    card = card.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_3()
                            .child(format!("Voice focus: {}", if focus { "On" } else { "Off" }))
                            .child(
                                action_button(
                                    ("voice-focus", index),
                                    if focus { "Turn off" } else { "Turn on" },
                                    can_focus,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        if can_focus {
                                            view.submit_control(
                                                ControlAction::Noise(
                                                    kind,
                                                    NoiseChange::VoiceFocus(!focus),
                                                ),
                                                format!(
                                                    "Voice focus {}",
                                                    if focus { "off" } else { "on" }
                                                ),
                                            );
                                            cx.notify();
                                        }
                                    },
                                )),
                            ),
                    );
                }
            }
            if let Some((adaptive, sensitivity)) = noise_adaptation(entry) {
                let adaptation_on = adaptive == NoiseSwitch::On;
                card = card.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(format!(
                            "Noise adaptation: {}",
                            if adaptation_on { "On" } else { "Off" }
                        ))
                        .child(
                            action_button(
                                ("noise-adaptation", index),
                                if adaptation_on { "Turn off" } else { "Turn on" },
                                enabled,
                                false,
                            )
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    if enabled {
                                        view.submit_control(
                                            ControlAction::Noise(
                                                kind,
                                                NoiseChange::Adaptive(!adaptation_on),
                                            ),
                                            format!(
                                                "Noise adaptation {}",
                                                if adaptation_on { "off" } else { "on" }
                                            ),
                                        );
                                        cx.notify();
                                    }
                                },
                            )),
                        ),
                );
                let mut choices = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child("Adaptation sensitivity");
                for (choice_index, (choice, label)) in [
                    (AdaptiveSensitivity::Low, "Low"),
                    (AdaptiveSensitivity::Standard, "Standard"),
                    (AdaptiveSensitivity::High, "High"),
                ]
                .into_iter()
                .enumerate()
                {
                    choices = choices.child(
                        action_button(
                            ("noise-sensitivity", index * 3 + choice_index),
                            label,
                            enabled,
                            sensitivity == choice,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if enabled {
                                view.submit_control(
                                    ControlAction::Noise(
                                        kind,
                                        NoiseChange::AdaptiveSensitivity(choice),
                                    ),
                                    format!("Adaptation sensitivity {label}"),
                                );
                                cx.notify();
                            }
                        })),
                    );
                }
                card = card.child(choices);
            }
            cards = cards.child(card);
        }
        for (index, entry) in snapshot.equalizers.iter().enumerate() {
            let enabled = editable && equalizer_ready(entry);
            let mut card = card("Equalizer").child(availability(
                session_ready,
                self.busy,
                entry.freshness,
                entry
                    .status
                    .as_ref()
                    .is_some_and(|status| status.enabled && status.errors.is_empty()),
            ));
            let (Some(capability), Some(parameters)) = (&entry.capability, &entry.parameters)
            else {
                cards = cards.child(card.child(note("Equalizer values have not been read yet.")));
                continue;
            };
            let kind = entry.kind;
            let current_preset = parameters.preset();
            let mut presets = div().flex().flex_wrap().gap_2();
            for (preset_index, preset) in capability.presets().iter().enumerate() {
                let preset_id = preset.id;
                let label = preset_label(preset);
                let request_label = format!("Equalizer preset {label}");
                presets = presets.child(
                    action_button(
                        ("eq-preset", index * 256 + preset_index),
                        label,
                        enabled,
                        current_preset == Some(preset_id),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if enabled {
                            view.submit_control(
                                ControlAction::Preset(kind, preset_id),
                                request_label.clone(),
                            );
                            cx.notify();
                        }
                    })),
                );
            }
            card = card.child(presets);
            let valid_shape = parameters.levels().len() == usize::from(capability.band_count())
                && capability.band_count() > 0
                && capability.level_steps() > 0
                && parameters
                    .levels()
                    .iter()
                    .all(|level| *level < capability.level_steps());
            let can_edit_curve = enabled
                && valid_shape
                && kind != EqualizerKind::PresetNonCustomizable
                && (kind == EqualizerKind::Custom
                    || current_preset.is_some_and(|preset| {
                        preset.is_custom_slot() && capability.supports_preset(preset)
                    }));
            if !parameters.levels().is_empty() {
                card = card.child(note(format!(
                    "{} bands · levels use device steps.",
                    parameters.levels().len()
                )));
            }
            if enabled && kind != EqualizerKind::PresetNonCustomizable && !can_edit_curve {
                card = card.child(note(if valid_shape { "Select an advertised custom preset to edit its bands." } else { "The current curve does not match the reported range. Refresh state before editing." }));
            }
            for (band_index, level) in parameters.levels().iter().copied().enumerate() {
                let label = curve_band_label(
                    band_index,
                    parameters.levels().len(),
                    entry.band_info.as_ref().filter(|_| valid_shape),
                );
                let mut band = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().w_32().child(label.clone()))
                    .child(format!("{level}"));
                for (direction, text, candidate) in [
                    (0, "−", level.checked_sub(1)),
                    (
                        1,
                        "+",
                        level
                            .checked_add(1)
                            .filter(|level| *level < capability.level_steps()),
                    ),
                ] {
                    let can_step = can_edit_curve && candidate.is_some();
                    let request_label = label.clone();
                    band = band.child(
                        action_button(
                            ("eq-band", index * 512 + band_index * 2 + direction),
                            text,
                            can_step,
                            false,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if can_step && let Some(value) = candidate {
                                view.submit_control(
                                    ControlAction::Band {
                                        kind,
                                        preset: current_preset,
                                        index: band_index,
                                        value,
                                    },
                                    format!("{request_label}, level {value}"),
                                );
                                cx.notify();
                            }
                        })),
                    );
                }
                card = card.child(band);
            }
            if entry.band_info.is_none() {
                card = card.child(
                    action_button(("eq-labels", index), "Read band labels", enabled, false)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if enabled {
                                view.submit_control(
                                    ControlAction::BandInfo(kind),
                                    "Equalizer band labels".into(),
                                );
                                cx.notify();
                            }
                        })),
                );
            }
            cards = cards.child(card);
        }
        cards
    }
}

fn noise_ready(entry: &NoiseState) -> bool {
    entry.freshness == Freshness::Current
        && entry.capability.is_some()
        && entry.parameters.is_some()
        && entry
            .status
            .is_some_and(|status| status.availability == NoiseAvailability::Enabled)
}
fn noise_adaptation(entry: &NoiseState) -> Option<(NoiseSwitch, AdaptiveSensitivity)> {
    if entry.kind != NoiseKind::AdaptiveNcAndAmbientLevel
        || entry.capability.as_ref()?.kind() != entry.kind
    {
        return None;
    }
    match entry.parameters? {
        NoiseParameters::Control {
            settings:
                NoiseSettings::AdaptiveNcAndAmbientLevel {
                    adaptive,
                    sensitivity,
                    ..
                },
            ..
        } => Some((adaptive, sensitivity)),
        _ => None,
    }
}
fn equalizer_ready(entry: &EqualizerState) -> bool {
    entry.freshness == Freshness::Current
        && entry.capability.is_some()
        && entry.parameters.is_some()
        && entry
            .status
            .as_ref()
            .is_some_and(|status| status.enabled && status.errors.is_empty())
}
fn unique_ambient(
    entry: &NoiseState,
    mode: AmbientMode,
) -> Option<ezsc_core::noise::AmbientCapability> {
    let mut entries = entry
        .capability
        .as_ref()?
        .ambient_modes()
        .iter()
        .filter(|entry| entry.mode == mode);
    let result = entries.next().copied()?;
    entries.next().is_none().then_some(result)
}
fn next_level(range: AmbientLevelRange, current: u8, increase: bool) -> Option<u8> {
    if !range.contains(current) {
        return None;
    }
    let mut levels = (range.minimum()..=range.maximum()).filter(|level| range.contains(*level));
    if increase {
        levels.find(|level| *level > current)
    } else {
        levels.rfind(|level| *level < current)
    }
}
fn preset_label(preset: &Preset) -> String {
    if let Ok(name) = preset.name_utf8()
        && !name.trim().is_empty()
    {
        return name.into();
    }
    match preset.id {
        PresetId::OFF => "Off".into(),
        PresetId::ROCK => "Rock".into(),
        PresetId::BRIGHT => "Bright".into(),
        PresetId::HEAVY => "Heavy".into(),
        PresetId::CLEAR => "Clear".into(),
        PresetId::HARD => "Hard".into(),
        PresetId::SOFT => "Soft".into(),
        PresetId::CUSTOM => "Custom".into(),
        PresetId::USER_1 => "Custom 1".into(),
        PresetId::USER_2 => "Custom 2".into(),
        PresetId::USER_3 => "Custom 3".into(),
        PresetId::USER_4 => "Custom 4".into(),
        PresetId::USER_5 => "Custom 5".into(),
        id => format!("Preset 0x{:02X}", id.as_raw()),
    }
}
fn curve_band_label(
    index: usize,
    band_count: usize,
    info: Option<&ezsc_core::equalizer::EqualizerBandInfo>,
) -> String {
    info.filter(|info| info.bands.len() == band_count)
        .and_then(|info| info.bands.get(index))
        .copied()
        .map(|info| band_label(index, info))
        .unwrap_or_else(|| format!("Band {}", index + 1))
}

fn band_label(index: usize, info: BandInformation) -> String {
    if info.is_clear_bass() {
        "Clear Bass".into()
    } else if let Some(hertz) = info.frequency_hz() {
        if hertz >= 1000 {
            format!("{} kHz", hertz as f32 / 1000.0)
        } else {
            format!("{hertz} Hz")
        }
    } else {
        format!("Band {}", index + 1)
    }
}
pub(super) fn card(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_3()
        .p_5()
        .rounded_xl()
        .bg(rgb(0x222227))
        .child(div().text_lg().child(title))
}
pub(super) fn note(text: impl Into<gpui::SharedString>) -> Div {
    div().text_sm().text_color(rgb(0xb7b7c4)).child(text.into())
}
pub(super) fn availability(ready: bool, busy: bool, freshness: Freshness, available: bool) -> Div {
    note(if !ready {
        "Controls become available when the connection is ready."
    } else if busy {
        "An operation is pending. Showing the latest device-reported values."
    } else if freshness != Freshness::Current {
        "State needs to be refreshed before editing."
    } else if !available {
        "This control is temporarily unavailable on the headset."
    } else {
        "Showing current values reported by the headset."
    })
}
pub(super) fn action_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    enabled: bool,
    active: bool,
) -> gpui::Stateful<Div> {
    let mut element = button(id, label);
    if active {
        element = element.bg(rgb(0x6666a6));
    }
    if !enabled {
        element = element.opacity(0.45).cursor_default();
    }
    element
}

struct NoisePresentation {
    mode: Option<NoiseMode>,
    nc_on: bool,
    ambient_mode: Option<AmbientMode>,
    level: Option<u8>,
}
impl NoisePresentation {
    fn from_settings(settings: NoiseSettings) -> Self {
        match settings {
            NoiseSettings::AutoNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
                ..
            }
            | NoiseSettings::DualSingleNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
                ..
            }
            | NoiseSettings::DualNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
            }
            | NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode,
                ambient_mode,
                level,
                ..
            } => Self {
                mode: Some(mode),
                nc_on: mode == NoiseMode::NoiseCancelling,
                ambient_mode: Some(ambient_mode),
                level: Some(level),
            },
            NoiseSettings::NcAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | NoiseSettings::NcModesAndAmbientLevel {
                ambient_mode,
                level,
                ..
            }
            | NoiseSettings::AmbientLevel {
                ambient_mode,
                level,
            } => Self {
                mode: None,
                nc_on: false,
                ambient_mode: Some(ambient_mode),
                level: Some(level),
            },
            NoiseSettings::NcModesAndAmbientSwitch { ambient_mode, .. } => Self {
                mode: None,
                nc_on: false,
                ambient_mode: Some(ambient_mode),
                level: None,
            },
            NoiseSettings::NcOnly { nc } => Self {
                mode: None,
                nc_on: nc == NoiseSwitch::On,
                ambient_mode: None,
                level: None,
            },
            _ => Self {
                mode: None,
                nc_on: false,
                ambient_mode: None,
                level: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezsc_core::noise::{ChangeStatus, NoiseCapability, NoiseStatus};

    fn adaptive_state() -> NoiseState {
        let kind = NoiseKind::AdaptiveNcAndAmbientLevel;
        let table = kind.function_id().table;
        NoiseState {
            kind,
            capability: Some(
                NoiseCapability::decode_response(table, &[0x61, 0x19, 1, 0, 1, 9, 2]).unwrap(),
            ),
            status: Some(NoiseStatus::decode_response(table, &[0x63, 0x19, 0]).unwrap()),
            parameters: Some(NoiseParameters::Control {
                change: ChangeStatus::Changed,
                effect: NoiseSwitch::Off,
                settings: NoiseSettings::AdaptiveNcAndAmbientLevel {
                    mode: NoiseMode::NoiseCancelling,
                    ambient_mode: AmbientMode::Normal,
                    level: 7,
                    adaptive: NoiseSwitch::Off,
                    sensitivity: AdaptiveSensitivity::Low,
                },
            }),
            freshness: Freshness::Current,
        }
    }

    #[test]
    fn adaptation_controls_require_matching_capability_and_parameters() {
        let state = adaptive_state();
        // Stored sensitivity is available even with adaptation and effects off;
        // no model identity or selected sound mode is needed to expose it.
        assert!(noise_ready(&state));
        assert_eq!(
            noise_adaptation(&state),
            Some((NoiseSwitch::Off, AdaptiveSensitivity::Low))
        );

        let mut missing = state.clone();
        missing.capability = None;
        assert_eq!(noise_adaptation(&missing), None);
        let mut wrong_kind = state.clone();
        wrong_kind.kind = NoiseKind::DualNcAndAmbientLevel;
        assert_eq!(noise_adaptation(&wrong_kind), None);
        let mut wrong_capability = state.clone();
        wrong_capability.capability = Some(NoiseCapability::nc_only());
        assert_eq!(noise_adaptation(&wrong_capability), None);

        for settings in [
            NoiseSettings::NcOnly {
                nc: NoiseSwitch::On,
            },
            NoiseSettings::AmbientLevel {
                ambient_mode: AmbientMode::Normal,
                level: 7,
            },
            NoiseSettings::DualNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Normal,
                level: 7,
            },
        ] {
            let mut non_adaptive = state.clone();
            non_adaptive.parameters = Some(NoiseParameters::Control {
                change: ChangeStatus::Changed,
                effect: NoiseSwitch::On,
                settings,
            });
            assert_eq!(noise_adaptation(&non_adaptive), None);
        }
    }

    #[test]
    fn adaptation_values_remain_visible_when_editing_is_unavailable() {
        let state = adaptive_state();
        let expected = noise_adaptation(&state);
        for freshness in [Freshness::Unknown, Freshness::Stale] {
            let mut outdated = state.clone();
            outdated.freshness = freshness;
            assert!(!noise_ready(&outdated));
            assert_eq!(noise_adaptation(&outdated), expected);
        }
        let mut unavailable = state.clone();
        unavailable.status = Some(
            NoiseStatus::decode_response(state.kind.function_id().table, &[0x63, 0x19, 1]).unwrap(),
        );
        assert!(!noise_ready(&unavailable));
        assert_eq!(noise_adaptation(&unavailable), expected);
        unavailable.status = None;
        assert!(!noise_ready(&unavailable));
        unavailable.parameters = None;
        assert_eq!(noise_adaptation(&unavailable), None);
    }

    #[test]
    fn ambient_steps_respect_partial_terminal_steps_and_invalid_current_values() {
        let range = AmbientLevelRange::new(1, 6, 2).unwrap();
        assert_eq!(next_level(range, 5, true), Some(6));
        assert_eq!(next_level(range, 6, false), Some(5));
        assert_eq!(next_level(range, 1, false), None);
        assert_eq!(next_level(range, 6, true), None);
        assert_eq!(next_level(range, 2, true), None);
    }

    #[test]
    fn empty_preset_labels_use_known_names_and_preserve_unknown_identity() {
        let capability = ezsc_core::equalizer::EqualizerCapability::decode_response(
            EqualizerKind::Preset.function_id().table,
            &[
                0x51, 0, 3, 7, 4, 0, 0, 0xa1, 0, 0x20, 0, 0x42, 3, b'N', b'e', b'w',
            ],
        )
        .unwrap();
        let names: Vec<_> = capability.presets().iter().map(preset_label).collect();
        assert_eq!(names, ["Off", "Custom 1", "Preset 0x20", "New"]);
    }

    #[test]
    fn labels_fall_back_when_reported_curve_shape_changes() {
        let info = ezsc_core::equalizer::EqualizerBandInfo::decode_response(
            EqualizerKind::Preset.function_id().table,
            &[0x5b, 0, 2, 1, 0, 31, 1, 0, 63],
        )
        .unwrap();
        assert_eq!(curve_band_label(0, 2, Some(&info)), "31 Hz");
        assert_eq!(curve_band_label(0, 3, Some(&info)), "Band 1");
        assert_eq!(curve_band_label(0, 2, None), "Band 1");
    }

    #[test]
    fn band_labels_need_explicit_metadata() {
        use ezsc_core::equalizer::BandInformationType;
        assert_eq!(
            band_label(
                2,
                BandInformation {
                    information_type: BandInformationType::NONE,
                    value: 1000
                }
            ),
            "Band 3"
        );
        assert_eq!(
            band_label(
                0,
                BandInformation {
                    information_type: BandInformationType::SPECIFIC,
                    value: 1
                }
            ),
            "Clear Bass"
        );
        assert_eq!(
            band_label(
                0,
                BandInformation {
                    information_type: BandInformationType::HERTZ,
                    value: 2500
                }
            ),
            "2.5 kHz"
        );
    }
}
