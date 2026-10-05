//! End-to-end: managed runs against a fake `claude` on PATH, in a throwaway
//! HOME. Exercises the real binary — background drainer, resume flags, the
//! transcript, status, kill, and resume-after-kill — with no network and no
//! real accounts.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Echoes the prompt as one stream-json turn and logs how it was called. A
/// prompt starting with `sleep` hangs (a turn to kill); `nap` takes 2 seconds
/// (a turn to message while it works).
const FAKE_CLAUDE: &str = r#"#!/bin/sh
prompt=$(cat)
sid=""; mode=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) mode="new"; sid="$2"; shift ;;
    --resume) mode="resume"; sid="$2"; shift ;;
  esac
  shift
done
echo "$mode $sid $prompt" >> "$FAKE_LOG"
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'"}'
case "$prompt" in
  sleep*) echo '{"type":"system","subtype":"task_summary","detail":"sleeping"}'; sleep 30 ;;
  nap*) echo '{"type":"system","subtype":"task_summary","detail":"napping"}'; sleep 2 ;;
esac
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"echo hi"}}]}}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","content":"hi","is_error":false}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"echo: '"$prompt"'"}]}}'
echo '{"type":"system","subtype":"post_turn_summary","status_detail":"echoed '"$prompt"'","needs_action":""}'
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":5,"total_cost_usd":0.01,"result":"echo: '"$prompt"'"}'
"#;

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    path: String,
    log: PathBuf,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(&fake, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    Env {
        path: format!("{}:/usr/bin:/bin", bin.display()),
        log: tmp.path().join("claude.log"),
        home,
        _tmp: tmp,
    }
}

fn cp(env: &Env, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_claude-proxy"))
        .args(args)
        .env_clear()
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", env.home.join(".config"))
        .env("PATH", &env.path)
        .env("FAKE_LOG", &env.log)
        .current_dir(&env.home)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn status(env: &Env, id: &str) -> Value {
    serde_json::from_slice(&cp(env, &["status", id, "--json"]).stdout).unwrap()
}

fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    let started = Instant::now();
    while !ok() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn a_run_is_started_messaged_read_killed_resumed_and_removed() {
    let env = setup();

    let out = cp(&env, &["run", "--account", "claude", "hello"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = stdout(&out);
    assert_eq!(id.len(), 6, "run prints just the id: {id:?}");

    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "echo: hello");

    // A follow-up resumes the same session.
    assert!(cp(&env, &["send", &id, "again"]).status.success());
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(stdout(&out), "echo: again");
    let log = std::fs::read_to_string(&env.log).unwrap();
    let calls: Vec<Vec<&str>> = log.lines().map(|l| l.split(' ').collect()).collect();
    assert_eq!(calls[0][0], "new");
    assert_eq!(calls[1][0], "resume");
    assert_eq!(calls[0][1], calls[1][1], "same session id");

    let text = stdout(&cp(&env, &["read", &id]));
    for needle in [
        "▶ you",
        "  hello",
        "● claude · turn 1",
        "  ⚙ Bash  echo hi",
        "    ↳ hi",
        "  echo: again",
        "✓ done · 1 steps",
    ] {
        assert!(
            text.contains(needle),
            "transcript lacks {needle:?}:\n{text}"
        );
    }
    let tail = stdout(&cp(&env, &["tail", &id, "-n", "1"]));
    assert!(tail.starts_with("✓ done"), "{tail}");

    let s = status(&env, &id);
    assert_eq!(s["state"], "idle");
    assert_eq!(s["turns"], 2);
    assert_eq!(s["summary"], "echoed again");
    assert_eq!(s["last_result"]["text"], "echo: again");

    // Kill a working turn; queued work stops and `send` resumes it.
    assert!(cp(&env, &["send", &id, "sleep please"]).status.success());
    wait_until("the turn is working", || {
        let s = status(&env, &id);
        s["state"] == "working" && s["activity"] == "sleeping"
    });
    let out = cp(&env, &["kill", &id]);
    assert!(stdout(&out).starts_with("Killed run"), "{}", stdout(&out));
    assert_eq!(status(&env, &id)["state"], "killed");
    let out = cp(&env, &["wait", &id, "--timeout", "10"]);
    assert_eq!(out.status.code(), Some(1), "a killed run waits as failed");
    assert!(stdout(&cp(&env, &["read", &id])).contains("■ killed"));

    assert!(cp(&env, &["send", &id, "back"]).status.success());
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(stdout(&out), "echo: back");
    assert_eq!(status(&env, &id)["state"], "idle");

    let runs: Value = serde_json::from_slice(&cp(&env, &["runs", "--json"]).stdout).unwrap();
    assert_eq!(runs[0]["id"], id.as_str());

    assert!(cp(&env, &["rm", &id]).status.success());
    assert!(!cp(&env, &["status", &id]).status.success());
}

#[test]
fn run_wait_blocks_and_prints_the_answer() {
    let env = setup();
    let out = cp(&env, &["run", "--account", "claude", "--wait", "quick"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "echo: quick");
}

#[test]
fn a_message_sent_while_working_is_delivered_as_the_next_turn() {
    let env = setup();
    let id = stdout(&cp(&env, &["run", "--account", "claude", "nap first"]));
    wait_until("the first turn is working", || {
        status(&env, &id)["activity"] == "napping"
    });

    assert!(cp(&env, &["send", &id, "then this"]).status.success());
    assert_eq!(status(&env, &id)["queued"], 1);

    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(stdout(&out), "echo: then this");
    let log = std::fs::read_to_string(&env.log).unwrap();
    let prompts: Vec<&str> = log
        .lines()
        .map(|l| l.splitn(3, ' ').nth(2).unwrap_or(""))
        .collect();
    assert_eq!(prompts, ["nap first", "then this"]);
    assert_eq!(status(&env, &id)["turns"], 2);
}
