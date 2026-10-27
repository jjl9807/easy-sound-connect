//! Opt-in hardware checks. Every mutation is followed by an awaited restoration.

#[path = "noise_exercise.rs"]
mod noise_exercise;

use std::{error::Error, time::Duration};

use bluetooth::CancellationToken;
use ezsc_core::{
    ConnectionState, DeviceHandle, DeviceManager, OperationResult, SessionConfig,
    equalizer::{EqualizerKind, EqualizerParameters, EqualizerSetting},
    noise::NoiseParameters,
};

type CheckResult = Result<(), Box<dyn Error>>;

fn confirmed(result: OperationResult) -> CheckResult {
    if result == OperationResult::Confirmed {
        Ok(())
    } else if result == OperationResult::AwaitingConfirmation {
        Err("The headset is waiting for an explicit user decision. This CLI check does not answer device prompts. End this control session and repeat the change in the GUI for interactive confirmation.".into())
    } else {
        Err(format!("Expected confirmed readback, received {result:?}").into())
    }
}

fn noise_matches(a: NoiseParameters, b: NoiseParameters) -> bool {
    match (a, b) {
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
        _ => false,
    }
}

fn equalizer_setting(parameters: &EqualizerParameters, curve: bool) -> EqualizerSetting {
    match (parameters.preset(), parameters.ult_mode()) {
        (Some(preset), Some(ult_mode)) => EqualizerSetting::PresetWithUlt {
            preset,
            ult_mode,
            levels: curve.then(|| parameters.levels().to_vec()),
        },
        (Some(preset), None) => EqualizerSetting::Preset {
            preset,
            levels: curve.then(|| parameters.levels().to_vec()),
        },
        (None, _) => EqualizerSetting::Custom {
            levels: parameters.levels().to_vec(),
        },
    }
}

fn eq_parameters(
    handle: &DeviceHandle,
    kind: EqualizerKind,
) -> Result<EqualizerParameters, Box<dyn Error>> {
    handle
        .snapshot()
        .equalizers
        .into_iter()
        .find(|state| state.kind == kind)
        .and_then(|state| state.parameters)
        .ok_or_else(|| "Missing EQ parameters".into())
}

