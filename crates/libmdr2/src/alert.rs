//! Device confirmation messages and explicit replies for local operations.
#![doc = include_str!("../docs/alert.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{CommandTable, FunctionId},
};

/// The user interaction requested by a device message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertActionType {
    /// Display-only confirmation; no protocol reply is required (`00`).
    ConfirmationOnly,
    /// The caller may accept or reject (`01`).
    AcceptOrReject,
    /// A positive confirmation requires a reply (`02`).
    ConfirmWithReply,
}

impl TryFrom<u8> for AlertActionType {
    type Error = CommandError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::ConfirmationOnly),
            1 => Ok(Self::AcceptOrReject),
            2 => Ok(Self::ConfirmWithReply),
            value => Err(CommandError::InvalidValue {
                field: "alert action type",
                value,
            }),
        }
    }
}

/// An explicit caller decision. There is no default or automatic acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertDecision {
    /// Decline the requested operation (`00`).
    Reject,
    /// Accept the requested operation (`01`).
    Accept,
}

/// A fixed-message identity. Unknown and service-related identities are readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedAlertMessage(u8);

impl FixedAlertMessage {
    /// A connection-priority change will disconnect the link (`00`).
    pub const CONNECTION_MODE_CHANGE: Self = Self(0x00);
    /// An assignment change will disconnect the link (`01`).
    pub const ASSIGNMENT_CHANGE: Self = Self(0x01);
    /// Changing multipoint will disable LDAC and disconnect (`06`).
    pub const MULTIPOINT_LDAC_DISABLED: Self = Self(0x06);
    /// Changing multipoint will disconnect (`07`).
    pub const MULTIPOINT_CHANGE: Self = Self(0x07);
    /// EQ and upscaling increase battery consumption (`08`).
    pub const EQ_AND_DSEE_CONSUMPTION: Self = Self(0x08);
    /// Playback control is assigned twice (`09`).
    pub const DUPLICATE_PLAYBACK_ASSIGNMENT: Self = Self(0x09);
    /// LDAC is unavailable in sound-quality priority (`2a`).
    pub const LDAC_UNAVAILABLE: Self = Self(0x2a);
    /// Two-device connection with LDAC requires confirmation (`70`).
    pub const MULTIPOINT_WITH_LDAC: Self = Self(0x70);
    /// Preserves a message identity without authorizing its operation.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }
    /// Returns the wire identity.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
    /// Whether the library implements replies for this local message.
    pub const fn supports_reply(self) -> bool {
        matches!(
            self.0,
            0x00 // Connection-priority change disconnects.
            | 0x01 // Local key-assignment change disconnects.
            | 0x06 // Multipoint change disables LDAC and disconnects.
            | 0x07 // Multipoint change disconnects.
            | 0x08 // EQ and DSEE increase battery consumption.
            | 0x09 // Duplicate playback-control assignment.
            | 0x0a // Disable touch-sensor panel.
            | 0x0b // Disable touch-sensor panel and reconnect.
            | 0x0c // Enable touch-sensor panel.
            | 0x0d // Enable touch-sensor panel and reconnect.
            | 0x1c // Select sound-quality connection priority.
            | 0x2a // LDAC unavailable in sound-quality priority.
            | 0x2b // Local sound functions increase battery consumption.
            | 0x30 // Changing to a Classic connection disconnects.
            | 0x53 // LDAC 990 warning.
            | 0x59 // Enable automatic volume optimization.
            | 0x5a // Enable automatic volume while safe-volume control is on.
            | 0x5b // Enable automatic volume while safe-volume limitation is on.
            | 0x5c // EQ changes are exclusive with BGM mode.
            | 0x5e // Assign left playback control.
            | 0x5f // Assign right playback control.
            | 0x65 // A key assignment enables repeated taps.
            | 0x66 // A key assignment disables repeated taps.
            | 0x70 // Enable two-device connection with LDAC.
            | 0x71 // Select quality priority while two devices are connected.
            | 0x72 // Two devices connected in the background with LDAC.
            | 0x73 // Select LDAC 990 while two devices are connected.
            | 0x74 // Change LE Audio to Classic sound-quality priority.
            | 0x75 // Change LE Audio to Classic connection-quality priority.
            | 0x76 // Change to Classic sound-quality priority.
            | 0x77 // Change to Classic connection-quality priority.
            | 0x86 // Enable a sound effect while stamina is on.
            | 0x87 // Enable a sound effect while stamina is on, without lighting.
            | 0x90 // Disable BGM mode.
        )
    }
}

