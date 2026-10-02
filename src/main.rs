//! claude-proxy — run Claude Code under multiple accounts at once.
//!
//! One binary, two roles, chosen by the name it is invoked under (`argv[0]`),
//! the busybox pattern:
//!
//! - invoked as **`claude-proxy`**: the manager (`add`, `list`, `remove`,
//!   `default`);
//! - invoked as **any other name** (e.g. `claude-gmail`, installed by `add`):
//!   the proxy — inject that account's token and become `claude`.
//!
//! The token is injected only into the child's environment; the default config
//! directory is untouched, so transcripts land in the usual place and the
//! primary `claude` login is never disturbed.

mod add;
mod cli;
mod install;
mod proxy;
mod quota;
mod registry;
mod store;

use std::ffi::OsString;
use std::path::PathBuf;

/// The name the manager answers to. Installed proxy commands have any other
/// basename and are routed to the proxy role.
const MANAGER_NAME: &str = "claude-proxy";

fn main() {
    let mut raw: Vec<OsString> = std::env::args_os().collect();
    let argv0 = raw.first().cloned().unwrap_or_default();
    let invoked = basename(&argv0);

    // Proxy role: invoked under an installed account name. Everything after the
    // program name is forwarded to `claude` verbatim.
    if invoked != MANAGER_NAME {
        let args: Vec<OsString> = raw.split_off(1);
        let store = store::default_store();
        match proxy::run(store.as_ref(), &invoked, &args) {
            Ok(code) => std::process::exit(code),
            Err(message) => {
                eprintln!("{invoked}: {message}");
                std::process::exit(1);
            }
        }
    }

    // Manager role.
    std::process::exit(cli::main());
}

/// The file name a path was invoked under, lossily, without extension noise
/// (`claude-gmail.exe` → `claude-gmail`).
fn basename(arg: &OsString) -> String {
    let path = PathBuf::from(arg);
    path.file_stem()
        .or_else(|| path.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| MANAGER_NAME.to_string())
}
