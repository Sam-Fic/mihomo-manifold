//! "Start on login", as an XDG autostart entry.
//!
//! The file's presence is the whole state. Unlike the polkit rule there is no
//! reason to record anything separately: this directory is ours to read, so
//! asking the filesystem answers the question exactly instead of approximately.
//!
//! An autostart entry is used rather than a generated systemd unit because it
//! is what a desktop session actually looks for, it needs no privileges, and
//! it does not care whether the session is systemd-managed.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::paths;

/// Unit names that would launch this app a second time. A user who set one up
/// by hand — which is a reasonable thing to do before the app offered this —
/// would otherwise end up with two launchers and no obvious reason why the
/// window sometimes appears twice.
const CONFLICTING_UNITS: [&str; 2] = [
    "mihomo-manifold.service",
    "io.github.cublae.MihomoManifold.service",
];

pub fn is_enabled() -> bool {
    paths::autostart_file().exists()
}

/// The entry, for the binary that is actually running.
///
/// Kept separate from the write so the escaping and the field set are testable.
pub fn entry_text(exe: &str) -> String {
    // The Exec value is a command line, so a path with a space in it has to be
    // quoted or the session would run the first word.
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MihomoManifold\n\
         Comment=Manage the mihomo proxy core: subscriptions, device identity and split routing\n\
         Exec=\"{exe}\"\n\
         Icon={}\n\
         Terminal=false\n\
         Hidden=false\n\
         X-GNOME-Autostart-enabled=true\n",
        crate::APP_ID
    )
}

pub fn enable() -> std::io::Result<()> {
    let path = paths::autostart_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // current_exe resolves through symlinks, which is what we want: the entry
    // should point at the binary, not at whatever link launched it.
    let exe = std::env::current_exe()?;
    std::fs::write(&path, entry_text(&exe.to_string_lossy()))
}

pub fn disable() -> std::io::Result<()> {
    match std::fs::remove_file(paths::autostart_file()) {
        Ok(()) => Ok(()),
        // Turning the setting off twice is not an error worth reporting.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Systemd units for this app that are present in a user-writable location.
///
/// Only the on-disk check, deliberately: the page is rebuilt on every state
/// change, and shelling out to `systemctl` each time to answer a cosmetic
/// question would cost a subprocess per redraw.
pub fn conflicting_unit() -> Option<String> {
    let user_dir = paths::xdg_config_home().join("systemd").join("user");
    let system_dirs = ["/etc/systemd/user", "/usr/lib/systemd/user"];
    CONFLICTING_UNITS.iter().find_map(|unit| {
        let in_user = user_dir.join(unit);
        let anywhere = system_dirs
            .iter()
            .any(|dir| Path::new(dir).join(unit).exists());
        if in_user.exists() || anywhere {
            Some(unit.to_string())
        } else {
            None
        }
    })
}

/// Best-effort: used to tell the user their unit will now double up.
pub fn unit_is_enabled(unit: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "is-enabled", unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_points_at_the_given_binary() {
        let text = entry_text("/usr/bin/mihomo-manifold");
        assert!(text.contains("Exec=\"/usr/bin/mihomo-manifold\""));
        assert!(text.contains("Type=Application"));
        assert!(text.contains(&format!("Icon={}", crate::APP_ID)));
        assert!(text.contains("Terminal=false"));
        assert!(text.ends_with('\n'), "the entry must end with a newline");
    }

    #[test]
    fn a_path_with_spaces_stays_one_argument() {
        // Unquoted, the session would exec the first word and silently start
        // nothing.
        let text = entry_text("/home/a b/mihomo-manifold");
        assert!(text.contains(r#"Exec="/home/a b/mihomo-manifold""#));
    }

    #[test]
    fn enable_then_disable_leaves_nothing_behind() {
        let path = paths::autostart_file();
        let before = path.exists();

        enable().expect("writing the autostart entry");
        assert!(path.exists(), "enable must create the entry");
        assert!(is_enabled());

        disable().expect("removing the autostart entry");
        assert!(!path.exists(), "disable must remove the entry");
        assert!(!is_enabled());

        // Idempotent, so a second toggle-off is not an error.
        disable().expect("disabling again is not a failure");

        if !before {
            let _ = std::fs::remove_file(&path);
        }
    }
}
