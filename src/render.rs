//! Turning a run's `events.jsonl` — Claude's stream-json plus claude-proxy's own
//! message/turn markers — into a readable transcript, one event line at a time
//! so `--follow` can render incrementally.

use serde_json::Value;

const ONE_LINE_MAX: usize = 140;

/// Display lines for one event line. `full` keeps tool inputs, tool output, and
/// messages whole; the default keeps one line per tool call and result.
pub fn render_line(line: &str, full: bool) -> Vec<String> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        // Claude printed something that is not an event (e.g. a login error).
        return vec![format!("  ! {}", one_line(line))];
    };
    let str_of = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("");
    match str_of("type") {
        "claude_proxy" => marker(&v, full),
        "assistant" => blocks(&v)
            .iter()
            .flat_map(|b| assistant_block(b, full))
            .collect(),
        "user" => blocks(&v)
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            .flat_map(|b| tool_result(b, full))
            .collect(),
        "result" => vec![result_line(&v)],
        "rate_limit_event" => rate_limit(&v).into_iter().collect(),
        "system" if full && str_of("subtype") == "post_turn_summary" => {
            let detail = str_of("status_detail");
            if detail.is_empty() {
                Vec::new()
            } else {
                vec![format!("  ≡ {detail}")]
            }
        }
        _ => Vec::new(),
    }
}

fn marker(v: &Value, full: bool) -> Vec<String> {
    match v.get("event").and_then(Value::as_str) {
        Some("message") => {
            let text = v.get("text").and_then(Value::as_str).unwrap_or("");
            let mut out = vec!["▶ you".to_string()];
            out.extend(indented(text, "  ", full));
            out
        }
        Some("turn_start") => vec![format!(
            "● {} · turn {}",
            v.get("account").and_then(Value::as_str).unwrap_or("?"),
            v.get("turn").and_then(Value::as_u64).unwrap_or(0)
        )],
        Some("turn_end") => {
            let killed = v.get("killed").and_then(Value::as_bool).unwrap_or(false);
            match v.get("exit").and_then(Value::as_i64) {
                _ if killed => vec!["■ killed".into()],
                Some(0) => Vec::new(),
                Some(code) => vec![format!("✗ claude exited with code {code}")],
                None => match v.get("error").and_then(Value::as_str) {
                    Some(e) => vec![format!("✗ {e}")],
                    None => vec!["✗ claude was stopped by a signal".into()],
                },
            }
        }
        _ => Vec::new(),
    }
}

fn blocks(v: &Value) -> Vec<Value> {
    v.pointer("/message/content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn assistant_block(b: &Value, full: bool) -> Vec<String> {
    match b.get("type").and_then(Value::as_str) {
        Some("text") => indented(
            b.get("text").and_then(Value::as_str).unwrap_or(""),
            "  ",
            true,
        ),
        Some("tool_use") => {
            let name = b.get("name").and_then(Value::as_str).unwrap_or("tool");
            let input = b.get("input").cloned().unwrap_or(Value::Null);
            if full {
                let pretty = serde_json::to_string_pretty(&input).unwrap_or_default();
                let mut out = vec![format!("  ⚙ {name}")];
                out.extend(indented(&pretty, "      ", true));
                out
            } else {
                vec![format!("  ⚙ {name}  {}", tool_summary(name, &input))]
            }
        }
        _ => Vec::new(),
    }
}

/// The one argument that says what a tool call does.
pub fn tool_summary(name: &str, input: &Value) -> String {
    let pick = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_string);
    let picked = match name {
        "Bash" => pick("command"),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => pick("file_path"),
        "Grep" | "Glob" => pick("pattern"),
        "WebFetch" => pick("url"),
        "WebSearch" => pick("query"),
        "Agent" | "Task" => pick("description"),
        "Skill" => pick("skill"),
        _ => None,
    };
    one_line(&picked.unwrap_or_else(|| input.to_string()))
}

fn tool_result(b: &Value, full: bool) -> Vec<String> {
    let text = match b.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    };
    let error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let arrow = if error { "    ✗ " } else { "    ↳ " };
    if full {
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        let mut out = vec![format!("{arrow}{first}")];
        out.extend(lines.map(|l| format!("      {l}")));
        return out;
    }
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let first = lines.next().unwrap_or("(no output)");
    let rest = lines.count();
    let more = if rest > 0 {
        format!("  (+{rest} lines)")
    } else {
        String::new()
    };
    vec![format!("{arrow}{}{more}", one_line(first))]
}

