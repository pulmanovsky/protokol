//! The app's identifier: `ua.protokol.app`. Earlier builds used the one
//! inherited from Meetily (`com.meetily.ai`), so their data lives under that
//! name. On the first start under the new identifier the data of an earlier
//! Protokol is copied over once; the old folders stay as they are, as a backup.
//!
//! The data of an installed Meetily is never taken: the old database must hold
//! a table only Protokol creates.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const IDENTIFIER: &str = "ua.protokol.app";
pub const OLD_IDENTIFIER: &str = "com.meetily.ai";

/// Tables only Protokol's own migrations create
const OWN_TABLES: [&[u8]; 2] = [b"speaker_overrides", b"edit_journal"];

/// Copies an earlier Protokol's data (app data and the web view's storage) to
/// the new identifier's folders. Runs before any window or database opens.
pub fn migrate_from_old_identifier() {
    let Some(data) = dirs::data_dir() else { return };
    let old = data.join(OLD_IDENTIFIER);
    let new = data.join(IDENTIFIER);
    match copy_once(&old, &new, is_protokol_data) {
        Ok(true) => log::info!("Data copied from {} to {} (the old folder is kept)", old.display(), new.display()),
        Ok(false) => {}
        Err(e) => {
            log::error!("Copying data from {} failed: {}", old.display(), e);
            return;
        }
    }
    // Interface settings kept by the web view (theme, remembered choices)
    #[cfg(target_os = "macos")]
    if new.is_dir() {
        if let Some(home) = dirs::home_dir() {
            let webkit = home.join("Library/WebKit");
            if let Err(e) = copy_once(&webkit.join(OLD_IDENTIFIER), &webkit.join(IDENTIFIER), |_| true) {
                log::warn!("Copying the interface settings failed: {}", e);
            }
        }
    }
}

/// Copies `old` to `new` when `new` does not exist yet and `accept(old)`;
/// through a temporary folder, so a stopped copy is never taken as done.
/// Returns whether it copied.
pub fn copy_once(old: &Path, new: &Path, accept: impl Fn(&Path) -> bool) -> io::Result<bool> {
    if new.exists() || !old.is_dir() || !accept(old) {
        return Ok(false);
    }
    let partial = partial_path(new);
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    copy_dir(old, &partial)?;
    fs::rename(&partial, new)?;
    Ok(true)
}

fn partial_path(new: &Path) -> PathBuf {
    let mut name = new.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".copying");
    new.with_file_name(name)
}

/// Copies a folder; on APFS `fs::copy` clones, so models take no extra space
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// An earlier Protokol's folder: its database has Protokol's own tables
fn is_protokol_data(dir: &Path) -> bool {
    let Ok(bytes) = fs::read(dir.join("meeting_minutes.sqlite")) else {
        return false;
    };
    OWN_TABLES.iter().any(|t| bytes.windows(t.len()).any(|w| w == *t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn copies_protokol_data_once_and_keeps_the_old_folder() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(OLD_IDENTIFIER);
        let new = root.path().join(IDENTIFIER);
        write(&old.join("meeting_minutes.sqlite"), "CREATE TABLE speaker_overrides (id)");
        write(&old.join("models/whisper/ggml-large-v3.bin"), "model");

        assert!(copy_once(&old, &new, is_protokol_data).unwrap());
        assert_eq!(fs::read_to_string(new.join("models/whisper/ggml-large-v3.bin")).unwrap(), "model");
        assert!(old.join("meeting_minutes.sqlite").exists(), "the old folder stays");
        assert!(!partial_path(&new).exists());

        // Second start: the new folder is there, nothing is copied again
        write(&old.join("meeting_minutes.sqlite"), "CREATE TABLE speaker_overrides (id) changed");
        assert!(!copy_once(&old, &new, is_protokol_data).unwrap());
        assert_eq!(fs::read_to_string(new.join("meeting_minutes.sqlite")).unwrap(), "CREATE TABLE speaker_overrides (id)");
    }

    #[test]
    fn leaves_a_meetily_install_alone() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(OLD_IDENTIFIER);
        let new = root.path().join(IDENTIFIER);
        write(&old.join("meeting_minutes.sqlite"), "CREATE TABLE meetings (id)");
        assert!(!copy_once(&old, &new, is_protokol_data).unwrap());
        assert!(!new.exists());
    }

    #[test]
    fn a_stopped_copy_is_done_again() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(OLD_IDENTIFIER);
        let new = root.path().join(IDENTIFIER);
        write(&old.join("meeting_minutes.sqlite"), "edit_journal");
        write(&partial_path(&new).join("half"), "x");
        assert!(copy_once(&old, &new, is_protokol_data).unwrap());
        assert!(!new.join("half").exists());
        assert!(new.join("meeting_minutes.sqlite").exists());
    }

    #[test]
    fn nothing_to_copy_on_a_new_mac() {
        let root = tempfile::tempdir().unwrap();
        assert!(!copy_once(&root.path().join(OLD_IDENTIFIER), &root.path().join(IDENTIFIER), |_| true).unwrap());
    }
}
