//! `claude-proxy add <label>` — sign an account in and make it a command.
//!
//! 1. run the OAuth login (browser + a pasted code) to get the long-lived token;
//! 2. store it in the keychain or a 0600 file — the token never touches the
//!    terminal, a file in the repo, or a log;
//! 3. copy this binary into the user's bin under the label, so `claude-<label>`
//!    is a command in every shell.

use crate::install;
use crate::oauth;
use crate::registry::{valid_label, Registry};
use crate::store::Store;

/// Outcome of a successful add, for the caller to report.
pub struct Added {
    pub label: String,
    pub bin_path: std::path::PathBuf,
    pub on_path: bool,
}

/// Run the whole add flow. `self_exe` is this binary (to copy), `bin_dir` the
/// install target, `interactive` whether a terminal is attached for the login.
pub fn run(
    store: &dyn Store,
    self_exe: &std::path::Path,
    bin_dir: &std::path::Path,
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

    let token = oauth::login(label, interactive)?;
    oauth::store_token(store, label, &token)?;

    let bin_path = install::install(self_exe, bin_dir, label)
        .map_err(|e| format!("could not install the {label:?} command: {e}"))?;

    registry.add(label);
    registry
        .save()
        .map_err(|e| format!("stored the token but could not update the registry: {e}"))?;

    Ok(Added {
        on_path: install::on_path(bin_dir),
        label: label.to_string(),
        bin_path,
    })
}
