//! Native connection probe with explicitly requested, reversible control checks.

mod exercise;

use std::{error::Error, time::Duration};

use bluetooth::{DeviceId, TransportKind};
use ezsc_core::{DeviceManager, DeviceSnapshot, SessionConfig};

const USAGE: &str = "Usage:\n  ezsc-probe --list\n  ezsc-probe --device <platform-id> --transport <gatt|rfcomm> [--controls] [--band-info] [--exercise-controls] [--watch <seconds>]\n\nPair the headset in system settings first. --list only enumerates paired devices.\nThe default probe reads identity and battery; --controls also reads noise and EQ.\n--band-info additionally queries optional EQ band labels.\n--exercise-controls explicitly CHANGES supported noise effect/mode/level/voice/\nadaptation settings, EQ preset and one custom curve band, verifies readback and\nrestores saved settings after each check.\n--watch receives updates (default: 10 seconds). Ctrl+C during an exercise waits\nfor the current check and restoration before releasing the connection.";

enum Command {
    List,
    Read {
        device: DeviceId,
        transport: TransportKind,
        watch: Duration,
        controls: bool,
        band_info: bool,
        exercise: bool,
    },
    Help,
}

fn parse() -> Result<Command, String> {
    let mut args = std::env::args().skip(1);
    let mut device = None;
    let mut transport = None;
    let mut watch = Duration::from_secs(10);
    let mut list = false;
    let mut controls = false;
    let mut band_info = false;
    let mut exercise = false;
    let mut watch_set = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--list" => list = true,
            "--controls" => controls = true,
            "--band-info" => {
                band_info = true;
                controls = true;
            }
            "--exercise-controls" => {
                exercise = true;
                controls = true;
            }
            "--device" => {
                device = Some(DeviceId(args.next().ok_or("--device needs a platform id")?))
            }
            "--transport" => {
                transport = Some(match args.next().as_deref() {
                    Some("gatt") => TransportKind::Gatt,
                    Some("rfcomm") => TransportKind::Rfcomm,
                    _ => return Err("--transport must be gatt or rfcomm".into()),
                })
            }
            "--watch" => {
                let seconds: u64 = args
                    .next()
                    .ok_or("--watch needs seconds")?
                    .parse()
                    .map_err(|_| "--watch needs an integer")?;
                if seconds > 86_400 {
                    return Err("--watch must be at most 86400 seconds".into());
                }
                watch = Duration::from_secs(seconds);
                watch_set = true;
            }
            _ => return Err(format!("Unknown argument: {arg}")),
        }
    }
    if list && (device.is_some() || transport.is_some() || watch_set || controls) {
        return Err("--list cannot be combined with connection options".into());
    }
    if list {
        return Ok(Command::List);
    }
    match (device, transport) {
        (Some(device), Some(transport)) => Ok(Command::Read {
            device,
            transport,
            watch,
            controls,
            band_info,
            exercise,
        }),
        (None, None) if !watch_set && !controls => Ok(Command::Help),
        _ => Err("Both --device and --transport are required".into()),
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let command = parse().map_err(|error| format!("{error}\n\n{USAGE}"))?;
    if matches!(command, Command::Help) {
        println!("{USAGE}");
        return Ok(());
    }
    let backend =
        tokio::time::timeout(Duration::from_secs(15), bluetooth::system_backend()).await??;
    let manager = DeviceManager::new(backend);
    let exercising = matches!(command, Command::Read { exercise: true, .. });
    let stop = bluetooth::CancellationToken::new();
    let operation = probe(&manager, command, stop.clone());
    tokio::pin!(operation);
    let result = tokio::select! {
        result = &mut operation => result,
        signal = tokio::signal::ctrl_c() => {
            stop.cancel();
            if exercising {
                eprintln!("Stopping after the current check and restoration...");
                operation.await
            } else {
                signal.map_err(Into::into)
            }
        },
    };
    let closed = manager.shutdown().await;
    result?;
    closed?;
    Ok(())
}

async fn probe(
    manager: &DeviceManager,
    command: Command,
    stop: bluetooth::CancellationToken,
) -> Result<(), Box<dyn Error>> {
    match command {
        Command::List => {
            let devices = manager.discover().await?;
            if devices.is_empty() {
                println!("No paired devices advertising MDR v2 services were found.");
            }
            for device in devices {
                println!(
                    "{}\n  id: {}\n  transports: {:?}",
                    device.name.as_deref().unwrap_or("Unnamed device"),
                    device.id.0,
                    device.transports
                );
            }
        }
        Command::Read {
            device,
            transport,
            watch,
            controls,
            band_info,
            exercise,
        } => {
            let mut handle = manager
                .connect(device, transport, SessionConfig::default())
                .await?;
            let mut updates = handle.subscribe();
            handle.initialize().await?;
            if controls {
                let advertised = handle.snapshot();
                for state in advertised.noise {
                    handle.refresh_noise(state.kind).await?;
                }
                for state in advertised.equalizers {
                    handle.refresh_equalizer(state.kind).await?;
                    if band_info {
                        handle.refresh_equalizer_band_info(state.kind).await?;
                    }
                }
                let snapshot = handle.snapshot();
                println!("Noise controls: {:#?}", snapshot.noise);
                println!("Equalizers: {:#?}", snapshot.equalizers);
            }
            if exercise {
                exercise::run(manager, &mut handle, &stop).await?;
                // A restoration may have opened a fresh connection after a fault.
                updates = handle.subscribe();
            }
            print_snapshot(&updates.borrow_and_update(), controls);
            let deadline = tokio::time::sleep(watch);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = &mut deadline => break,
                    changed = updates.changed() => {
                        if changed.is_err() { break; }
                        let snapshot = updates.borrow_and_update().clone();
                        print_snapshot(&snapshot, controls);
                        if matches!(snapshot.connection, ezsc_core::ConnectionState::Disconnected | ezsc_core::ConnectionState::Failed) { break; }
                    }
                }
            }
            handle.close().await?;
        }
        Command::Help => {}
    }
    Ok(())
}

fn print_snapshot(snapshot: &DeviceSnapshot, controls: bool) {
    let mut lines = app::snapshot_lines(snapshot);
    if controls {
        lines.extend(app::control_lines(snapshot));
    }
    println!("\n{}", lines.join("\n"));
}
