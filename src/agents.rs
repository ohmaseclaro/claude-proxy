//! The commands that manage background runs: start, list, inspect, read and
//! follow, message, wait for, kill, and delete. The run engine is `runs`; this
//! is the presentation on top of it.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use crate::quota::{now_secs, short_duration, PRIMARY_LABEL};
use crate::render::render_line;
use crate::runs::{self, Meta, NewRun, State, WaitError};

pub fn run(
    message: &str,
    account: &str,
    name: Option<String>,
    cwd: Option<PathBuf>,
    wait: bool,
    as_json: bool,
    claude_args: Vec<String>,
) -> Result<i32, String> {
    let message = read_message(message)?;
    let account = crate::cli::resolve_account(account)?;
    let cwd = match cwd {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?,
    };
    let cwd = cwd
        .canonicalize()
        .map_err(|e| format!("bad --cwd {}: {e}", cwd.display()))?;
    let meta = runs::start(NewRun {
        account,
        name,
        cwd: cwd.to_string_lossy().into_owned(),
        claude_args,
        message,
    })?;
    eprintln!(
        "claude-proxy: run {} on {} — `claude-proxy watch {}` to follow",
        meta.id, meta.account, meta.id
    );
    if as_json {
        println!("{}", status_json(&meta));
    } else if !wait {
        println!("{}", meta.id);
    }
    if wait {
        return wait_and_print(&meta.id, None);
    }
    Ok(0)
}

pub fn runs(as_json: bool) -> Result<i32, String> {
    let all = runs::list();
    if as_json {
        let rows: Vec<_> = all.iter().map(status_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return Ok(0);
    }
    if all.is_empty() {
        println!("No runs yet. Start one with:  claude-proxy run \"<task>\"");
        return Ok(0);
    }
    let now = now_secs();
    let account_w = all
        .iter()
        .map(|m| m.account.len())
        .max()
        .unwrap_or(7)
        .max(7);
    println!(
        "{:<6}  {:<7}  {:<account_w$}  {:>5}  {:>9}  WHAT",
        "ID", "STATE", "ACCOUNT", "TURNS", "ACTIVE"
    );
    for m in &all {
        let what = m
            .activity
            .clone()
            .or_else(|| m.summary.clone())
            .or_else(|| m.name.clone())
            .unwrap_or_default();
        println!(
            "{:<6}  {:<7}  {:<account_w$}  {:>5}  {:>9}  {}",
            m.id,
            runs::effective_state(m).as_str(),
            m.account,
            m.turns,
            format!("{} ago", short_duration(now - m.updated_at)),
            truncate(&what, 70),
        );
    }
    Ok(0)
}

pub fn status(id: &str, as_json: bool) -> Result<i32, String> {
    let m = runs::load(id)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status_json(&m)).unwrap_or_default()
        );
        return Ok(0);
    }
    let now = now_secs();
    let state = runs::effective_state(&m);
    let title = match &m.name {
        Some(name) => format!("run {} · {name}", m.id),
        None => format!("run {}", m.id),
    };
    println!("{title}");
    let mut state_line = format!("{} · turn {}", state.as_str(), m.turns);
    if state == State::Working {
        if let Some(t) = m.turn_started_at {
            state_line.push_str(&format!(" · {}", short_duration(now - t)));
        }
    } else {
        state_line.push_str(&format!(" · {} ago", short_duration(now - m.updated_at)));
    }
    let row = |k: &str, v: &str| println!("  {k:<8} {v}");
    row("state", &state_line);
    row("account", &m.account);
    if let Some(a) = &m.activity {
        row("doing", a);
    }
    if let Some(s) = &m.summary {
        row("last", s);
    }
    if let Some(n) = &m.needs_action {
        row("needs", n);
    }
    if let Some((text, ok)) = runs::last_result(id) {
        let label = if ok { "result" } else { "error" };
        row(label, &truncate(text.lines().next().unwrap_or(""), 100));
    }
    let queued = runs::queued(&runs::run_dir(id));
    if queued > 0 {
        row("queued", &format!("{queued} message(s)"));
    }
    row("cost", &format!("${:.2}", m.cost_usd));
    row("cwd", &m.cwd);
    let resume = if m.account == PRIMARY_LABEL {
        "claude".to_string()
    } else {
        m.account.clone()
    };
    row(
        "session",
        &format!(
            "{}  (take over when idle: cd {} && {resume} --resume {})",
            m.session_id, m.cwd, m.session_id
        ),
    );
    Ok(0)
}

