//! Device sessions, capability-selected operations, and shared live state.
//!
//! A [`DeviceManager`] owns independent per-device actors. Call
//! [`DeviceHandle::initialize`] after connecting, subscribe to snapshots, and
//! explicitly close the manager before stopping its Tokio runtime.
//! Bluetooth writes, MDR link ACKs, and operation readback are distinct stages.

#![forbid(unsafe_code)]

mod controls;
mod manager;
mod runtime;
mod state;

pub use controls::NoiseChange;
pub use libmdr2::{equalizer, noise};
pub use manager::DeviceManager;
pub use runtime::{DeviceHandle, OperationResult, SessionConfig};
pub use state::*;

/// An operation or connection failure. Presentation belongs to the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Bluetooth(bluetooth::Error),
    InvalidConfiguration(&'static str),
    Protocol(String),
    NotReady,
    Unsupported(&'static str),
    Unavailable(&'static str),
    QueueFull,
    Cancelled,
    Disconnected,
    Timeout(TimeoutStage),
    AlreadyConnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutStage {
    Discovery,
    Connect,
    Queue,
    Operation,
    Write,
    Ack,
    Response,
    Close,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bluetooth(error) => write!(f, "Bluetooth: {error}"),
            Self::InvalidConfiguration(message) => write!(f, "Invalid configuration: {message}"),
            Self::Protocol(message) => write!(f, "MDR protocol: {message}"),
            Self::NotReady => f.write_str("Device state has not been initialized"),
            Self::Unsupported(message) => write!(f, "Unsupported: {message}"),
            Self::Unavailable(message) => write!(f, "Unavailable: {message}"),
            Self::QueueFull => f.write_str("Device operation queue is full"),
            Self::Cancelled => f.write_str("Operation cancelled"),
            Self::Disconnected => f.write_str("Control connection closed"),
            Self::Timeout(stage) => write!(f, "Timeout during {stage:?}"),
            Self::AlreadyConnected => f.write_str("Device already has a control session"),
        }
    }
}
impl std::error::Error for Error {}
impl From<bluetooth::Error> for Error {
    fn from(value: bluetooth::Error) -> Self {
        Self::Bluetooth(value)
    }
}
impl From<libmdr2::command::CommandError> for Error {
    fn from(value: libmdr2::command::CommandError) -> Self {
        Self::Protocol(value.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;
