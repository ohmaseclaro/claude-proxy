//! End-to-end: managed runs against a fake `claude` on PATH, in a throwaway
//! HOME. Exercises the real binary — background drainer, resume flags, the
//! transcript, status, kill, and resume-after-kill — with no network and no
//! real accounts.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Speaks Claude's stream-json: reads one user message per stdin line, and
/// for each echoes it back (`isReplay`), plays one turn, and records it in the
/// session file the way Claude does. Logs every message with how the process
/// was started (`new`, `resume`, `fork:<parent>`; `warm` for later messages to
/// the same process). A prompt starting with `sleep` hangs (a turn to kill);
/// `nap` takes 2 seconds (a turn to message while it works); `limited` fails
/// with a usage limit on the account `acct-a`; `ask` puts a permission prompt
/// to the run's `__permit` server the way Claude does (`ask question`: a
/// question, `ask plan`: plan approval) and echoes the decision.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
echo "run=${CLAUDE_PROXY_RUN:-} $*" >> "$FAKE_ARGS_LOG"
sid=""; mode=""; fork=""; cfg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --session-id) mode="new"; sid="$2"; shift ;;
    --resume) mode="resume"; sid="$2"; shift ;;
    --fork-session) fork="$sid" ;;
    --mcp-config=*) cfg="${1#--mcp-config=}" ;;
  esac
  shift
done
[ -n "$fork" ] && mode="fork:$fork"
acct=$(basename "${CLAUDE_CONFIG_DIR:-primary}")
while IFS= read -r line; do
  prompt=$(printf '%s\n' "$line" | sed 's/.*"content":"\(.*\)","role":"user".*/\1/')
  echo "$mode $sid $acct $prompt" >> "$FAKE_LOG"
  mode="warm"
  mkdir -p "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/fake"
  echo "$prompt" >> "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/fake/$sid.jsonl"
  echo '{"type":"system","subtype":"init","session_id":"'"$sid"'"}'
  echo '{"type":"user","isReplay":true,"message":{"role":"user","content":"'"$prompt"'"}}'
  case "$prompt" in
    sleep*) echo '{"type":"system","subtype":"task_summary","detail":"sleeping"}'; sleep 30 ;;
    nap*) echo '{"type":"system","subtype":"task_summary","detail":"napping"}'; sleep 2 ;;
    limited*)
      if [ "$acct" = "acct-a" ]; then
        echo '{"type":"assistant","message":{"content":[{"type":"text","text":"usage limit reached"}]},"error":"rate_limit"}'
        echo '{"type":"result","subtype":"success","is_error":true,"result":"usage limit reached"}'
        exit 1
      fi ;;
    ask*)
      cmd=$(sed 's/.*"command":"\([^"]*\)".*/\1/' "$cfg")
      id=$(sed 's/.*"__permit","\([^"]*\)".*/\1/' "$cfg")
      tool=Bash; input='{"command":"rm -rf build"}'
      case "$prompt" in
        *question*) tool=AskUserQuestion
          input='{"questions":[{"question":"Which colour?","options":[{"label":"Red"},{"label":"Blue"}]}]}' ;;
        *plan*) tool=ExitPlanMode; input='{"plan":"do it"}' ;;
      esac
      prompt=$(printf '%s\n' \
        '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
        '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
        '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"permit","arguments":{"tool_name":"'"$tool"'","input":'"$input"',"tool_use_id":"t1"}}}' \
        | "$cmd" __permit "$id" | tail -n 1 | sed 's/.*"text":"\(.*\)","type":"text".*/\1/') ;;
  esac
  echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"echo hi"}}]}}'
  echo '{"type":"user","message":{"content":[{"type":"tool_result","content":"hi","is_error":false}]}}'
  echo '{"type":"assistant","message":{"content":[{"type":"text","text":"echo: '"$prompt"'"}]}}'
  echo '{"type":"system","subtype":"post_turn_summary","status_detail":"echoed '"$prompt"'","needs_action":""}'
  echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":5,"total_cost_usd":0.01,"result":"echo: '"$prompt"'"}'
done
"#;

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    path: String,
    log: PathBuf,
    args_log: PathBuf,
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
        args_log: tmp.path().join("args.log"),
        home,
        bin,
        _tmp: tmp,
    }
}

fn cp(env: &Env, args: &[&str]) -> Output {
    run_as(
        env,
        Path::new(env!("CARGO_BIN_EXE_claude-proxy")),
        args,
        &[],
    )
}

