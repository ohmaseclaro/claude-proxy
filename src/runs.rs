//! Managed runs: a proxied Claude session that another agent drives by id —
//! start it in the background, read or follow its transcript, check its status,
//! send follow-up messages, wait for it, or kill it.
//!
//! Each run pins one Claude session id. Every message becomes one headless turn
//! (`claude -p --output-format stream-json`, the first with `--session-id`, the
//! rest with `--resume`), so the conversation carries over, and a message sent
//! while a turn is working is delivered when that turn ends. A detached drainer
//! (`claude-proxy __drain <id>`) works through the run's inbox and exits once it
//! is empty. It holds `run.lock` while alive — that lock, not a pid, is how
//! every other command knows whether the run is working.
//!
//! `~/.config/claude-proxy/runs/<id>/`: `meta.json` (written only by the
//! drainer once it starts), `events.jsonl` (the transcript: Claude's
//! stream-json plus our message/turn markers), `inbox/` (queued messages),
//! `account` (a pending account switch), `stop` (a kill in progress),
//! `pending` (a drainer was requested and has not taken the lock yet),
//! `stderr.log`, `run.lock`.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths::{account_config_dir, config_dir, home};
use crate::quota::{self, now_secs, PRIMARY_LABEL};
use crate::registry::Registry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Created or messaged; the drainer has not started the turn yet.
    Queued,
    /// A turn is running.
    Working,
    /// The last turn finished cleanly; `send` continues the conversation.
    Idle,
    /// The last turn failed; queued messages wait for the next `send`.
    Failed,
    /// Stopped by `kill`; `send` resumes it.
    Killed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Working => "working",
            State::Idle => "idle",
            State::Failed => "failed",
            State::Killed => "killed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub account: String,
    pub cwd: String,
    pub session_id: String,
    #[serde(default)]
    pub claude_args: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub turns: u32,
    pub state: State,
    #[serde(default)]
    pub turn_started_at: Option<i64>,
    #[serde(default)]
    pub turn_pid: Option<u32>,
    #[serde(default)]
    pub last_exit: Option<i32>,
    /// What the current turn is doing, from Claude's `task_summary`.
    #[serde(default)]
    pub activity: Option<String>,
    /// What the last turn did, from Claude's `post_turn_summary`.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub needs_action: Option<String>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub cost_usd: f64,
    /// Accounts the run may use, in preference order; empty is `auto` (every
    /// logged-in account, best quota first).
    #[serde(default)]
    pub pool: Vec<String>,
    /// Why the run last moved to another account.
    #[serde(default)]
    pub moved: Option<String>,
}

pub fn runs_dir() -> PathBuf {
    config_dir().join("runs")
}

pub fn run_dir(id: &str) -> PathBuf {
    runs_dir().join(id)
}

pub fn events_path(id: &str) -> PathBuf {
    run_dir(id).join("events.jsonl")
}

pub fn load(id: &str) -> Result<Meta, String> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err(format!("no run {id:?}"));
    }
    let bytes = fs::read(run_dir(id).join("meta.json"))
        .map_err(|_| format!("no run {id:?}. See:  claude-proxy runs"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("run {id:?} is unreadable: {e}"))
}

fn save(dir: &Path, meta: &mut Meta) {
    meta.updated_at = now_secs();
    let tmp = dir.join("meta.json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(meta) {
        if fs::write(&tmp, bytes).is_ok() {
            let _ = fs::rename(&tmp, dir.join("meta.json"));
        }
    }
}

/// The config dir `claude` runs with for this account (`None` is the primary).
fn account_dir(account: &str) -> Option<PathBuf> {
    (account != PRIMARY_LABEL).then(|| account_config_dir(account))
}

pub struct NewRun {
    pub account: String,
    pub pool: Vec<String>,
    pub name: Option<String>,
    pub cwd: String,
    pub claude_args: Vec<String>,
    pub message: String,
}

