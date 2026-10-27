//! Successfully initialized devices, independent of live connection state.

use std::{
    collections::HashSet,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use bluetooth::{DeviceId, TransportKind};
use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const MAX_FILE_SIZE: u64 = 512 * 1024;
const MAX_DEVICES: usize = 128;
const MAX_TEXT_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SavedTransport {
    Rfcomm,
    Gatt,
}

impl From<TransportKind> for SavedTransport {
    fn from(value: TransportKind) -> Self {
        match value {
            TransportKind::Rfcomm => Self::Rfcomm,
            TransportKind::Gatt => Self::Gatt,
        }
    }
}

impl From<SavedTransport> for TransportKind {
    fn from(value: SavedTransport) -> Self {
        match value {
            SavedTransport::Rfcomm => Self::Rfcomm,
            SavedTransport::Gatt => Self::Gatt,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedDevice {
    pub(super) id: String,
    pub(super) name: Option<String>,
    pub(super) model_name: Option<String>,
    pub(super) transport: SavedTransport,
}

impl SavedDevice {
    pub(super) fn device_id(&self) -> DeviceId {
        DeviceId(self.id.clone())
    }

    pub(super) fn display_name(&self) -> &str {
        self.model_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or_else(|| self.name.as_deref().filter(|name| !name.is_empty()))
            .unwrap_or(&self.id)
    }

    pub(super) fn transport_kind(&self) -> TransportKind {
        self.transport.into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedDevices {
    pub(super) version: u32,
    pub(super) entries: Vec<SavedDevice>,
    pub(super) last_selected: Option<String>,
}

impl Default for SavedDevices {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
            last_selected: None,
        }
    }
}

#[derive(Debug)]
pub(super) enum SavedDeviceError {
    Io(io::Error),
    Invalid(serde_json::Error),
    MissingVersion,
    UnsupportedVersion(u64),
    InvalidCatalog(&'static str),
    TooLarge,
}

impl fmt::Display for SavedDeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "Could not access saved devices: {error}"),
            Self::Invalid(error) => write!(f, "Saved devices could not be decoded: {error}"),
            Self::MissingVersion => f.write_str("Saved devices have no valid format version"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "Saved devices use unsupported format version {version}; the existing file was kept"
            ),
            Self::InvalidCatalog(reason) => write!(f, "Saved devices are invalid: {reason}"),
            Self::TooLarge => f.write_str("Saved devices exceed the 512 KiB size limit"),
        }
    }
}

impl std::error::Error for SavedDeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SavedDeviceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for SavedDeviceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error)
    }
}

fn path() -> Result<PathBuf, SavedDeviceError> {
    Ok(crate::config::directory()?.join("devices.json"))
}

impl SavedDevices {
    /// Returns an empty catalog for a missing file and reports other read errors.
    pub(super) fn load() -> (Self, Option<String>) {
        match path().and_then(|path| Self::read_from(&path)) {
            Ok(devices) => (devices, None),
            Err(SavedDeviceError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                (Self::default(), None)
            }
            Err(error) => (Self::default(), Some(error.to_string())),
        }
    }

    /// Atomically saves this catalog without replacing an invalid or newer file.
    pub(super) fn save(&self) -> Result<(), SavedDeviceError> {
        self.save_to(&path()?)
    }

    pub(super) fn find(&self, id: &DeviceId) -> Option<&SavedDevice> {
        self.entries.iter().find(|device| device.id == id.0)
    }

    /// Remembers a successful connection and selects that device.
    pub(super) fn upsert(&mut self, device: SavedDevice) {
        self.last_selected = Some(device.id.clone());
        match self.entries.iter_mut().find(|entry| entry.id == device.id) {
            Some(entry) => *entry = device,
            None => self.entries.push(device),
        }
    }

    /// Forgets the local record only; system pairing is unaffected.
    pub(super) fn remove(&mut self, id: &DeviceId) {
        self.entries.retain(|device| device.id != id.0);
        if self.last_selected.as_ref() == Some(&id.0) {
            self.last_selected = None;
        }
    }

    fn validate(&self) -> Result<(), SavedDeviceError> {
        if self.version != VERSION {
            return Err(SavedDeviceError::UnsupportedVersion(u64::from(
                self.version,
            )));
        }
        if self.entries.len() > MAX_DEVICES {
            return Err(SavedDeviceError::InvalidCatalog(
                "more than 128 device records",
            ));
        }
        let mut identities = HashSet::new();
        for entry in &self.entries {
            if entry.id.is_empty()
                || entry.id.len() > MAX_TEXT_BYTES
                || entry.id.chars().any(char::is_control)
            {
                return Err(SavedDeviceError::InvalidCatalog(
                    "device identity is empty, too long or contains control characters",
                ));
            }
            if entry
                .name
                .iter()
                .chain(&entry.model_name)
                .any(|name| name.len() > MAX_TEXT_BYTES)
            {
                return Err(SavedDeviceError::InvalidCatalog("device name is too long"));
            }
            if !identities.insert(&entry.id) {
                return Err(SavedDeviceError::InvalidCatalog(
                    "duplicate device identities",
                ));
            }
        }
        if self
            .last_selected
            .as_ref()
            .is_some_and(|id| !identities.contains(id))
        {
            return Err(SavedDeviceError::InvalidCatalog(
                "last selected device is not in the saved list",
            ));
        }
        Ok(())
    }

