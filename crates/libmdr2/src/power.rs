//! Device-local power settings on command tables 1 and 2.

#![doc = include_str!("../docs/power.md")]

use crate::{
    battery::Percentage,
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// A power-setting layout selected by its advertised function identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PowerKind {
    /// Table 1, inquiry `04`, function `24`: automatic power-off options.
    AutoPowerOff,
    /// Table 1, inquiry `05`, function `25`: options including removal from ears.
    AutoPowerOffWithWearingDetection,
    /// Table 1, inquiry `06`, function `26`: power-saving switch.
    PowerSaving,
    /// Table 1, inquiry `0c`, function `2c`: caring-charge switch.
    CaringCharge,
    /// Table 2, inquiry `00`, function `20`: automatic standby switch.
    AutoStandby,
    /// Table 2, inquiry `01`, function `22`: caring-charge switch and threshold.
    CaringChargeWithThreshold,
}

impl PowerKind {
    /// Returns the command table required by this layout.
    pub const fn table(self) -> CommandTable {
        match self {
            Self::AutoStandby | Self::CaringChargeWithThreshold => CommandTable::TABLE_2,
            _ => CommandTable::TABLE_1,
        }
    }

    /// Returns the inquiry byte following each POWER opcode.
    pub const fn inquiry(self) -> u8 {
        match self {
            Self::AutoPowerOff => 0x04,
            Self::AutoPowerOffWithWearingDetection => 0x05,
            Self::PowerSaving => 0x06,
            Self::CaringCharge => 0x0c,
            Self::AutoStandby => 0x00,
            Self::CaringChargeWithThreshold => 0x01,
        }
    }

    /// Returns the function identity that advertises this setting.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: self.table(),
            code: match self {
                Self::AutoPowerOff => 0x24,
                Self::AutoPowerOffWithWearingDetection => 0x25,
                Self::PowerSaving => 0x26,
                Self::CaringCharge => 0x2c,
                Self::AutoStandby => 0x20,
                Self::CaringChargeWithThreshold => 0x22,
            },
        }
    }

    /// Selects a known setting from a table-aware function identity.
    pub fn from_function(function: FunctionId) -> Option<Self> {
        match (function.table, function.code) {
            (CommandTable::TABLE_1, 0x24) => Some(Self::AutoPowerOff),
            (CommandTable::TABLE_1, 0x25) => Some(Self::AutoPowerOffWithWearingDetection),
            (CommandTable::TABLE_1, 0x26) => Some(Self::PowerSaving),
            (CommandTable::TABLE_1, 0x2c) => Some(Self::CaringCharge),
            (CommandTable::TABLE_2, 0x20) => Some(Self::AutoStandby),
            (CommandTable::TABLE_2, 0x22) => Some(Self::CaringChargeWithThreshold),
            _ => None,
        }
    }

    /// Selects a known power-setting layout by table and inquiry.
    ///
    /// Battery layouts and user power-off are separate APIs. In particular,
    /// table-1 inquiry `00` does not select table-2 automatic standby.
    pub fn from_wire(table: CommandTable, inquiry: u8) -> Result<Self, CommandError> {
        let kind = match (table, inquiry) {
            (CommandTable::TABLE_1, 0x04) => Self::AutoPowerOff,
            (CommandTable::TABLE_1, 0x05) => Self::AutoPowerOffWithWearingDetection,
            (CommandTable::TABLE_1, 0x06) => Self::PowerSaving,
            (CommandTable::TABLE_1, 0x0c) => Self::CaringCharge,
            (CommandTable::TABLE_2, 0x00) => Self::AutoStandby,
            (CommandTable::TABLE_2, 0x01) => Self::CaringChargeWithThreshold,
            (CommandTable::TABLE_1 | CommandTable::TABLE_2, value) => {
                return Err(CommandError::InvalidValue {
                    field: "power inquiry",
                    value,
                });
            }
            _ => return Err(CommandError::UnsupportedTable(table.as_raw())),
        };
        Ok(kind)
    }

    /// Whether this layout defines a dynamic capability query.
    pub const fn has_capability_query(self) -> bool {
        !matches!(self, Self::PowerSaving | Self::CaringCharge)
    }

    const fn has_timer_options(self) -> bool {
        matches!(
            self,
            Self::AutoPowerOff | Self::AutoPowerOffWithWearingDetection
        )
    }
}

