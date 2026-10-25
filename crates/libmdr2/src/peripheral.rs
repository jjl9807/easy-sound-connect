//! Device-reported audio sources, local source preferences and targeted operations.

#![doc = include_str!("../docs/peripheral.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{BluetoothAddress, CommandTable, FunctionId},
};

/// A table-2 PERIPHERAL wire layout selected by advertised function identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PeripheralKind {
    /// Device records without a Bluetooth class (`00`, function `30`).
    Devices,
    /// Source switching and its keep-source preference (`01`, function `31`).
    SourceSwitch,
    /// Records with a three-byte Bluetooth class (`02`, functions `32` or `33`).
    DevicesWithClass,
    /// Music handover preference (`03`, function `34`).
    MusicHandover,
}

impl PeripheralKind {
    /// Returns the inquiry following each PERIPHERAL opcode.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::Devices => 0,
            Self::SourceSwitch => 1,
            Self::DevicesWithClass => 2,
            Self::MusicHandover => 3,
        }
    }

    /// Returns the alternative table-2 identities advertising this wire layout.
    ///
    /// Inquiry `02` is shared by Classic and Classic/LE variants; its payload
    /// does not identify which of those function codes was advertised.
    pub const fn function_ids(self) -> &'static [FunctionId] {
        match self {
            Self::Devices => &[FunctionId {
                table: CommandTable::TABLE_2,
                code: 0x30,
            }],
            Self::SourceSwitch => &[FunctionId {
                table: CommandTable::TABLE_2,
                code: 0x31,
            }],
            Self::DevicesWithClass => &[
                FunctionId {
                    table: CommandTable::TABLE_2,
                    code: 0x32,
                },
                FunctionId {
                    table: CommandTable::TABLE_2,
                    code: 0x33,
                },
            ],
            Self::MusicHandover => &[FunctionId {
                table: CommandTable::TABLE_2,
                code: 0x34,
            }],
        }
    }

    /// Selects a known wire layout from a table-aware function identity.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        if function.table != CommandTable::TABLE_2 {
            return None;
        }
        match function.code {
            0x30 => Some(Self::Devices),
            0x31 => Some(Self::SourceSwitch),
            0x32 | 0x33 => Some(Self::DevicesWithClass),
            0x34 => Some(Self::MusicHandover),
            _ => None,
        }
    }

    /// Whether the layout has a dynamic capability query (`30`).
    pub const fn has_capability_query(self) -> bool {
        matches!(self, Self::Devices | Self::DevicesWithClass)
    }

    /// Whether the layout has a status query (`32`).
    pub const fn has_status_query(self) -> bool {
        self.has_capability_query()
    }
}

impl TryFrom<u8> for PeripheralKind {
    type Error = CommandError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Devices),
            1 => Ok(Self::SourceSwitch),
            2 => Ok(Self::DevicesWithClass),
            3 => Ok(Self::MusicHandover),
            value => Err(CommandError::InvalidValue {
                field: "peripheral inquiry",
                value,
            }),
        }
    }
}

/// Device-advertised capacity and transfer metadata; these are not current counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceLimits {
    /// Maximum paired-device count as advertised.
    pub max_paired: u8,
    /// Maximum connected-device count as advertised.
    pub max_connected: u8,
    /// Whether the device reports file transfer possible during multiple connections.
    /// This field does not enable any file-transfer API.
    pub file_transfer_possible: bool,
}

/// A dynamic device-list capability or fixed source-preference descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeripheralCapability {
    kind: PeripheralKind,
    limits: Option<DeviceLimits>,
}

impl PeripheralCapability {
    /// Constructs a fixed source-switch or handover descriptor after discovery.
    pub fn fixed(kind: PeripheralKind) -> Result<Self, CommandError> {
        if kind.has_capability_query() {
            return Err(CommandError::UnsupportedOperation {
                operation: "fixed peripheral capability",
                inquiry: kind.inquiry(),
            });
        }
        Ok(Self { kind, limits: None })
    }