fn result_line(v: &Value) -> String {
    let ok = v.get("subtype").and_then(Value::as_str) == Some("success")
        && !v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    let mut parts = vec![if ok {
        "✓ done".to_string()
    } else {
        format!(
            "✗ {}",
            v.get("subtype").and_then(Value::as_str).unwrap_or("error")
        )
    }];
    if let Some(n) = v.get("num_turns").and_then(Value::as_u64) {
        parts.push(format!("{n} steps"));
    }
    if let Some(ms) = v.get("duration_ms").and_then(Value::as_f64) {
        parts.push(format!("{:.0}s", ms / 1000.0));
    }
    if let Some(cost) = v.get("total_cost_usd").and_then(Value::as_f64) {
        parts.push(format!("${cost:.2}"));
    }
    let mut line = parts.join(" · ");
    if !ok {
        if let Some(text) = v.get("result").and_then(Value::as_str) {
            line.push_str(&format!("\n  {}", one_line(text)));
        }
    }
    line
}

/// Only worth a line when Claude is warning or refusing.
fn rate_limit(v: &Value) -> Option<String> {
    let info = v.get("rate_limit_info")?;
    let status = info.get("status").and_then(Value::as_str).unwrap_or("");
    if status.is_empty() || status == "allowed" {
        return None;
    }
    let kind = info
        .get("rateLimitType")
        .and_then(Value::as_str)
        .unwrap_or("usage");
    let used = info
        .get("utilization")
        .and_then(Value::as_f64)
        .map(|u| format!(" {:.0}%", u * 100.0))
        .unwrap_or_default();
    Some(format!("  ⚠ rate limit: {kind}{used} ({status})"))
}

fn indented(text: &str, prefix: &str, full: bool) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| format!("{prefix}{l}")).collect();
    if !full && lines.len() > 3 {
        let more = lines.len() - 3;
        lines.truncate(3);
        lines.push(format!("{prefix}(+{more} lines)"));
    }
    lines
}

fn one_line(s: &str) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > ONE_LINE_MAX {
        let cut: String = flat.chars().take(ONE_LINE_MAX).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(line: &str) -> Vec<String> {
        render_line(line, false)
    }

    #[test]
    fn renders_messages_turns_tools_and_results() {
        assert_eq!(
            r(r#"{"type":"claude_proxy","event":"message","text":"fix the tests"}"#),
            ["▶ you", "  fix the tests"]
        );
        assert_eq!(
            r(r#"{"type":"claude_proxy","event":"turn_start","turn":2,"account":"claude-gmail"}"#),
            ["● claude-gmail · turn 2"]
        );
        assert_eq!(
            r(r#"{"type":"assistant","message":{"content":[
                {"type":"thinking","thinking":""},
                {"type":"tool_use","name":"Bash","input":{"command":"cargo test","description":"x"}},
                {"type":"text","text":"All green."}]}}"#),
            ["  ⚙ Bash  cargo test", "  All green."]
        );
        assert_eq!(
            r(r#"{"type":"user","message":{"content":[
                {"type":"tool_result","content":"line one\nline two\n\nline three","is_error":false}]}}"#),
            ["    ↳ line one  (+2 lines)"]
        );
        assert_eq!(
            r(
                r#"{"type":"result","subtype":"success","is_error":false,"num_turns":3,
                "duration_ms":42000,"total_cost_usd":0.256,"result":"done"}"#
            ),
            ["✓ done · 3 steps · 42s · $0.26"]
        );
    }

    #[test]
    fn failures_kills_and_noise() {
        assert_eq!(
            r(r#"{"type":"claude_proxy","event":"turn_end","exit":null,"killed":true}"#),
            ["■ killed"]
        );
        assert_eq!(
            r(r#"{"type":"claude_proxy","event":"turn_end","exit":1,"killed":false}"#),
            ["✗ claude exited with code 1"]
        );
        assert!(r(r#"{"type":"claude_proxy","event":"turn_end","exit":0}"#).is_empty());
        assert!(r(r#"{"type":"system","subtype":"hook_started"}"#).is_empty());
        assert_eq!(
            r("Not logged in · Please run /login"),
            ["  ! Not logged in · Please run /login"]
        );
        assert_eq!(
            r(
                r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning",
                "rateLimitType":"seven_day","utilization":0.97}}"#
            ),
            ["  ⚠ rate limit: seven_day 97% (allowed_warning)"]
        );
        assert!(
            r(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#).is_empty()
        );
    }

    #[test]
    fn full_mode_keeps_everything() {
        let out = render_line(
            r#"{"type":"user","message":{"content":[
                {"type":"tool_result","content":[{"type":"text","text":"a\nb"}],"is_error":true}]}}"#,
            true,
        );
        assert_eq!(out, ["    ✗ a", "      b"]);
    }

    #[test]
    fn tool_summary_picks_the_telling_argument() {
        let input = serde_json::json!({"file_path": "/x/y.rs", "old_string": "a"});
        assert_eq!(tool_summary("Edit", &input), "/x/y.rs");
        let other = serde_json::json!({"k": 1});
        assert_eq!(tool_summary("Mystery", &other), r#"{"k":1}"#);
    }
}