/// An automatic power-off option identity, preserving unknown wire values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PowerOffOption(u8);

impl PowerOffOption {
    /// Power off after five minutes (`00`).
    pub const FIVE_MINUTES: Self = Self(0x00);
    /// Power off after thirty minutes (`01`).
    pub const THIRTY_MINUTES: Self = Self(0x01);
    /// Power off after sixty minutes (`02`).
    pub const SIXTY_MINUTES: Self = Self(0x02);
    /// Power off after one hundred eighty minutes (`03`).
    pub const ONE_HUNDRED_EIGHTY_MINUTES: Self = Self(0x03);
    /// Power off after fifteen minutes (`04`).
    pub const FIFTEEN_MINUTES: Self = Self(0x04);
    /// Power off when removed from the ears (`10`), available in inquiry `05`.
    pub const WHEN_REMOVED: Self = Self(0x10);
    /// Disable automatic power-off (`11`).
    pub const DISABLED: Self = Self(0x11);

    /// Preserves an option byte without asserting that it can be written.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }

    /// Returns the original option byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }

    /// Returns the delay in minutes for a known timer option.
    pub const fn minutes(self) -> Option<u16> {
        match self.0 {
            0x00 => Some(5),
            0x01 => Some(30),
            0x02 => Some(60),
            0x03 => Some(180),
            0x04 => Some(15),
            _ => None,
        }
    }

    fn is_known_for(self, kind: PowerKind) -> bool {
        kind.has_timer_options()
            && (self.minutes().is_some()
                || self == Self::DISABLED
                || (kind == PowerKind::AutoPowerOffWithWearingDetection
                    && self == Self::WHEN_REMOVED))
    }
}

/// The device's advertised power-setting descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerCapability {
    kind: PowerKind,
    options: Vec<PowerOffOption>,
    standby_candidate: Option<u8>,
    charge_threshold: Option<Percentage>,
}

impl PowerCapability {
    /// Creates the fieldless descriptor for power saving or basic caring charge.
    ///
    /// Call only after discovering the corresponding function. Other layouts
    /// require a capability response and return `UnsupportedOperation` here.
    pub fn fixed(kind: PowerKind) -> Result<Self, CommandError> {
        if kind.has_capability_query() {
            return Err(CommandError::UnsupportedOperation {
                operation: "fixed power capability",
                inquiry: kind.inquiry(),
            });
        }
        Ok(Self::empty(kind))
    }

    fn empty(kind: PowerKind) -> Self {
        Self {
            kind,
            options: Vec::new(),
            standby_candidate: None,
            charge_threshold: None,
        }
    }

    /// Decodes an exact `POWER_RET_CAPABILITY` (`21`) response.
    ///
    /// Option counts are unsigned. Order, duplicates and unknown option IDs are
    /// retained. Caring-charge thresholds must be in `1..=100` percent.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        let kind = header(table, payload, 0x21)?;
        if !kind.has_capability_query() {
            return Err(CommandError::UnsupportedOperation {
                operation: "power capability response",
                inquiry: kind.inquiry(),
            });
        }
        require_minimum(payload, 3)?;
        let mut capability = Self::empty(kind);
        if kind.has_timer_options() {
            require_length(payload, 3 + usize::from(payload[2]))?;
            capability.options = payload[3..]
                .iter()
                .copied()
                .map(PowerOffOption::from_raw)
                .collect();
        } else {
            require_length(payload, 3)?;
            match kind {
                PowerKind::AutoStandby => capability.standby_candidate = Some(payload[2]),
                PowerKind::CaringChargeWithThreshold => {
                    if payload[2] == 0 {
                        return Err(CommandError::InvalidValue {
                            field: "caring-charge threshold",
                            value: payload[2],
                        });
                    }
                    capability.charge_threshold = Some(payload[2].try_into()?);
                }
                _ => unreachable!(),
            }
        }
        Ok(capability)
    }

    /// Returns the descriptor's layout and command-table identity.
    pub const fn kind(&self) -> PowerKind {
        self.kind
    }

    /// Returns automatic power-off options in device order.
    pub fn options(&self) -> &[PowerOffOption] {
        &self.options
    }

    /// Returns the raw automatic-standby candidate, if this descriptor has one.
    ///
    /// Candidate `00` means fifteen minutes; other identities have no known
    /// duration and do not authorize setting writes.
    pub const fn standby_candidate(&self) -> Option<u8> {
        self.standby_candidate
    }

    /// Returns the advertised automatic-standby delay, when understood.
    pub const fn standby_minutes(&self) -> Option<u16> {
        match self.standby_candidate {
            Some(0) => Some(15),
            _ => None,
        }
    }

    /// Returns the caring-charge threshold in percent, if advertised.
    ///
    /// This threshold is read-only capability data, not a writable percentage.
    pub const fn charge_threshold(&self) -> Option<Percentage> {
        self.charge_threshold
    }
}