/// Create a run, queue its first message, and start it in the background.
pub fn start(new: NewRun) -> Result<Meta, String> {
    let (id, dir) = loop {
        let id = format!("{:06x}", random_u64() & 0xff_ffff);
        let dir = run_dir(&id);
        if !dir.exists() {
            break (id, dir);
        }
    };
    fs::create_dir_all(dir.join("inbox")).map_err(|e| format!("could not create the run: {e}"))?;
    let now = now_secs();
    let mut meta = Meta {
        id: id.clone(),
        name: new.name,
        account: new.account,
        cwd: new.cwd,
        session_id: new_uuid(),
        claude_args: new.claude_args,
        created_at: now,
        updated_at: now,
        turns: 0,
        state: State::Queued,
        turn_started_at: None,
        turn_pid: None,
        last_exit: None,
        activity: None,
        summary: None,
        needs_action: None,
        last_result: None,
        cost_usd: 0.0,
        pool: new.pool,
        moved: None,
    };
    save(&dir, &mut meta);
    enqueue(&dir, &new.message)?;
    request_drainer(&id)?;
    Ok(meta)
}

/// Queue a follow-up message — optionally moving the run to another account
/// spec (see [`parse_pool`]) — and make sure a drainer is there to deliver it.
pub fn send(id: &str, message: &str, account: Option<&str>) -> Result<(), String> {
    load(id)?;
    let dir = run_dir(id);
    if let Some(account) = account {
        fs::write(dir.join("account"), account)
            .map_err(|e| format!("could not switch the account: {e}"))?;
    }
    // A new message is how a killed run is resumed.
    let _ = fs::remove_file(dir.join("stop"));
    enqueue(&dir, message)?;
    // Always, even with a drainer alive: it may be exiting after a failure
    // without looking at the inbox again. An extra one waits on the lock and
    // finds nothing left to do.
    request_drainer(id)
}

/// Mark the run as having work about to start, then start a drainer. The mark
/// covers the moment before the drainer holds the lock, so nothing mistakes
/// the run for finished in between.
fn request_drainer(id: &str) -> Result<(), String> {
    fs::write(run_dir(id).join("pending"), "")
        .map_err(|e| format!("could not queue the run: {e}"))?;
    spawn_drainer(id)
}

fn enqueue(dir: &Path, message: &str) -> Result<(), String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let name = format!("{nanos:024}-{:016x}.txt", random_u64());
    let tmp = dir.join("inbox").join(format!(".{name}"));
    fs::write(&tmp, message)
        .and_then(|()| fs::rename(&tmp, dir.join("inbox").join(&name)))
        .map_err(|e| format!("could not queue the message: {e}"))
}

