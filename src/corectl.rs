//! Lifecycle of the mihomo process. The GUI owns the core as a child process and
//! talks to it over the external controller; when TUN is on, the binary is
//! expected to be the capability wrapper installed by the NixOS module, so the
//! GUI itself never needs privileges.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::config::AppConfig;
use crate::i18n::{t, tf};
use crate::paths;

static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Where the NixOS module installs the capability wrapper for the core.
pub const NIXOS_WRAPPER: &str = "/run/wrappers/bin/mihomo";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreStatus {
    Stopped,
    /// Running as our child.
    Running,
    /// Reachable on the controller port but not started by this GUI.
    Adopted,
    Failed(String),
}

/// Whether the resolved core binary can actually open a TUN device — and if not,
/// which of the two very different reasons applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunReadiness {
    Ready,
    /// No such binary.
    Missing(String),
    /// It exists and carries the capabilities, but this session may not execute
    /// it: on NixOS that means the group membership has not been picked up yet.
    NotPermitted(String),
    /// Executable, but without CAP_NET_ADMIN.
    NoCapabilities(String),
    /// `getcap` is not installed, so there is nothing to check against.
    Unknown,
}

impl TunReadiness {
    /// The warning to show, or `None` when there is nothing to complain about.
    pub fn warning(&self) -> Option<String> {
        match self {
            TunReadiness::Ready | TunReadiness::Unknown => None,
            TunReadiness::Missing(binary) => Some(tf(
                "TUN is on but no core binary was found at {}. Set its path in Settings.",
                &[binary],
            )),
            TunReadiness::NotPermitted(path) => Some(tf(
                "TUN is on and {} is set up correctly, but this session may not run it. Log out and back in so the mihomo group applies.",
                &[path],
            )),
            TunReadiness::NoCapabilities(path) => Some(tf(
                "TUN is on but {} has no CAP_NET_ADMIN. Enable programs.mihomo-manifold.tun in your NixOS configuration.",
                &[path],
            )),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            TunReadiness::Ready => t("The core binary can create the TUN device.").to_string(),
            TunReadiness::Unknown => {
                t("getcap is not installed, so privileges could not be checked.").to_string()
            }
            _ => format!("⚠ {}", self.warning().unwrap_or_default()),
        }
    }
}

/// `exec` is whether the binary could be run at all, `caps` the output of
/// `getcap` when that tool exists. Kept free of I/O so every branch is testable.
fn classify(
    path: String,
    exec: Result<(), std::io::ErrorKind>,
    caps: Option<String>,
) -> TunReadiness {
    match exec {
        Err(std::io::ErrorKind::PermissionDenied) => TunReadiness::NotPermitted(path),
        Err(_) => TunReadiness::Missing(path),
        Ok(()) => match caps {
            Some(text) if text.to_lowercase().contains("cap_net_admin") => TunReadiness::Ready,
            Some(_) => TunReadiness::NoCapabilities(path),
            // getcap is not always installed; do not cry wolf.
            None => TunReadiness::Unknown,
        },
    }
}

pub fn tun_readiness(binary: &str) -> TunReadiness {
    let Some(path) = which(binary) else {
        return TunReadiness::Missing(binary.to_string());
    };

    // Running it is the only honest test of whether we are allowed to: the
    // capability wrapper is mode 0710, so group membership decides.
    let exec = Command::new(&path)
        .arg("-v")
        .output()
        .map(|_| ())
        .map_err(|err| err.kind());

    let caps = Command::new("getcap")
        .arg(&path)
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned());

    classify(path, exec, caps)
}

