//! Making a proxy a command you can type anywhere.
//!
//! `add` links this very binary into the user's bin directory under the proxy
//! name, which makes `claude-gmail` a first-class command on `$PATH` in every
//! shell. When invoked under that name, `main` sees it in `argv[0]` and runs as
//! the proxy. A symlink, not a copy, so upgrading `claude-proxy` upgrades every
//! proxy command with it (a copy would keep running the version it was added
//! with). Windows needs privileges for symlinks, so there it is a copy.

use std::io;
use std::path::{Path, PathBuf};

use crate::paths::home;

/// The default install directory: `~/.local/bin`.
///
/// On `$PATH` for the common shells, needs no sudo, and is the conventional
/// home for user binaries. Overridable by the caller for tests and for users
/// who keep their bin elsewhere.
pub fn default_bin_dir() -> PathBuf {
    home().join(".local").join("bin")
}

/// Install `source` (this binary) into `bin_dir` as `label`.
pub fn install(source: &Path, bin_dir: &Path, label: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(bin_dir)?;
    let dest = bin_dir.join(label);
    // Build under a temp name then rename, so an existing command (possibly
    // running) is replaced atomically.
    let tmp = bin_dir.join(format!(".{label}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let source = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    place(&source, &tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

#[cfg(unix)]
fn place(source: &Path, tmp: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source, tmp)
}

#[cfg(not(unix))]
fn place(source: &Path, tmp: &Path) -> io::Result<()> {
    std::fs::copy(source, tmp).map(|_| ())
}

/// Remove an installed proxy command. Absent is not an error.
pub fn uninstall(bin_dir: &Path, label: &str) -> io::Result<()> {
    match std::fs::remove_file(bin_dir.join(label)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Whether `dir` is on the caller's `$PATH`, so we can warn when it is not.
pub fn on_path(dir: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == dir))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_puts_the_binary_on_path_under_the_label() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-proxy");
        std::fs::write(&src, b"#!/bin/sh\necho hi\n").unwrap();
        let bin = dir.path().join("bin");

        let dest = install(&src, &bin, "claude-gmail").unwrap();
        assert_eq!(dest, bin.join("claude-gmail"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"#!/bin/sh\necho hi\n");

        // A link to the manager, so upgrading it upgrades the command.
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            src.canonicalize().unwrap()
        );

        uninstall(&bin, "claude-gmail").unwrap();
        assert!(std::fs::symlink_metadata(&dest).is_err());
        assert!(src.exists(), "uninstall removes the link, not the manager");
        uninstall(&bin, "claude-gmail").unwrap(); // absent is fine
    }

    #[test]
    fn reinstall_replaces_an_existing_command() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let v1 = dir.path().join("v1");
        std::fs::write(&v1, b"one").unwrap();
        let v2 = dir.path().join("v2");
        std::fs::write(&v2, b"two").unwrap();
        install(&v1, &bin, "p").unwrap();
        install(&v2, &bin, "p").unwrap();
        assert_eq!(std::fs::read(bin.join("p")).unwrap(), b"two");
    }
}