/// Availability of a power setting, independent of its on/off parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerStatus {
    kind: PowerKind,
    /// Whether the control is currently operable (`00` available, `01` unavailable).
    pub enabled: bool,
    /// Caring-charge notice flag: `00` necessary, `01` unnecessary.
    ///
    /// Present only for table-2 caring charge. It is neither a percentage nor
    /// the charging state of the battery.
    pub battery_notice_required: Option<bool>,
}

impl PowerStatus {
    /// Returns the setting to which this availability applies.
    pub const fn kind(self) -> PowerKind {
        self.kind
    }

    /// Decodes `POWER_RET_STATUS` (`23`) for a supported setting.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x23)
    }

    /// Decodes `POWER_NTFY_STATUS` (`25`) for a supported setting.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x25)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        let has_notice = kind == PowerKind::CaringChargeWithThreshold;
        require_length(payload, if has_notice { 4 } else { 3 })?;
        Ok(Self {
            kind,
            enabled: positive_zero(payload[2], "power availability")?,
            battery_notice_required: if has_notice {
                Some(positive_zero(payload[3], "battery notice")?)
            } else {
                None
            },
        })
    }
}

/// The editable fields of a power-setting parameter message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PowerSettings {
    /// The active option and separately stored timer selection.
    AutoPowerOff {
        /// The active option: timer, removed from ears, or disabled.
        active: PowerOffOption,
        /// The stored time-selection field, retained when disabling the timer.
        selected_time: PowerOffOption,
    },
    /// An on/off setting; the wire uses `00` for on and `01` for off.
    Switch {
        /// Whether the setting is on.
        enabled: bool,
    },
}

/// A complete power-setting parameter message, without framing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerParameters {
    kind: PowerKind,
    /// Editable setting fields. Encoding rejects a shape incompatible with `kind`.
    pub settings: PowerSettings,
}

impl PowerParameters {
    /// Constructs fields for a selected layout, checking their structural shape.
    ///
    /// Automatic power-off option identities are checked against capabilities
    /// when encoding a write, not during construction or response decoding.
    pub fn new(kind: PowerKind, settings: PowerSettings) -> Result<Self, CommandError> {
        require_shape(kind, settings)?;
        Ok(Self { kind, settings })
    }

    /// Returns this parameter message's layout and table identity.
    pub const fn kind(self) -> PowerKind {
        self.kind
    }