/// A flexible-message identity, separate from fixed-message identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FlexibleAlertMessage(u8);

impl FlexibleAlertMessage {
    /// Preserves a flexible-message identity.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }
    /// Returns the original byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
    /// Whether this message describes a supported local setting or connection flow.
    pub const fn supports_reply(self) -> bool {
        matches!(
            self.0,
            0x00 // Battery consumption from three simultaneous settings.
            | 0x01 // Functions exclusive with multipoint.
            | 0x07 // Battery-consumption confirmation requiring a reply.
            | 0x08 // Functions exclusive with BGM mode.
            | 0x09 // Functions exclusive with EQ.
            | 0x0b // Battery consumption from activating all settings.
            | 0x0d // Connection change with listed LE Audio restrictions.
            | 0x11 // Change to LE Audio with listed restrictions.
        )
    }
}

/// A feature mentioned by a flexible alert, retaining unknown identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AlertItem(u8);

impl AlertItem {
    /// Preserves the reported feature byte.
    pub const fn from_raw(value: u8) -> Self {
        Self(value)
    }
    /// Returns the reported feature byte.
    pub const fn as_raw(self) -> u8 {
        self.0
    }
    /// Whether this item is supported in a local confirmation reply.
    pub const fn supports_reply(self) -> bool {
        matches!(
            self.0,
            0x00 // Equalizer.
            | 0x01 // DSEE.
            | 0x02 // Speak-to-Chat.
            | 0x03 // Automatic volume control.
            | 0x06 // LDAC.
            | 0x07 // Sound-quality connection priority.
            | 0x0b // Multipoint.
            | 0x0d // BGM mode.
            | 0x10 // LE Audio.
            | 0x14 // Noise cancelling.
            | 0x18 // Connection mode.
        )
    }
}

/// A device message. Its fields can be inspected but cannot be changed in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertMessage {
    inquiry: u8,
    contents: AlertContents,
}

/// The independently tagged bodies of `ALERT_NTFY_PARAM` (`99`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AlertContents {
    /// A fixed or foreground-triggered message.
    Fixed {
        /// Fixed-message identity, including unsupported messages.
        message: FixedAlertMessage,
        /// Requested user interaction.
        action: AlertActionType,
    },
    /// A message with a list of affected features.
    Flexible {
        /// Flexible-message identity.
        message: FlexibleAlertMessage,
        /// Affected features in wire order, retaining duplicates and unknowns.
        items: Vec<AlertItem>,
        /// Requested user interaction.
        action: AlertActionType,
    },
    /// A left/right selection message, retained for diagnostics only.
    ///
    /// The known message identities configure voice assistants and have no
    /// reply API in this library.
    LeftRight {
        /// Original message identity.
        message: u8,
        /// Default side: `false` for left, `true` for right.
        default_right: bool,
    },
}

impl AlertMessage {
    /// Decodes an exact table-1 `99` message.
    pub fn decode_notification(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(p, 2)?;
        require_opcode(p[0], 0x99)?;
        let contents = match p[1] {
            0 | 4 => {
                require_length(p, 4)?;
                AlertContents::Fixed {
                    message: FixedAlertMessage(p[2]),
                    action: p[3].try_into()?,
                }
            }
            2 => {
                require_length(p, 4)?;
                let default_right = match p[3] {
                    0 => false,
                    1 => true,
                    value => {
                        return Err(CommandError::InvalidValue {
                            field: "alert default side",
                            value,
                        });
                    }
                };
                AlertContents::LeftRight {
                    message: p[2],
                    default_right,
                }
            }
            6 => {
                require_minimum(p, 5)?;
                let count = usize::from(p[3]);
                require_length(p, 5 + count)?;
                AlertContents::Flexible {
                    message: FlexibleAlertMessage(p[2]),
                    items: p[4..4 + count].iter().copied().map(AlertItem).collect(),
                    action: p[4 + count].try_into()?,
                }
            }
            inquiry => {
                return Err(CommandError::UnsupportedOperation {
                    operation: "alert message",
                    inquiry,
                });
            }
        };
        Ok(Self {
            inquiry: p[1],
            contents,
        })
    }

