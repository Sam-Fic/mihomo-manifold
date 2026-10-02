//! XDG locations. Config (secrets: subscription URLs with access tokens) and
//! runtime state are kept in plain files with 0600/0700 permissions — see the
//! project decision to skip the Secret Service.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    xdg_config_home().join("mihomo-manifold")
}

pub fn xdg_config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("mihomo-manifold")
}

/// Mutable settings written by the UI.
pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

/// Declarative defaults from the home-manager module, merged underneath.
pub fn defaults_file() -> PathBuf {
    config_dir().join("defaults.json")
}

/// Raw YAML as downloaded from each subscription, one file per profile id.
pub fn profiles_dir() -> PathBuf {
    state_dir().join("profiles")
}

/// The working directory handed to the core as `-d`.
pub fn core_dir() -> PathBuf {
    state_dir().join("core")
}

/// The config we generate from our own template and feed to the core.
pub fn generated_config() -> PathBuf {
    core_dir().join("config.yaml")
}

/// The throwaway config a temporary core is started with for offline latency
/// tests. Lives in the same `-d` directory as the real one so it reuses the
/// geodata files instead of downloading them again.
pub fn scratch_config() -> PathBuf {
    core_dir().join("scratch.yaml")
}

pub fn core_log() -> PathBuf {
    state_dir().join("core.log")
}

/// A temporary test core gets its own log, so its startup noise never shows up
/// in the Logs page or the "it would not start" tail.
pub fn scratch_log() -> PathBuf {
    state_dir().join("scratch.log")
}

/// The XDG autostart entry, which is what "start on login" means for a desktop
/// application. Not private: it holds no secrets and has to be readable by the
/// session's autostart machinery.
pub fn autostart_file() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
        .join("autostart")
        .join(format!("{}.desktop", crate::APP_ID))
}

/// Create a directory tree with 0700 on every component we own.
pub fn ensure_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Write a file that only the owner can read. Used for anything holding tokens.
pub fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    drop(f);
    // Re-assert the mode: an existing file keeps its old permissions.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    fs::rename(&tmp, path)
}
