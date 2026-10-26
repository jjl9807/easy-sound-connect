//! Independent, capability-selected NC/ambient configuration round trips.
//! Each case restores its own saved baseline before another case can start.

use bluetooth::CancellationToken;
use ezsc_core::{
    DeviceHandle, DeviceManager, Freshness, NoiseChange, NoiseState,
    noise::{
        AdaptiveSensitivity, AmbientCapability, AmbientLevelRange, AmbientMode, ChangeStatus,
        NoiseAvailability, NoiseCapability, NoiseKind, NoiseMode, NoiseParameters, NoiseSettings,
        NoiseSwitch,
    },
};

use super::{CheckResult, confirmed, finish, noise_matches, recover};

struct NoiseCase {
    label: &'static str,
    change: NoiseChange,
    restore: Vec<NoiseChange>,
    expected: NoiseParameters,
    scope: &'static str,
}
#[derive(Default)]
struct NoisePlan {
    cases: Vec<NoiseCase>,
    skipped: Vec<String>,
}

#[derive(Default)]
struct NoiseResults {
    failures: Vec<String>,
}

impl NoiseResults {
    fn record(&mut self, label: &str, tested: CheckResult, restored: CheckResult) -> CheckResult {
        // Restoration failure prevents every subsequent mutation. An unsuccessful
        // check is safe to collect only after its full saved baseline is verified.
        if restored.is_err() {
            return finish(label, tested, restored);
        }
        if let Err(error) = finish(label, tested, restored) {
            let failure = format!("{label}: {error}");
            eprintln!("{failure}; unsuccessful check recorded after verified restoration");
            self.failures.push(failure);
        }
        Ok(())
    }

    fn finish(self) -> CheckResult {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "Noise checks did not confirm the expected outcome in {} case(s); each saved baseline was restored and verified:\n{}",
                self.failures.len(),
                self.failures.join("\n")
            )
            .into())
        }
    }
}

pub(super) async fn run(
    manager: &DeviceManager,
    handle: &mut DeviceHandle,
    stop: &CancellationToken,
    advertised: Vec<NoiseState>,
) -> CheckResult {
    let mut results = NoiseResults::default();
    for entry in advertised {
        if stop.is_cancelled() {
            return results.finish();
        }
        if entry.kind == NoiseKind::ButtonCycle {
            println!("Noise button cycle: not part of reversible sound-control checks; skipped");
            continue;
        }
        handle.refresh_noise(entry.kind).await?;
        // A cancellation arriving during the read must prevent the first write.
        if stop.is_cancelled() {
            return results.finish();
        }
        let state = noise_state(handle, entry.kind)?;
        if !available(&state) {
            println!(
                "Noise {:?}: current available state is missing; skipped",
                entry.kind
            );
            continue;
        }
        let capability = state.capability.ok_or("Missing noise capability")?;
        let original = state.parameters.ok_or("Missing noise parameters")?;
        let plan = plan(&capability, original);
        println!("Saved noise {:?}: {original:?}", entry.kind);
        for reason in plan.skipped {
            println!("Noise {:?}: {reason}; skipped", entry.kind);
        }
        for case in plan.cases {
            if stop.is_cancelled() {
                return results.finish();
            }
            handle.refresh_noise(entry.kind).await?;
            if stop.is_cancelled() {
                return results.finish();
            }
            let before = noise_state(handle, entry.kind)?;
            if !available(&before) {
                return Err(format!(
                    "{}: noise control became unavailable before the check",
                    case.label
                )
                .into());
            }
            if before.capability.as_ref() != Some(&capability) {
                return Err(format!(
                    "{}: capability changed after restoration planning; no check write was made",
                    case.label
                )
                .into());
            }
            if before
                .parameters
                .is_none_or(|actual| !noise_matches(original, actual))
            {
                return Err(format!(
                    "{}: the saved baseline changed before mutation; no check write was made",
                    case.label
                )
                .into());
            }
            println!("{}: request {:?}; {}", case.label, case.change, case.scope);
            let tested = async {
                let result = handle.set_noise(entry.kind, case.change).await;
                let actual = noise_state(handle, entry.kind)?
                    .parameters
                    .ok_or("Missing noise readback")?;
                println!("{}: result {result:?}; readback {actual:?}", case.label);
                confirmed(result?)?;
                if !noise_matches(case.expected, actual) {
                    return Err(format!(
                        "{}: readback changed more than the requested field",
                        case.label
                    )
                    .into());
                }
                Ok(())
            }
            .await;
            // Once a write has been attempted, cancellation and test failure
            // both reach restoration. Never select/drop this cleanup future.
            let restored = restore(manager, handle, entry.kind, original, &case).await;
            results.record(case.label, tested, restored)?;
            if stop.is_cancelled() {
                println!(
                    "Cancellation requested; the current noise check was restored before stopping."
                );
                return results.finish();
            }
        }
    }
    results.finish()
}

