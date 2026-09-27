//! The systemd-resolved authorisation, as a rule the app installs on request.
//!
//! In TUN mode the core points the system resolver at its own tunnel interface,
//! which systemd-resolved exposes as three separate write actions (`dns`,
//! `domain`, `default-route`). systemd ships all three as `auth_admin`, so
//! without a rule every start asks for the admin password three times — and
//! dismissing one leaves the resolver pointing outside the tunnel.
//!
//! There is no core option to skip this: the integration is not configurable,
//! because dropping it is what makes TUN leak DNS. So the fix has to live in
//! polkit.
//!
//! What is on disk is the truth rather than a preference, so there is no
//! `config.json` field: [`installed`] reports the state, and it is recorded by
//! the install itself. The rule directory is not readable by ordinary users on
//! a stock polkit install, so the app cannot stat the rule to find out — and
//! asking polkit would be a second way to be wrong.

use anyhow::{anyhow, bail, Context, Result};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Where polkit reads administrator-installed rules from.
const RULE_PATH: &str = "/etc/polkit-1/rules.d/49-mihomo-manifold.rules";

/// The three actions the core needs, and nothing else. Read-only actions
/// (`dump-server-state` and friends) keep their defaults.
const ACTIONS: [&str; 3] = [
    "org.freedesktop.resolve1.set-dns-servers",
    "org.freedesktop.resolve1.set-domains",
    "org.freedesktop.resolve1.set-default-route",
];

/// Staging file for the rule handed to pkexec. It lives in our own 0700 state
/// directory, so no other local user can replace it between the write and root
/// reading it.
fn staging() -> PathBuf {
    crate::paths::state_dir().join("resolver-auth.rules")
}

fn marker() -> PathBuf {
    crate::paths::state_dir().join("resolver-auth")
}

/// Whether a rule is currently installed, as far as this app can tell.
pub fn installed() -> bool {
    marker().exists()
}

/// Only worth offering where the core would actually reach for these actions.
pub fn applies() -> bool {
    tun_worthy_system() && std::path::Path::new("/run/systemd/resolve").exists()
}

/// The rule, for the user that is going to run the core.
///
/// Kept separate from I/O so the escaping and the action list are testable.
pub fn rule_text(user: &str) -> String {
    let actions = ACTIONS
        .iter()
        .map(|id| format!("        \"{id}\""))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "// Written by MihomoManifold. Removes the password prompt the core's TUN\n\
         // mode would otherwise raise three times per start.\n\
         //\n\
         // Scoped to the three write actions the core performs, for one user.\n\
         // Remove with: sudo rm {RULE_PATH}\n\
         polkit.addRule(function (action, subject) {{\n\
         \x20   if (subject.user !== \"{user}\") {{\n\
         \x20       return undefined;\n\
         \x20   }}\n\
         \n\
         \x20   var allowed = [\n\
         {actions}\n\
         \x20   ];\n\
         \n\
         \x20   if (allowed.indexOf(action.id) !== -1) {{\n\
         \x20       return polkit.Result.YES;\n\
         \x20   }}\n\
         \n\
         \x20   return undefined;\n\
         }});\n",
        user = js_string(user)
    )
}

/// Quote a value for a JavaScript string literal.
///
/// The user name reaches us from the environment, and what it lands in is a
/// root-owned file that polkit evaluates — so a stray quote would otherwise be
/// a way to append a rule of one's choosing. Real accounts cannot contain
/// these, but `$USER` is not an account database.
fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\'' => out.push_str("\\'"),
            '\n' | '\r' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// systemd-resolved is what turns those three actions into password prompts;
/// without it the core writes resolv.conf-adjacent state some other way.
fn tun_worthy_system() -> bool {
    std::fs::read_link("/etc/resolv.conf")
        .map(|target| target.to_string_lossy().contains("systemd/resolve"))
        .unwrap_or(false)
}

fn current_user() -> Result<String> {
    if let Ok(user) = std::env::var("USER") {
        if !user.trim().is_empty() {
            return Ok(user);
        }
    }
    let out = Command::new("id")
        .arg("-un")
        .stderr(Stdio::null())
        .output()
        .context("looking up the current user name")?;
    if !out.status.success() {
        bail!("could not determine the current user name");
    }
    let user = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if user.is_empty() {
        bail!("could not determine the current user name");
    }
    Ok(user)
}