/// Reopen only after the failed generation has released its native transport.
async fn recover(manager: &DeviceManager, handle: &mut DeviceHandle) -> CheckResult {
    let snapshot = handle.snapshot();
    if snapshot.connection != ConnectionState::Ready {
        eprintln!("Reconnecting to restore saved settings...");
        if let Err(error) = manager.disconnect(&snapshot.device).await {
            // Historical transport errors can survive successful native cleanup.
            eprintln!("Previous connection cleanup reported: {error}");
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            // The manager remains responsible for gating reconnect on cleanup.
            match manager
                .connect(
                    snapshot.device.clone(),
                    snapshot.transport,
                    SessionConfig::default(),
                )
                .await
            {
                Ok(reconnected) => {
                    *handle = reconnected;
                    break;
                }
                Err(ezsc_core::Error::AlreadyConnected)
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
        handle.initialize().await?;
    }
    Ok(())
}

fn finish(label: &str, tested: CheckResult, restored: CheckResult) -> CheckResult {
    match restored {
        Ok(()) => {
            println!("{label}: original settings restored and verified");
            tested?;
            println!("{label}: checks completed");
            Ok(())
        }
        Err(error) => Err(format!(
            "{label}: ORIGINAL SETTINGS NOT CONFIRMED RESTORED: {error}; check result: {tested:?}"
        )
        .into()),
    }
}

pub async fn run(
    manager: &DeviceManager,
    handle: &mut DeviceHandle,
    stop: &CancellationToken,
) -> CheckResult {
    println!(
        "Starting explicit reversible control checks. Saved values are printed before changes."
    );
    let advertised = handle.snapshot();
    noise_exercise::run(manager, handle, stop, advertised.noise).await?;

    for entry in advertised.equalizers {
        if stop.is_cancelled() {
            return Ok(());
        }
        handle.refresh_equalizer(entry.kind).await?;
        let state = handle
            .snapshot()
            .equalizers
            .into_iter()
            .find(|state| state.kind == entry.kind)
            .ok_or("Missing EQ state")?;
        if state
            .status
            .as_ref()
            .is_none_or(|status| !status.enabled || !status.errors.is_empty())
        {
            println!("Equalizer {:?}: unavailable; skipped", entry.kind);
            continue;
        }
        let capability = state.capability.ok_or("Missing EQ capability")?;
        let original = state.parameters.ok_or("Missing EQ parameters")?;
        let restore_original = equalizer_setting(
            &original,
            original.preset().is_none_or(|id| id.is_custom_slot()),
        );
        // Reject an unrestorable baseline before selecting another preset.
        original.validate_capability(&capability)?;
        restore_original.encode_set(&capability)?;
        println!("Saved active EQ {:?}: {original:?}", entry.kind);
        if stop.is_cancelled() {
            return Ok(());
        }
        let tested = async {
            if let Some(alternative) = capability
                .presets()
                .iter()
                .find(|preset| Some(preset.id) != original.preset() && !preset.id.is_custom_slot())
            {
                confirmed(
                    handle
                        .set_equalizer_preset(entry.kind, alternative.id)
                        .await?,
                )?;
                println!(
                    "EQ preset 0x{:02x}: change confirmed by readback",
                    alternative.id.as_raw()
                );
            }
            if stop.is_cancelled() {
                return Ok(());
            }
            let editable = original
                .preset()
                .filter(|id| id.is_custom_slot())
                .or_else(|| {
                    capability
                        .presets()
                        .iter()
                        .find(|preset| preset.id.is_custom_slot())
                        .map(|preset| preset.id)
                });
            if entry.kind != EqualizerKind::PresetNonCustomizable
                && (editable.is_some() || entry.kind == EqualizerKind::Custom)
            {
                if let Some(preset) = editable {
                    confirmed(handle.set_equalizer_preset(entry.kind, preset).await?)?;
                }
                // Capture this slot's own curve AFTER selecting it, before any edit.
                let saved_curve = eq_parameters(handle, entry.kind)?;
                saved_curve.validate_capability(&capability)?;
                if saved_curve.preset() != editable {
                    return Err("Active EQ slot changed before the curve backup".into());
                }
                equalizer_setting(&saved_curve, true).encode_set(&capability)?;
                println!("Saved editable EQ slot: {saved_curve:?}");
                if !stop.is_cancelled()
                    && capability.level_steps() > 1
                    && !saved_curve.levels().is_empty()
                {
                    let changed = async {
                        let level = saved_curve.levels()[0];
                        let value = if level + 1 < capability.level_steps() {
                            level + 1
                        } else {
                            level - 1
                        };
                        confirmed(
                            handle
                                .set_equalizer_band(entry.kind, saved_curve.preset(), 0, value)
                                .await?,
                        )?;
                        Ok(())
                    }
                    .await;
                    let restored = async {
                        recover(manager, handle).await?;
                        confirmed(
                            handle
                                .set_equalizer(entry.kind, equalizer_setting(&saved_curve, true))
                                .await?,
                        )?;
                        if eq_parameters(handle, entry.kind)? != saved_curve {
                            return Err("Custom curve differs from its saved original".into());
                        }
                        Ok(())
                    }
                    .await;
                    finish("EQ custom curve", changed, restored)?;
                }
            } else {
                println!("EQ custom curve: no editable curve advertised; skipped");
            }
            Ok(())
        }
        .await;
        let restored = async {
            recover(manager, handle).await?;
            confirmed(handle.set_equalizer(entry.kind, restore_original).await?)?;
            if eq_parameters(handle, entry.kind)? != original {
                return Err("Active EQ differs from the saved original".into());
            }
            Ok(())
        }
        .await;
        finish("Active EQ", tested, restored)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_failure_survives_a_successful_change() {
        let error = finish("EQ", Ok(()), Err("disconnected".into()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("NOT CONFIRMED RESTORED"));
        assert!(error.contains("disconnected"));
    }

    #[test]
    fn adjustment_is_not_reported_as_success() {
        assert!(confirmed(OperationResult::Adjusted).is_err());
    }

    #[test]
    fn pending_confirmation_explains_that_cli_does_not_answer_prompts() {
        let error = confirmed(OperationResult::AwaitingConfirmation)
            .unwrap_err()
            .to_string();
        assert!(error.contains("explicit user decision"));
        assert!(error.contains("does not answer device prompts"));
    }
}
