use crate::Error;
use crate::controls::{supports_audio, supports_system};
use bluetooth::{DeviceId, TransportKind};
use libmdr2::{
    audio::{AudioCapability, AudioKind, AudioParameters, AudioStatus},
    battery::{BatteryKind, BatteryReading, BatteryReadings, BatteryStatus},
    command::{CapabilityInfo, ProtocolInfo, SupportedFunction},
    common::{AudioCodec, CommonKind},
    device::{DeviceText, SeriesAndColor},
    equalizer::{
        EqualizerBandInfo, EqualizerCapability, EqualizerKind, EqualizerParameters, EqualizerStatus,
    },
    noise::{NoiseCapability, NoiseKind, NoiseParameters, NoiseStatus},
    system::{SpeakToChatSettings, SystemCapability, SystemKind, SystemParameters, SystemStatus},
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

/// A supported DSEE or connection-priority layout and its current parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioState {
    pub kind: AudioKind,
    pub capability: Option<AudioCapability>,
    pub status: Option<AudioStatus>,
    pub parameters: Option<AudioParameters>,
    pub freshness: Freshness,
}

/// The current codec reported by the device, independently of audio preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioCodecState {
    /// `None` has not been read; `Some(AudioCodec::UNSETTLED)` is a device report.
    pub codec: Option<AudioCodec>,
    pub freshness: Freshness,
}

/// Speak-to-Chat ordinary and extended parameters have independent freshness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemState {
    pub kind: SystemKind,
    pub capability: Option<SystemCapability>,
    pub status: Option<SystemStatus>,
    pub parameters: Option<SystemParameters>,
    pub freshness: Freshness,
    pub extended: Option<SpeakToChatSettings>,
    pub extended_freshness: Freshness,
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
    pub audio_parameters_notifications: u64,
    pub system_parameters_notifications: u64,
    pub system_extended_notifications: u64,
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
    /// Raw series/color metadata, used only for product presentation.
    pub appearance: Option<SeriesAndColor>,
    pub batteries: Vec<BatteryEndpoint>,
    pub noise: Vec<NoiseState>,
    pub equalizers: Vec<EqualizerState>,
    pub audio: Vec<AudioState>,
    /// Present only when the device advertises the table-1 codec report.
    pub audio_codec: Option<AudioCodecState>,
    pub system: Vec<SystemState>,
    pub pending_prompt: Option<crate::DevicePrompt>,
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
            appearance: None,
            batteries: Vec::new(),
            noise: Vec::new(),
            equalizers: Vec::new(),
            audio: Vec::new(),
            audio_codec: None,
            system: Vec::new(),
            pending_prompt: None,
            last_error: None,
            message_counters: MessageCounters::default(),
            unhandled_messages: 0,
        }
    }

    pub(crate) fn add_function(&mut self, advertised: SupportedFunction) {
        let battery = BatteryKind::from_function(advertised.id);
        let noise = NoiseKind::from_function(advertised.id);
        let eq = EqualizerKind::from_function(advertised.id);
        let audio = AudioKind::from_function(advertised.id).filter(|kind| supports_audio(*kind));
        let system = SystemKind::from_function(advertised.id).filter(|kind| supports_system(*kind));
        let codec = CommonKind::from_function(advertised.id) == Some(CommonKind::AudioCodec);
        self.functions.push(FunctionSupport {
            readable: battery.is_some()
                || noise.is_some()
                || eq.is_some()
                || audio.is_some()
                || system.is_some()
                || codec,
            writable: noise.is_some_and(|kind| kind != NoiseKind::ButtonCycle)
                || eq.is_some()
                || audio.is_some()
                || system.is_some(),
            advertised,
        });
        if codec && self.audio_codec.is_none() {
            self.audio_codec = Some(AudioCodecState {
                codec: None,
                freshness: Freshness::Unknown,
            });
        }
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
        if let Some(kind) = audio
            && !self.audio.iter().any(|entry| entry.kind == kind)
        {
            self.audio.push(AudioState {
                kind,
                capability: None,
                status: None,
                parameters: None,
                freshness: Freshness::Unknown,
            });
        }
        if let Some(kind) = system
            && !self.system.iter().any(|entry| entry.kind == kind)
        {
            self.system.push(SystemState {
                kind,
                capability: None,
                status: None,
                parameters: None,
                freshness: Freshness::Unknown,
                extended: None,
                extended_freshness: Freshness::Unknown,
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
        if let Some(state) = &mut self.audio_codec
            && state.codec.is_some()
        {
            state.freshness = Freshness::Stale;
        }
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
        for entry in &mut self.audio {
            if entry.parameters.is_some() {
                entry.freshness = Freshness::Stale;
            }
        }
        for entry in &mut self.system {
            if entry.parameters.is_some() {
                entry.freshness = Freshness::Stale;
            }
            if entry.extended.is_some() {
                entry.extended_freshness = Freshness::Stale;
            }
        }
    }
}
