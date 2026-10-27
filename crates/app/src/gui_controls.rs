//! Capability-driven controls. Draft slider positions never replace device state.

use crate::gui_daily_controls::{audio_change_ready, system_change_ready};
use crate::preferences::Language;
use crate::{Desktop, Event, Session};
#[cfg(test)]
use ezsc_core::noise::AmbientLevelRange;
use ezsc_core::{
    AudioChange, ConnectionState, DeviceHandle, EqualizerState, Freshness, NoiseChange, NoiseState,
    SystemChange,
    audio::AudioKind,
    equalizer::{BandInformation, EqualizerKind, Preset, PresetId},
    noise::{
        AdaptiveSensitivity, AmbientMode, NoiseAvailability, NoiseKind, NoiseMode, NoiseParameters,
        NoiseSettings, NoiseSwitch,
    },
    system::SystemKind,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::Button,
    menu::{DropdownMenu, PopupMenuItem},
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
};
use gpui_kit::{
    App, Context, Div, ElementId, Entity, FocusHandle, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use std::collections::{HashMap, VecDeque};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
impl ControlAction {
    fn same_slider(self, other: Self) -> bool {
        match (self, other) {
            (
                Self::Noise(a, NoiseChange::AmbientLevel(_)),
                Self::Noise(b, NoiseChange::AmbientLevel(_)),
            ) => a == b,
            (
                Self::Band {
                    kind: a,
                    preset: ap,
                    index: ai,
                    ..
                },
                Self::Band {
                    kind: b,
                    preset: bp,
                    index: bi,
                    ..
                },
            ) => a == b && ap == bp && ai == bi,
            _ => false,
        }
    }
}

/// Keeps the newest unsent value per control, without discarding other bands.
#[derive(Default)]
pub(super) struct PendingControls {
    queue: VecDeque<(u64, ControlAction, String)>,
    intents: Vec<(u64, ControlAction, u64)>,
    revision: u64,
}
impl PendingControls {
    pub(super) fn clear(&mut self) {
        self.queue.clear();
        self.intents.clear();
    }
    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    fn register(&mut self, epoch: u64, action: ControlAction) -> Option<u64> {
        self.intents
            .retain(|(generation, _, _)| *generation == epoch);
        let existing = self
            .intents
            .iter()
            .position(|(_, target, _)| target.same_slider(action));
        if existing.is_none() && self.intents.len() >= 256 {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        let intent = (epoch, action, self.revision);
        if let Some(index) = existing {
            self.intents[index] = intent;
        } else {
            self.intents.push(intent);
        }
        self.queue
            .retain(|(generation, queued, _)| *generation == epoch && !queued.same_slider(action));
        Some(self.revision)
    }
    fn is_latest(&self, epoch: u64, action: ControlAction, revision: u64) -> bool {
        self.intents.iter().any(|(generation, target, latest)| {
            *generation == epoch && target.same_slider(action) && *latest == revision
        })
    }
    fn push(&mut self, epoch: u64, action: ControlAction, label: String) -> bool {
        if let Some(item) = self
            .queue
            .iter_mut()
            .find(|(generation, queued, _)| *generation == epoch && queued.same_slider(action))
        {
            *item = (epoch, action, label);
            return true;
        }
        if self.queue.len() == 64 {
            return false;
        }
        self.queue.push_back((epoch, action, label));
        true
    }
}

impl Session {
    pub(super) fn submit_control(&mut self, action: ControlAction, label: String) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if self.preview || self.settings_blocked() || snapshot.connection != ConnectionState::Ready
        {
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
        if !action.same_slider(action) {
            self.pending_controls.clear();
        }
        self.busy = true;
        self.message = format!(
            "{}: {label}. {}",
            self.preferences.language.text("Pending", "等待确认"),
            self.preferences
                .language
                .text("Waiting for the headset's readback…", "正在等待耳机回读…")
        );
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

    fn register_slider_intent(&mut self, epoch: u64, action: ControlAction) -> Option<u64> {
        if self.preview
            || epoch != self.epoch
            || self.replying_to.is_some()
            || !self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.connection == ConnectionState::Ready && snapshot.pending_prompt.is_none()
            })
        {
            return None;
        }
        self.pending_controls.register(epoch, action)
    }

    fn submit_slider(&mut self, epoch: u64, intent: u64, action: ControlAction, label: String) {
        if self.preview
            || !self.pending_controls.is_latest(epoch, action, intent)
            || epoch != self.epoch
            || self.replying_to.is_some()
            || !self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.connection == ConnectionState::Ready && snapshot.pending_prompt.is_none()
            })
        {
            return;
        }
        if self.busy {
            if !self.pending_controls.push(epoch, action, label) {
                self.message = self
                    .preferences
                    .language
                    .text(
                        "Too many pending controls. Wait for the headset before editing again.",
                        "待处理的设置过多，请等待耳机响应后再编辑。",
                    )
                    .into();
            }
        } else {
            self.submit_control(action, label);
        }
    }

    pub(super) fn drain_pending_control(&mut self) {
        if self.settings_blocked() {
            return;
        }
        while let Some((epoch, action, label)) = self.pending_controls.queue.pop_front() {
            if epoch == self.epoch {
                self.submit_control(action, label);
                if self.busy {
                    break;
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct ControlWidgets {
    epoch: Option<u64>,
    sliders: HashMap<String, ControlSlider>,
}
struct ControlSlider {
    state: Entity<SliderState>,
    focus: FocusHandle,
    _subscription: Subscription,
    interacting: bool,
    last_submitted: Option<u8>,
    revision: u64,
    intent: Option<u64>,
    values: Vec<u8>,
    label: String,
}

impl Desktop {
    #[allow(clippy::too_many_arguments)]
    fn flush_slider(
        &mut self,
        key: &str,
        identity: gpui_kit::EntityId,
        revision: u64,
        epoch: u64,
        value: u8,
        action: ControlAction,
        label: String,
        cx: &mut Context<Self>,
    ) {
        let Some(widget) = self.controls.sliders.get_mut(key) else {
            return;
        };
        if widget.state.entity_id() != identity
            || widget.revision != revision
            || widget.last_submitted == Some(value)
        {
            return;
        }
        let Some(intent) = widget.intent else {
            return;
        };
        widget.last_submitted = Some(value);
        self.session.update(cx, |session, cx| {
            session.submit_slider(epoch, intent, action, label);
            cx.notify();
        });
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn control_slider(
        &mut self,
        key: String,
        label: String,
        values: Vec<u8>,
        current: u8,
        enabled: bool,
        action: impl Fn(u8) -> ControlAction + Clone + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let epoch = self.session.read(cx).epoch;
        if self.controls.epoch != Some(epoch) {
            self.controls.sliders.clear();
            self.controls.epoch = Some(epoch);
        }
        let current_index = values.iter().position(|value| *value == current);
        let enabled = enabled && current_index.is_some() && values.len() > 1;
        let index = current_index.unwrap_or(0) as f32;
        if self
            .controls
            .sliders
            .get(&key)
            .is_some_and(|entry| entry.values != values)
        {
            self.controls.sliders.remove(&key);
        }
        if !self.controls.sliders.contains_key(&key) {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(0.)
                    .max(values.len().saturating_sub(1) as f32)
                    .step(1.)
                    .default_value(index)
            });
            let event_key = key.clone();
            let event_action = action.clone();
            let subscription =
                cx.subscribe(&state, move |view, slider, event: &SliderEvent, cx| {
                    let value = match event {
                        SliderEvent::Change(value) | SliderEvent::Release(value) => value.end(),
                    };
                    if !value.is_finite() {
                        return;
                    }
                    let Some(widget) = view.controls.sliders.get_mut(&event_key) else {
                        return;
                    };
                    widget.interacting = matches!(event, SliderEvent::Change(_));
                    widget.revision = widget.revision.wrapping_add(1);
                    let revision = widget.revision;
                    let Some(value) = widget.values.get(value.round().max(0.) as usize).copied()
                    else {
                        return;
                    };
                    let action = event_action(value);
                    let event_label = widget.label.clone();
                    if matches!(event, SliderEvent::Change(_)) {
                        let intent = view.session.update(cx, |session, _| {
                            session.register_slider_intent(epoch, action)
                        });
                        let widget = view
                            .controls
                            .sliders
                            .get_mut(&event_key)
                            .expect("slider exists");
                        widget.intent = intent;
                        widget.last_submitted = None;
                    }
                    let label = format!("{event_label}: {value}");
                    let identity = slider.entity_id();
                    if matches!(event, SliderEvent::Release(_)) {
                        view.flush_slider(
                            &event_key, identity, revision, epoch, value, action, label, cx,
                        );
                    } else {
                        // A brief pause previews the latest draft; releasing always submits
                        // immediately. Superseded timers cannot enqueue stale pointer events.
                        let key = event_key.clone();
                        cx.spawn(async move |view, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(120))
                                .await;
                            let _ = view.update(cx, |view, cx| {
                                view.flush_slider(
                                    &key, identity, revision, epoch, value, action, label, cx,
                                );
                            });
                        })
                        .detach();
                    }
                    cx.notify();
                });
            self.controls.sliders.insert(
                key.clone(),
                ControlSlider {
                    state,
                    focus: cx.focus_handle(),
                    _subscription: subscription,
                    interacting: false,
                    last_submitted: Some(current),
                    revision: 0,
                    intent: None,
                    values,
                    label: label.clone(),
                },
            );
        }
        let session = self.session.read(cx);
        let settled = !session.busy && session.pending_controls.is_empty();
        let widget = self.controls.sliders.get_mut(&key).expect("slider created");
        widget.label = label.clone();
        if !enabled {
            widget.interacting = false;
            widget.revision = widget.revision.wrapping_add(1);
            widget.intent = None;
        }
        if !widget.interacting
            && (settled || !enabled)
            && widget.state.read(cx).value().end() != index
        {
            widget
                .state
                .update(cx, |state, cx| state.set_value(index, window, cx));
            widget.last_submitted = None;
        }
        let state = widget.state.clone();
        let focus = widget.focus.clone();
        let draft = widget
            .values
            .get(state.read(cx).value().end().round().max(0.) as usize)
            .copied()
            .unwrap_or(current);
        let session = self.session.clone();
        let key_state = state.clone();
        let key_values = widget.values.clone();
        let key_label = label.clone();
        let keyboard_action = action;
        div()
            .id(SharedString::from(key))
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .track_focus(&focus)
            .tab_stop(enabled)
            .on_key_down(move |event, window, cx| {
                if !enabled {
                    return;
                }
                let current = key_state.read(cx).value().end().round() as usize;
                let last = key_values.len().saturating_sub(1);
                let next = match event.keystroke.key.as_str() {
                    "left" | "down" => current.saturating_sub(1),
                    "right" | "up" => (current + 1).min(last),
                    "home" => 0,
                    "end" => last,
                    _ => return,
                };
                if let Some(value) = key_values.get(next) {
                    key_state.update(cx, |state, cx| state.set_value(next as f32, window, cx));
                    session.update(cx, |session, cx| {
                        let action = keyboard_action(*value);
                        if let Some(intent) = session.register_slider_intent(epoch, action) {
                            session.submit_slider(
                                epoch,
                                intent,
                                action,
                                format!("{key_label}: {value}"),
                            );
                        }
                        cx.notify();
                    });
                }
                cx.stop_propagation();
            })
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(label.clone())
                    .child(format!("{draft}")),
            )
            .child(
                Slider::new(&state)
                    .disabled(!enabled)
                    .accessibility_label(label)
                    .w_full(),
            )
    }

    pub(super) fn render_noise_controls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let (snapshot, busy, blocked) = (
            state.snapshot.clone(),
            state.busy,
            state.settings_blocked() || state.preview,
        );
        let mut cards = div().flex().flex_col().gap_4();
        let Some(snapshot) = snapshot else {
            return cards;
        };
        let ready = snapshot.connection == ConnectionState::Ready;
        let editable = ready && !blocked;
        let slider_editable = ready
            && !self.session.read(cx).preview
            && snapshot.pending_prompt.is_none()
            && self.session.read(cx).replying_to.is_none();
        for (index, entry) in snapshot.noise.iter().enumerate() {
            if entry.kind == NoiseKind::ButtonCycle {
                continue;
            }
            let enabled = editable && noise_ready(entry);
            let kind = entry.kind;
            let mut section =
                card(language.text("Noise control", "降噪与环境声"), cx).child(availability(
                    ready,
                    busy,
                    entry.freshness,
                    entry
                        .status
                        .is_some_and(|status| status.availability == NoiseAvailability::Enabled),
                    language,
                    cx,
                ));
            let Some(NoiseParameters::Control {
                effect, settings, ..
            }) = entry.parameters
            else {
                cards = cards.child(section.child(note(
                    language.text("Waiting for headset values…", "正在读取耳机状态…"),
                    cx,
                )));
                continue;
            };
            let presentation = NoisePresentation::from_settings(settings);
            let effect_on = effect == NoiseSwitch::On;
            let mut modes = div().flex().flex_wrap().gap_2();
            if let Some(mode) = presentation.mode {
                for (slot, choice, label) in [
                    (
                        0,
                        NoiseMode::NoiseCancelling,
                        language.text("Noise cancelling", "降噪"),
                    ),
                    (
                        1,
                        NoiseMode::AmbientSound,
                        language.text("Ambient sound", "环境声"),
                    ),
                ] {
                    modes = modes.child(control_button(
                        ("noise-mode", index * 3 + slot),
                        label,
                        enabled,
                        effect_on && mode == choice,
                        ControlAction::Noise(kind, NoiseChange::Mode(choice)),
                        self.session.clone(),
                    ));
                }
            } else {
                let label = if matches!(settings, NoiseSettings::NcOnly { .. }) {
                    language.text("Noise cancelling", "降噪")
                } else {
                    language.text("On", "开启")
                };
                let action = if matches!(settings, NoiseSettings::NcOnly { .. }) {
                    NoiseChange::Mode(NoiseMode::NoiseCancelling)
                } else {
                    NoiseChange::Effect(true)
                };
                modes = modes.child(control_button(
                    ("noise-effect-on", index),
                    label,
                    enabled,
                    effect_on
                        && (!matches!(settings, NoiseSettings::NcOnly { .. })
                            || presentation.nc_on),
                    ControlAction::Noise(kind, action),
                    self.session.clone(),
                ));
            }
            modes = modes.child(control_button(
                ("noise-off", index),
                language.text("Off", "关闭"),
                enabled,
                !effect_on,
                ControlAction::Noise(kind, NoiseChange::Effect(false)),
                self.session.clone(),
            ));
            section = section.child(modes);
            if let Some(ambient_mode) = presentation.ambient_mode {
                if let Some(level) = presentation.level {
                    if let Some(range) =
                        unique_ambient(entry, ambient_mode).and_then(|capability| capability.levels)
                    {
                        let values = (range.minimum()..=range.maximum())
                            .filter(|value| range.contains(*value))
                            .collect();
                        section = section.child(self.control_slider(
                            format!("ambient-{kind:?}-{ambient_mode:?}"),
                            language.text("Ambient sound level", "环境声等级").into(),
                            values,
                            level,
                            slider_editable && noise_ready(entry),
                            move |value| {
                                ControlAction::Noise(kind, NoiseChange::AmbientLevel(value))
                            },
                            window,
                            cx,
                        ));
                    } else {
                        section = section.child(note(
                            format!(
                                "{}: {level}",
                                language.text("Ambient sound level", "环境声等级")
                            ),
                            cx,
                        ));
                    }
                }
                let focus = ambient_mode == AmbientMode::Voice;
                let target = if focus {
                    AmbientMode::Normal
                } else {
                    AmbientMode::Voice
                };
                if unique_ambient(entry, AmbientMode::Voice).is_some() {
                    let can_focus = enabled
                        && unique_ambient(entry, target).is_some_and(|capability| {
                            match (capability.levels, presentation.level) {
                                (Some(range), Some(level)) => range.contains(level),
                                (None, None) => true,
                                _ => false,
                            }
                        });
                    let session = self.session.clone();
                    let label = language.text("Voice focus", "语音聚焦");
                    section = section.child(
                        Switch::new(("noise-voice", index))
                            .label(label)
                            .checked(focus)
                            .disabled(!can_focus)
                            .on_change(move |value, _, cx| {
                                session.update(cx, |session, cx| {
                                    session.submit_control(
                                        ControlAction::Noise(kind, NoiseChange::VoiceFocus(*value)),
                                        label.into(),
                                    );
                                    cx.notify();
                                })
                            }),
                    );
                }
            }
            if !self.compact
                && let Some((adaptive, sensitivity)) = noise_adaptation(entry)
            {
                let session = self.session.clone();
                let label = language.text("Automatic adjustment", "自动调节");
                section = section.child(
                    Switch::new(("noise-adaptive", index))
                        .label(label)
                        .checked(adaptive == NoiseSwitch::On)
                        .disabled(!enabled)
                        .on_change(move |value, _, cx| {
                            session.update(cx, |session, cx| {
                                session.submit_control(
                                    ControlAction::Noise(kind, NoiseChange::Adaptive(*value)),
                                    label.into(),
                                );
                                cx.notify();
                            })
                        }),
                );
                let mut choices = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(language.text("Sensitivity", "灵敏度"));
                for (slot, choice, label) in [
                    (0, AdaptiveSensitivity::Low, language.text("Low", "低")),
                    (
                        1,
                        AdaptiveSensitivity::Standard,
                        language.text("Standard", "标准"),
                    ),
                    (2, AdaptiveSensitivity::High, language.text("High", "高")),
                ] {
                    choices = choices.child(control_button(
                        ("noise-sensitivity", index * 3 + slot),
                        label,
                        enabled,
                        sensitivity == choice,
                        ControlAction::Noise(kind, NoiseChange::AdaptiveSensitivity(choice)),
                        self.session.clone(),
                    ));
                }
                section = section.child(choices);
            }
            cards = cards.child(section);
        }
        cards
    }

    pub(super) fn render_equalizer_controls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let language = self.language(cx);
        let state = self.session.read(cx);
        let (snapshot, busy, blocked) = (
            state.snapshot.clone(),
            state.busy,
            state.settings_blocked() || state.preview,
        );
        let mut cards = div().flex().flex_col().gap_4();
        let Some(snapshot) = snapshot else {
            return cards;
        };
        let ready = snapshot.connection == ConnectionState::Ready;
        let editable = ready && !blocked;
        let slider_editable = ready
            && !self.session.read(cx).preview
            && snapshot.pending_prompt.is_none()
            && self.session.read(cx).replying_to.is_none();
        for (index, entry) in snapshot.equalizers.iter().enumerate() {
            let enabled = editable && equalizer_ready(entry);
            let mut section = card(language.text("Equalizer", "均衡器"), cx).child(availability(
                ready,
                busy,
                entry.freshness,
                entry
                    .status
                    .as_ref()
                    .is_some_and(|status| status.enabled && status.errors.is_empty()),
                language,
                cx,
            ));
            let (Some(capability), Some(parameters)) = (&entry.capability, &entry.parameters)
            else {
                cards = cards.child(section.child(note(
                    language.text("Waiting for equalizer values…", "正在读取均衡器…"),
                    cx,
                )));
                continue;
            };
            let kind = entry.kind;
            let preset = parameters.preset();
            if self.compact {
                let items: Vec<_> = capability
                    .presets()
                    .iter()
                    .map(|item| (item.id, localized_preset(item, language)))
                    .collect();
                let current = items
                    .iter()
                    .find(|(id, _)| Some(*id) == preset)
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| language.text("Select preset", "选择预设").into());
                let session = self.session.clone();
                let dropdown = Button::new(("tray-eq", index))
                    .label(current)
                    .dropdown_caret(true)
                    .disabled(!enabled)
                    .w_full()
                    .dropdown_menu(move |mut menu, _, _| {
                        for (id, label) in &items {
                            let id = *id;
                            let label = label.clone();
                            let session = session.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label.clone())
                                    .checked(preset == Some(id))
                                    .disabled(!enabled)
                                    .on_click(move |_, _, cx| {
                                        session.update(cx, |session, cx| {
                                            session.submit_control(
                                                ControlAction::Preset(kind, id),
                                                label.clone(),
                                            );
                                            cx.notify();
                                        })
                                    }),
                            );
                        }
                        menu.scrollable(true).max_h(px(260.))
                    });
                cards = cards.child(section.child(dropdown));
                continue;
            }
            let mut presets = div().flex().flex_wrap().gap_2();
            for (slot, item) in capability.presets().iter().enumerate() {
                let label = localized_preset(item, language);
                presets = presets.child(control_button(
                    ("eq-preset", index * 256 + slot),
                    label,
                    enabled,
                    preset == Some(item.id),
                    ControlAction::Preset(kind, item.id),
                    self.session.clone(),
                ));
            }
            section = section.child(presets);
            let valid_shape = parameters.levels().len() == usize::from(capability.band_count())
                && capability.band_count() > 0
                && capability.level_steps() > 0
                && parameters
                    .levels()
                    .iter()
                    .all(|value| *value < capability.level_steps());
            let can_edit = slider_editable
                && equalizer_ready(entry)
                && valid_shape
                && kind != EqualizerKind::PresetNonCustomizable
                && (kind == EqualizerKind::Custom
                    || preset.is_some_and(|value| {
                        value.is_custom_slot() && capability.supports_preset(value)
                    }));
            if valid_shape {
                section = section.child(equalizer_curve(
                    parameters.levels().to_vec(),
                    capability.level_steps(),
                    cx,
                ));
            }
            if enabled && kind != EqualizerKind::PresetNonCustomizable && !can_edit {
                section = section.child(note(
                    if valid_shape {
                        language.text(
                            "Choose a custom preset to edit the curve.",
                            "请选择自定义预设以编辑曲线。",
                        )
                    } else {
                        language.text(
                            "Refresh the equalizer before editing.",
                            "请刷新均衡器状态后再编辑。",
                        )
                    },
                    cx,
                ));
            }
            for (band, level) in parameters.levels().iter().copied().enumerate() {
                let label = if entry.band_info.is_some() {
                    curve_band_label(
                        band,
                        parameters.levels().len(),
                        entry.band_info.as_ref().filter(|_| valid_shape),
                    )
                } else {
                    format!("{} {}", language.text("Band", "频段"), band + 1)
                };
                section = section.child(self.control_slider(
                    format!("eq-{kind:?}-{preset:?}-{band}"),
                    label,
                    (0..capability.level_steps()).collect(),
                    level,
                    can_edit,
                    move |value| ControlAction::Band {
                        kind,
                        preset,
                        index: band,
                        value,
                    },
                    window,
                    cx,
                ));
            }
            section = section.child(note(
                language.text(
                    "Values use the headset's native steps.",
                    "数值使用耳机原生档位。",
                ),
                cx,
            ));
            if entry.band_info.is_none() {
                section = section.child(control_button(
                    ("eq-labels", index),
                    language.text("Read frequency labels", "读取频率标签"),
                    enabled,
                    false,
                    ControlAction::BandInfo(kind),
                    self.session.clone(),
                ));
            }
            cards = cards.child(section);
        }
        cards
    }
}

