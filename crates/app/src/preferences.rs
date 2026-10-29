//! Versioned client preferences, independent of any headset connection.

use std::{
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const MAX_FILE_SIZE: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "en-US")]
    English,
    #[serde(rename = "zh-CN")]
    Chinese,
}

impl Language {
    pub fn tr(self, key: &str) -> String {
        app::i18n::translate(self.locale(), key)
    }

    pub fn tr_args(self, key: &str, arguments: &[(&str, String)]) -> String {
        app::i18n::translate_args(self.locale(), key, arguments)
    }

    pub fn locale(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh-CN",
        }
    }

    fn from_locale(locale: &str) -> Self {
        if locale
            .split(['-', '_', '.', '@'])
            .next()
            .is_some_and(|language| language.eq_ignore_ascii_case("zh"))
        {
            Self::Chinese
        } else {
            Self::English
        }
    }
}

impl Default for Language {
    fn default() -> Self {
        sys_locale::get_locale()
            .as_deref()
            .map(Self::from_locale)
            .unwrap_or(Self::English)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
    pub language: Language,
    pub theme: ThemeMode,
    pub tray_enabled: bool,
    pub close_to_tray: bool,
    pub start_at_login: bool,
    pub start_hidden: bool,
    pub notifications: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: VERSION,
            language: Language::default(),
            theme: ThemeMode::System,
            tray_enabled: true,
            close_to_tray: true,
            start_at_login: false,
            start_hidden: false,
            notifications: true,
        }
    }
}

#[derive(Debug)]
pub enum PreferenceError {
    NoConfigDirectory,
    Io(io::Error),
    Invalid(serde_json::Error),
    MissingVersion,
    UnsupportedVersion(u64),
    TooLarge,
}

impl fmt::Display for PreferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfigDirectory => {
                f.write_str("The client configuration directory is unavailable")
            }
            Self::Io(error) => write!(f, "Could not access client preferences: {error}"),
            Self::Invalid(error) => write!(f, "Client preferences are not valid: {error}"),
            Self::MissingVersion => f.write_str("Client preferences have no valid format version"),
            Self::UnsupportedVersion(version) => write!(
                f,
                "Client preferences use unsupported format version {version}; the existing file was kept"
            ),
            Self::TooLarge => f.write_str("Client preferences exceed the 64 KiB size limit"),
        }
    }
}

impl std::error::Error for PreferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PreferenceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for PreferenceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Invalid(error)
    }
}

pub fn config_path() -> Result<PathBuf, PreferenceError> {
    crate::config::directory()
        .map(|directory| directory.join("preferences.json"))
        .map_err(|_| PreferenceError::NoConfigDirectory)
}

impl Preferences {
    /// Uses defaults when the file is absent or unreadable, and reports read failures.
    /// Invalid and unsupported files remain untouched, including on later saves.
    pub fn load() -> (Self, Option<String>) {
        match config_path().and_then(|path| Self::read_from(&path)) {
            Ok(preferences) => (preferences, None),
            Err(PreferenceError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                (Self::default(), None)
            }
            Err(error) => (Self::default(), Some(error.to_string())),
        }
    }

    /// Atomically replaces a supported, valid file or creates a new one.
    /// It never overwrites malformed data or preferences from a newer version.
    pub fn save(&self) -> Result<(), PreferenceError> {
        self.save_to(&config_path()?)
    }

    fn read_from(path: &Path) -> Result<Self, PreferenceError> {
        let file = fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_FILE_SIZE {
            return Err(PreferenceError::TooLarge);
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(PreferenceError::MissingVersion)?;
        if version != u64::from(VERSION) {
            return Err(PreferenceError::UnsupportedVersion(version));
        }
        Ok(serde_json::from_value(value)?)
    }

    fn save_to(&self, path: &Path) -> Result<(), PreferenceError> {
        if self.version != VERSION {
            return Err(PreferenceError::UnsupportedVersion(u64::from(self.version)));
        }
        // Recheck at save time: another version may have changed the file since startup.
        match Self::read_from(path) {
            Ok(_) => {}
            Err(PreferenceError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = path.parent().ok_or(PreferenceError::NoConfigDirectory)?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, self)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| PreferenceError::Io(error.error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_future_files_survive_attempted_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        for bytes in [
            b"{ damaged".as_slice(),
            br#"{"version":2,"future_data":"keep this"}"#.as_slice(),
            br#"{"theme":"dark"}"#.as_slice(),
            br#"{"version":1,"language":"unsupported"}"#.as_slice(),
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(Preferences::default().save_to(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn supported_file_is_replaced_and_optional_fields_are_defaulted() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/preferences.json");
        let mut preferences = Preferences {
            language: Language::Chinese,
            theme: ThemeMode::Dark,
            ..Preferences::default()
        };
        preferences.save_to(&path).unwrap();
        assert_eq!(Preferences::read_from(&path).unwrap(), preferences);
        preferences.theme = ThemeMode::Light;
        preferences.notifications = false;
        preferences.save_to(&path).unwrap();
        assert_eq!(Preferences::read_from(&path).unwrap(), preferences);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);

        fs::write(&path, br#"{"version":1,"language":"en-US"}"#).unwrap();
        let loaded = Preferences::read_from(&path).unwrap();
        assert_eq!(loaded.language, Language::English);
        assert_eq!(loaded.theme, ThemeMode::System);
        assert!(!loaded.start_at_login);
    }

    #[test]
    fn oversized_file_is_bounded_and_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        let bytes = vec![b' '; MAX_FILE_SIZE as usize + 1];
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            Preferences::read_from(&path),
            Err(PreferenceError::TooLarge)
        ));
        assert!(Preferences::default().save_to(&path).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn locale_selection_uses_language_subtags_and_falls_back_to_english() {
        for locale in ["zh-CN", "zh_Hans_CN", "zh_CN.UTF-8", "ZH-sg"] {
            assert_eq!(Language::from_locale(locale), Language::Chinese);
        }
        for locale in ["", "C", "en_US.UTF-8", "de-DE", "zhongwen"] {
            assert_eq!(Language::from_locale(locale), Language::English);
        }
    }
}
