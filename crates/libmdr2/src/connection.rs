//! Device link requests and caller-provided LE Audio state reports.
#![doc = include_str!("../docs/link-events.md")]

use crate::{
    command::{CommandError, require_length, require_minimum, require_opcode, require_table},
    types::{BluetoothAddress, CommandTable, FunctionId},
};

/// The phase of a device-requested reconnection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconnectionPhase {
    /// The request is under confirmation (`00`).
    UnderConfirmation,
    /// The device requests execution (`01`).
    Execute,
}

/// An unsolicited device request that the connection owner must interpret.
///
/// Decoding does not disconnect, reconnect, pair a device or create another session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectionNotice {
    /// A table-1 COMMON reconnection request, inquiry `08`.
    Reconnect {
        /// Confirmation or execution phase.
        phase: ReconnectionPhase,
        /// Address carried by the request; the caller validates its relationship
        /// to the current device before opening or closing a transport.
        target: BluetoothAddress,
    },
    /// A fieldless request to release the GATT control connection, inquiry `0a`.
    DisconnectGatt,
}

impl ConnectionNotice {
    /// Returns the table-1 function identity associated with this notice.
    pub const fn function_id(self) -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_1,
            code: match self {
                Self::Reconnect { .. } => 0x1a,
                Self::DisconnectGatt => 0x1c,
            },
        }
    }
    /// Decodes an exact table-1 `COMMON_NTFY_PARAM` (`19`) payload.
    pub fn decode_notification(table: CommandTable, p: &[u8]) -> Result<Self, CommandError> {
        require_table(table, CommandTable::TABLE_1)?;
        require_minimum(p, 2)?;
        require_opcode(p[0], 0x19)?;
        match p[1] {
            8 => {
                require_length(p, 20)?;
                let phase = match p[2] {
                    0 => ReconnectionPhase::UnderConfirmation,
                    1 => ReconnectionPhase::Execute,
                    value => {
                        return Err(CommandError::InvalidValue {
                            field: "reconnection request phase",
                            value,
                        });
                    }
                };
                Ok(Self::Reconnect {
                    phase,
                    target: BluetoothAddress::parse_ascii(&p[3..])?,
                })
            }
            10 => {
                require_length(p, 2)?;
                Ok(Self::DisconnectGatt)
            }
            inquiry => Err(CommandError::UnsupportedOperation {
                operation: "connection notice",
                inquiry,
            }),
        }
    }
}

/// A host-observed LE Audio profile state, independent of MDR control transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostLeAudioState {
    /// The host audio profile is connecting (`00`).
    Connecting,
    /// The host audio profile is connected (`01`).
    Connected,
    /// The host audio profile is disconnecting (`02`).
    Disconnecting,
    /// The host audio profile is disconnected (`03`).
    Disconnected,
}

/// A caller-supplied audio-profile state report sent to the device.
///
/// The library cannot observe the host Bluetooth stack. Use this message only
/// when the operating-system adapter has reliable profile state for the address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeAudioStateReport {
    /// State observed by the host.
    pub state: HostLeAudioState,
    /// Accessory Bluetooth address associated with that state.
    pub target: BluetoothAddress,
}

impl LeAudioStateReport {
    /// Returns table-2 function `60`, advertising this report workflow.
    pub const fn function_id() -> FunctionId {
        FunctionId {
            table: CommandTable::TABLE_2,
            code: 0x60,
        }
    }
    /// Returns the command table required by the encoded payload.
    pub const fn table() -> CommandTable {
        CommandTable::TABLE_2
    }
    /// Encodes `68 00 state address[17]` on table 2, with no dedicated response.
    pub fn encode(self) -> [u8; 20] {
        let mut payload = [0; 20];
        payload[0] = 0x68;
        payload[2] = match self.state {
            HostLeAudioState::Connecting => 0,
            HostLeAudioState::Connected => 1,
            HostLeAudioState::Disconnecting => 2,
            HostLeAudioState::Disconnected => 3,
        };
        payload[3..].copy_from_slice(&self.target.to_ascii());
        payload
    }
}
