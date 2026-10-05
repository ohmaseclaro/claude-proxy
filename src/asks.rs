//! Decisions a run cannot make alone — permission prompts and Claude's
//! `AskUserQuestion` — handed to whoever manages the run.
//!
//! Every turn starts Claude with `--permission-prompt-tool` pointing at
//! `claude-proxy __permit <id>`, a one-tool MCP server. When Claude needs a
//! decision, the server records it as `asks/<key>.json` and blocks until
//! `allow`, `deny`, or `answer` writes `asks/<key>.answer`.

use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::quota::now_secs;
use crate::runs::{self, marker, run_dir};

pub const SERVER: &str = "claude-proxy";
pub const PROMPT_TOOL: &str = "mcp__claude-proxy__permit";

pub struct Ask {
    pub tool: String,
    pub input: Value,
    key: String,
}

impl Ask {
    pub fn is_question(&self) -> bool {
        self.tool == "AskUserQuestion"
    }

    pub fn questions(&self) -> Vec<String> {
        self.input
            .get("questions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|q| q.get("question").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }
}

/// Unanswered asks, oldest first. Only meaningful while the run is alive: the
/// drainer clears them when a turn ends.
pub fn pending(dir: &Path) -> Vec<Ask> {
    let asks = dir.join("asks");
    let mut keys: Vec<String> = fs::read_dir(&asks)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some(e.file_name().to_str()?.strip_suffix(".json")?.to_string()))
        .filter(|k| !asks.join(format!("{k}.answer")).exists())
        .collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|key| {
            let v: Value =
                serde_json::from_slice(&fs::read(asks.join(format!("{key}.json"))).ok()?).ok()?;
            Some(Ask {
                tool: v["tool"].as_str().unwrap_or("tool").to_string(),
                input: v["input"].clone(),
                key,
            })
        })
        .collect()
}

pub fn clear(dir: &Path) {
    let _ = fs::remove_dir_all(dir.join("asks"));
}

pub enum Reply {
    Allow,
    Deny(String),
    Answer(Vec<String>),
}

/// Answer the oldest pending ask of run `id`, returning it.
pub fn reply(id: &str, reply: Reply) -> Result<Ask, String> {
    runs::load(id)?;
    let dir = run_dir(id);
    let ask = pending(&dir)
        .into_iter()
        .next()
        .filter(|_| runs::alive(&dir))
        .ok_or_else(|| {
            format!("run {id} is not waiting for an answer. See:  claude-proxy status {id}")
        })?;
    let answer = match reply {
        Reply::Allow => json!({"behavior": "allow"}),
        Reply::Deny(message) => json!({"behavior": "deny", "message": message}),
        Reply::Answer(texts) => {
            if !ask.is_question() {
                return Err(format!(
                    "run {id} is asking to use {}, not asking a question: \
                     claude-proxy allow {id}  or  claude-proxy deny {id} \"<reason>\"",
                    ask.tool
                ));
            }
            let questions = ask.questions();
            if texts.len() != questions.len() {
                return Err(format!(
                    "run {id} asked {} question(s); give one answer each, in order:\n{}",
                    questions.len(),
                    questions
                        .iter()
                        .map(|q| format!("  {q}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }
            let answers: Map<String, Value> = questions
                .into_iter()
                .zip(texts.into_iter().map(Value::String))
                .collect();
            json!({"behavior": "allow", "answers": answers})
        }
    };
    write_atomic(
        &dir.join("asks").join(format!("{}.answer", ask.key)),
        &answer,
    )
    .map_err(|e| format!("could not answer run {id}: {e}"))?;
    Ok(ask)
}

/// The MCP server Claude talks to over stdio: newline-delimited JSON-RPC.
pub fn serve(id: &str) -> Result<(), String> {
    let dir = run_dir(id);
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // Notifications carry no id and get no reply.
        let Some(req_id) = req.get("id").cloned() else {
            continue;
        };
        let params = &req["params"];
        let mut reply = match req["method"].as_str().unwrap_or("") {
            "initialize" => json!({"result": {
                "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2024-11-05"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER, "version": env!("CARGO_PKG_VERSION")},
            }}),
            "tools/list" => json!({"result": {"tools": [{
                "name": "permit",
                "description": "Asks the agent managing this claude-proxy run for a decision.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": {"type": "string"},
                        "input": {"type": "object"},
                        "tool_use_id": {"type": "string"},
                    },
                    "required": ["tool_name", "input"],
                },
            }]}}),
            "tools/call" => {
                let decision = decide(&dir, id, &params["arguments"]);
                json!({"result": {"content": [{"type": "text", "text": decision.to_string()}]}})
            }
            "ping" => json!({"result": {}}),
            _ => json!({"error": {"code": -32601, "message": "method not found"}}),
        };
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = req_id;
        let mut bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if out.write_all(&bytes).and_then(|()| out.flush()).is_err() {
            break;
        }
    }
    Ok(())
}

/// Post one ask and block until it is answered, the run is killed, or Claude
/// goes away. Returns the permission-prompt-tool decision.
fn decide(dir: &Path, id: &str, args: &Value) -> Value {
    let tool = args["tool_name"].as_str().unwrap_or("tool").to_string();
    let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
    let asks = dir.join("asks");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let key = format!("{nanos:024}");
    let ask =
        json!({"tool": tool, "input": input, "tool_use_id": args["tool_use_id"], "at": now_secs()});
    if fs::create_dir_all(&asks)
        .and_then(|()| write_atomic(&asks.join(format!("{key}.json")), &ask))
        .is_err()
    {
        return json!({"behavior": "deny", "message": "claude-proxy could not record the request."});
    }
    marker(
        dir,
        json!({"event": "ask", "run": id, "tool": tool, "input": input}),
    );

    let parent = parent_id();
    let answer_path = asks.join(format!("{key}.answer"));
    let answer = loop {
        if let Some(a) = fs::read(&answer_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            break a;
        }
        if dir.join("stop").exists() || parent_id() != parent {
            break json!({"behavior": "deny", "message": "The run was stopped."});
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    let _ = fs::remove_file(asks.join(format!("{key}.json")));
    let _ = fs::remove_file(&answer_path);
    marker(
        dir,
        json!({"event": "answer", "behavior": answer["behavior"],
               "message": answer["message"], "answers": answer["answers"]}),
    );

    if answer["behavior"] == "allow" {
        let mut updated = input;
        if let (Some(fields), Some(answers)) = (updated.as_object_mut(), answer.get("answers")) {
            fields.insert("answers".into(), answers.clone());
        }
        json!({"behavior": "allow", "updatedInput": updated})
    } else {
        json!({"behavior": "deny", "message": answer["message"].as_str().unwrap_or("Denied.")})
    }
}

#[cfg(unix)]
fn parent_id() -> u32 {
    std::os::unix::process::parent_id()
}

#[cfg(not(unix))]
fn parent_id() -> u32 {
    0
}

fn write_atomic(path: &Path, value: &Value) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, serde_json::to_vec(value)?)?;
    fs::rename(tmp, path)
}