    /// Decodes an exact table-2 `PERIPHERAL_RET_CAPABILITY` (`31`) payload.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x31)?;
        require_devices(kind, "peripheral capability response")?;
        require_length(payload, 5)?;
        Ok(Self {
            kind,
            limits: Some(DeviceLimits {
                max_paired: payload[2],
                max_connected: payload[3],
                file_transfer_possible: positive_zero(
                    payload[4],
                    "peripheral transfer availability",
                )?,
            }),
        })
    }

    /// Returns the descriptor's wire layout.
    pub const fn kind(&self) -> PeripheralKind {
        self.kind
    }

    /// Returns capacity metadata for device-list layouts.
    pub const fn limits(&self) -> Option<DeviceLimits> {
        self.limits
    }
}

/// The device's current Bluetooth discovery mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralMode {
    /// Normal mode (`00`).
    Normal,
    /// Pairing/inquiry-scan mode (`01`). This report does not start pairing.
    Pairing,
}

/// Availability and discovery mode for a device-list layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeripheralStatus {
    kind: PeripheralKind,
    /// The current device discovery mode.
    pub mode: PeripheralMode,
    /// Whether device-list controls are currently available.
    pub enabled: bool,
}

impl PeripheralStatus {
    /// Returns the status layout.
    pub const fn kind(self) -> PeripheralKind {
        self.kind
    }

    /// Decodes a `33` device-list status response.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x33)
    }

    /// Decodes a `35` status notification independently of read responses.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x35)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_devices(kind, "peripheral status")?;
        require_length(payload, 4)?;
        let mode = match payload[2] {
            0 => PeripheralMode::Normal,
            1 => PeripheralMode::Pairing,
            value => {
                return Err(CommandError::InvalidValue {
                    field: "peripheral mode",
                    value,
                });
            }
        };
        Ok(Self {
            kind,
            mode,
            enabled: positive_zero(payload[3], "peripheral availability")?,
        })
    }
}

/// One device-reported paired audio source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDevice {
    address: BluetoothAddress,
    connection_slot: u8,
    bluetooth_class: Option<u32>,
    name: Vec<u8>,
}

impl SourceDevice {
    /// Returns the parsed source address.
    pub const fn address(&self) -> BluetoothAddress {
        self.address
    }

    /// Returns the raw connection-slot value. Zero means not connected.
    /// Positive values are retained without assuming a maximum or list index.
    pub const fn connection_slot(&self) -> u8 {
        self.connection_slot
    }

    /// Whether this record reports a current connection.
    pub const fn is_connected(&self) -> bool {
        self.connection_slot != 0
    }

    /// Returns the 24-bit class of device for inquiry `02`.
    pub const fn bluetooth_class(&self) -> Option<u32> {
        self.bluetooth_class
    }

    /// Returns the original friendly-name bytes.
    pub fn name_bytes(&self) -> &[u8] {
        &self.name
    }

    /// Validates the friendly name as UTF-8 without replacing invalid bytes.
    pub fn name_utf8(&self) -> Result<&str, std::str::Utf8Error> {
        std::str::from_utf8(&self.name)
    }
}

/// A complete device list preserving device order and raw playback-right metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceList {
    kind: PeripheralKind,
    devices: Vec<SourceDevice>,
    playback_right: u8,
}

impl DeviceList {
    /// Returns the device-record layout.
    pub const fn kind(&self) -> PeripheralKind {
        self.kind
    }

    /// Returns the device records in wire order, including duplicate addresses.
    pub fn devices(&self) -> &[SourceDevice] {
        &self.devices
    }

    /// Returns the final playback-right byte without treating it as a vector index.
    pub const fn playback_right_raw(&self) -> u8 {
        self.playback_right
    }

    fn decode(kind: PeripheralKind, payload: &[u8]) -> Result<Self, CommandError> {
        require_minimum(payload, 4)?;
        let count = usize::from(payload[2]);
        let with_class = kind == PeripheralKind::DevicesWithClass;
        let record_header = if with_class { 22 } else { 19 };
        let mut offset = 3;
        let mut devices = Vec::with_capacity(count);
        for _ in 0..count {
            require_minimum(payload, offset + record_header)?;
            let address = BluetoothAddress::parse_ascii(&payload[offset..offset + 17])?;
            let connection_slot = payload[offset + 17];
            let bluetooth_class = with_class.then(|| {
                u32::from_be_bytes([
                    0,
                    payload[offset + 18],
                    payload[offset + 19],
                    payload[offset + 20],
                ])
            });
            let length = payload[offset + record_header - 1];
            if !(1..=128).contains(&length) {
                return Err(CommandError::InvalidValue {
                    field: "source device name length",
                    value: length,
                });
            }
            offset += record_header;
            let end = offset + usize::from(length);
            require_minimum(payload, end)?;
            devices.push(SourceDevice {
                address,
                connection_slot,
                bluetooth_class,
                name: payload[offset..end].to_vec(),
            });
            offset = end;
        }
        require_length(payload, offset + 1)?;
        Ok(Self {
            kind,
            devices,
            playback_right: payload[offset],
        })
    }