fn equalizer_curve(levels: Vec<u8>, steps: u8, cx: &App) -> Div {
    let color = cx.theme().primary;
    let grid = cx.theme().border;
    div().h(px(120.)).w_full().child(
        gpui_kit::canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let width = bounds.size.width;
                let height = bounds.size.height;
                for row in 0..=4 {
                    let y = bounds.origin.y + height * (row as f32 / 4.);
                    let mut line = gpui_kit::PathBuilder::stroke(px(1.));
                    line.move_to(gpui_kit::point(bounds.origin.x, y));
                    line.line_to(gpui_kit::point(bounds.origin.x + width, y));
                    if let Ok(path) = line.build() {
                        window.paint_path(path, grid);
                    }
                }
                if levels.len() < 2 || steps < 2 {
                    return;
                }
                let mut curve = gpui_kit::PathBuilder::stroke(px(2.));
                for (index, value) in levels.iter().enumerate() {
                    let point = gpui_kit::point(
                        bounds.origin.x + width * (index as f32 / (levels.len() - 1) as f32),
                        bounds.origin.y + height * (1. - f32::from(*value) / f32::from(steps - 1)),
                    );
                    if index == 0 {
                        curve.move_to(point);
                    } else {
                        curve.line_to(point);
                    }
                }
                if let Ok(path) = curve.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full(),
    )
}