/// pkexec exit codes worth telling apart: 126 is the usual "dismissed the
/// dialog", 127 "could not run it at all".
fn explain(status: Option<i32>, stderr: &str) -> anyhow::Error {
    match status {
        Some(126) => anyhow!("Authorisation was cancelled — the rule was not installed."),
        Some(127) => anyhow!("pkexec could not be run, so the rule was not installed."),
        _ => {
            let detail = stderr.trim();
            if detail.is_empty() {
                anyhow!("the rule could not be installed")
            } else {
                anyhow!("the rule could not be installed: {detail}")
            }
        }
    }
}

/// Install the rule. Blocks on the user answering the polkit dialog, so it must
/// not run on the UI thread.
pub async fn install() -> Result<()> {
    tokio::task::spawn_blocking(install_blocking)
        .await
        .context("installing the polkit rule")?
}

fn install_blocking() -> Result<()> {
    let user = current_user()?;
    crate::paths::ensure_dir(&crate::paths::state_dir())
        .context("preparing the state directory")?;

    let staged = staging();
    std::fs::write(&staged, rule_text(&user)).context("writing the rule for pkexec")?;
    // The directory is 0700 and ours, so this only stops other local users
    // reading a file that is about to be handed to root.
    set_private(&staged)?;

    let out = Command::new("pkexec")
        .arg("install")
        .args(["-m", "644", "-o", "root", "-g", "root"])
        .arg(&staged)
        .arg(RULE_PATH)
        .output()
        .context("running pkexec")?;
    let _ = std::fs::remove_file(&staged);

    if !out.status.success() {
        return Err(explain(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
    }

    std::fs::write(marker(), user).context("recording that the rule is installed")?;
    Ok(())
}

/// Undo [`install`].
pub async fn uninstall() -> Result<()> {
    tokio::task::spawn_blocking(uninstall_blocking)
        .await
        .context("removing the polkit rule")?
}

fn uninstall_blocking() -> Result<()> {
    let out = Command::new("pkexec")
        .arg("rm")
        .args(["-f", RULE_PATH])
        .output()
        .context("running pkexec")?;
    if !out.status.success() {
        return Err(explain(out.status.code(), &String::from_utf8_lossy(&out.stderr)));
    }
    let _ = std::fs::remove_file(marker());
    Ok(())
}

fn set_private(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("restricting the staged rule")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_the_user_and_only_the_three_write_actions() {
        let rule = rule_text("alice");
        assert!(rule.contains("subject.user !== \"alice\""));
        for id in ACTIONS {
            assert!(rule.contains(id), "missing {id}");
        }
        // Nothing beyond the write actions may be let through.
        assert!(!rule.contains("dump-server-state"));
        assert!(!rule.contains("register-service"));
    }

    #[test]
    fn a_user_name_cannot_break_out_of_the_string_literal() {
        // polkit evaluates this as JavaScript, so an unescaped quote in the
        // name would let it append a rule of the caller's choosing to a file
        // that ends up owned by root.
        let rule = rule_text("a\" || polkit.Result.YES || \"");
        assert!(rule.contains(r#"subject.user !== "a\" || polkit.Result.YES || \"""#));
        // The line must still be exactly two delimiters and nothing more: the
        // injection attempt stays inside the literal.
        let line = rule
            .lines()
            .find(|l| l.contains("subject.user"))
            .expect("comparison line");
        assert_eq!(unescaped_quotes(line), 2, "quoting broke open in: {line}");
    }

    /// Quotes that actually delimit a literal, i.e. not `\"`.
    fn unescaped_quotes(line: &str) -> usize {
        let mut count = 0;
        let mut escaped = false;
        for ch in line.chars() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn control_characters_are_escaped_rather_than_emitted() {
        assert_eq!(js_string("a\nb"), "a\\nb");
        assert_eq!(js_string("a\\b"), "a\\\\b");
        assert_eq!(js_string("a\u{1}b"), "a\\u0001b");
        // Ordinary names are untouched, so the file stays readable.
        assert_eq!(js_string("sam"), "sam");
    }

    #[test]
    fn rule_is_valid_javascript_shape() {
        let rule = rule_text("bob");
        assert!(rule.trim_start().starts_with("//"));
        assert!(rule.contains("polkit.addRule(function (action, subject) {"));
        assert!(rule.trim_end().ends_with("});"));
        // Braces have to balance or polkit silently drops the file.
        assert_eq!(
            rule.matches('{').count(),
            rule.matches('}').count(),
            "unbalanced braces in:\n{rule}"
        );
    }

    #[test]
    fn cancelled_authorization_is_reported_as_such() {
        let err = explain(Some(126), "").to_string();
        assert!(err.contains("cancelled"), "{err}");
    }
}