fn status_json(m: &Meta) -> serde_json::Value {
    let dir = runs::run_dir(&m.id);
    json!({
        "id": m.id,
        "name": m.name,
        "state": runs::effective_state(m).as_str(),
        "account": m.account,
        "turns": m.turns,
        "activity": m.activity,
        "summary": m.summary,
        "needs_action": m.needs_action,
        "last_result": runs::last_result(&m.id).map(|(text, ok)| json!({"text": text, "ok": ok})),
        "queued": runs::queued(&dir),
        "working": runs::alive(&dir),
        "cost_usd": m.cost_usd,
        "cwd": m.cwd,
        "session_id": m.session_id,
        "created_at": m.created_at,
        "updated_at": m.updated_at,
        "turn_started_at": m.turn_started_at,
        "transcript": runs::events_path(&m.id),
    })
}

pub fn read(
    id: &str,
    lines: Option<usize>,
    follow: bool,
    full: bool,
    as_json: bool,
) -> Result<i32, String> {
    runs::load(id)?;
    let path = runs::events_path(id);
    let render = |line: &str| -> Vec<String> {
        if as_json {
            vec![line.to_string()]
        } else {
            render_line(line, full)
        }
    };

    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let complete = content.rfind('\n').map_or(0, |i| i + 1);
    let blocks: Vec<Vec<String>> = content[..complete]
        .lines()
        .map(render)
        .filter(|b| !b.is_empty())
        .collect();
    let skip = lines.map_or(0, |n| blocks.len().saturating_sub(n));
    for block in &blocks[skip..] {
        for line in block {
            println!("{line}");
        }
    }
    if !follow {
        return Ok(0);
    }

    let mut offset = complete as u64;
    let mut pending = String::new();
    loop {
        // Decide before reading, so the last lines are printed after it ends.
        let done = runs::finished(id)?;
        if let Ok(mut f) = std::fs::File::open(&path) {
            let mut fresh = String::new();
            if f.seek(SeekFrom::Start(offset)).is_ok() && f.read_to_string(&mut fresh).is_ok() {
                offset += fresh.len() as u64;
                pending.push_str(&fresh);
            }
        }
        while let Some(i) = pending.find('\n') {
            let line: String = pending.drain(..=i).collect();
            for out in render(line.trim_end()) {
                println!("{out}");
            }
        }
        if done {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    if !as_json {
        let m = runs::load(id)?;
        eprintln!(
            "— run {} is {} (`claude-proxy send {} \"…\"` to continue)",
            id,
            runs::effective_state(&m).as_str(),
            id
        );
    }
    Ok(0)
}

pub fn result(id: &str) -> Result<i32, String> {
    runs::load(id)?;
    match runs::last_result(id) {
        Some((text, ok)) => {
            println!("{text}");
            Ok(if ok { 0 } else { 1 })
        }
        None => Err(format!(
            "run {id} has no result for its last turn yet. See:  claude-proxy status {id}"
        )),
    }
}

pub fn send(id: &str, message: &str, account: Option<&str>) -> Result<i32, String> {
    let message = read_message(message)?;
    let account = account.map(crate::cli::resolve_account).transpose()?;
    runs::send(id, &message, account.as_deref())?;
    let to = account.map(|a| format!(" on {a}")).unwrap_or_default();
    eprintln!("claude-proxy: queued for run {id}{to} — `claude-proxy watch {id}` to follow");
    Ok(0)
}

pub fn wait(id: &str, timeout: Option<u64>) -> Result<i32, String> {
    wait_and_print(id, timeout.map(Duration::from_secs))
}

fn wait_and_print(id: &str, timeout: Option<Duration>) -> Result<i32, String> {
    match runs::wait(id, timeout) {
        Ok(m) => {
            let state = runs::effective_state(&m);
            if let Some((text, _)) = runs::last_result(id) {
                println!("{text}");
            }
            eprintln!("claude-proxy: run {id} is {}", state.as_str());
            Ok(if state == State::Idle { 0 } else { 1 })
        }
        Err(WaitError::Timeout(m)) => {
            eprintln!(
                "claude-proxy: run {id} is still {} (timed out)",
                runs::effective_state(&m).as_str()
            );
            Ok(124)
        }
        Err(WaitError::Other(e)) => Err(e),
    }
}

pub fn kill(id: &str) -> Result<i32, String> {
    if runs::kill(id)? {
        println!(
            "Killed run {id}. Queued messages are kept; `claude-proxy send {id} \"…\"` resumes it."
        );
    } else {
        println!("Run {id} is not running.");
    }
    Ok(0)
}

pub fn rm(id: &str) -> Result<i32, String> {
    runs::remove(id)?;
    println!("Removed run {id}.");
    Ok(0)
}

fn read_message(message: &str) -> Result<String, String> {
    let text = if message == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("could not read the message from stdin: {e}"))?;
        buf
    } else {
        message.to_string()
    };
    if text.trim().is_empty() {
        return Err("the message is empty".into());
    }
    Ok(text)
}

fn truncate(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}