fn noise_state(
    handle: &DeviceHandle,
    kind: NoiseKind,
) -> Result<NoiseState, Box<dyn std::error::Error>> {
    handle
        .snapshot()
        .noise
        .into_iter()
        .find(|state| state.kind == kind)
        .ok_or_else(|| "Missing noise state".into())
}
fn available(state: &NoiseState) -> bool {
    state.freshness == Freshness::Current
        && state
            .status
            .is_some_and(|status| status.availability == NoiseAvailability::Enabled)
        && state.capability.is_some()
        && state.parameters.is_some()
}

async fn restore(
    manager: &DeviceManager,
    handle: &mut DeviceHandle,
    kind: NoiseKind,
    original: NoiseParameters,
    case: &NoiseCase,
) -> CheckResult {
    let mut errors = Vec::new();
    // In particular Mode(original) enables the effect, so its original switch
    // must be restored last. Attempt that final restoration even if mode fails.
    for change in &case.restore {
        println!("{}: restoring {change:?}", case.label);
        let result = async {
            recover(manager, handle).await?;
            let result = handle.set_noise(kind, *change).await;
            println!(
                "{}: restoration result {result:?}; state {:?}",
                case.label,
                noise_state(handle, kind)
                    .ok()
                    .and_then(|state| state.parameters)
            );
            confirmed(result?)
        }
        .await;
        if let Err(error) = result {
            eprintln!(
                "{}: restoration attempt reported {error}; remaining restoration steps will still run",
                case.label
            );
            errors.push(error.to_string());
        }
    }
    // The final fresh query decides whether restoration succeeded. A previous
    // request may have failed after the headset already applied the saved value.
    recover(manager, handle).await?;
    handle.refresh_noise(kind).await?;
    let actual = noise_state(handle, kind)?
        .parameters
        .ok_or("Missing final restored noise state")?;
    println!("{}: final restoration readback {actual:?}", case.label);
    if !noise_matches(original, actual) {
        return Err(format!("Noise settings differ from saved original {original:?}; actual {actual:?}; restoration errors: {errors:?}").into());
    }
    Ok(())
}

