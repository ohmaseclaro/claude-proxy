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

/// Echoes the prompt as one stream-json turn, records it in the session file
/// the way Claude does, and logs how it was called. A prompt starting with
/// `sleep` hangs (a turn to kill); `nap` takes 2 seconds (a turn to message
/// while it works); `limited` fails with a usage limit on the account `acct-a`.
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
acct=$(basename "${CLAUDE_CONFIG_DIR:-primary}")
echo "$mode $sid $acct $prompt" >> "$FAKE_LOG"
mkdir -p "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/fake"
echo "$prompt" >> "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/fake/$sid.jsonl"
echo '{"type":"system","subtype":"init","session_id":"'"$sid"'"}'
case "$prompt" in
  sleep*) echo '{"type":"system","subtype":"task_summary","detail":"sleeping"}'; sleep 30 ;;
  nap*) echo '{"type":"system","subtype":"task_summary","detail":"napping"}'; sleep 2 ;;
  limited*)
    if [ "$acct" = "acct-a" ]; then
      echo '{"type":"assistant","message":{"content":[{"type":"text","text":"usage limit reached"}]},"error":"rate_limit"}'
      echo '{"type":"result","subtype":"success","is_error":true,"result":"usage limit reached"}'
      exit 1
    fi ;;
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
    std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    let registry = home.join(".config/claude-proxy/registry.json");
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry, r#"{"labels":["acct-a","acct-b"]}"#).unwrap();
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
        .map(|l| l.splitn(4, ' ').nth(3).unwrap_or(""))
        .collect();
    assert_eq!(prompts, ["nap first", "then this"]);
    assert_eq!(status(&env, &id)["turns"], 2);
}

#[test]
fn a_usage_limit_moves_the_session_to_the_next_account() {
    let env = setup();
    let id = stdout(&cp(
        &env,
        &["run", "--account", "acct-a,acct-b", "limited job"],
    ));
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        stdout(&cp(&env, &["read", &id]))
    );
    assert!(
        stdout(&out).starts_with(
            "echo: (claude-proxy) Your previous turn stopped because the Claude account \
             acct-a hit its usage limit."
        ),
        "{}",
        stdout(&out)
    );

    // Same session, resumed on the other account — the message had already
    // reached the session, so it is not sent twice.
    let log = std::fs::read_to_string(&env.log).unwrap();
    let calls: Vec<Vec<&str>> = log.lines().map(|l| l.splitn(4, ' ').collect()).collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0][..3], ["new", calls[0][1], "acct-a"]);
    assert_eq!(calls[0][3], "limited job");
    assert_eq!(calls[1][..3], ["resume", calls[0][1], "acct-b"]);

    let s = status(&env, &id);
    assert_eq!(s["state"], "idle");
    assert_eq!(s["account"], "acct-b");
    assert_eq!(s["moved"], "from acct-a, which hit its usage limit");
    assert!(stdout(&cp(&env, &["read", &id]))
        .contains("⇄ acct-a hit its usage limit — continuing on acct-b"));

    // Follow-ups stay on the account it moved to.
    assert!(cp(&env, &["send", &id, "and then"]).status.success());
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: and then"
    );
    let log = std::fs::read_to_string(&env.log).unwrap();
    assert!(log.lines().last().unwrap().contains(" acct-b and then"));
}

#[test]
fn a_pinned_account_does_not_move() {
    let env = setup();
    let id = stdout(&cp(&env, &["run", "--account", "acct-a", "limited job"]));
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(1));
    let s = status(&env, &id);
    assert_eq!(s["state"], "failed");
    assert_eq!(s["account"], "acct-a");
    assert!(stdout(&cp(&env, &["read", &id]))
        .contains("⇄ acct-a hit its usage limit — no other account to continue on"));
    // Moving it by hand works.
    assert!(cp(&env, &["send", &id, "--account", "acct-b", "go on"])
        .status
        .success());
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: go on"
    );
    assert_eq!(status(&env, &id)["account"], "acct-b");
}

#[test]
fn wait_after_send_reports_the_new_turn_even_under_status_probes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let env = Arc::new(setup());
    let id = stdout(&cp(&env, &["run", "--account", "acct-a", "limited start"]));
    // Every `status` briefly takes the run lock — the probes the drainer and
    // `wait` must not trip over.
    let stop = Arc::new(AtomicBool::new(false));
    let probers: Vec<_> = (0..3)
        .map(|_| {
            let (env, id, stop) = (Arc::clone(&env), id.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    cp(&env, &["status", &id]);
                }
            })
        })
        .collect();

    assert_eq!(
        cp(&env, &["wait", &id, "--timeout", "30"]).status.code(),
        Some(1)
    );
    for i in 0..12 {
        // Alternate a turn that fails (pinned, no failover) with one that
        // succeeds, so most sends land on a failed run.
        let fails = i % 2 == 1;
        let msg = if fails {
            format!("limited {i}")
        } else {
            format!("ok {i}")
        };
        assert!(cp(&env, &["send", &id, &msg]).status.success());
        let out = cp(&env, &["wait", &id, "--timeout", "30"]);
        if fails {
            assert_eq!(out.status.code(), Some(1), "step {i}");
            assert_eq!(stdout(&out), "usage limit reached", "step {i}");
        } else {
            assert_eq!(out.status.code(), Some(0), "step {i}");
            assert_eq!(stdout(&out), format!("echo: {msg}"), "step {i}");
        }
    }

    stop.store(true, Ordering::Relaxed);
    for p in probers {
        p.join().unwrap();
    }
}