    fn unique_device(&self, address: BluetoothAddress) -> Result<&SourceDevice, CommandError> {
        let mut matching = self
            .devices
            .iter()
            .filter(|device| device.address == address);
        match (matching.next(), matching.next()) {
            (Some(device), None) => Ok(device),
            _ => Err(CommandError::InvalidParameter {
                field: "unique device-list target address",
            }),
        }
    }
}

/// A source-switch result identity, preserving unknown results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceSwitchResult(u8);

impl SourceSwitchResult {
    /// Successful switch (`00`).
    pub const SUCCESS: Self = Self(0);
    /// Unspecified failure (`01`).
    pub const FAILED: Self = Self(1);
    /// Switch failed during a call (`02`).
    pub const CALLING: Self = Self(2);
    /// The A2DP source is not connected (`03`).
    pub const A2DP_NOT_CONNECTED: Self = Self(3);
    /// Another voice-assistant priority prevents switching (`04`).
    pub const VOICE_ASSISTANT_PRIORITY: Self = Self(4);

    /// Preserves an unknown or known result code.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original result byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Classifies known results without treating unknown values as success.
    pub const fn outcome(self) -> OperationOutcome {
        match self.0 {
            0 => OperationOutcome::Succeeded,
            1..=4 => OperationOutcome::Failed,
            _ => OperationOutcome::Unknown,
        }
    }
}

/// Device-list or local source-preference parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeripheralParameters {
    /// Paired-device records in inquiry `00` or `02`.
    Devices(DeviceList),
    /// The keep-source setting; only notifications include a result byte.
    SourceKeep {
        /// The wire OnOff setting (`00` means on).
        enabled: bool,
        /// The result in a `39 01` notification; absent from a `37 01` response.
        result: Option<SourceSwitchResult>,
    },
    /// Music handover's wire OnOff value; no UI inversion is applied.
    MusicHandover(bool),
}

impl PeripheralParameters {
    /// Returns the parameter layout.
    pub const fn kind(&self) -> PeripheralKind {
        match self {
            Self::Devices(list) => list.kind,
            Self::SourceKeep { .. } => PeripheralKind::SourceSwitch,
            Self::MusicHandover(_) => PeripheralKind::MusicHandover,
        }
    }

    /// Decodes `PERIPHERAL_RET_PARAM` (`37`).
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x37)
    }

    /// Decodes `PERIPHERAL_NTFY_PARAM` (`39`), including its extra source result.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x39)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        match kind {
            PeripheralKind::Devices | PeripheralKind::DevicesWithClass => {
                DeviceList::decode(kind, payload).map(Self::Devices)
            }
            PeripheralKind::SourceSwitch => {
                let notification = opcode == 0x39;
                require_length(payload, if notification { 4 } else { 3 })?;
                Ok(Self::SourceKeep {
                    enabled: positive_zero(payload[2], "keep-source setting")?,
                    result: notification.then(|| SourceSwitchResult::from_raw(payload[3])),
                })
            }
            PeripheralKind::MusicHandover => {
                require_length(payload, 3)?;
                Ok(Self::MusicHandover(positive_zero(
                    payload[2],
                    "music hand-over setting",
                )?))
            }
        }
    }
}

/// A local source preference, distinct from a targeted connection operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralSetting {
    /// Set the source-keep wire preference.
    SourceKeep(bool),
    /// Set the music handover wire preference.
    MusicHandover(bool),
}

