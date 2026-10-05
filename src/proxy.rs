//! Running as a proxy: become `claude` for one account.
//!
//! When the binary is invoked under a proxy name (via `argv[0]`), it points
//! `claude` at that account's own config directory and hands off with every
//! argument untouched.
//!
//! The dedicated config dir is the whole mechanism. Claude keys its login and
//! identity off `CLAUDE_CONFIG_DIR` (on macOS the credential item is derived
//! from the dir), so each proxy logs in once, as its own account, and is
//! unaffected by the primary `~/.claude` login or by other proxies. Any
//! inherited credential or host identity is cleared so it cannot override the
//! account.

use std::ffi::OsString;
use std::path::Path;

use crate::paths::account_config_dir;

/// Set by a host app (Claude Desktop, the Agent SDK) for its own Claude
/// session. Inherited, they would tie the account's Claude to that session's
/// identity.
const HOST_SESSION_VARS: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_ACCOUNT_UUID",
    "CLAUDE_CODE_ORGANIZATION_UUID",
    "CLAUDE_CODE_USER_EMAIL",
    "CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_SESSION_ID",
];

/// The environment `claude` sees under a proxy, as (key, value|unset) pairs.
///
/// Pure, so the policy is testable without spawning anything:
/// - `CLAUDE_CONFIG_DIR` points at the account's own dir (or is unset for the
///   primary profile — it may be inherited when `auto` runs inside a proxy
///   session), isolating login and identity from every other account;
/// - inherited credentials (`CLAUDE_CODE_OAUTH_TOKEN`, `ANTHROPIC_API_KEY`, …)
///   and a host app's session identity are unset, because any of them would
///   override the account's own stored login and silently use the wrong one.
pub enum EnvOp {
    Set(&'static str, String),
    Unset(&'static str),
}

pub fn proxy_env(config_dir: Option<&str>) -> Vec<EnvOp> {
    let mut ops = vec![match config_dir {
        Some(dir) => EnvOp::Set("CLAUDE_CONFIG_DIR", dir.to_string()),
        None => EnvOp::Unset("CLAUDE_CONFIG_DIR"),
    }];
    ops.extend(HOST_SESSION_VARS.iter().map(|k| EnvOp::Unset(k)));
    ops
}

/// Run `claude` as the proxy `label`, forwarding `args`. Returns the child's
/// exit code, or an error before the child is reached.
pub fn run(label: &str, args: &[OsString]) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?;
    let mut command = interactive_command(label, &cwd)?;
    command.args(args);
    exec_or_status(command, label)
}

/// Run `claude` on the primary (`~/.claude`) profile, forwarding `args`.
pub fn run_primary(args: &[OsString]) -> Result<i32, String> {
    let mut command = claude_command(None);
    command.args(args);
    exec_or_status(command, crate::quota::PRIMARY_LABEL)
}

/// `claude` for `label` (or the primary) as a person would start it in `cwd`:
/// with the user's MCP servers, which a profile does not have on its own.
pub fn interactive_command(label: &str, cwd: &Path) -> Result<std::process::Command, String> {
    if label == crate::quota::PRIMARY_LABEL {
        return Ok(claude_command(None));
    }
    let config_dir = account_config_dir(label);
    std::fs::create_dir_all(&config_dir)
        .map_err(|e| format!("could not create the config dir for {label:?}: {e}"))?;
    let mut command = claude_command(Some(&config_dir));
    let servers = crate::mcp::user_servers(cwd);
    if !servers.is_empty() {
        let file = crate::mcp::shared_file(&servers);
        crate::mcp::write(&file, servers)
            .map_err(|e| format!("could not write the MCP config: {e}"))?;
        command.arg(crate::mcp::flag(&file));
    }
    Ok(command)
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
        for var in [
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_ACCOUNT_UUID",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            assert!(unset.contains(&var), "{var}");
        }
    }

    #[test]
    fn the_primary_profile_clears_an_inherited_config_dir() {
        let (set, unset) = split(proxy_env(None));
        assert!(!set.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(unset.contains(&"CLAUDE_CONFIG_DIR"));
        assert!(unset.contains(&"CLAUDE_CODE_OAUTH_TOKEN"));
    }
}
