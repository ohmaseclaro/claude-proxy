//! Running as a proxy: become `claude` for one account.
//!
//! When the binary is invoked under a proxy name (via `argv[0]`), it injects
//! that account's long-lived token and hands off to the real `claude` with
//! every argument untouched. The default config dir is left alone, so
//! transcripts land in `~/.claude/projects` exactly as they would for plain
//! `claude` — the token is the only thing that differs, and it lives only in
//! this process's environment.

use std::ffi::OsString;

use crate::store::Store;

/// The environment `claude` sees under a proxy, as (key, value|unset) pairs.
///
/// Pure, so the policy is testable without spawning anything:
/// - `CLAUDE_CODE_OAUTH_TOKEN` is set to the account's token;
/// - `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN` are unset, because either
///   would take precedence over the OAuth token and silently bill the wrong
///   account (the gatik wrapper's hard-won note).
pub enum EnvOp {
    Set(&'static str, String),
    Unset(&'static str),
}

pub fn proxy_env(token: &str) -> Vec<EnvOp> {
    vec![
        EnvOp::Set("CLAUDE_CODE_OAUTH_TOKEN", token.to_string()),
        EnvOp::Unset("ANTHROPIC_API_KEY"),
        EnvOp::Unset("ANTHROPIC_AUTH_TOKEN"),
    ]
}

/// Run `claude` as `label`, forwarding `args`. Returns the child's exit code,
/// or an error before the child is reached.
pub fn run(store: &dyn Store, label: &str, args: &[OsString]) -> Result<i32, String> {
    let token = store
        .get(label)
        .map_err(|e| format!("could not read the token for {label:?}: {e}"))?
        .ok_or_else(|| {
            format!("no token stored for {label:?}. Create it with:  claude-proxy add {label}")
        })?;

    let mut command = std::process::Command::new("claude");
    command.args(args);
    for op in proxy_env(&token) {
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
    fn proxy_env_injects_the_token_and_clears_the_overriding_keys() {
        let ops = proxy_env("sk-ant-oat01-tok");
        let mut set = None;
        let mut unset = Vec::new();
        for op in ops {
            match op {
                EnvOp::Set(k, v) => set = Some((k, v)),
                EnvOp::Unset(k) => unset.push(k),
            }
        }
        assert_eq!(
            set,
            Some(("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat01-tok".to_string()))
        );
        assert!(unset.contains(&"ANTHROPIC_API_KEY"));
        assert!(unset.contains(&"ANTHROPIC_AUTH_TOKEN"));
    }
}