/// Pending messages, oldest first, removed from the inbox.
fn take_inbox(dir: &Path) -> Vec<String> {
    let mut names: Vec<PathBuf> = fs::read_dir(dir.join("inbox"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            !p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|p| {
            let text = fs::read_to_string(&p).ok();
            let _ = fs::remove_file(&p);
            text
        })
        .collect()
}

pub fn queued(dir: &Path) -> usize {
    fs::read_dir(dir.join("inbox"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .count()
}

fn lock_file(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("run.lock"))
}

/// Whether a drainer is alive for this run.
pub fn alive(dir: &Path) -> bool {
    match lock_file(dir) {
        Ok(f) => matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

/// Nothing running and nothing about to run.
pub fn finished(id: &str) -> Result<bool, String> {
    let meta = load(id)?;
    let dir = run_dir(id);
    // Messages left queued behind a kill do not keep it unfinished.
    Ok(!alive(&dir)
        && !pending(&dir)
        && (queued(&dir) == 0 || matches!(meta.state, State::Failed | State::Killed)))
}

fn pending(dir: &Path) -> bool {
    dir.join("pending").exists()
}

/// The state to show: a run the drainer abandoned mid-turn is failed.
pub fn effective_state(meta: &Meta) -> State {
    let dir = run_dir(&meta.id);
    if !alive(&dir) && pending(&dir) {
        return State::Queued;
    }
    match meta.state {
        State::Working if !alive(&dir) => State::Failed,
        State::Idle if queued(&dir) > 0 => State::Queued,
        s => s,
    }
}

fn spawn_drainer(id: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not locate claude-proxy: {e}"))?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir(id).join("stderr.log"))
        .map_err(|e| format!("could not open the run log: {e}"))?;
    let mut cmd = Command::new(exe);
    cmd.arg("__drain")
        .arg(id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: it survives the caller's shell and terminal.
        cmd.arg0("claude-proxy").process_group(0);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start the run: {e}"))
}

/// The drainer's main loop: one message batch at a time, until the inbox is
/// empty, a message cannot be delivered, or the run is killed.
pub fn drain(id: &str) -> Result<(), String> {
    let dir = run_dir(id);
    let lock = lock_file(&dir).map_err(|e| e.to_string())?;
    // Blocking, never `try_lock`: `alive()` probes this lock by briefly taking
    // it, and a drainer that gave up on contention could leave its message
    // queued forever. Behind another drainer, it simply runs after it.
    lock.lock().map_err(|e| e.to_string())?;
    let _ = fs::remove_file(dir.join("pending"));
    let mut meta = load(id)?;
    loop {
        if dir.join("stop").exists() {
            meta.state = State::Killed;
            save(&dir, &mut meta);
            return Ok(());
        }
        let messages = take_inbox(&dir);
        if messages.is_empty() {
            if matches!(meta.state, State::Working | State::Queued) {
                meta.state = State::Idle;
            }
            meta.activity = None;
            save(&dir, &mut meta);
            // Re-check after letting go of the lock: a message queued while we
            // were finishing would otherwise wait for the next `send`.
            let _ = lock.unlock();
            if queued(&dir) == 0 || lock.lock().is_err() {
                return Ok(());
            }
            // Another drainer may have run in between.
            meta = load(id)?;
            continue;
        }
        if let Ok(spec) = fs::read_to_string(dir.join("account")) {
            let _ = fs::remove_file(dir.join("account"));
            match parse_pool(spec.trim()).and_then(|pool| Ok((first_account(&pool)?, pool))) {
                Ok((account, pool)) => {
                    meta.account = account;
                    meta.pool = pool;
                    meta.moved = None;
                }
                Err(e) => marker(
                    &dir,
                    json!({"event": "note", "text": format!("account switch ignored: {e}")}),
                ),
            }
        }
        let text = messages.join("\n\n");
        marker(&dir, json!({"event": "message", "text": text}));
        if !deliver(&dir, &mut meta, &text) {
            return Ok(());
        }
    }
}

/// Why an account could not serve a turn — the failures moving to another
/// account can fix (overloaded or server errors are not account-specific).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    Limit,
    Auth,
}

impl Reason {
    fn describe(self) -> &'static str {
        match self {
            Reason::Limit => "hit its usage limit",
            Reason::Auth => "could not sign in",
        }
    }
}

/// What a turn reported besides its transcript.
#[derive(Default)]
struct Signals {
    result_error: bool,
    /// The typed `error` Claude puts on an assistant message wrapping an API
    /// error (`rate_limit`, `billing_error`, `authentication_failed`, …).
    api_error: Option<String>,
    /// A `rate_limit_event` with status `rejected`.
    rejected: bool,
    /// Claude printed `Not logged in` instead of starting.
    not_logged_in: bool,
}

fn classify(s: &Signals) -> Option<Reason> {
    match s.api_error.as_deref() {
        Some("rate_limit" | "billing_error") => Some(Reason::Limit),
        Some(
            "authentication_failed"
            | "oauth_org_not_allowed"
            | "account_on_hold"
            | "verification_required",
        ) => Some(Reason::Auth),
        _ if s.rejected => Some(Reason::Limit),
        _ if s.not_logged_in => Some(Reason::Auth),
        _ => None,
    }
}

enum TurnEnd {
    Done,
    Failed(Option<Reason>),
    Killed,
}

/// Deliver one message, moving the conversation to another account when the
/// current one hits a usage limit or cannot sign in. The session is shared by
/// every profile (`projects` is linked), so the new account resumes the same
/// session id — nothing is copied. Returns whether the drainer should go on.
fn deliver(dir: &Path, meta: &mut Meta, original: &str) -> bool {
    let mut tried = vec![meta.account.clone()];
    let mut text = original.to_string();
    loop {
        let before = session_len(meta);
        let reason = match run_turn(dir, meta, &text) {
            TurnEnd::Done => return true,
            TurnEnd::Killed | TurnEnd::Failed(None) => return false,
            TurnEnd::Failed(Some(reason)) => reason,
        };
        // Measured on the failed account's view, before switching.
        let reached_session = session_len(meta) > before;
        let from = meta.account.clone();
        if reason == Reason::Limit {
            quota::invalidate(&from);
        }
        let next = if dir.join("stop").exists() {
            None
        } else {
            next_account(meta, &tried)
        };
        marker(
            dir,
            json!({"event": "failover", "from": from, "to": next, "reason": reason.describe()}),
        );
        let Some(next) = next else {
            return false;
        };
        meta.moved = Some(format!("from {from}, which {}", reason.describe()));
        meta.account = next.clone();
        tried.push(next);
        text = if reached_session {
            // The message is already in the session: ask for the rest of the
            // work instead of repeating it.
            format!(
                "(claude-proxy) Your previous turn stopped because the Claude account \
                 {from} {}. This conversation now continues on another account — pick up \
                 exactly where you left off.",
                reason.describe()
            )
        } else {
            original.to_string()
        };
    }
}

/// The account to continue on, never one already tried for this message. One
/// named account is pinned; a list is tried in order; `auto` picks by quota.
fn next_account(meta: &Meta, tried: &[String]) -> Option<String> {
    match meta.pool.as_slice() {
        [_] => None,
        [] => {
            let candidates: Vec<_> = quota::accounts()
                .into_iter()
                .filter(|a| !tried.contains(&a.label))
                .collect();
            let reports = quota::reports(&candidates, false);
            quota::choose(&reports, now_secs()).map(|i| reports[i].label.clone())
        }
        pool => next_in_pool(pool, tried),
    }
}

fn next_in_pool(pool: &[String], tried: &[String]) -> Option<String> {
    pool.iter().find(|a| !tried.contains(a)).cloned()
}

/// An account spec: `auto` (empty pool), one name, or `a,b,c` in preference
/// order. `claude` is the primary profile.
pub fn parse_pool(spec: &str) -> Result<Vec<String>, String> {
    if spec.trim() == "auto" {
        return Ok(Vec::new());
    }
    let registry = Registry::load();
    let mut pool: Vec<String> = Vec::new();
    for label in spec.split(',').map(str::trim).filter(|l| !l.is_empty()) {
        if label != PRIMARY_LABEL && !registry.has(label) {
            return Err(format!(
                "no account named {label:?}. See:  claude-proxy list"
            ));
        }
        if !pool.iter().any(|p| p == label) {
            pool.push(label.to_string());
        }
    }
    if pool.is_empty() {
        return Err("no account given".into());
    }
    Ok(pool)
}

/// The account a pool starts on: its first name, or the best by quota.
pub fn first_account(pool: &[String]) -> Result<String, String> {
    if let Some(first) = pool.first() {
        return Ok(first.clone());
    }
    let reports = quota::reports(&quota::accounts(), false);
    quota::choose(&reports, now_secs())
        .map(|i| reports[i].label.clone())
        .ok_or_else(|| "no logged-in account to use. Add one with:  claude-proxy add <name>".into())
}

/// The session's transcript as the current account's Claude sees it. Searched
/// by id, so the project-directory naming does not matter.
fn session_file(meta: &Meta) -> Option<PathBuf> {
    let projects = account_dir(&meta.account)
        .unwrap_or_else(|| home().join(".claude"))
        .join("projects");
    let name = format!("{}.jsonl", meta.session_id);
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .find(|p| p.is_file())
}

fn session_len(meta: &Meta) -> u64 {
    session_file(meta)
        .and_then(|p| fs::metadata(p).ok())
        .map_or(0, |m| m.len())
}

/// One headless turn on the run's current account.
fn run_turn(dir: &Path, meta: &mut Meta, text: &str) -> TurnEnd {
    meta.turns += 1;
    meta.state = State::Working;
    meta.turn_started_at = Some(now_secs());
    meta.activity = None;
    meta.summary = None;
    meta.needs_action = None;
    save(dir, meta);
    marker(
        dir,
        json!({"event": "turn_start", "turn": meta.turns, "account": meta.account}),
    );

    // Built first: it links the shared setup, so the session is visible below.
    let mut cmd = crate::proxy::claude_command(account_dir(&meta.account).as_deref());
    let session_flag = if session_file(meta).is_some() {
        "--resume"
    } else {
        "--session-id"
    };
    cmd.args(["-p", "--output-format", "stream-json", "--verbose"])
        .arg(session_flag)
        .arg(&meta.session_id)
        .args(&meta.claude_args)
        .current_dir(&meta.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Ok(log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("stderr.log"))
    {
        cmd.stderr(log);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own group, so `kill` stops it and every tool process it started.
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let error = if e.kind() == std::io::ErrorKind::NotFound {
                "`claude` is not on PATH".to_string()
            } else {
                format!("could not start claude: {e}")
            };
            marker(
                dir,
                json!({"event": "turn_end", "turn": meta.turns, "exit": null, "error": error}),
            );
            meta.state = State::Failed;
            meta.last_result = Some(error);
            save(dir, meta);
            return TurnEnd::Failed(None);
        }
    };
    meta.turn_pid = Some(child.id());
    save(dir, meta);
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }

    let mut signals = Signals::default();
    if let (Some(out), Ok(mut events)) = (
        child.stdout.take(),
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl")),
    ) {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = writeln!(events, "{line}");
            match serde_json::from_str::<Value>(&line) {
                Ok(v) => {
                    if observe(meta, &v, &mut signals) {
                        save(dir, meta);
                    }
                }
                Err(_) if line.trim_start().starts_with("Not logged in") => {
                    signals.not_logged_in = true;
                }
                Err(_) => {}
            }
        }
    }
    let code = child.wait().ok().and_then(|s| s.code());
    let killed = dir.join("stop").exists();
    meta.turn_pid = None;
    meta.last_exit = code;
    meta.activity = None;
    marker(
        dir,
        json!({"event": "turn_end", "turn": meta.turns, "exit": code, "killed": killed}),
    );
    let end = if killed {
        TurnEnd::Killed
    } else if code != Some(0) || signals.result_error {
        TurnEnd::Failed(classify(&signals))
    } else {
        TurnEnd::Done
    };
    meta.state = match end {
        TurnEnd::Done => State::Working,
        TurnEnd::Failed(_) => State::Failed,
        TurnEnd::Killed => State::Killed,
    };
    save(dir, meta);
    end
}

/// Fold one stream-json event into the run's status and the turn's signals.
/// Returns whether the status changed.
fn observe(meta: &mut Meta, v: &Value, signals: &mut Signals) -> bool {
    let text = |key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
    };
    match v.get("type").and_then(Value::as_str) {
        Some("system") => match v.get("subtype").and_then(Value::as_str) {
            Some("task_summary") => {
                let changed = meta.activity != text("detail");
                meta.activity = text("detail");
                changed
            }
            Some("post_turn_summary") => {
                meta.summary = text("status_detail");
                meta.needs_action = text("needs_action");
                true
            }
            _ => false,
        },
        Some("assistant") => {
            if let Some(error) = text("error") {
                signals.api_error = Some(error);
            }
            false
        }
        Some("result") => {
            meta.last_result = text("result");
            meta.cost_usd += v
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            signals.result_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            true
        }
        Some("rate_limit_event") => {
            if let Some(info) = v.get("rate_limit_info") {
                quota::record_rate_limit(&meta.account, info);
                if info.get("status").and_then(Value::as_str) == Some("rejected") {
                    signals.rejected = true;
                }
            }
            false
        }
        _ => false,
    }
}

