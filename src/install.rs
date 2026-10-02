//! Making a proxy a command you can type anywhere.
//!
//! `add` copies this very binary into the user's bin directory under the proxy
//! name. A copy, not a symlink: it keeps working if the original binary is
//! moved or upgraded, and it is what makes `claude-gmail` a first-class command
//! on `$PATH` in every shell. When invoked under that name, `main` sees it in
//! `argv[0]` and runs as the proxy.

use std::io;
use std::path::{Path, PathBuf};

use crate::store::home;

/// The default install directory: `~/.local/bin`.
///
/// On `$PATH` for the common shells, needs no sudo, and is the conventional
/// home for user binaries. Overridable by the caller for tests and for users
/// who keep their bin elsewhere.
pub fn default_bin_dir() -> PathBuf {
    home().join(".local").join("bin")
}

/// Copy `source` (this binary) into `bin_dir` as `label`, executable.
pub fn install(source: &Path, bin_dir: &Path, label: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(bin_dir)?;
    let dest = bin_dir.join(label);
    // Copy to a temp name then rename, so an in-use target (a running proxy of
    // the same name) is replaced atomically rather than truncated mid-read.
    let tmp = bin_dir.join(format!(".{label}.tmp"));
    std::fs::copy(source, &tmp)?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
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

#[cfg(unix)]
fn set_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_copies_the_binary_under_the_label_and_marks_it_runnable() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("claude-proxy");
        std::fs::write(&src, b"#!/bin/sh\necho hi\n").unwrap();
        let bin = dir.path().join("bin");

        let dest = install(&src, &bin, "claude-gmail").unwrap();
        assert_eq!(dest, bin.join("claude-gmail"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"#!/bin/sh\necho hi\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
        }

        uninstall(&bin, "claude-gmail").unwrap();
        assert!(!dest.exists());
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