fn which(binary: &str) -> Option<String> {
    if binary.contains('/') {
        return Path::new(binary).exists().then(|| binary.to_string());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

pub fn is_child_alive() -> bool {
    let mut guard = CHILD.lock().unwrap();
    match guard.as_mut() {
        Some(child) => match child.try_wait() {
            Ok(None) => true,
            // Reap it so a later start does not see a zombie.
            _ => {
                *guard = None;
                false
            }
        },
        None => false,
    }
}

/// Write the generated config and (re)start the core against it.
pub fn start(cfg: &AppConfig, generated_yaml: &str) -> Result<()> {
    let dir = paths::core_dir();
    paths::ensure_dir(&dir).context("creating the core working directory")?;
    let config_path = paths::generated_config();
    paths::write_private(&config_path, generated_yaml).context("writing the generated config")?;

    if is_child_alive() {
        stop();
    }

    let binary = cfg.core.resolve_binary();
    let resolved = which(&binary).ok_or_else(|| {
        anyhow!(tf(
            "mihomo binary not found: {}\nSet its path in Settings.",
            &[&binary],
        ))
    })?;

    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::core_log())
        .context("opening the core log")?;
    let log_err = log.try_clone()?;

    let child = Command::new(&resolved)
        .arg("-d")
        .arg(&dir)
        .arg("-f")
        .arg(&config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .with_context(|| format!("starting {resolved}"))?;

    *CHILD.lock().unwrap() = Some(child);
    Ok(())
}

/// Stops the core, if it is ours to stop.
///
/// Returns whether there was one. A core that was merely adopted is running
/// under whatever started it, and killing a process this GUI did not launch is
/// not a decision it should make on the user's behalf — so `false` here means
/// "nothing was running that belongs to us", not "everything is fine".
pub fn stop() -> bool {
    let mut guard = CHILD.lock().unwrap();
    match guard.take() {
        Some(mut child) => {
            let _ = child.kill();
            let _ = child.wait();
            true
        }
        None => false,
    }
}

/// Last lines of the core log, for the "it would not start" case.
pub fn tail_log(lines: usize) -> String {
    let Ok(content) = std::fs::read_to_string(paths::core_log()) else {
        return String::new();
    };
    let all: Vec<&str> = content.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Real UID of a process, read from `/proc/<pid>/status`.
///
/// Only ever used to refuse signalling somebody else's process. A core started
/// by root is left alone no matter what the user asks for: this is a desktop
/// app, not a service manager.
fn real_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("Uid:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn signal(pid: u32, sig: &str) {
    // std has no kill(2); /bin/kill is on every Linux install we target, and
    // two signals do not justify pulling in a C dependency.
    let _ = Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether `argv0` names the same binary as `want`.
///
/// `argv[0]` is the only handle on an external core, so the comparison is
/// deliberately strict: resolved paths must be equal, and the file-name
/// fallback only applies when the process could not be resolved at all.
fn names_same_binary(argv0: &str, want: &Path, want_name: Option<&std::ffi::OsStr>) -> bool {
    if argv0.is_empty() {
        return false;
    }
    match Path::new(argv0).canonicalize() {
        Ok(path) => path == want,
        Err(_) => Path::new(argv0).file_name() == want_name,
    }
}

/// PIDs of cores running `binary` that this GUI did not start.
///
/// The listening port cannot be mapped to a PID: a core carrying capabilities
/// makes `/proc/<pid>/fd` unreadable without `CAP_SYS_PTRACE`, which is exactly
/// the case for any core able to open a TUN device. So the match is on the
/// command line instead — the path in `argv[0]` against the configured binary,
/// restricted to our own uid.
pub fn external_cores(binary: &str) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let me = std::process::id();
    let me_uid = real_uid(me);
    let want = Path::new(binary)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(binary).to_path_buf());
    let want_name = want.file_name().map(|n| n.to_owned());

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me || real_uid(pid) != me_uid {
            continue;
        }
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let argv0 = String::from_utf8_lossy(&raw)
            .split('\0')
            .next()
            .unwrap_or_default()
            .to_string();
        if names_same_binary(&argv0, &want, want_name.as_deref()) {
            found.push(pid);
        }
    }
    found.sort_unstable();
    found
}

/// Stops a core this GUI did not start, and reports which PIDs it signalled.
///
/// SIGTERM first and only then SIGKILL, because a core killed outright leaves
/// its TUN device and routes behind: the tunnel interface stays up with no
/// process to feed it, and the host's networking is left pointing into a black
/// hole. A core that has been asked nicely gets to tear that down itself.
pub fn stop_external(binary: &str) -> Vec<u32> {
    let pids = external_cores(binary);
    if pids.is_empty() {
        return pids;
    }
    for &pid in &pids {
        signal(pid, "TERM");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if pids.iter().all(|pid| !alive(*pid)) {
            return pids;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    for &pid in &pids {
        if alive(pid) {
            signal(pid, "KILL");
        }
    }
    pids
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    #[test]
    fn external_core_matching_is_strict_where_it_can_be() {
        let want = Path::new("/usr/bin/mihomo");
        let name = Some(std::ffi::OsStr::new("mihomo"));

        // A resolvable path has to be the real thing, not merely similar.
        assert!(names_same_binary("/bin/mihomo", want, name)); // /bin -> /usr/bin
        assert!(!names_same_binary("/usr/bin/other", want, name));
        assert!(!names_same_binary("", want, name)); // kernel thread
    }

    #[test]
    fn an_argv_that_cannot_be_resolved_falls_back_to_the_file_name() {
        // A process started through PATH or a script has an argv[0] that does
        // not resolve, and there is nothing else to go on. This is the loosest
        // the match ever gets: it requires the name to match *and* the process
        // to be running as the same user.
        let nowhere = Path::new("/nonexistent/dir/mihomo");
        let name = Some(std::ffi::OsStr::new("mihomo"));
        assert!(names_same_binary("mihomo", nowhere, name));
        assert!(names_same_binary("/opt/vanished/mihomo", nowhere, name));
        assert!(!names_same_binary("clash", nowhere, name));
        assert!(!names_same_binary("", nowhere, name));
    }

    #[test]
    fn nothing_matches_a_binary_that_does_not_exist() {
        assert!(
            external_cores("/nonexistent/definitely-not-here").is_empty(),
            "a bogus path must not match anything, including this very process"
        );
    }

    #[test]
    fn a_process_can_see_its_own_uid() {
        assert_eq!(real_uid(std::process::id()), real_uid(std::process::id()));
        assert!(real_uid(std::process::id()).is_some());
    }

    #[test]
    fn stopping_without_a_core_reports_that_nothing_was_stopped() {
        // The return value is what stops the UI from claiming a stop that never
        // happened when it has merely adopted a core started elsewhere.
        let mut guard = CHILD.lock().unwrap();
        *guard = None;
        // The guard has to go before calling stop(), or this deadlocks on the
        // very mutex stop() needs.
        drop(guard);
        assert!(
            !stop(),
            "an empty child slot must not report a successful stop"
        );
    }

    #[test]
    fn group_membership_is_not_a_missing_capability() {
        let readiness = classify(
            "/run/wrappers/bin/mihomo".into(),
            Err(ErrorKind::PermissionDenied),
            None,
        );
        assert_eq!(
            readiness,
            TunReadiness::NotPermitted("/run/wrappers/bin/mihomo".into())
        );
        let warning = readiness.warning().unwrap();
        assert!(warning.contains("Log out and back in"), "{warning}");
        assert!(!warning.contains("CAP_NET_ADMIN"), "{warning}");
    }

    #[test]
    fn capabilities_are_read_from_getcap() {
        let caps = Some(
            "/run/wrappers/bin/mihomo cap_net_bind_service,cap_net_admin,cap_net_raw=ep\n"
                .to_string(),
        );
        assert_eq!(classify("p".into(), Ok(()), caps), TunReadiness::Ready);
        assert_eq!(
            classify("p".into(), Ok(()), Some(String::new())),
            TunReadiness::NoCapabilities("p".into())
        );
    }

    #[test]
    fn no_getcap_means_no_warning() {
        let readiness = classify("p".into(), Ok(()), None);
        assert_eq!(readiness, TunReadiness::Unknown);
        assert!(readiness.warning().is_none());
    }
}