fn marker(dir: &Path, mut event: Value) {
    event["type"] = json!("claude_proxy");
    event["at"] = json!(now_secs());
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("events.jsonl"))
    {
        let _ = writeln!(f, "{event}");
    }
}

/// Stop a run: the current turn is terminated (escalating to SIGKILL) and the
/// drainer exits; queued messages are kept. Returns whether it was running.
pub fn kill(id: &str) -> Result<bool, String> {
    load(id)?;
    let dir = run_dir(id);
    if !alive(&dir) && !pending(&dir) {
        return Ok(false);
    }
    fs::write(dir.join("stop"), "").map_err(|e| format!("could not stop the run: {e}"))?;
    let started = Instant::now();
    while alive(&dir) || pending(&dir) {
        if started.elapsed() > Duration::from_secs(10) {
            return Err("the run did not stop within 10s".into());
        }
        // The pid can land in meta just after we look, so keep checking.
        if let Ok(meta) = load(id) {
            if let Some(pid) = meta.turn_pid {
                let force = started.elapsed() > Duration::from_secs(3);
                signal_group(pid, force);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(true)
}

#[cfg(unix)]
fn signal_group(pid: u32, force: bool) {
    let signal = if force { "-KILL" } else { "-TERM" };
    let _ = Command::new("kill")
        .args([signal, &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn signal_group(pid: u32, _force: bool) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub enum WaitError {
    Timeout(Box<Meta>),
    Other(String),
}

/// Block until the run has nothing running or queued.
pub fn wait(id: &str, timeout: Option<Duration>) -> Result<Meta, WaitError> {
    let started = Instant::now();
    loop {
        if finished(id).map_err(WaitError::Other)? {
            return load(id).map_err(WaitError::Other);
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            return Err(load(id).map_or_else(WaitError::Other, |m| WaitError::Timeout(Box::new(m))));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Every run, most recently active first.
pub fn list() -> Vec<Meta> {
    let mut all: Vec<Meta> = fs::read_dir(runs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| load(&e.file_name().to_string_lossy()).ok())
        .collect();
    all.sort_by_key(|m| std::cmp::Reverse(m.updated_at));
    all
}

pub fn remove(id: &str) -> Result<(), String> {
    load(id)?;
    let dir = run_dir(id);
    if alive(&dir) {
        return Err(format!(
            "run {id} is working; kill it first:  claude-proxy kill {id}"
        ));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("could not remove run {id}: {e}"))
}

/// The final text of the last turn, from the transcript, and whether it
/// succeeded. `None` when the last turn produced no result (it is still
/// working, or was killed or crashed) — never an older turn's answer.
pub fn last_result(id: &str) -> Option<(String, bool)> {
    last_result_in(&fs::read_to_string(events_path(id)).ok()?)
}

fn last_result_in(transcript: &str) -> Option<(String, bool)> {
    for line in transcript.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("result") => {
                let ok = v.get("subtype").and_then(Value::as_str) == Some("success")
                    && !v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                let text = v.get("result").and_then(Value::as_str).unwrap_or("");
                return Some((text.to_string(), ok));
            }
            Some("claude_proxy")
                if v.get("event").and_then(Value::as_str) == Some("turn_start") =>
            {
                return None;
            }
            _ => {}
        }
    }
    None
}

fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    h.finish()
}

/// A random (v4-shaped) UUID for `--session-id`.
fn new_uuid() -> String {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&random_u64().to_be_bytes());
    b[8..].copy_from_slice(&random_u64().to_be_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_is_v4_shaped_and_unique() {
        let a = new_uuid();
        let b = new_uuid();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        let parts: Vec<_> = a.split('-').map(str::len).collect();
        assert_eq!(parts, [8, 4, 4, 4, 12]);
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn inbox_is_taken_oldest_first_and_emptied() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("inbox")).unwrap();
        enqueue(t.path(), "first").unwrap();
        enqueue(t.path(), "second").unwrap();
        assert_eq!(queued(t.path()), 2);
        assert_eq!(take_inbox(t.path()), ["first", "second"]);
        assert_eq!(queued(t.path()), 0);
    }

    fn meta() -> Meta {
        Meta {
            id: "x".into(),
            name: None,
            account: "claude-test-account-that-has-no-cache".into(),
            cwd: "/".into(),
            session_id: "s".into(),
            claude_args: vec![],
            created_at: 0,
            updated_at: 0,
            turns: 1,
            state: State::Working,
            turn_started_at: None,
            turn_pid: None,
            last_exit: None,
            activity: None,
            summary: None,
            needs_action: None,
            last_result: None,
            cost_usd: 0.0,
            pool: vec![],
            moved: None,
        }
    }

    #[test]
    fn observe_tracks_activity_summary_results_and_api_errors() {
        let mut meta = meta();
        let mut sig = Signals::default();
        observe(
            &mut meta,
            &json!({"type":"system","subtype":"task_summary","detail":"Running tests"}),
            &mut sig,
        );
        assert_eq!(meta.activity.as_deref(), Some("Running tests"));
        observe(
            &mut meta,
            &json!({"type":"system","subtype":"post_turn_summary",
                    "status_detail":"fixed the parser","needs_action":""}),
            &mut sig,
        );
        assert_eq!(meta.summary.as_deref(), Some("fixed the parser"));
        assert_eq!(meta.needs_action, None);
        observe(
            &mut meta,
            &json!({"type":"assistant","message":{"content":[]},"error":"rate_limit"}),
            &mut sig,
        );
        assert_eq!(sig.api_error.as_deref(), Some("rate_limit"));
        observe(
            &mut meta,
            &json!({"type":"result","subtype":"error_max_turns","is_error":true,
                    "result":"stopped","total_cost_usd":0.5}),
            &mut sig,
        );
        assert!(sig.result_error);
        assert_eq!(meta.last_result.as_deref(), Some("stopped"));
        assert_eq!(meta.cost_usd, 0.5);
    }

    #[test]
    fn only_account_failures_trigger_a_move() {
        let with = |api: Option<&str>, rejected: bool, not_logged_in: bool| {
            classify(&Signals {
                result_error: true,
                api_error: api.map(str::to_string),
                rejected,
                not_logged_in,
            })
        };
        assert_eq!(with(Some("rate_limit"), false, false), Some(Reason::Limit));
        assert_eq!(
            with(Some("billing_error"), false, false),
            Some(Reason::Limit)
        );
        assert_eq!(
            with(Some("authentication_failed"), false, false),
            Some(Reason::Auth)
        );
        assert_eq!(with(None, true, false), Some(Reason::Limit));
        assert_eq!(with(None, false, true), Some(Reason::Auth));
        // Not the account's fault: moving would not help.
        assert_eq!(with(Some("overloaded"), false, false), None);
        assert_eq!(with(Some("server_error"), false, false), None);
        assert_eq!(with(None, false, false), None);
    }

    #[test]
    fn a_pool_is_tried_in_order_and_never_repeats() {
        let pool: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let tried = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(next_in_pool(&pool, &tried(&["a"])).as_deref(), Some("b"));
        assert_eq!(
            next_in_pool(&pool, &tried(&["a", "b"])).as_deref(),
            Some("c")
        );
        assert_eq!(next_in_pool(&pool, &tried(&["a", "b", "c"])), None);
        // One named account is pinned.
        let mut m = meta();
        m.pool = vec!["a".into()];
        assert_eq!(next_account(&m, &tried(&["a"])), None);
    }

    #[test]
    fn last_result_never_reports_an_older_turns_answer() {
        let t1 = r#"{"type":"claude_proxy","event":"turn_start","turn":1}
{"type":"result","subtype":"success","is_error":false,"result":"first"}
{"type":"claude_proxy","event":"turn_end","exit":0}"#;
        assert_eq!(last_result_in(t1), Some(("first".into(), true)));
        let t2 = format!(
            "{t1}\n{}\n{}",
            r#"{"type":"claude_proxy","event":"turn_start","turn":2}"#,
            r#"{"type":"claude_proxy","event":"turn_end","exit":null,"killed":true}"#
        );
        assert_eq!(last_result_in(&t2), None);
    }

    #[test]
    fn load_rejects_path_like_ids() {
        for bad in ["", "../x", "a/b", "a.b"] {
            assert!(load(bad).is_err(), "{bad:?}");
        }
    }
}
