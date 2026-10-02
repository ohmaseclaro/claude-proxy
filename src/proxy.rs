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

use crate::paths::account_config_dir;

/// The environment `claude` sees under a proxy, as (key, value|unset) pairs.
///
/// Pure, so the policy is testable without spawning anything:
/// - `CLAUDE_CONFIG_DIR` points at the account's own dir, isolating login,
///   identity, settings, and transcripts from the primary and other proxies;
/// - `CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, and `ANTHROPIC_AUTH_TOKEN`
///   are unset, because any inherited one would override the account's own
///   stored login and silently use the wrong credentials.
pub enum EnvOp {
    Set(&'static str, String),
    Unset(&'static str),
}

pub fn proxy_env(config_dir: &str) -> Vec<EnvOp> {
    vec![
        EnvOp::Set("CLAUDE_CONFIG_DIR", config_dir.to_string()),
        EnvOp::Unset("CLAUDE_CODE_OAUTH_TOKEN"),
        EnvOp::Unset("ANTHROPIC_API_KEY"),
        EnvOp::Unset("ANTHROPIC_AUTH_TOKEN"),
    ]
}

/// Run `claude` as `label`, forwarding `args`. Returns the child's exit code,
/// or an error before the child is reached.
pub fn run(label: &str, args: &[OsString]) -> Result<i32, String> {
    let config_dir = account_config_dir(label);
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("could not create the config dir for {label:?}: {e}"))?;
    let config_dir = config_dir.to_string_lossy().into_owned();

    let mut command = std::process::Command::new("claude");
    command.args(args);
    for op in proxy_env(&config_dir) {
        match op {
            EnvOp::Set(k, v) => {
                command.env(k, v);
            }
            EnvOp::Unset(k) => {
                command.env_remove(k);
            }
        }
    }
    exec_or_status(command, label)
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

    #[test]
    fn proxy_env_sets_the_config_dir_and_clears_inherited_credentials() {
        let ops = proxy_env("/home/me/.config/claude-proxy/accounts/gmail");
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
}