    fn read_from(path: &Path) -> Result<Self, SavedDeviceError> {
        let file = fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_SIZE {
            return Err(SavedDeviceError::TooLarge);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(SavedDeviceError::MissingVersion)?;
        if version != u64::from(VERSION) {
            return Err(SavedDeviceError::UnsupportedVersion(version));
        }
        let devices: Self = serde_json::from_value(value)?;
        devices.validate()?;
        Ok(devices)
    }

    fn save_to(&self, path: &Path) -> Result<(), SavedDeviceError> {
        self.validate()?;
        // Check again on each save to protect externally edited or newer files.
        match Self::read_from(path) {
            Ok(_) => {}
            Err(SavedDeviceError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let bytes = serde_json::to_vec_pretty(self)?;
        if bytes.len() as u64 + 1 > MAX_FILE_SIZE {
            return Err(SavedDeviceError::TooLarge);
        }
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid saved devices path")
        })?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| SavedDeviceError::Io(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str) -> SavedDevice {
        SavedDevice {
            id: id.into(),
            name: Some("Bluetooth alias".into()),
            model_name: Some("Test headset".into()),
            transport: SavedTransport::Rfcomm,
        }
    }

    #[test]
    fn successful_records_roundtrip_and_update_without_duplicate_devices() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/devices.json");
        let mut saved = SavedDevices::default();
        saved.upsert(device("one"));
        saved.upsert(device("two"));
        saved.save_to(&path).unwrap();
        assert_eq!(SavedDevices::read_from(&path).unwrap(), saved);
        let mut updated = device("one");
        updated.name = Some("New alias".into());
        updated.transport = SavedTransport::Gatt;
        saved.upsert(updated.clone());
        saved.save_to(&path).unwrap();
        let loaded = SavedDevices::read_from(&path).unwrap();
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.last_selected.as_deref(), Some("one"));
        assert_eq!(loaded.find(&updated.device_id()), Some(&updated));
        assert_eq!(updated.transport_kind(), TransportKind::Gatt);
        assert_eq!(updated.display_name(), "Test headset");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        saved.remove(&DeviceId("two".into()));
        assert_eq!(saved.last_selected.as_deref(), Some("one"));
        saved.remove(&DeviceId("one".into()));
        saved.save_to(&path).unwrap();
        assert_eq!(
            SavedDevices::read_from(&path).unwrap(),
            SavedDevices::default()
        );
    }

    #[test]
    fn malformed_future_duplicate_and_dangling_selection_files_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("devices.json");
        let duplicate = SavedDevices {
            entries: vec![device("one"), device("one")],
            ..SavedDevices::default()
        };
        let dangling = SavedDevices {
            last_selected: Some("missing".into()),
            ..SavedDevices::default()
        };
        let cases = [
            b"{ damaged".to_vec(),
            br#"{"version":2,"future_data":"keep this"}"#.to_vec(),
            br#"{"entries":[],"last_selected":null}"#.to_vec(),
            br#"{"version":1,"entries":[{"id":"one","transport":"unknown"}],"last_selected":null}"#
                .to_vec(),
            serde_json::to_vec(&duplicate).unwrap(),
            serde_json::to_vec(&dangling).unwrap(),
        ];
        for bytes in cases {
            fs::write(&path, &bytes).unwrap();
            assert!(SavedDevices::read_from(&path).is_err());
            assert!(SavedDevices::default().save_to(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn bounded_catalog_and_invalid_new_records_never_replace_valid_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("devices.json");
        SavedDevices::default().save_to(&path).unwrap();
        let original = fs::read(&path).unwrap();
        let mut invalid = SavedDevices::default();
        invalid.upsert(device(""));
        assert!(invalid.save_to(&path).is_err());
        invalid.entries[0].id = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(invalid.save_to(&path).is_err());
        invalid = SavedDevices::default();
        for index in 0..=MAX_DEVICES {
            invalid.upsert(device(&format!("device-{index}")));
        }
        assert!(invalid.save_to(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let too_large = vec![b' '; MAX_FILE_SIZE as usize + 1];
        fs::write(&path, &too_large).unwrap();
        assert!(matches!(
            SavedDevices::read_from(&path),
            Err(SavedDeviceError::TooLarge)
        ));
        assert!(SavedDevices::default().save_to(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), too_large);
    }
}
