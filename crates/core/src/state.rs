use crate::Error;
use bluetooth::{DeviceId, TransportKind};
use libmdr2::{
    battery::{BatteryKind, BatteryReading, BatteryReadings, BatteryStatus},
    command::{CapabilityInfo, ProtocolInfo, SupportedFunction},
    device::DeviceText,
    equalizer::{
        EqualizerBandInfo, EqualizerCapability, EqualizerKind, EqualizerParameters, EqualizerStatus,
    },
    noise::{NoiseCapability, NoiseKind, NoiseParameters, NoiseStatus},
    types::DisplayLanguage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Negotiating,
    Synchronizing,
    Ready,
    Closing,
    Disconnected,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Unknown,
    Current,
    Stale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryComponent {
    Headset,
    Left,
    Right,
    Case,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatteryEndpoint {
    pub component: BatteryComponent,
    pub reading: Option<BatteryReading>,
    pub freshness: Freshness,
    pub source: BatteryKind,
}

/// Advertising, implementation coverage, live availability and freshness are
/// separate facts. Availability is carried by each feature's current status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSupport {
    pub advertised: SupportedFunction,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoiseState {
    pub kind: NoiseKind,
    pub capability: Option<NoiseCapability>,
    pub status: Option<NoiseStatus>,
    pub parameters: Option<NoiseParameters>,
    pub freshness: Freshness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EqualizerState {
    pub kind: EqualizerKind,
    pub capability: Option<EqualizerCapability>,
    pub status: Option<EqualizerStatus>,
    pub parameters: Option<EqualizerParameters>,
    /// Optional metadata, associated only after its kind/count match capability.
    pub band_info: Option<EqualizerBandInfo>,
    pub freshness: Freshness,
}

/// Accepted messages in one connection generation, without retaining payloads.
/// Counters saturate at `u64::MAX`; protocol-filtered duplicate frames do not
/// count twice. Notifications are independent of matched read responses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageCounters {
    pub responses: u64,
    pub notifications: u64,
    pub noise_parameters_notifications: u64,
    pub equalizer_parameters_notifications: u64,
}

/// Shared state for all views, belonging to one connection generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub device: DeviceId,
    pub generation: u64,
    pub transport: TransportKind,
    pub connection: ConnectionState,
    pub language: DisplayLanguage,
    pub protocol: Option<ProtocolInfo>,
    pub capability_info: Option<CapabilityInfo>,
    pub functions: Vec<FunctionSupport>,
    pub model_name: Option<DeviceText>,
    pub firmware_version: Option<DeviceText>,
    pub batteries: Vec<BatteryEndpoint>,
    pub noise: Vec<NoiseState>,
    pub equalizers: Vec<EqualizerState>,
    pub last_error: Option<Error>,
    pub message_counters: MessageCounters,
    /// No unbounded raw-message log is retained.
    pub unhandled_messages: u64,
}

impl DeviceSnapshot {
    pub(crate) fn new(
        info: &bluetooth::ConnectionInfo,
        generation: u64,
        language: DisplayLanguage,
    ) -> Self {
        Self {
            device: info.device.clone(),
            generation,
            transport: info.transport,
            connection: ConnectionState::Connected,
            language,
            protocol: None,
            capability_info: None,
            functions: Vec::new(),
            model_name: None,
            firmware_version: None,
            batteries: Vec::new(),
            noise: Vec::new(),
            equalizers: Vec::new(),
            last_error: None,
            message_counters: MessageCounters::default(),
            unhandled_messages: 0,
        }
    }

    pub(crate) fn add_function(&mut self, advertised: SupportedFunction) {
        let battery = BatteryKind::from_function(advertised.id);
        let noise = NoiseKind::from_function(advertised.id);
        let eq = EqualizerKind::from_function(advertised.id);
        self.functions.push(FunctionSupport {
            readable: battery.is_some() || noise.is_some() || eq.is_some(),
            writable: noise.is_some_and(|kind| kind != NoiseKind::ButtonCycle) || eq.is_some(),
            advertised,
        });
        if let Some(kind) = battery {
            let components: &[BatteryComponent] = match kind {
                BatteryKind::Single | BatteryKind::SingleWithThreshold => {
                    &[BatteryComponent::Headset]
                }
                BatteryKind::LeftRight | BatteryKind::LeftRightWithThreshold => {
                    &[BatteryComponent::Left, BatteryComponent::Right]
                }
                BatteryKind::Cradle | BatteryKind::CradleWithThreshold => &[BatteryComponent::Case],
                _ => &[],
            };
            for component in components {
                if !self
                    .batteries
                    .iter()
                    .any(|entry| entry.component == *component)
                {
                    self.batteries.push(BatteryEndpoint {
                        component: *component,
                        reading: None,
                        freshness: Freshness::Unknown,
                        source: kind,
                    });
                }
            }
        }
        if let Some(kind) = noise
            && !self.noise.iter().any(|entry| entry.kind == kind)
        {
            self.noise.push(NoiseState {
                kind,
                capability: None,
                status: None,
                parameters: None,
                freshness: Freshness::Unknown,
            });
        }
        if let Some(kind) = eq
            && !self.equalizers.iter().any(|entry| entry.kind == kind)
        {
            self.equalizers.push(EqualizerState {
                kind,
                capability: None,
                status: None,
                parameters: None,
                band_info: None,
                freshness: Freshness::Unknown,
            });
        }
    }

    pub(crate) fn battery(&mut self, status: BatteryStatus) {
        if !self
            .functions
            .iter()
            .any(|entry| entry.advertised.id == status.kind.function_id())
        {
            return;
        }
        let mut update = |component, reading| {
            if let Some(entry) = self
                .batteries
                .iter_mut()
                .find(|entry| entry.component == component)
            {
                entry.reading = Some(reading);
                entry.source = status.kind;
                entry.freshness = Freshness::Current;
            }
        };
        match status.batteries {
            BatteryReadings::Single(reading) => update(BatteryComponent::Headset, reading),
            BatteryReadings::LeftRight { left, right } => {
                update(BatteryComponent::Left, left);
                update(BatteryComponent::Right, right);
            }
            BatteryReadings::Cradle(reading) => update(BatteryComponent::Case, reading),
            _ => {}
        }
    }

    pub(crate) fn stale(&mut self) {
        for entry in &mut self.batteries {
            if entry.reading.is_some() {
                entry.freshness = Freshness::Stale;
            }
        }
        for entry in &mut self.noise {
            if entry.parameters.is_some() {
                entry.freshness = Freshness::Stale;
            }
        }
        for entry in &mut self.equalizers {
            if entry.parameters.is_some() {
                entry.freshness = Freshness::Stale;
            }
        }
    }
}
