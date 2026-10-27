//! Paths shared by persistent client state and desktop instance coordination.

use std::{io, path::PathBuf};

/// Resolves the application's directory without creating it or importing old data.
pub(super) fn directory() -> io::Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|directories| directories.home_dir().join(".ezsc"))
        .filter(|path| path.is_absolute())
        .ok_or_else(|| io::Error::other("The user's home directory is unavailable"))
}