fn plan(capability: &NoiseCapability, original: NoiseParameters) -> NoisePlan {
    let mut plan = NoisePlan::default();
    let NoiseParameters::Control {
        effect, settings, ..
    } = original
    else {
        plan.skipped
            .push("not a noise-control parameter layout".into());
        return plan;
    };
    if let Err(error) = original.encode_set(capability) {
        plan.skipped.push(format!(
            "the saved baseline cannot be restored using the advertised capability: {error}"
        ));
        return plan;
    }
    // Turning the total effect off may also reset an explicit mode. Restore
    // that saved mode before the saved switch, even for an effect-only check.
    // A device-adjusted forward result still remains Adjusted, not Confirmed.
    let mut effect_restore = Vec::new();
    if let Some(saved_mode) = mode(settings) {
        effect_restore.push(NoiseChange::Mode(saved_mode));
    }
    effect_restore.push(NoiseChange::Effect(effect == NoiseSwitch::On));
    plan.add(
        capability,
        original,
        "Noise effect",
        NoiseChange::Effect(effect != NoiseSwitch::On),
        effect_restore,
        "total-effect configuration round trip",
    );
    if let Some(mode) = mode(settings) {
        let alternate = if mode == NoiseMode::NoiseCancelling {
            NoiseMode::AmbientSound
        } else {
            NoiseMode::NoiseCancelling
        };
        plan.add(
            capability,
            original,
            "Noise mode",
            NoiseChange::Mode(alternate),
            vec![
                NoiseChange::Mode(mode),
                NoiseChange::Effect(effect == NoiseSwitch::On),
            ],
            "explicit NC/ambient selection; original effect switch is restored last",
        );
    } else {
        plan.skipped
            .push("NC/ambient mode switching has no explicit mode field in this layout".into());
    }
    if let Some((ambient_mode, level)) = ambient(settings) {
        if let Some(level) = level {
            let alternate = unique_ambient(capability, ambient_mode)
                .and_then(|entry| entry.levels)
                .and_then(|range| adjacent_level(range, level));
            if let Some(alternate) = alternate {
                plan.add(
                    capability,
                    original,
                    "Ambient level",
                    NoiseChange::AmbientLevel(alternate),
                    vec![NoiseChange::AmbientLevel(level)],
                    "stored level round trip; does not enable an inactive ambient effect",
                );
            } else {
                plan.skipped.push("ambient level has no distinct valid adjacent value, or its stored inactive value is outside the active range".into());
            }
        } else {
            plan.skipped
                .push("ambient processing is on/off only; no level adjustment".into());
        }
        let alternate = if ambient_mode == AmbientMode::Voice {
            AmbientMode::Normal
        } else {
            AmbientMode::Voice
        };
        if unique_ambient(capability, ambient_mode).is_some_and(|entry| level_valid(entry, level))
            && unique_ambient(capability, alternate).is_some_and(|entry| level_valid(entry, level))
        {
            plan.add(capability, original, "Voice focus", NoiseChange::VoiceFocus(alternate == AmbientMode::Voice), vec![NoiseChange::VoiceFocus(ambient_mode == AmbientMode::Voice)], "stored ambient-processing round trip; current level is valid in both modes, with no clamping");
        } else {
            plan.skipped.push("voice/normal switching is not uniquely advertised with the saved level valid in both modes".into());
        }
    } else {
        plan.skipped
            .push("ambient level and voice focus are absent from this layout".into());
    }
    if let NoiseSettings::AdaptiveNcAndAmbientLevel {
        adaptive,
        sensitivity,
        ..
    } = settings
    {
        plan.add(
            capability,
            original,
            "Noise adaptation",
            NoiseChange::Adaptive(adaptive != NoiseSwitch::On),
            vec![NoiseChange::Adaptive(adaptive == NoiseSwitch::On)],
            "adaptation-switch configuration round trip; no claim about audible adaptation",
        );
        let alternate = if sensitivity == AdaptiveSensitivity::Standard {
            AdaptiveSensitivity::High
        } else {
            AdaptiveSensitivity::Standard
        };
        plan.add(
            capability,
            original,
            "Adaptive sensitivity",
            NoiseChange::AdaptiveSensitivity(alternate),
            vec![NoiseChange::AdaptiveSensitivity(sensitivity)],
            "stored sensitivity round trip; does not enable adaptation or claim an audible change",
        );
    } else {
        plan.skipped
            .push("adaptation and sensitivity fields are absent from this layout".into());
    }
    plan
}

