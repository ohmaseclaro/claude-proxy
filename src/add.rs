//! `claude-proxy add <label>` — log in an account and make it a command.
//!
//! The flow mirrors how you would do it by hand, automated:
//!   1. run `claude setup-token`, which opens the browser for whichever
//!      account you choose and mints a **long-lived** token (an ordinary
//!      login's access token would expire in hours; the proxy injects this one
//!      with no chance to refresh, so it must be the durable kind);
//!   2. capture that token from the command's output, falling back to a hidden
//!      paste if it is not on stdout;
//!   3. store it, register the label, and copy this binary into the user's bin
//!      under the label so it is a command everywhere.
//!
//! The login runs with its own isolated config directory, so completing it
//! never touches your existing `claude` login.

use crate::install;
use crate::registry::{valid_label, Registry};
use crate::store::Store;

/// Find a Claude token in arbitrary text (the captured output of
/// `setup-token`). Pure, so the scan is tested without running anything.
///
/// Matches `sk-ant-` followed by the token body, and returns the longest such
/// run — a defensive choice if the output also mentions a shorter id.
pub fn scan_token(text: &str) -> Option<String> {
    const PREFIX: &str = "sk-ant-";
    let mut best: Option<&str> = None;
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(rel) = text[i..].find(PREFIX) {
        let start = i + rel;
        let mut end = start + PREFIX.len();
        while end < bytes.len()
            && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'-' | b'_'))
        {
            end += 1;
        }
        let candidate = &text[start..end];
        // A real token is well past the prefix; ignore a bare "sk-ant-".
        if candidate.len() > PREFIX.len() + 10 && best.is_none_or(|b| candidate.len() > b.len()) {
            best = Some(candidate);
        }
        i = end.max(start + 1);
    }
    best.map(str::to_string)
}

/// Outcome of a successful add, for the caller to report.
pub struct Added {
    pub label: String,
    pub bin_path: std::path::PathBuf,
    pub on_path: bool,
}

/// Run the whole add flow. `self_exe` is the path to this binary (to copy),
/// `bin_dir` the install target, `interactive` whether a paste fallback may
/// prompt.
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

    let token = capture_login(label, interactive)?;

    store
        .set(label, &token)
        .map_err(|e| format!("could not store the token: {e}"))?;
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

/// Run `claude setup-token` under an isolated config dir and return the token.
fn capture_login(label: &str, interactive: bool) -> Result<String, String> {
    use std::process::Stdio;

    let probe_dir = tempfile::Builder::new()
        .prefix("claude-proxy-login-")
        .tempdir()
        .map_err(|e| format!("could not create a temp login dir: {e}"))?;

    eprintln!("Opening `claude setup-token` to sign in {label:?} (your browser will open).");
    eprintln!("Choose the account you want {label:?} to use, then authorize.\n");

    // stdin + stderr inherited so the interactive flow and browser prompts work;
    // stdout captured so we can read the token the command prints at the end.
    let mut command = std::process::Command::new("claude");
    command
        .arg("setup-token")
        .env("CLAUDE_CONFIG_DIR", probe_dir.path())
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());

    let out = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "`claude` is not on PATH — install Claude Code first".to_string()
        } else {
            format!("could not run `claude setup-token`: {e}")
        }
    })?;

    if let Some(token) = scan_token(&String::from_utf8_lossy(&out.stdout)) {
        return Ok(token);
    }
    // Some versions print the token only to the terminal, or persist it in the
    // isolated dir; look there before asking the user.
    if let Some(token) = scan_dir_for_token(probe_dir.path()) {
        return Ok(token);
    }
    if !out.status.success() {
        return Err("`claude setup-token` did not complete — nothing was stored".into());
    }
    if !interactive {
        return Err("could not read the token from `claude setup-token`".into());
    }
    paste_token()
}

/// Last resort: the user pastes the token `setup-token` showed them, hidden.
fn paste_token() -> Result<String, String> {
    eprintln!("\nCould not read the token automatically.");
    let entered = rpassword::prompt_password("Paste the token shown above (hidden): ")
        .map_err(|e| format!("could not read the pasted token: {e}"))?;
    scan_token(&entered)
        .or_else(|| {
            let t = entered.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
        .ok_or_else(|| "no token entered".to_string())
}

/// Scan an isolated config dir's credential files for a token.
fn scan_dir_for_token(dir: &std::path::Path) -> Option<String> {
    for name in [".credentials.json", ".claude.json", "credentials.json"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(name)) {
            if let Some(token) = scan_token(&text) {
                return Some(token);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_token_out_of_noisy_output() {
        let out = "Success! Your token is:\n\nsk-ant-oat01-AbC123_def-456XYZ\n\nKeep it safe.\n";
        assert_eq!(
            scan_token(out).as_deref(),
            Some("sk-ant-oat01-AbC123_def-456XYZ")
        );
    }

    #[test]
    fn prefers_the_token_over_a_shorter_id_mention() {
        let out = "id sk-ant-abc then token sk-ant-oat01-longlonglonglonglong123456";
        assert_eq!(
            scan_token(out).as_deref(),
            Some("sk-ant-oat01-longlonglonglonglong123456")
        );
    }

    #[test]
    fn no_token_in_plain_text_is_none() {
        assert_eq!(scan_token("no secrets here, just prose"), None);
        assert_eq!(scan_token("sk-ant-"), None, "a bare prefix is not a token");
    }

    #[test]
    fn scans_a_token_from_a_credentials_json_blob() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-fromfilefromfile123456"}}"#;
        assert_eq!(
            scan_token(json).as_deref(),
            Some("sk-ant-oat01-fromfilefromfile123456")
        );
    }
}