fn run_as(env: &Env, exe: &Path, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", env.home.join(".config"))
        .env("PATH", &env.path)
        .env("FAKE_LOG", &env.log)
        .env("FAKE_ARGS_LOG", &env.args_log)
        // Each turn's drainer exits at once unless a test asks for a warm one.
        .env("CLAUDE_PROXY_IDLE_SECS", "0")
        .current_dir(&env.home);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
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

#[test]
fn a_permission_prompt_waits_for_allow_or_deny() {
    let env = setup();
    let id = stdout(&cp(&env, &["run", "--account", "claude", "ask first"]));

    // `wait` returns as soon as the run needs a decision, and prints it.
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let asked = stdout(&out);
    assert!(
        asked.contains("? permission to use Bash  rm -rf build"),
        "{asked}"
    );
    assert!(
        asked.contains(&format!("claude-proxy allow {id}")),
        "{asked}"
    );
    let s = status(&env, &id);
    assert_eq!(s["state"], "waiting");
    assert_eq!(s["asks"][0]["tool"], "Bash");
    assert_eq!(s["asks"][0]["input"]["command"], "rm -rf build");

    assert!(cp(&env, &["allow", &id]).status.success());
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out),
        r#"echo: {"behavior":"allow","updatedInput":{"command":"rm -rf build"}}"#
    );
    // Nothing left to answer.
    assert!(!cp(&env, &["allow", &id]).status.success());

    assert!(cp(&env, &["send", &id, "ask again"]).status.success());
    assert_eq!(
        cp(&env, &["wait", &id, "--timeout", "30"]).status.code(),
        Some(2)
    );
    assert!(cp(&env, &["deny", &id, "not in this repo"])
        .status
        .success());
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        r#"echo: {"behavior":"deny","message":"not in this repo"}"#
    );

    let text = stdout(&cp(&env, &["read", &id]));
    for needle in [
        "? permission to use Bash  rm -rf build",
        "  ↪ allowed",
        "  ↪ denied: not in this repo",
    ] {
        assert!(
            text.contains(needle),
            "transcript lacks {needle:?}:\n{text}"
        );
    }
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    assert!(
        args.contains(&format!("run={id} ")),
        "the run's id reaches Claude's environment: {args}"
    );
    assert!(args.contains("--permission-prompt-tool mcp__claude-proxy__permit"));
}

#[test]
fn a_question_is_answered_with_answer() {
    let env = setup();
    let id = stdout(&cp(&env, &["run", "--account", "claude", "ask question"]));
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).contains("? Which colour?  [Red | Blue]"));
    // One answer per question.
    assert!(!cp(&env, &["answer", &id, "a", "b"]).status.success());
    assert!(cp(&env, &["answer", &id, "blue"]).status.success());
    let out = cp(&env, &["wait", &id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains(r#""answers":{"Which colour?":"blue"}"#),
        "{}",
        stdout(&out)
    );
    assert!(stdout(&cp(&env, &["read", &id])).contains("  ↪ Which colour? → blue"));
}

#[test]
fn a_fork_starts_from_a_copy_of_another_session() {
    let env = setup();
    let parent = "11111111-2222-4333-8444-555555555555";
    let projects = env.home.join(".claude/projects/elsewhere");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join(format!("{parent}.jsonl")), "{}\n").unwrap();

    // With no id, the Claude session it runs inside.
    let out = run_as(
        &env,
        Path::new(env!("CARGO_BIN_EXE_claude-proxy")),
        &["run", "--account", "claude", "--fork", "go"],
        &[("CLAUDE_CODE_SESSION_ID", parent)],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = stdout(&out);
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: go"
    );
    assert!(cp(&env, &["send", &id, "more"]).status.success());
    cp(&env, &["wait", &id, "--timeout", "30"]);

    let log = std::fs::read_to_string(&env.log).unwrap();
    let calls: Vec<Vec<&str>> = log.lines().map(|l| l.splitn(4, ' ').collect()).collect();
    assert_eq!(calls[0][0], format!("fork:{parent}"));
    assert_ne!(calls[0][1], parent, "the fork gets its own session id");
    assert_eq!(calls[1][..2], ["resume", calls[0][1]]);
    assert_eq!(status(&env, &id)["fork_from"], parent);

    assert!(!cp(&env, &["run", "--fork-from", "nope", "go"])
        .status
        .success());
}