impl NoisePlan {
    fn add(
        &mut self,
        capability: &NoiseCapability,
        original: NoiseParameters,
        label: &'static str,
        change: NoiseChange,
        restore: Vec<NoiseChange>,
        scope: &'static str,
    ) {
        let Some(expected) = projected(original, change) else {
            self.skipped
                .push(format!("{label} is not safely projectable"));
            return;
        };
        let mut restored = expected;
        let valid = expected.encode_set(capability).is_ok()
            && restore.iter().all(|change| {
                if let Some(next) = projected(restored, *change)
                    && next.encode_set(capability).is_ok()
                {
                    restored = next;
                    true
                } else {
                    false
                }
            })
            && noise_matches(original, restored);
        if valid {
            self.cases.push(NoiseCase {
                label,
                change,
                restore,
                expected,
                scope,
            });
        } else {
            self.skipped
                .push(format!("{label} has no validated exact restoration path"));
        }
    }
}

fn unique_ambient(capability: &NoiseCapability, mode: AmbientMode) -> Option<AmbientCapability> {
    let mut entries = capability
        .ambient_modes()
        .iter()
        .filter(|entry| entry.mode == mode);
    let found = entries.next().copied()?;
    entries.next().is_none().then_some(found)
}
fn level_valid(capability: AmbientCapability, level: Option<u8>) -> bool {
    match (capability.levels, level) {
        (Some(range), Some(level)) => range.contains(level),
        (None, None) => true,
        _ => false,
    }
}
fn adjacent_level(range: AmbientLevelRange, current: u8) -> Option<u8> {
    if !range.contains(current) {
        return None;
    }
    let mut allowed = (range.minimum()..=range.maximum()).filter(|level| range.contains(*level));
    allowed
        .clone()
        .find(|level| *level > current)
        .or_else(|| allowed.rfind(|level| *level < current))
}
fn mode(settings: NoiseSettings) -> Option<NoiseMode> {
    match settings {
        NoiseSettings::AutoNcAndAmbientLevel { mode, .. }
        | NoiseSettings::DualSingleNcAndAmbientLevel { mode, .. }
        | NoiseSettings::DualNcAndAmbientLevel { mode, .. }
        | NoiseSettings::AdaptiveNcAndAmbientLevel { mode, .. } => Some(mode),
        _ => None,
    }
}
fn ambient(settings: NoiseSettings) -> Option<(AmbientMode, Option<u8>)> {
    match settings {
        NoiseSettings::NcModesAndAmbientSwitch { ambient_mode, .. } => Some((ambient_mode, None)),
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
        | NoiseSettings::AutoNcAndAmbientLevel {
            ambient_mode,
            level,
            ..
        }
        | NoiseSettings::DualSingleNcAndAmbientLevel {
            ambient_mode,
            level,
            ..
        }
        | NoiseSettings::DualNcAndAmbientLevel {
            ambient_mode,
            level,
            ..
        }
        | NoiseSettings::AdaptiveNcAndAmbientLevel {
            ambient_mode,
            level,
            ..
        }
        | NoiseSettings::AmbientLevel {
            ambient_mode,
            level,
        } => Some((ambient_mode, Some(level))),
        _ => None,
    }
}

