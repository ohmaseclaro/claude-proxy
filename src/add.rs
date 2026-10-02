//! `claude-proxy add <label>` — create an isolated Claude profile, log it in,
//! and install it as a command.
//!
//! The account lives in its own `CLAUDE_CONFIG_DIR` under
//! `~/.config/claude-proxy/accounts/<label>`. We spawn Claude's own login there,
//! so Claude mints and stores its own credentials for that profile (and refreshes
//! them itself); claude-proxy never handles the token. Afterwards the profile is
//! fully independent of the primary `~/.claude` login and of other proxies.

use std::path::Path;
use std::process::Command;

use crate::install;
use crate::paths::account_config_dir;
use crate::registry::{valid_label, Registry};

/// Outcome of a successful add, for the caller to report.
pub struct Added {
    pub label: String,
    pub bin_path: std::path::PathBuf,
    pub on_path: bool,
}

/// Run the whole add flow. `self_exe` is this binary (to copy), `bin_dir` the
/// install target, `interactive` whether a terminal is attached for the login.
pub fn run(
    self_exe: &Path,
    bin_dir: &Path,
    label: &str,
    interactive: bool,
) -> Result<Added, String> {
    if !valid_label(label) {
        return Err(format!(
            "invalid label {label:?}: use letters, digits, and . _ - (starting alphanumeric)"
        ));
    }
    let mut registry = Registry::load();
    if registry.has(label) {
        return Err(format!(
            "{label:?} already exists. Remove it first:  claude-proxy remove {label}"
        ));
    }
    if !interactive {
        return Err(
            "adding an account needs an interactive terminal: it signs in through Claude's own \
             browser login. Run `claude-proxy add` directly in your shell."
                .into(),
        );
    }

    let dir = account_config_dir(label);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create the profile dir for {label:?}: {e}"))?;
    crate::shared::link_shared(&dir);
    seed_profile(&dir);

    eprintln!("\nSigning in {label:?} — opening Claude's login.");
    eprintln!("Log in with the account you want {label:?} to use (its own browser session),");
    eprintln!("then type /exit (or press Ctrl-C) to finish.\n");

    // Claude runs in the account's own profile dir and does its own login. We
    // clear any inherited credential so it cannot short-circuit the login.
    let status = Command::new("claude")
        .env("CLAUDE_CONFIG_DIR", &dir)
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .status()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "`claude` is not on PATH — install Claude Code first".to_string()
            } else {
                format!("could not run `claude`: {e}")
            }
        })?;
    // A Ctrl-C exit is expected and fine; the login is verified below regardless.
    let _ = status;

    if !profile_is_logged_in(&dir) {
        return Err(format!(
            "no login was completed for {label:?} — nothing was installed. \
             Run `claude-proxy add {label}` again and finish the sign-in."
        ));
    }

    let bin_path = install::install(self_exe, bin_dir, label)
        .map_err(|e| format!("could not install the {label:?} command: {e}"))?;
    registry.add(label);
    registry
        .save()
        .map_err(|e| format!("signed in but could not update the registry: {e}"))?;

    Ok(Added {
        on_path: install::on_path(bin_dir),
        label: label.to_string(),
        bin_path,
    })
}

/// Whether the profile holds a completed login, checked without a network call:
/// Claude records the authenticated account in the profile's `.claude.json`.
fn profile_is_logged_in(dir: &Path) -> bool {
    let text = match std::fs::read_to_string(dir.join(".claude.json")) {
        Ok(t) => t,
        Err(_) => return false,
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("oauthAccount")
                .and_then(|a| a.get("accountUuid"))
                .and_then(|u| u.as_str())
                .map(|u| !u.trim().is_empty())
        })
        .unwrap_or(false)
}

/// Pre-fill a couple of Claude's first-run preferences so the sign-in is not
/// preceded by the theme/onboarding prompt. Best-effort: if it fails, Claude
/// just shows its normal first-run screen.
fn seed_profile(dir: &Path) {
    let path = dir.join(".claude.json");
    if path.exists() {
        return;
    }
    let seed = serde_json::json!({
        "theme": "dark",
        "hasCompletedOnboarding": true,
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&seed) {
        let _ = std::fs::write(path, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logged_in_only_when_an_account_uuid_is_present() {
        let dir = tempfile::tempdir().unwrap();
        // No file yet.
        assert!(!profile_is_logged_in(dir.path()));
        // Onboarding-only, no account.
        std::fs::write(dir.path().join(".claude.json"), r#"{"theme":"dark"}"#).unwrap();
        assert!(!profile_is_logged_in(dir.path()));
        // A completed login records the account.
        std::fs::write(
            dir.path().join(".claude.json"),
            r#"{"oauthAccount":{"accountUuid":"abc-123"}}"#,
        )
        .unwrap();
        assert!(profile_is_logged_in(dir.path()));
    }

    #[test]
    fn seed_profile_writes_once_and_does_not_clobber() {
        let dir = tempfile::tempdir().unwrap();
        seed_profile(dir.path());
        let first = std::fs::read_to_string(dir.path().join(".claude.json")).unwrap();
        assert!(first.contains("\"theme\""));
        // A second call must not overwrite an existing profile.
        std::fs::write(dir.path().join(".claude.json"), r#"{"real":"data"}"#).unwrap();
        seed_profile(dir.path());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".claude.json")).unwrap(),
            r#"{"real":"data"}"#
        );
    }
}