pub(super) fn control_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    selected: bool,
    action: ControlAction,
    session: Entity<Session>,
) -> Button {
    let label = label.into();
    action_button(id, label.clone(), enabled, selected).on_click(move |_, _, cx| {
        session.update(cx, |session, cx| {
            session.submit_control(action, label.to_string());
            cx.notify();
        });
    })
}

fn localized_preset(preset: &Preset, language: Language) -> String {
    if preset.name_utf8().is_ok_and(|name| !name.trim().is_empty()) {
        return preset_label(preset);
    }
    if preset.id == PresetId::OFF {
        language.text("Off", "关闭").into()
    } else if preset.id == PresetId::CUSTOM {
        language.text("Custom", "自定义").into()
    } else if preset.id == PresetId::USER_1 {
        language.text("Custom 1", "自定义 1").into()
    } else if preset.id == PresetId::USER_2 {
        language.text("Custom 2", "自定义 2").into()
    } else {
        preset_label(preset)
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
#[cfg(test)]
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
pub(super) fn card(title: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_3()
        .p(px(20.))
        .rounded(px(20.))
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_size(px(15.))
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(title.into()),
        )
}
pub(super) fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}
pub(super) fn availability(
    ready: bool,
    busy: bool,
    freshness: Freshness,
    available: bool,
    language: Language,
    cx: &App,
) -> Div {
    note(
        if !ready {
            language.text(
                "Connect your headset to use this control.",
                "连接耳机后可使用此功能。",
            )
        } else if busy {
            language.text("Applying changes…", "正在应用更改…")
        } else if freshness != Freshness::Current {
            language.text("Refresh state before editing.", "请刷新状态后再编辑。")
        } else if !available {
            language.text(
                "Temporarily unavailable on the headset.",
                "耳机当前暂不可用。",
            )
        } else {
            language.text("Synced with your headset", "已与耳机同步")
        },
        cx,
    )
}
pub(super) fn action_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    enabled: bool,
    active: bool,
) -> Button {
    Button::new(id)
        .label(label)
        .small()
        .selected(active)
        .disabled(!enabled)
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

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn band(index: usize, value: u8) -> ControlAction {
        ControlAction::Band {
            kind: EqualizerKind::Custom,
            preset: None,
            index,
            value,
        }
    }

    #[test]
    fn newer_window_intent_invalidates_old_timers_and_unsent_values() {
        let mut queue = PendingControls::default();
        let main = queue.register(1, band(0, 3)).unwrap();
        assert!(queue.push(1, band(0, 3), "main".into()));
        let tray = queue.register(1, band(0, 8)).unwrap();
        assert!(!queue.is_latest(1, band(0, 3), main));
        assert!(queue.is_latest(1, band(0, 8), tray));
        assert!(queue.is_empty());
        assert!(queue.push(1, band(0, 8), "tray".into()));
        // A different frequency has its own final intent.
        let other = queue.register(1, band(1, 5)).unwrap();
        assert!(queue.is_latest(1, band(0, 8), tray));
        assert!(queue.is_latest(1, band(1, 5), other));
        assert_eq!(queue.queue.len(), 1);
        queue.clear();
        assert!(!queue.is_latest(1, band(0, 8), tray));
        let resumed = queue.register(1, band(0, 8)).unwrap();
        assert_ne!(resumed, tray);
    }

    #[test]
    fn coalescing_keeps_the_final_value_of_each_band() {
        let mut queue = PendingControls::default();
        assert!(queue.push(3, band(0, 1), "first".into()));
        assert!(queue.push(3, band(1, 4), "second".into()));
        assert!(queue.push(3, band(0, 7), "final".into()));
        assert_eq!(queue.queue.len(), 2);
        assert!(
            matches!(queue.queue.pop_front(), Some((3, ControlAction::Band { index: 0, value: 7, .. }, label)) if label == "final")
        );
        assert!(matches!(
            queue.queue.pop_front(),
            Some((
                3,
                ControlAction::Band {
                    index: 1,
                    value: 4,
                    ..
                },
                _
            ))
        ));
    }

    #[test]
    fn queue_is_bounded_but_can_replace_an_existing_target_when_full() {
        let mut queue = PendingControls::default();
        for index in 0..64 {
            assert!(queue.push(1, band(index, 1), "value".into()));
        }
        assert!(!queue.push(1, band(64, 1), "new".into()));
        assert!(queue.push(1, band(0, 9), "last".into()));
        assert_eq!(queue.queue.len(), 64);
        assert!(matches!(
            queue.queue.front(),
            Some((1, ControlAction::Band { value: 9, .. }, _))
        ));
        queue.clear();
        assert!(queue.is_empty());
    }

    #[test]
    fn control_identity_does_not_merge_different_connections_or_presets() {
        let mut queue = PendingControls::default();
        assert!(queue.push(1, band(0, 1), "old".into()));
        assert!(queue.push(2, band(0, 2), "new".into()));
        assert!(queue.push(
            2,
            ControlAction::Band {
                kind: EqualizerKind::Custom,
                preset: Some(PresetId::USER_1),
                index: 0,
                value: 3
            },
            "slot".into()
        ));
        assert_eq!(queue.queue.len(), 3);
    }
}