    /// Returns the inquiry, retaining fixed (`00`) versus foreground (`04`).
    pub const fn inquiry(&self) -> u8 {
        self.inquiry
    }
    /// Returns the reported body without reinterpreting unknown messages.
    pub const fn contents(&self) -> &AlertContents {
        &self.contents
    }
    /// Encodes an explicit answer to a supported local message as `98`.
    ///
    /// Display-only messages, unsupported message/item identities, assistant
    /// selection and negative answers to positive-only confirmations are rejected.
    /// No generic transaction ID exists: the caller must associate this snapshot
    /// with its current connection and outstanding operation before replying.
    pub fn encode_reply(&self, decision: AlertDecision) -> Result<Vec<u8>, CommandError> {
        let (message, action, supported) = match &self.contents {
            AlertContents::Fixed { message, action } => {
                (message.as_raw(), *action, message.supports_reply())
            }
            AlertContents::Flexible {
                message,
                items,
                action,
            } => (
                message.as_raw(),
                *action,
                message.supports_reply() && items.iter().all(|item| item.supports_reply()),
            ),
            AlertContents::LeftRight { .. } => {
                return Err(CommandError::UnsupportedOperation {
                    operation: "assistant alert reply",
                    inquiry: self.inquiry,
                });
            }
        };
        if !supported || action == AlertActionType::ConfirmationOnly {
            return Err(CommandError::UnsupportedOperation {
                operation: "alert reply",
                inquiry: self.inquiry,
            });
        }
        if action == AlertActionType::ConfirmWithReply && decision == AlertDecision::Reject {
            return Err(CommandError::InvalidParameter {
                field: "positive-only alert decision",
            });
        }
        Ok(vec![
            0x98,
            self.inquiry,
            message,
            u8::from(decision == AlertDecision::Accept),
        ])
    }
}

/// LE Audio alert-notification state (`93/95 05`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeAudioAlertStatus {
    /// Whether the device reports the alert flag as on (`00`).
    pub enabled: bool,
}

impl LeAudioAlertStatus {
    /// Returns the function advertising LE Audio alert notifications.
    pub const fn function_id() -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: 0x94,
        }
    }
    /// Decodes an exact `ALERT_RET_STATUS` response for inquiry `05`.
    pub fn decode_response(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0x93)
    }
    /// Decodes an exact `ALERT_NTFY_STATUS` notification for inquiry `05`.
    pub fn decode_notification(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        Self::decode(table, p, 0x95)
    }
    fn decode(table: CommandTable, p: &[u8], opcode: u8) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_length(p, 3)?;
        require_opcode(p[0], opcode)?;
        if p[1] != 5 {
            return Err(CommandError::UnexpectedInquiry {
                expected: 5,
                actual: p[1],
            });
        }
        Ok(Self {
            enabled: match p[2] {
                0 => true,
                1 => false,
                value => {
                    return Err(CommandError::InvalidValue {
                        field: "LE Audio alert state",
                        value,
                    });
                }
            },
        })
    }
}

/// Explicit LE Audio notice confirmations, encoded on table 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeAudioConfirmation {
    /// Acknowledge the current notice.
    Confirm,
    /// Acknowledge and request that the device stop showing the notice.
    ConfirmAndDoNotShowAgain,
}

impl LeAudioConfirmation {
    /// Encodes `94 05 confirmation`. A link ACK confirms delivery only.
    pub const fn encode(self) -> [u8; 3] {
        [
            0x94,
            5,
            match self {
                Self::Confirm => 0,
                Self::ConfirmAndDoNotShowAgain => 1,
            },
        ]
    }
}

/// Commands to enable fixed-message delivery or report application foreground state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertControl {
    /// Enable fixed alerts after discovering table-1 function `90`.
    EnableFixedMessages,
    /// Notify the device that the application entered the foreground.
    ApplicationForeground,
}

impl AlertControl {
    /// Encodes the fixed table-1 SET_STATUS payload; no RET is defined.
    pub const fn encode(self) -> [u8; 3] {
        [
            0x94,
            match self {
                Self::EnableFixedMessages => 0,
                Self::ApplicationForeground => 4,
            },
            0,
        ]
    }
}