    /// Decodes `POWER_RET_PARAM` (`27`), preserving both auto-off option fields.
    pub fn decode_response(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x27)
    }

    /// Decodes `POWER_NTFY_PARAM` (`29`) separately from query responses.
    pub fn decode_notification(table: CommandTable, payload: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, payload, 0x29)
    }

    fn decode(table: CommandTable, payload: &[u8], opcode: u8) -> Result<Self, CommandError> {
        let kind = header(table, payload, opcode)?;
        require_length(payload, if kind.has_timer_options() { 4 } else { 3 })?;
        let settings = if kind.has_timer_options() {
            PowerSettings::AutoPowerOff {
                active: PowerOffOption::from_raw(payload[2]),
                selected_time: PowerOffOption::from_raw(payload[3]),
            }
        } else {
            PowerSettings::Switch {
                enabled: positive_zero(payload[2], "power setting")?,
            }
        };
        Ok(Self { kind, settings })
    }

    /// Encodes `POWER_SET_PARAM` (`28`) against the matching capability.
    ///
    /// The active auto-off option must be understood and advertised. When timer
    /// candidates exist, the selected time must be an advertised timer. Devices
    /// with only removal/disabled options still carry the second field: retain
    /// its known value without requiring a timer candidate that is not offered.
    /// Unknown option identities remain readable but are not writable.
    ///
    /// Submit the payload on `self.kind().table()`. Check current availability
    /// separately; link ACKs do not confirm that settings took effect. These SET messages have
    /// no dedicated response, so reconcile parameter reports or a serialized read.
    pub fn encode_set(&self, capability: &PowerCapability) -> Result<Vec<u8>, CommandError> {
        require_table(capability.kind.table(), self.kind.table())?;
        if self.kind != capability.kind {
            return Err(CommandError::CapabilityMismatch {
                expected: self.kind.inquiry(),
                actual: capability.kind.inquiry(),
            });
        }
        require_shape(self.kind, self.settings)?;
        let mut payload = vec![0x28, self.kind.inquiry()];
        match self.settings {
            PowerSettings::AutoPowerOff {
                active,
                selected_time,
            } => {
                if !active.is_known_for(self.kind) || !capability.options.contains(&active) {
                    return Err(CommandError::InvalidParameter {
                        field: "advertised automatic power-off option",
                    });
                }
                let has_timers = capability
                    .options
                    .iter()
                    .any(|option| option.minutes().is_some());
                if !selected_time.is_known_for(self.kind)
                    || (has_timers
                        && (selected_time.minutes().is_none()
                            || !capability.options.contains(&selected_time)))
                {
                    return Err(CommandError::InvalidParameter {
                        field: "automatic power-off selected time",
                    });
                }
                payload.extend_from_slice(&[active.as_raw(), selected_time.as_raw()]);
            }
            PowerSettings::Switch { enabled } => {
                if self.kind == PowerKind::AutoStandby && capability.standby_minutes().is_none() {
                    return Err(CommandError::InvalidCapability {
                        field: "automatic-standby candidate",
                    });
                }
                payload.push(u8::from(!enabled));
            }
        }
        Ok(payload)
    }
}

/// The user-requested power-off command on table 1.
///
/// Check that the device advertises function `23` before sending this command.
/// Encoding does not verify device support or prompt the user for confirmation.
/// This type only encodes a payload; it neither sends it nor powers off a device.
/// The command has no dedicated RET and can end the control connection. A
/// disconnect alone does not establish that power-off succeeded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerOff;

impl PowerOff {
    /// Returns the function identity required for user power-off.
    pub const fn function_id() -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: 0x23,
        }
    }

    /// Encodes the fixed user power-off payload `24 03 01` for table 1.
    pub const fn encode() -> [u8; 3] {
        [0x24, 0x03, 0x01]
    }
}

fn header(table: CommandTable, payload: &[u8], opcode: u8) -> Result<PowerKind, CommandError> {
    require_minimum(payload, 2)?;
    require_opcode(payload[0], opcode)?;
    PowerKind::from_wire(table, payload[1])
}

fn positive_zero(value: u8, field: &'static str) -> Result<bool, CommandError> {
    match value {
        0 => Ok(true),
        1 => Ok(false),
        value => Err(CommandError::InvalidValue { field, value }),
    }
}

fn require_shape(kind: PowerKind, settings: PowerSettings) -> Result<(), CommandError> {
    if kind.has_timer_options() == matches!(settings, PowerSettings::AutoPowerOff { .. }) {
        Ok(())
    } else {
        Err(CommandError::InvalidParameter {
            field: "power setting layout",
        })
    }
}
