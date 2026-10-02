//! The manager: `claude-proxy <subcommand>`.
//!
//! This is the role the binary takes when invoked under its own name. `add`
//! creates an account's isolated profile and signs it in, `list` shows what
//! exists, `remove` uninstalls a command. Running an account is the *other* role
//! (`proxy`), reached by invoking the installed command.

use std::io::IsTerminal;

use clap::{Parser, Subcommand};

use crate::install;
use crate::paths::account_config_dir;
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
    /// List the configured proxies.
    List,
    /// Remove a proxy's command (its profile and transcripts are kept).
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
        Command::List => list_cmd(),
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

fn list_cmd() -> Result<i32, String> {
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
        println!("{line}");
    }
    Ok(0)
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
        "Its profile (login + transcripts) is kept at:\n  {}",
        account_config_dir(label).display()
    );
    println!("Delete that directory yourself if you want it gone.");
    Ok(0)
}