impl PeripheralSetting {
    /// Encodes table-2 `38` against the matching fixed descriptor.
    /// Verify advertised support and reconcile state separately; a link ACK is not
    /// confirmation that the preference took effect.
    pub fn encode_set(self, capability: &PeripheralCapability) -> Result<Vec<u8>, CommandError> {
        let (kind, enabled) = match self {
            Self::SourceKeep(enabled) => (PeripheralKind::SourceSwitch, enabled),
            Self::MusicHandover(enabled) => (PeripheralKind::MusicHandover, enabled),
        };
        require_kind(capability.kind, kind)?;
        Ok(vec![0x38, kind.inquiry(), u8::from(!enabled)])
    }
}

/// Progress classification of an operation result; unknown codes are not success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationOutcome {
    /// The device reports success for the reported action.
    Succeeded,
    /// The device reports an error.
    Failed,
    /// The operation is still in progress.
    InProgress,
    /// The device reports a busy condition.
    Busy,
    /// The result is unknown or inconsistent with its action field.
    Unknown,
}

/// A connection-result action identity, retained even when no setter is exposed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConnectivityAction(u8);

impl ConnectivityAction {
    /// Disconnect (`00`).
    pub const DISCONNECT: Self = Self(0);
    /// Connect (`01`).
    pub const CONNECT: Self = Self(1);
    /// Unpair (`02`), readable only; this module exposes no unpair command.
    pub const UNPAIR: Self = Self(2);

    /// Preserves an action identity.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original action byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
}

/// An unmodified connection-operation result code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConnectivityResult(u8);

impl ConnectivityResult {
    /// Preserves a known or unknown result byte.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original result byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    fn outcome(self, action: ConnectivityAction) -> OperationOutcome {
        if action.0 > 2 || self.0 >> 4 != action.0 {
            return OperationOutcome::Unknown;
        }
        match self.0 & 0x0f {
            0 => OperationOutcome::Succeeded,
            1 => OperationOutcome::Failed,
            2 => OperationOutcome::InProgress,
            3 => OperationOutcome::Busy,
            _ => OperationOutcome::Unknown,
        }
    }
}

/// Fields in a targeted operation-result notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralResultData {
    /// A result for a device connection action.
    Connectivity {
        /// Reported action identity.
        action: ConnectivityAction,
        /// Original result code, interpreted together with the action.
        result: ConnectivityResult,
        /// The action's target address.
        address: BluetoothAddress,
    },
    /// A result for an audio-source switch.
    SourceSwitch {
        /// Source-switch result identity.
        result: SourceSwitchResult,
        /// The target source address.
        address: BluetoothAddress,
    },
}

/// A table-2 `PERIPHERAL_NTFY_EXTENDED_PARAM` (`3d`) operation report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeripheralResultReport {
    kind: PeripheralKind,
    /// Action-specific report fields.
    pub data: PeripheralResultData,
}

impl PeripheralResultReport {
    /// Returns the result's inquiry layout.
    pub const fn kind(self) -> PeripheralKind {
        self.kind
    }

    /// Decodes a `3d` result notification. It is not a query response or link ACK.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x3d)?;
        let data = match kind {
            PeripheralKind::Devices | PeripheralKind::DevicesWithClass => {
                require_length(payload, 21)?;
                PeripheralResultData::Connectivity {
                    action: ConnectivityAction::from_raw(payload[2]),
                    result: ConnectivityResult::from_raw(payload[3]),
                    address: BluetoothAddress::parse_ascii(&payload[4..])?,
                }
            }
            PeripheralKind::SourceSwitch => {
                require_length(payload, 20)?;
                PeripheralResultData::SourceSwitch {
                    result: SourceSwitchResult::from_raw(payload[2]),
                    address: BluetoothAddress::parse_ascii(&payload[3..])?,
                }
            }
            PeripheralKind::MusicHandover => {
                return Err(CommandError::UnsupportedOperation {
                    operation: "peripheral extended result",
                    inquiry: kind.inquiry(),
                });
            }
        };
        Ok(Self { kind, data })
    }

    /// Classifies the report without marking in-progress, busy or unknown as success.
    /// An action/result-family mismatch is unknown rather than successful.
    pub fn outcome(self) -> OperationOutcome {
        match self.data {
            PeripheralResultData::Connectivity { action, result, .. } => result.outcome(action),
            PeripheralResultData::SourceSwitch { result, .. } => result.outcome(),
        }
    }
}

