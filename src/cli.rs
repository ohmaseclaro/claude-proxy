//! The manager: `claude-proxy <subcommand>`.
//!
//! This is the role the binary takes when invoked under its own name. It owns
//! the three lifecycle verbs — `add` logs an account in and installs its
//! command, `list` shows what exists (optionally with live usage), `remove`
//! tears one down. Running an account is the *other* role (`proxy`), reached by
//! invoking the installed command, not through here.

use std::io::IsTerminal;

use clap::{Parser, Subcommand};

use crate::install;
use crate::quota;
use crate::registry::Registry;
use crate::store::{default_store, Store};

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
    /// Log in an account and install it as a command (e.g. `claude-gmail`).
    Add {
        /// The command name to create.
        label: String,
    },
    /// List the configured proxies.
    List {
        /// Also fetch each account's remaining usage from Anthropic.
        #[arg(long)]
        quota: bool,
    },
    /// Remove a proxy: delete its stored token and its installed command.
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
    let store = default_store();
    match dispatch(cli.command, store.as_ref()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("claude-proxy: {message}");
            1
        }
    }
}

fn dispatch(command: Command, store: &dyn Store) -> Result<i32, String> {
    match command {
        Command::Add { label } => add_cmd(store, &label),
        Command::List { quota } => list_cmd(store, quota),
        Command::Remove { label } => remove_cmd(store, &label),
    }
}

fn add_cmd(store: &dyn Store, label: &str) -> Result<i32, String> {
    let self_exe = std::env::current_exe()
        .map_err(|e| format!("could not locate this binary to install: {e}"))?;
    let bin_dir = install::default_bin_dir();
    let interactive = std::io::stdin().is_terminal();

    let added = crate::add::run(store, &self_exe, &bin_dir, label, interactive)?;

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

fn list_cmd(store: &dyn Store, with_quota: bool) -> Result<i32, String> {
    let registry = Registry::load();
    if registry.labels.is_empty() {
        println!("No proxies yet. Create one with:  claude-proxy add <name>");
        return Ok(0);
    }
    let bin_dir = install::default_bin_dir();
    for label in &registry.labels {
        let mut line = format!("  {label}");
        if !bin_dir.join(label).exists() {
            line.push_str("  (command missing — run `claude-proxy add` to reinstall)");
        }
        if with_quota {
            let note = match store.get(label) {
                Ok(Some(token)) => quota::fetch(&token).summary(),
                Ok(None) => "no token stored".to_string(),
                Err(e) => format!("token unreadable: {e}"),
            };
            line.push_str(&format!("  [{note}]"));
        }
        println!("{line}");
    }
    Ok(0)
}

fn remove_cmd(store: &dyn Store, label: &str) -> Result<i32, String> {
    let mut registry = Registry::load();
    if !registry.has(label) {
        return Err(format!("no proxy named {label:?}. See:  claude-proxy list"));
    }
    let bin_dir = install::default_bin_dir();
    install::uninstall(&bin_dir, label)
        .map_err(|e| format!("could not remove the {label:?} command: {e}"))?;
    store
        .delete(label)
        .map_err(|e| format!("could not delete the stored token: {e}"))?;
    registry.remove(label);
    registry
        .save()
        .map_err(|e| format!("could not update the registry: {e}"))?;
    println!("Removed {label}: command and stored token deleted.");
    Ok(0)
}