#[test]
fn a_worktree_run_works_on_its_own_branch_and_is_cleaned_up() {
    let env = setup();
    let repo = env.home.join("repo");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("sub/file.txt"), "x").unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .current_dir(&repo)
            .env("HOME", &env.home)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "init"]);
    let sub = repo.join("sub");
    let start = || {
        stdout(&cp(
            &env,
            &[
                "run",
                "--account",
                "claude",
                "--worktree",
                "--cwd",
                sub.to_str().unwrap(),
                "hi",
            ],
        ))
    };

    let id = start();
    assert_eq!(id.len(), 6, "{id}");
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let s = status(&env, &id);
    let tree = PathBuf::from(s["worktree"]["path"].as_str().unwrap());
    assert!(s["cwd"].as_str().unwrap().ends_with("/sub"), "{s}");
    assert!(tree.join("sub/file.txt").exists());
    assert_eq!(s["worktree"]["branch"], format!("claude-proxy/{id}"));

    assert!(cp(&env, &["rm", &id]).status.success());
    assert!(!tree.exists());
    assert!(git(&["branch", "--list", &format!("claude-proxy/{id}")]).is_empty());

    // Uncommitted work is never thrown away.
    let id = start();
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let tree = PathBuf::from(status(&env, &id)["worktree"]["path"].as_str().unwrap());
    std::fs::write(tree.join("new.txt"), "work").unwrap();
    let out = cp(&env, &["rm", &id]);
    assert!(
        stdout(&out).contains("Kept its worktree"),
        "{}",
        stdout(&out)
    );
    assert!(tree.join("new.txt").exists());
}

#[test]
fn profiles_get_the_users_mcp_servers() {
    let env = setup();
    std::fs::write(
        env.home.join(".claude.json"),
        r#"{"mcpServers":{"demo":{"command":"demo-server"}}}"#,
    )
    .unwrap();
    let config_of = |id: &str| -> (Value, u32) {
        let path = env
            .home
            .join(format!(".config/claude-proxy/runs/{id}/mcp.json"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        (
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap(),
            mode & 0o777,
        )
    };

    let id = stdout(&cp(&env, &["run", "--account", "acct-a", "hello"]));
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let (config, mode) = config_of(&id);
    assert_eq!(config["mcpServers"]["demo"]["command"], "demo-server");
    assert_eq!(config["mcpServers"]["claude-proxy"]["args"][0], "__permit");
    assert_eq!(mode, 0o600, "server entries can carry API keys");

    // The primary profile has them already.
    let id = stdout(&cp(&env, &["run", "--account", "claude", "hello"]));
    cp(&env, &["wait", &id, "--timeout", "30"]);
    assert!(config_of(&id).0["mcpServers"].get("demo").is_none());

    // An account's own command gets them too, ahead of its arguments.
    let proxy = env.bin.join("acct-a");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_claude-proxy"), &proxy).unwrap();
    assert!(run_as(&env, &proxy, &["mcp", "list"], &[]).status.success());
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    let last = args.lines().last().unwrap();
    assert!(last.starts_with("run= --mcp-config="), "{last}");
    assert!(last.ends_with(" mcp list"), "{last}");
}

#[test]
fn a_warm_claude_takes_follow_ups_without_restarting() {
    let env = setup();
    let warm = [("CLAUDE_PROXY_IDLE_SECS", "60")];
    let exe = Path::new(env!("CARGO_BIN_EXE_claude-proxy"));
    let id = stdout(&run_as(
        &env,
        exe,
        &["run", "--account", "claude", "hello"],
        &warm,
    ));
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: hello"
    );
    // Idle, yet still holding its Claude: `wait` returned all the same.
    assert_eq!(status(&env, &id)["state"], "idle");

    assert!(cp(&env, &["send", &id, "again"]).status.success());
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: again"
    );
    let log = std::fs::read_to_string(&env.log).unwrap();
    let modes: Vec<&str> = log.lines().map(|l| l.split(' ').next().unwrap()).collect();
    assert_eq!(modes, ["new", "warm"], "one process for both messages");

    // `kill` on an idle run only lets go of its Claude.
    let out = cp(&env, &["kill", &id]);
    assert!(stdout(&out).contains("is not running"), "{}", stdout(&out));
    assert_eq!(status(&env, &id)["state"], "idle");
    assert!(cp(&env, &["send", &id, "later"]).status.success());
    assert_eq!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])),
        "echo: later"
    );
    let log = std::fs::read_to_string(&env.log).unwrap();
    assert!(log.lines().last().unwrap().starts_with("resume "), "{log}");
    assert!(
        cp(&env, &["rm", &id]).status.success(),
        "rm lets go of a warm run"
    );
}