/// A local operation targeting one already reported audio source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralOperation {
    /// Connect a paired source present in the supplied device list.
    Connect(BluetoothAddress),
    /// Disconnect a source present in the supplied device list.
    Disconnect(BluetoothAddress),
    /// Switch playback to a source that the supplied list reports connected.
    SwitchSource(BluetoothAddress),
}

/// A validated table-2 operation payload and its address/action correlation key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedPeripheralOperation {
    /// Full command payload, without framing; submit it on command table 2.
    pub payload: Vec<u8>,
    /// Matches operation-result reports without treating progress as success.
    pub expected_result: ExpectedPeripheralResult,
}

/// Correlates a result's inquiry, action and address; not a unique transaction ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedPeripheralResult {
    kind: PeripheralKind,
    action: Option<ConnectivityAction>,
    address: BluetoothAddress,
}

impl ExpectedPeripheralResult {
    /// Returns the target result layout.
    pub const fn kind(self) -> PeripheralKind {
        self.kind
    }

    /// Returns the target source address.
    pub const fn address(self) -> BluetoothAddress {
        self.address
    }

    /// Matches identity fields, independently of the reported outcome.
    ///
    /// Always inspect [`PeripheralResultReport::outcome`] after a match.
    /// Identical repeated operations have identical keys: serialize them and
    /// establish fresh state after ambiguous timeouts instead of attributing a
    /// delayed report to a new operation.
    pub fn matches(self, report: &PeripheralResultReport) -> bool {
        if self.kind != report.kind {
            return false;
        }
        match report.data {
            PeripheralResultData::Connectivity {
                action, address, ..
            } => self.action == Some(action) && self.address == address,
            PeripheralResultData::SourceSwitch { address, .. } => {
                self.action.is_none() && self.address == address
            }
        }
    }
}

impl PeripheralOperation {
    /// Encodes a table-2 `3c` operation against a capability and current device list.
    ///
    /// Targets must occur exactly once. Connect/disconnect require the list's
    /// matching capability layout. Source switching requires a source-switch
    /// descriptor and a connected target in either device-list layout.
    /// The caller owns freshness, current availability and connection identity.
    /// No operation here creates a pairing, unpairs a device or performs I/O.
    pub fn encode(
        self,
        capability: &PeripheralCapability,
        list: &DeviceList,
    ) -> Result<EncodedPeripheralOperation, CommandError> {
        let (address, action) = match self {
            Self::Connect(address) => (address, Some(ConnectivityAction::CONNECT)),
            Self::Disconnect(address) => (address, Some(ConnectivityAction::DISCONNECT)),
            Self::SwitchSource(address) => (address, None),
        };
        let target = list.unique_device(address)?;
        let kind = if action.is_some() {
            require_kind(capability.kind, list.kind)?;
            require_devices(capability.kind, "source connection operation")?;
            list.kind
        } else {
            require_kind(capability.kind, PeripheralKind::SourceSwitch)?;
            if !target.is_connected() {
                return Err(CommandError::InvalidParameter {
                    field: "connected source-switch target",
                });
            }
            PeripheralKind::SourceSwitch
        };
        let mut payload = vec![0x3c, kind.inquiry()];
        if let Some(action) = action {
            payload.push(action.as_raw());
        }
        payload.extend_from_slice(&address.to_ascii());
        Ok(EncodedPeripheralOperation {
            payload,
            expected_result: ExpectedPeripheralResult {
                kind,
                action,
                address,
            },
        })
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<PeripheralKind, CommandError> {
    require_table(table, CommandTable::TABLE_2)?;
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    payload[1].try_into()
}

fn require_devices(kind: PeripheralKind, operation: &'static str) -> Result<(), CommandError> {
    if kind.has_capability_query() {
        Ok(())
    } else {
        Err(CommandError::UnsupportedOperation {
            operation,
            inquiry: kind.inquiry(),
        })
    }
}

fn require_kind(actual: PeripheralKind, expected: PeripheralKind) -> Result<(), CommandError> {
    if actual == expected {
        Ok(())
    } else {
        Err(CommandError::CapabilityMismatch {
            expected: expected.inquiry(),
            actual: actual.inquiry(),
        })
    }
}

fn positive_zero(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}
