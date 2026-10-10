//! Where claude-proxy keeps its files, cross-platform.
//!
//! The registry lives directly under the config dir; each proxy's isolated
//! Claude profile lives under `accounts/<label>`. No secrets live here — Claude
//! owns its own credentials inside each account's profile dir.

use std::path::{Path, PathBuf};

/// `~/.config/claude-proxy` (or `$XDG_CONFIG_HOME/claude-proxy`).
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Path::new(&xdg).join("claude-proxy");
        }
    }
    home().join(".config").join("claude-proxy")
}

/// The isolated Claude config directory for one proxy.
///
/// Each proxy gets its own, so its login, identity, settings, and transcripts
/// never share state with the primary `~/.claude` login or with another proxy.
/// `claude` is pointed here via `CLAUDE_CONFIG_DIR`.
pub fn account_config_dir(label: &str) -> PathBuf {
    config_dir().join("accounts").join(label)
}

/// The user's home directory, cross-platform.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        if !up.is_empty() {
            return PathBuf::from(up);
        }
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_dir_is_under_the_config_dir_and_label_scoped() {
        let a = account_config_dir("personal");
        let b = account_config_dir("work");
        assert!(a.starts_with(config_dir()));
        assert!(a.ends_with("accounts/personal"));
        assert_ne!(a, b);
    }
}
