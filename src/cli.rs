//! The manager: `claude-proxy <subcommand>`.
//!
//! This is the role the binary takes when invoked under its own name. `add`
//! creates an account's isolated profile and signs it in, `list` shows every
//! account with its remaining quota, `auto` runs Claude on whichever account has
//! the most room, `remove` uninstalls a command. Running one specific account is
//! the *other* role (`proxy`), reached by invoking its installed command.

use std::ffi::OsString;
use std::io::IsTerminal;

use clap::{Parser, Subcommand};
use serde_json::json;

use crate::install;
use crate::paths::account_config_dir;
use crate::quota::{self, Report};
use crate::registry::Registry;

#[derive(Parser)]
#[command(
    name = "claude-proxy",
    version,
    about = "Run Claude Code under multiple accounts, each its own command."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create an isolated profile, sign it in, and install it as a command.
    Add {
        /// The command name to create (e.g. `claude-gmail`).
        label: String,
    },
    /// List every account with its remaining quota (cached for 15 minutes).
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Ignore the cache and fetch fresh figures.
        #[arg(long)]
        refresh: bool,
    },
    /// Run `claude` on the account with the most quota left. Every argument is
    /// forwarded unchanged, e.g. `claude-proxy auto -p "fix the test"`.
    #[command(disable_help_flag = true)]
    Auto {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Remove a proxy's command (its profile and login are kept).
    Remove {
        /// The command name to remove.
        label: String,
    },
}

/// Entry point for the manager role. Returns a process exit code.
pub fn main() -> i32 {
    // `parse` exits the process itself on `--help`, `--version`, or a usage
    // error, so anything past here is a real subcommand.
    let cli = Cli::parse();
    match dispatch(cli.command) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("claude-proxy: {message}");
            1
        }
    }
}

fn dispatch(command: Command) -> Result<i32, String> {
    match command {
        Command::Add { label } => add_cmd(&label),
        Command::List { json, refresh } => list_cmd(json, refresh),
        Command::Auto { args } => auto_cmd(&args),
        Command::Remove { label } => remove_cmd(&label),
    }
}

fn add_cmd(label: &str) -> Result<i32, String> {
    let self_exe = std::env::current_exe()
        .map_err(|e| format!("could not locate this binary to install: {e}"))?;
    let bin_dir = install::default_bin_dir();
    let interactive = std::io::stdin().is_terminal();

    let added = crate::add::run(&self_exe, &bin_dir, label, interactive)?;

    println!("\n✓ {} is ready.", added.label);
    println!("  installed: {}", added.bin_path.display());
    println!("  run it:    {} [any claude arguments]", added.label);
    if !added.on_path {
        println!(
            "\nNote: {} is not on your PATH. Add it to use the command directly:",
            bin_dir.display()
        );
        println!("  export PATH=\"{}:$PATH\"", bin_dir.display());
    }
    Ok(0)
}

fn list_cmd(as_json: bool, refresh: bool) -> Result<i32, String> {
    let reports = quota::reports(&quota::accounts(), refresh);
    let now = quota::now_secs();
    let pick = quota::choose(&reports, now);

    if as_json {
        let accounts: Vec<_> = reports
            .iter()
            .map(|r| {
                json!({
                    "label": r.label,
                    "primary": r.primary,
                    "email": r.email,
                    "logged_in": r.logged_in,
                    "pressure": r.pressure(now),
                    "windows": r.windows.iter().map(|w| json!({
                        "name": w.name,
                        "used": w.used_at(now),
                        "resets_at": w.resets_at,
                    })).collect::<Vec<_>>(),
                    "fetched_at": r.fetched_at,
                    "error": r.error,
                })
            })
            .collect();
        let out = json!({
            "pick": pick.map(|i| reports[i].label.clone()),
            "cache_ttl_secs": quota::CACHE_TTL_SECS,
            "accounts": accounts,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(0);
    }

    let names: Vec<String> = reports.iter().map(display_name).collect();
    let name_w = names.iter().map(|n| n.chars().count()).max().unwrap_or(0);
    let email_w = reports
        .iter()
        .map(|r| r.email.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(0);
    let bin_dir = install::default_bin_dir();
    for (i, r) in reports.iter().enumerate() {
        let marker = if pick == Some(i) { "→" } else { " " };
        let mut line = format!(
            "{marker} {:name_w$}  {:email_w$}  {}",
            names[i],
            r.email.as_deref().unwrap_or("-"),
            quota_text(r, now),
        );
        if !r.primary && !bin_dir.join(&r.label).exists() {
            line.push_str("  (command missing — `claude-proxy add` to reinstall)");
        }
        println!("{}", line.trim_end());
    }
    println!(
        "\n→ = what `claude-proxy auto` would use · (…) = time to reset · \
         cached {} min (--refresh to update)",
        quota::CACHE_TTL_SECS / 60
    );
    Ok(0)
}

fn auto_cmd(args: &[OsString]) -> Result<i32, String> {
    let reports = quota::reports(&quota::accounts(), false);
    let now = quota::now_secs();
    let pick = quota::choose(&reports, now)
        .ok_or("no logged-in account to use. Add one with:  claude-proxy add <name>")?;
    let r = &reports[pick];
    eprintln!("claude-proxy: auto → {} ({})", r.label, quota_text(r, now));
    if r.pressure(now).is_some_and(|p| p >= 100.0) {
        eprintln!("claude-proxy: warning — every account is at a usage limit");
    }
    if r.primary {
        crate::proxy::run_primary(args)
    } else {
        crate::proxy::run(&r.label, args)
    }
}

fn display_name(r: &Report) -> String {
    if r.primary {
        format!("{} (primary)", r.label)
    } else {
        r.label.clone()
    }
}

fn quota_text(r: &Report, now: i64) -> String {
    if !r.logged_in {
        return "not logged in".into();
    }
    if r.windows.is_empty() {
        return match &r.error {
            Some(e) => format!("quota unknown — {e}"),
            None => "quota unknown".into(),
        };
    }
    let mut text = r
        .windows
        .iter()
        .map(|w| match w.resets_at.filter(|t| *t > now) {
            Some(t) => format!(
                "{} {:.0}% ({})",
                w.name,
                w.used_at(now),
                quota::short_duration(t - now)
            ),
            None => format!("{} {:.0}%", w.name, w.used_at(now)),
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if let Some(at) = r.fetched_at.filter(|at| now - at >= quota::CACHE_TTL_SECS) {
        text.push_str(&format!(" [as of {} ago]", quota::short_duration(now - at)));
    }
    if let Some(e) = &r.error {
        text.push_str(&format!(" [{e}]"));
    }
    text
}

fn remove_cmd(label: &str) -> Result<i32, String> {
    let mut registry = Registry::load();
    if !registry.has(label) {
        return Err(format!("no proxy named {label:?}. See:  claude-proxy list"));
    }
    let bin_dir = install::default_bin_dir();
    install::uninstall(&bin_dir, label)
        .map_err(|e| format!("could not remove the {label:?} command: {e}"))?;
    registry.remove(label);
    registry
        .save()
        .map_err(|e| format!("could not update the registry: {e}"))?;

    println!("Removed the {label} command.");
    println!(
        "Its profile (and login) is kept at:\n  {}",
        account_config_dir(label).display()
    );
    println!("Delete that directory yourself if you want it gone.");
    Ok(0)
}
