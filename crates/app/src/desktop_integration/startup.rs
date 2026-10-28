//! User-owned login entries. Calls are made only when the preference is changed.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::PathBuf;

#[cfg(target_os = "linux")]
fn entry_path() -> Result<PathBuf, String> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or_else(|| "The user configuration directory is unavailable".to_owned())?;
    Ok(config
        .join("autostart")
        .join(format!("{}.desktop", app::APP_ID)))
}

#[cfg(target_os = "macos")]
fn entry_path() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|p| {
            PathBuf::from(p)
                .join("Library/LaunchAgents")
                .join(format!("{}.plist", app::APP_ID))
        })
        .ok_or_else(|| "The user library directory is unavailable".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn autostart_enabled() -> Result<bool, String> {
    entry_path()?
        .try_exists()
        .map_err(|error| error.to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    use std::io::Write;
    let path = entry_path()?;
    if !enabled {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = executable
        .to_str()
        .ok_or_else(|| "The executable path is not valid Unicode".to_owned())?;
    let data = entry_contents(executable)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid login entry path".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let serial = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = parent.join(format!(".ezsc-login-{}-{serial}.tmp", std::process::id()));
    let mut created = false;
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        created = true;
        file.write_all(data.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
    })();
    if result.is_err() && created {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(target_os = "linux")]
fn entry_contents(executable: &str) -> Result<String, String> {
    if executable.contains(['\n', '\r', '\0']) {
        return Err("The executable path cannot be represented in a login entry".into());
    }
    // Desktop-entry string unescaping precedes Exec's quoting rules. Backslash,
    // quotes, dollar and backtick therefore need the corresponding two levels.
    let mut quoted = String::new();
    for ch in executable.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' => quoted.push_str("\\\\\""),
            '$' => quoted.push_str("\\\\$"),
            '`' => quoted.push_str("\\\\`"),
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName={}\nExec=\"{quoted}\" --background\nTerminal=false\nStartupWMClass={}\nX-GNOME-Autostart-enabled=true\n",
        app::APP_NAME,
        app::APP_ID,
    ))
}

#[cfg(target_os = "macos")]
fn entry_contents(executable: &str) -> Result<String, String> {
    if executable.contains(['\n', '\r', '\0']) {
        return Err("The executable path cannot be represented in a login entry".into());
    }
    let path = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array><string>{path}</string><string>--background</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
        app::APP_ID,
    ))
}

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const ENTRY_NAME: &str = app::APP_ID;

#[cfg(windows)]
pub fn autostart_enabled() -> Result<bool, String> {
    let key = match winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(RUN_KEY) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    match key.get_value::<String, _>(ENTRY_NAME) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(RUN_KEY)
        .map_err(|error| error.to_string())?;
    if enabled {
        let path = std::env::current_exe().map_err(|error| error.to_string())?;
        let path = path
            .to_str()
            .ok_or_else(|| "The executable path is not valid Unicode".to_owned())?;
        key.set_value(ENTRY_NAME, &format!("\"{path}\" --background"))
            .map_err(|error| error.to_string())
    } else {
        match key.delete_value(ENTRY_NAME) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn login_command_quotes_paths_and_literal_field_codes() {
        let content = entry_contents("/opt/Sound Connect/100%/ezsc").unwrap();
        assert!(content.contains("Exec=\"/opt/Sound Connect/100%%/ezsc\" --background\n"));
        assert!(content.contains("Name=Easy Sound Connect\n"));
        assert!(content.contains("StartupWMClass=com.jjl9807.ezsc\n"));
        assert!(!content.contains("KeepAlive"));
        assert!(entry_contents("/tmp/bad\ncommand").is_err());
    }
}
