//! Claude Code hooks the plugin installs (`claude-proxy __hook <event>`, the
//! hook's JSON on stdin).
//!
//! - `prompt` (UserPromptSubmit) remembers the session's permission mode, so a
//!   run it starts inherits it the way a subagent would.
//! - `pre-tool-use` (Agent|Task) turns a subagent call back with the
//!   equivalent `claude-proxy run`, unless the description starts `[direct]`.

use std::fs;
use std::io::Read;

use serde_json::Value;

use crate::paths::config_dir;

/// Subagent types Claude defines itself; `--agent` only knows the user's own.
const BUILT_IN_AGENTS: &[&str] = &[
    "general-purpose",
    "Explore",
    "Plan",
    "claude-code-guide",
    "statusline-setup",
    "output-style-setup",
];

pub fn run(event: &str) -> i32 {
    // Lets the plugin's wrapper tell this binary from one without hooks.
    if event == "supported" {
        return 0;
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(v) = serde_json::from_str::<Value>(&input) else {
        return 0;
    };
    match event {
        "prompt" => {
            remember_mode(&v);
            0
        }
        "pre-tool-use" => match redirect(&v, policy_applies()) {
            Some(message) => {
                eprintln!("{message}");
                2
            }
            None => 0,
        },
        _ => 0,
    }
}

fn remember_mode(v: &Value) {
    let (Some(session), Some(mode)) = (v["session_id"].as_str(), v["permission_mode"].as_str())
    else {
        return;
    };
    if valid_session_id(session) {
        let dir = config_dir().join("sessions");
        // ponytail: one small file per session, never pruned; prune by age if it ever matters.
        let _ = fs::create_dir_all(&dir).and_then(|()| fs::write(dir.join(session), mode));
    }
}

/// The permission mode last seen for a Claude session.
pub fn session_mode(session: &str) -> Option<String> {
    if !valid_session_id(session) {
        return None;
    }
    fs::read_to_string(config_dir().join("sessions").join(session))
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

fn valid_session_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Not inside a run (runs do not start runs), and not turned off.
fn policy_applies() -> bool {
    std::env::var_os("CLAUDE_PROXY_RUN").is_none()
        && std::env::var("CLAUDE_PROXY_POLICY").map_or(true, |p| p != "off")
}

/// Why a subagent call should be a run instead, with the command to use.
fn redirect(v: &Value, applies: bool) -> Option<String> {
    let tool = v["tool_name"].as_str()?;
    if !applies || !matches!(tool, "Agent" | "Task") {
        return None;
    }
    let input = &v["tool_input"];
    let description = input["description"].as_str().unwrap_or("");
    if description.trim_start().starts_with("[direct]") {
        return None;
    }
    let mut cmd = String::from("claude-proxy run");
    if let Some(agent) = input["subagent_type"]
        .as_str()
        .filter(|a| !BUILT_IN_AGENTS.contains(a))
    {
        cmd.push_str(&format!(" --agent {}", shell_quote(agent)));
    }
    if input["isolation"] == "worktree" {
        cmd.push_str(" --worktree");
    }
    if !description.is_empty() {
        cmd.push_str(&format!(" --name {}", shell_quote(description)));
    }
    cmd.push_str(" -");
    if let Some(model) = input["model"].as_str() {
        cmd.push_str(&format!(" -- --model {}", shell_quote(model)));
    }
    Some(format!(
        "The user's claude-proxy policy sends subagent work to managed runs, so this {tool} \
         call was not made. Run instead:\n\n  {cmd} <<'EOF'\n  <the same prompt>\n  EOF\n\n\
         Then run `claude-proxy wait <id>` on its own — in the background if you will be \
         notified when it ends, otherwise in the foreground — where exit code 2 means it is \
         asking you something; and tell the user the run id, its account, and \
         `claude-proxy watch <id>`. If the user said not to use claude-proxy for this, repeat \
         the {tool} call with \"[direct]\" at the start of its description."
    ))
}

fn shell_quote(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subagent_calls_are_sent_to_runs_unless_direct() {
        let call =
            |input: Value| redirect(&json!({"tool_name": "Agent", "tool_input": input}), true);
        let message = call(
            json!({"subagent_type": "gsd-executor", "description": "fix it",
                                   "isolation": "worktree", "model": "sonnet"}),
        )
        .unwrap();
        assert!(
            message.contains(
                "claude-proxy run --agent gsd-executor --worktree --name 'fix it' - -- --model sonnet"
            ),
            "{message}"
        );
        let builtin = call(json!({"subagent_type": "Explore", "description": "look"})).unwrap();
        assert!(
            builtin.contains("claude-proxy run --name look -"),
            "{builtin}"
        );
        assert!(call(json!({"description": "[direct] quick look"})).is_none());
        assert!(redirect(&json!({"tool_name": "Bash", "tool_input": {}}), true).is_none());
        let inside_a_run = json!({"tool_name": "Agent", "tool_input": {"description": "x"}});
        assert!(redirect(&inside_a_run, false).is_none());
    }

    #[test]
    fn session_ids_cannot_escape_the_sessions_dir() {
        assert!(valid_session_id("7ecce39f-0ebf-43d2-bdff-c0fa9272d4b0"));
        for bad in ["", "../x", "a/b", "a.b"] {
            assert!(!valid_session_id(bad), "{bad:?}");
        }
    }
}