#[test]
fn always_and_accept_edits_outlive_the_request() {
    let env = setup();
    let id = stdout(&cp(&env, &["run", "--account", "claude", "ask first"]));
    assert_eq!(
        cp(&env, &["wait", &id, "--timeout", "30"]).status.code(),
        Some(2)
    );
    assert!(cp(&env, &["allow", &id, "--always"]).status.success());
    let out = stdout(&cp(&env, &["wait", &id, "--timeout", "30"]));
    assert!(
        out.contains(r#""updatedPermissions":[{"behavior":"allow","destination":"session","rules":[{"ruleContent":"rm -rf build","toolName":"Bash"}],"type":"addRules"}]"#),
        "{out}"
    );
    assert!(
        stdout(&cp(&env, &["read", &id])).contains("↪ allowed · from now on: Bash(rm -rf build)")
    );
    // A later Claude process starts with the rule.
    assert!(cp(&env, &["send", &id, "hi"]).status.success());
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    assert!(
        args.lines()
            .last()
            .unwrap()
            .contains("--allowedTools Bash(rm -rf build) --permission-prompt-tool"),
        "{args}"
    );

    // Approving a plan: later processes accept edits instead of planning again.
    let id = stdout(&cp(
        &env,
        &[
            "run",
            "--account",
            "claude",
            "ask plan",
            "--",
            "--permission-mode",
            "plan",
        ],
    ));
    assert_eq!(
        cp(&env, &["wait", &id, "--timeout", "30"]).status.code(),
        Some(2)
    );
    assert!(cp(&env, &["allow", &id, "--accept-edits"]).status.success());
    assert!(
        stdout(&cp(&env, &["wait", &id, "--timeout", "30"])).contains(r#""mode":"acceptEdits""#)
    );
    assert!(cp(&env, &["send", &id, "go"]).status.success());
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    assert!(
        args.lines()
            .last()
            .unwrap()
            .ends_with("--permission-mode plan --permission-mode acceptEdits"),
        "{args}"
    );
}

#[test]
fn a_run_inherits_the_permission_mode_of_the_session_starting_it() {
    use std::io::Write;
    let env = setup();
    let exe = Path::new(env!("CARGO_BIN_EXE_claude-proxy"));
    let session = "11111111-2222-4333-8444-555555555555";
    let mut hook = Command::new(exe)
        .args(["__hook", "prompt"])
        .env_clear()
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", env.home.join(".config"))
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    hook.stdin
        .take()
        .unwrap()
        .write_all(
            format!(r#"{{"session_id":"{session}","permission_mode":"acceptEdits"}}"#).as_bytes(),
        )
        .unwrap();
    assert!(hook.wait().unwrap().success());

    let vars = [("CLAUDE_CODE_SESSION_ID", session)];
    let id = stdout(&run_as(
        &env,
        exe,
        &["run", "--account", "claude", "hi"],
        &vars,
    ));
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    assert!(
        args.lines()
            .last()
            .unwrap()
            .ends_with("--permission-mode acceptEdits"),
        "{args}"
    );

    // An explicit mode wins.
    let id = stdout(&run_as(
        &env,
        exe,
        &[
            "run",
            "--account",
            "claude",
            "hi",
            "--",
            "--permission-mode",
            "plan",
        ],
        &vars,
    ));
    cp(&env, &["wait", &id, "--timeout", "30"]);
    let args = std::fs::read_to_string(&env.args_log).unwrap();
    let last = args.lines().last().unwrap();
    assert!(
        last.ends_with("--permission-mode plan") && !last.contains("acceptEdits"),
        "{last}"
    );
}

#[test]
fn the_hook_turns_subagent_calls_into_runs() {
    use std::io::Write;
    let env = setup();
    let hook = |vars: &[(&str, &str)], description: &str| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_claude-proxy"));
        cmd.args(["__hook", "pre-tool-use"])
            .env_clear()
            .env("HOME", &env.home)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let input = format!(
            r#"{{"tool_name":"Agent","tool_input":{{"subagent_type":"gsd-executor","description":"{description}","prompt":"p"}}}}"#
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let out = hook(&[], "fix it");
    assert_eq!(out.status.code(), Some(2), "blocked");
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("claude-proxy run --agent gsd-executor --name 'fix it' -"));
    assert_eq!(hook(&[], "[direct] fix it").status.code(), Some(0));
    assert_eq!(
        hook(&[("CLAUDE_PROXY_RUN", "abc123")], "fix it")
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        hook(&[("CLAUDE_PROXY_POLICY", "off")], "fix it")
            .status
            .code(),
        Some(0)
    );
}