// Build the expected acceptance result independently from the core operation.
// Comparing the whole result checks preservation of every untouched field.
fn projected(mut parameters: NoiseParameters, change: NoiseChange) -> Option<NoiseParameters> {
    let NoiseParameters::Control {
        change: status,
        effect,
        settings,
    } = &mut parameters
    else {
        return None;
    };
    *status = ChangeStatus::Changed;
    match change {
        NoiseChange::Effect(value) => {
            *effect = if value {
                NoiseSwitch::On
            } else {
                NoiseSwitch::Off
            }
        }
        NoiseChange::Mode(value) => {
            let field = match settings {
                NoiseSettings::AutoNcAndAmbientLevel { mode, .. }
                | NoiseSettings::DualSingleNcAndAmbientLevel { mode, .. }
                | NoiseSettings::DualNcAndAmbientLevel { mode, .. }
                | NoiseSettings::AdaptiveNcAndAmbientLevel { mode, .. } => mode,
                _ => return None,
            };
            *field = value;
            *effect = NoiseSwitch::On;
        }
        NoiseChange::AmbientLevel(value) => {
            let field = match settings {
                NoiseSettings::NcAndAmbientLevel { level, .. }
                | NoiseSettings::NcModesAndAmbientLevel { level, .. }
                | NoiseSettings::AutoNcAndAmbientLevel { level, .. }
                | NoiseSettings::DualSingleNcAndAmbientLevel { level, .. }
                | NoiseSettings::DualNcAndAmbientLevel { level, .. }
                | NoiseSettings::AdaptiveNcAndAmbientLevel { level, .. }
                | NoiseSettings::AmbientLevel { level, .. } => level,
                _ => return None,
            };
            *field = value;
        }
        NoiseChange::VoiceFocus(value) => {
            let field = match settings {
                NoiseSettings::NcModesAndAmbientSwitch { ambient_mode, .. }
                | NoiseSettings::NcAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::NcModesAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::AutoNcAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::DualSingleNcAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::DualNcAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::AdaptiveNcAndAmbientLevel { ambient_mode, .. }
                | NoiseSettings::AmbientLevel { ambient_mode, .. } => ambient_mode,
                _ => return None,
            };
            *field = if value {
                AmbientMode::Voice
            } else {
                AmbientMode::Normal
            };
        }
        NoiseChange::Adaptive(value) => {
            let NoiseSettings::AdaptiveNcAndAmbientLevel { adaptive, .. } = settings else {
                return None;
            };
            *adaptive = if value {
                NoiseSwitch::On
            } else {
                NoiseSwitch::Off
            };
        }
        NoiseChange::AdaptiveSensitivity(value) => {
            let NoiseSettings::AdaptiveNcAndAmbientLevel { sensitivity, .. } = settings else {
                return None;
            };
            *sensitivity = value;
        }
    }
    Some(parameters)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezsc_core::{OperationResult, noise::NcValue};

    #[test]
    fn restored_adjustment_and_failure_allow_independent_checks_but_fail_the_suite() {
        let mut results = NoiseResults::default();
        let mut visited = Vec::new();
        for (label, tested) in [
            ("Ambient level", confirmed(OperationResult::Adjusted)),
            ("Voice focus", Err("readback timed out".into())),
            (
                "Adaptive sensitivity",
                confirmed(OperationResult::Confirmed),
            ),
        ] {
            visited.push(label);
            results.record(label, tested, Ok(())).unwrap();
        }
        assert_eq!(visited.len(), 3);
        let error = results.finish().unwrap_err().to_string();
        assert!(error.contains("2 case(s)"));
        assert!(error.contains("Ambient level: Expected confirmed readback, received Adjusted"));
        assert!(error.contains("Voice focus: readback timed out"));
        assert!(!error.contains("Adaptive sensitivity"));
    }

    #[test]
    fn unverified_restoration_stops_before_the_next_independent_check() {
        let mut results = NoiseResults::default();
        let mut visited = Vec::new();
        let result = (|| -> CheckResult {
            for (label, restored) in [
                ("Ambient level", Err("saved settings differ".into())),
                ("Voice focus", Ok(())),
            ] {
                visited.push(label);
                results.record(label, confirmed(OperationResult::Adjusted), restored)?;
            }
            results.finish()
        })();
        assert_eq!(visited, ["Ambient level"]);
        let error = result.unwrap_err().to_string();
        assert!(error.contains("ORIGINAL SETTINGS NOT CONFIRMED RESTORED"));
        assert!(error.contains("saved settings differ"));
        assert!(error.contains("Adjusted"));
    }

    fn capability(kind: NoiseKind, modes: &[u8]) -> NoiseCapability {
        let mut payload = vec![0x61, kind.inquiry()];
        if kind != NoiseKind::NcOnly {
            payload.push(
                (modes.len()
                    / if kind == NoiseKind::NcModesAndAmbientSwitch {
                        1
                    } else {
                        4
                    }) as u8,
            );
            payload.extend(modes);
        }
        NoiseCapability::decode_response(kind.function_id().table, &payload).unwrap()
    }
    fn parameters(effect: NoiseSwitch, settings: NoiseSettings) -> NoiseParameters {
        NoiseParameters::Control {
            change: ChangeStatus::Changing,
            effect,
            settings,
        }
    }
    fn adaptive(effect: NoiseSwitch) -> NoiseParameters {
        parameters(
            effect,
            NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: NoiseMode::NoiseCancelling,
                ambient_mode: AmbientMode::Normal,
                level: 5,
                adaptive: NoiseSwitch::Off,
                sensitivity: AdaptiveSensitivity::Low,
            },
        )
    }

    #[test]
    fn effect_check_restores_saved_ambient_mode_after_device_normalization() {
        let capability = capability(
            NoiseKind::AdaptiveNcAndAmbientLevel,
            &[0, 1, 6, 2, 1, 1, 6, 2],
        );
        let original = parameters(
            NoiseSwitch::On,
            NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Normal,
                level: 5,
                adaptive: NoiseSwitch::Off,
                sensitivity: AdaptiveSensitivity::Low,
            },
        );
        let plan = plan(&capability, original);
        let case = plan
            .cases
            .iter()
            .find(|case| case.label == "Noise effect")
            .unwrap();
        assert_eq!(
            case.restore,
            [
                NoiseChange::Mode(NoiseMode::AmbientSound),
                NoiseChange::Effect(true),
            ]
        );
        // Model the observed adjustment explicitly: Off also resets the mode.
        let normalized = parameters(
            NoiseSwitch::Off,
            NoiseSettings::AdaptiveNcAndAmbientLevel {
                mode: NoiseMode::NoiseCancelling,
                ambient_mode: AmbientMode::Normal,
                level: 5,
                adaptive: NoiseSwitch::Off,
                sensitivity: AdaptiveSensitivity::Low,
            },
        );
        assert!(!noise_matches(case.expected, normalized));
        let restored = case.restore.iter().fold(normalized, |state, change| {
            projected(state, *change).unwrap()
        });
        assert!(noise_matches(original, restored));
    }

    #[test]
    fn effect_check_keeps_an_originally_off_effect_off_after_restoring_mode() {
        let capability = capability(
            NoiseKind::AdaptiveNcAndAmbientLevel,
            &[0, 1, 6, 2, 1, 1, 6, 2],
        );
        let original = adaptive(NoiseSwitch::Off);
        let plan = plan(&capability, original);
        let case = plan
            .cases
            .iter()
            .find(|case| case.label == "Noise effect")
            .unwrap();
        assert_eq!(case.restore.last(), Some(&NoiseChange::Effect(false)));
        let restored = case.restore.iter().fold(case.expected, |state, change| {
            projected(state, *change).unwrap()
        });
        assert!(noise_matches(original, restored));
    }

    #[test]
    fn originally_off_explicit_mode_restores_mode_then_off() {
        let capability = capability(
            NoiseKind::AdaptiveNcAndAmbientLevel,
            &[0, 1, 6, 2, 1, 1, 6, 2],
        );
        let original = adaptive(NoiseSwitch::Off);
        let plan = plan(&capability, original);
        let case = plan
            .cases
            .iter()
            .find(|case| case.label == "Noise mode")
            .unwrap();
        assert_eq!(
            case.restore,
            [
                NoiseChange::Mode(NoiseMode::NoiseCancelling),
                NoiseChange::Effect(false)
            ]
        );
        assert!(matches!(
            case.expected,
            NoiseParameters::Control {
                effect: NoiseSwitch::On,
                settings: NoiseSettings::AdaptiveNcAndAmbientLevel {
                    mode: NoiseMode::AmbientSound,
                    ..
                },
                ..
            }
        ));
        let actual = case.restore.iter().fold(case.expected, |state, change| {
            projected(state, *change).unwrap()
        });
        assert!(noise_matches(original, actual));
        assert_eq!(plan.cases.len(), 6);
    }

    #[test]
    fn every_explicit_mode_layout_has_a_reversible_plan_without_model_checks() {
        let layouts = [
            NoiseSettings::AutoNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                nc: NcValue::AutoDual,
                ambient_mode: AmbientMode::Normal,
                level: 3,
            },
            NoiseSettings::DualSingleNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                nc: NcValue::Single,
                ambient_mode: AmbientMode::Normal,
                level: 3,
            },
            NoiseSettings::DualNcAndAmbientLevel {
                mode: NoiseMode::AmbientSound,
                ambient_mode: AmbientMode::Normal,
                level: 3,
            },
        ];
        for settings in layouts {
            let capability = capability(settings.kind(), &[0, 1, 6, 2, 1, 1, 6, 2]);
            let original = parameters(NoiseSwitch::On, settings);
            let plan = plan(&capability, original);
            assert!(plan.cases.iter().any(|case| case.label == "Noise mode"));
            for case in plan.cases {
                let restored = case.restore.iter().fold(case.expected, |state, change| {
                    projected(state, *change).unwrap()
                });
                assert!(noise_matches(restored, original));
            }
        }
    }

    #[test]
    fn adjacent_levels_include_non_step_terminal_maximum_and_never_clamp() {
        let range = AmbientLevelRange::new(1, 6, 2).unwrap();
        assert_eq!(adjacent_level(range, 5), Some(6));
        assert_eq!(adjacent_level(range, 6), Some(5));
        assert_eq!(adjacent_level(range, 2), None);
        assert_eq!(
            adjacent_level(AmbientLevelRange::new(3, 3, 1).unwrap(), 3),
            None
        );
    }

    #[test]
    fn voice_focus_requires_saved_level_valid_in_both_unique_modes() {
        let original = adaptive(NoiseSwitch::On);
        let disjoint = capability(
            NoiseKind::AdaptiveNcAndAmbientLevel,
            &[0, 1, 6, 2, 1, 2, 4, 1],
        );
        let plan = plan(&disjoint, original);
        assert!(!plan.cases.iter().any(|case| case.label == "Voice focus"));
        assert!(
            plan.skipped
                .iter()
                .any(|reason| reason.contains("both modes"))
        );
    }

    #[test]
    fn inactive_legacy_zero_does_not_become_an_active_ambient_level() {
        let capability = capability(NoiseKind::NcAndAmbientLevel, &[0, 1, 6, 2, 1, 1, 6, 2]);
        let original = parameters(
            NoiseSwitch::On,
            NoiseSettings::NcAndAmbientLevel {
                nc: NoiseSwitch::On,
                ambient_mode: AmbientMode::Normal,
                level: 0,
            },
        );
        let plan = plan(&capability, original);
        assert_eq!(plan.cases.len(), 1);
        assert_eq!(plan.cases[0].label, "Noise effect");
    }

    #[test]
    fn nc_only_and_ambient_switch_layouts_do_not_gain_nonexistent_fields() {
        let nc = plan(
            &capability(NoiseKind::NcOnly, &[]),
            parameters(
                NoiseSwitch::Off,
                NoiseSettings::NcOnly {
                    nc: NoiseSwitch::Off,
                },
            ),
        );
        assert_eq!(nc.cases.len(), 1);
        let switch = plan(
            &capability(NoiseKind::NcModesAndAmbientSwitch, &[0, 1]),
            parameters(
                NoiseSwitch::On,
                NoiseSettings::NcModesAndAmbientSwitch {
                    nc: NcValue::Dual,
                    ambient_mode: AmbientMode::Normal,
                    ambient: NoiseSwitch::Off,
                },
            ),
        );
        assert_eq!(
            switch
                .cases
                .iter()
                .map(|case| case.label)
                .collect::<Vec<_>>(),
            ["Noise effect", "Voice focus"]
        );
    }
}
