//! The list of configured proxies and which one is the default.
//!
//! A small JSON file beside the config dir. It holds no secrets — only the list
//! of proxy labels.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::config_dir;

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Registry {
    pub fn path() -> PathBuf {
        config_dir().join("registry.json")
    }

    pub fn load() -> Registry {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Registry {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Registry::default(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        bytes.push(b'\n');
        std::fs::write(path, bytes)
    }

    pub fn has(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// Add a label (idempotent), keeping the list sorted.
    pub fn add(&mut self, label: &str) {
        if !self.has(label) {
            self.labels.push(label.to_string());
            self.labels.sort();
        }
    }

    /// Remove a label.
    pub fn remove(&mut self, label: &str) {
        self.labels.retain(|l| l != label);
    }
}

/// A proxy label must be a safe command name: it becomes a file in the user's
/// bin and a Keychain service, so no slashes, spaces, or leading dash.
pub fn valid_label(label: &str) -> bool {
    // `claude` would shadow the real binary (the proxy would exec itself) and
    // names the primary profile in `list`/`auto`.
    !matches!(label, "claude" | "claude-proxy")
        && !label.is_empty()
        && label.len() <= 64
        && label
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_labels_are_safe_command_names() {
        for ok in ["claude-gmail", "work", "acct.2", "a_b-c", "x"] {
            assert!(valid_label(ok), "{ok} should be valid");
        }
        for bad in [
            "",
            "-leading",
            "has space",
            "a/b",
            "a;b",
            "../etc",
            "a$b",
            "claude",
            "claude-proxy",
        ] {
            assert!(!valid_label(bad), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn add_is_sorted_and_idempotent() {
        let mut r = Registry::default();
        r.add("work");
        r.add("gmail");
        assert_eq!(r.labels, vec!["gmail", "work"], "kept sorted");
        r.add("gmail");
        assert_eq!(r.labels, vec!["gmail", "work"], "adding twice is a no-op");
    }

    #[test]
    fn remove_drops_the_label() {
        let mut r = Registry::default();
        r.add("gmail");
        r.add("work");
        r.remove("gmail");
        assert_eq!(r.labels, vec!["work"]);
        r.remove("work");
        assert!(r.labels.is_empty());
        // Removing an absent label is a no-op.
        r.remove("nope");
    }

    #[test]
    fn load_from_missing_is_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let r = Registry::load_from(&dir.path().join("nope.json"));
        assert_eq!(r, Registry::default());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let mut r = Registry::default();
        r.add("gmail");
        r.add("work");
        r.save_to(&path).unwrap();
        assert_eq!(Registry::load_from(&path), r);
    }
}
