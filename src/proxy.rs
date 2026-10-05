//! Running as a proxy: become `claude` for one account.
//!
//! When the binary is invoked under a proxy name (via `argv[0]`), it points
//! `claude` at that account's own config directory and hands off with every
//! argument untouched.
//!
//! The dedicated config dir is the whole mechanism. Claude keys its login,
//! identity, settings, and transcripts off `CLAUDE_CONFIG_DIR` (on macOS the
//! credential item is derived from the dir), so each proxy has a completely
//! independent Claude install: it logs in once, as its own account, and is
//! unaffected by the primary `~/.claude` login or by other proxies. An injected
//! `CLAUDE_CODE_OAUTH_TOKEN` is deliberately *not* used — it authenticates
//! headless `-p` runs but not interactive sessions (which check the stored
//! login), and it would fight the dir's own credential. Any inherited one is
//! cleared so a global token can't override the account. The cost is that a
//! proxy's transcripts live under its account dir, not `~/.claude/projects`.

use std::ffi::OsString;
use std::path::Path;

use crate::paths::account_config_dir;

/// The environment `claude` sees under a proxy, as (key, value|unset) pairs.
///
/// Pure, so the policy is testable without spawning anything:
/// - `CLAUDE_CONFIG_DIR` points at the account's own dir (or is unset for the
///   primary profile — it may be inherited when `auto` runs inside a proxy
///   session), isolating login and identity from every other account;
/// - `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, and `ANTHROPIC_AUTH_TOKEN`
///   are unset, because any inherited one would override the account's own
///   stored login and silently use the wrong credentials.
pub enum EnvOp {
    Set(&'static str, String),
    Unset(&'static str),
}

pub fn proxy_env(config_dir: Option<&str>) -> Vec<EnvOp> {
    vec![
        match config_dir {
            Some(dir) => EnvOp::Set("CLAUDE_CONFIG_DIR", dir.to_string()),
            None => EnvOp::Unset("CLAUDE_CONFIG_DIR"),
        },
        EnvOp::Unset("CLAUDE_CODE_OAUTH_TOKEN"),
        EnvOp::Unset("ANTHROPIC_API_KEY"),
        EnvOp::Unset("ANTHROPIC_AUTH_TOKEN"),
    ]
}

/// Run `claude` as the proxy `label`, forwarding `args`. Returns the child's
/// exit code, or an error before the child is reached.
pub fn run(label: &str, args: &[OsString]) -> Result<i32, String> {
    let config_dir = account_config_dir(label);
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("could not create the config dir for {label:?}: {e}"))?;
    let mut command = claude_command(Some(&config_dir));
    command.args(args);
    exec_or_status(command, label)
}

/// Run `claude` on the primary (`~/.claude`) profile, forwarding `args`.
pub fn run_primary(args: &[OsString]) -> Result<i32, String> {
    let mut command = claude_command(None);
    command.args(args);
    exec_or_status(command, crate::quota::PRIMARY_LABEL)
}

/// A `claude` command for an account — `None` is the primary profile — with
/// the shared setup linked, the config dir set, and inherited credentials
/// cleared. Not yet run.
pub fn claude_command(config_dir: Option<&Path>) -> std::process::Command {
    if let Some(dir) = config_dir {
        let _ = std::fs::create_dir_all(dir);
        crate::shared::link_shared(dir);
    }
    let dir = config_dir.map(|d| d.to_string_lossy().into_owned());
    let mut command = std::process::Command::new("claude");
    for op in proxy_env(dir.as_deref()) {
        match op {
            EnvOp::Set(k, v) => {
                command.env(k, v);
            }
            EnvOp::Unset(k) => {
                command.env_remove(k);
            }
        }
    }
    command
}

/// On Unix, replace this process with `claude` so signals and the exit code
/// pass through exactly. Elsewhere, spawn and forward the status.
#[cfg(unix)]
fn exec_or_status(mut command: std::process::Command, _label: &str) -> Result<i32, String> {
    use std::os::unix::process::CommandExt;
    // `exec` only returns on failure.
    let err = command.exec();
    Err(classify_spawn_error(err))
}

#[cfg(not(unix))]
fn exec_or_status(mut command: std::process::Command, _label: &str) -> Result<i32, String> {
    match command.status() {
        Ok(status) => Ok(status.code().unwrap_or(1)),
        Err(err) => Err(classify_spawn_error(err)),
    }
}

fn classify_spawn_error(err: std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        "`claude` is not on PATH — install Claude Code first".into()
    } else {
        format!("could not run `claude`: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(
        ops: Vec<EnvOp>,
    ) -> (
        std::collections::HashMap<&'static str, String>,
        Vec<&'static str>,
    ) {
        let mut set = std::collections::HashMap::new();
        let mut unset = Vec::new();
        for op in ops {
            match op {
                EnvOp::Set(k, v) => {
                    set.insert(k, v);
                }
                EnvOp::Unset(k) => unset.push(k),
            }
        }
        (set, unset)
    }

    #[test]
    fn proxy_env_sets_the_config_dir_and_clears_inherited_credentials() {
        let (set, unset) = split(proxy_env(Some(
            "/home/me/.config/claude-proxy/accounts/gmail",
        )));
        assert_eq!(
            set.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/home/me/.config/claude-proxy/accounts/gmail")
        );
        // No credential is injected; any inherited one is cleared so the
        // account's own stored login is what claude uses.
        assert!(unset.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(unset.contains(&"ANTHROPIC_API_KEY"));
        assert!(unset.contains(&"ANTHROPIC_AUTH_TOKEN"));
    }

    #[test]
    fn the_primary_profile_clears_an_inherited_config_dir() {
        let (set, unset) = split(proxy_env(None));
        assert!(!set.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(unset.contains(&"CLAUDE_CONFIG_DIR"));
        assert!(unset.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
    }
}
