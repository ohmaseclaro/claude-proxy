//! Where each proxy's long-lived token lives.
//!
//! The token is the one secret this tool handles, so it never touches a file
//! inside a repo, never a shell profile, and never the process argv. On macOS
//! it goes in the login Keychain (the same place Claude Code keeps its own);
//! everywhere else, a mode-0600 file under the user's config directory.
//!
//! The backend is chosen at runtime but the trait is the seam: tests use
//! [`FileStore`] pointed at a temp directory and never touch a real Keychain.

use std::io;
use std::path::{Path, PathBuf};

/// A token store, keyed by proxy label.
pub trait Store {
    /// The token for `label`, or `None` if nothing is stored.
    fn get(&self, label: &str) -> io::Result<Option<String>>;
    /// Store (replacing any existing) the token for `label`.
    fn set(&self, label: &str, token: &str) -> io::Result<()>;
    /// Remove the token for `label`. Removing an absent label is not an error.
    fn delete(&self, label: &str) -> io::Result<()>;
}

/// The production store for this platform.
pub fn default_store() -> Box<dyn Store> {
    #[cfg(target_os = "macos")]
    {
        Box::new(KeychainStore)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(FileStore::at(credentials_dir()))
    }
}

/// `~/.config/claude-proxy` (or `$XDG_CONFIG_HOME/claude-proxy`), where the
/// registry and — off macOS — the credential files live.
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Path::new(&xdg).join("claude-proxy");
        }
    }
    home().join(".config").join("claude-proxy")
}

#[cfg(not(target_os = "macos"))]
fn credentials_dir() -> PathBuf {
    config_dir().join("credentials")
}

/// The isolated Claude config directory for one proxy.
///
/// Each proxy gets its own, so its login identity, settings, and transcripts
/// never share state with the primary `~/.claude` login or with another proxy.
/// `claude` is pointed here via `CLAUDE_CONFIG_DIR`.
pub fn account_config_dir(label: &str) -> PathBuf {
    config_dir().join("accounts").join(label)
}

/// The user's home directory, cross-platform.
pub fn home() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        if !up.is_empty() {
            return PathBuf::from(up);
        }
    }
    PathBuf::from(".")
}

/// macOS login-Keychain store, via `security(1)`.
///
/// Shelling to `security` rather than linking the Security framework keeps the
/// item owned by `/usr/bin/security` in its XARA partition list — a native
/// write would stamp this tool's own code identity on the item and make every
/// later read raise a Keychain dialog. (ai-usagebar learned this the hard way;
/// the lesson is borrowed here.)
#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl KeychainStore {
    fn service(label: &str) -> String {
        format!("claude-proxy-{label}")
    }
    fn account() -> String {
        std::env::var("USER").unwrap_or_else(|_| "claude-proxy".into())
    }
}

#[cfg(target_os = "macos")]
impl Store for KeychainStore {
    fn get(&self, label: &str) -> io::Result<Option<String>> {
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-a",
                &Self::account(),
                "-s",
                &Self::service(label),
                "-w",
            ])
            .output()?;
        if !out.status.success() {
            return Ok(None); // not found is the common, non-error case
        }
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((!token.is_empty()).then_some(token))
    }

    fn set(&self, label: &str, token: &str) -> io::Result<()> {
        // `security add-generic-password -w <value>` with no value reads the
        // passphrase from /dev/tty, so a piped stdin is ignored and the user is
        // prompted. Feeding the whole command to `security -i` instead puts the
        // secret on stdin — out of the process table *and* out of the tty
        // prompt. Pattern borrowed from ai-usagebar.
        let command = compose_write_command(&Self::account(), &Self::service(label), token)
            .ok_or_else(|| io::Error::other("token is not storable (newline or too long)"))?;
        write_via_security_stdin(&command)
    }

    fn delete(&self, label: &str) -> io::Result<()> {
        let status = std::process::Command::new("/usr/bin/security")
            .args([
                "delete-generic-password",
                "-a",
                &Self::account(),
                "-s",
                &Self::service(label),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // A missing item exits non-zero; that is not an error for delete.
        let _ = status;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
use std::process::Stdio;

/// Operational cap for one `security -i` command line, a margin below the
/// undocumented reader limit. Our tokens are ~100 bytes, far under it.
#[cfg(target_os = "macos")]
const SECURITY_STDIN_SAFE_MAX: usize = 4000;

/// Quote one value for `security -i`'s line tokenizer (backslash escapes inside
/// a double-quoted token). `None` on a newline, which would end the line early
/// and let the rest be read as a further command.
#[cfg(target_os = "macos")]
fn quote_for_security_stdin(value: &str) -> Option<String> {
    if value.contains('\n') || value.contains('\r') {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Some(out)
}

/// Compose the `add-generic-password` line, `None` if any part cannot be quoted
/// or the whole exceeds the reader's safe maximum.
#[cfg(target_os = "macos")]
fn compose_write_command(account: &str, service: &str, token: &str) -> Option<String> {
    let command = format!(
        "add-generic-password -U -a {} -s {} -w {}\n",
        quote_for_security_stdin(account)?,
        quote_for_security_stdin(service)?,
        quote_for_security_stdin(token)?,
    );
    (command.len() <= SECURITY_STDIN_SAFE_MAX).then_some(command)
}

/// Feed one composed command to `security -i` over stdin, keeping the secret out
/// of argv and off the tty prompt.
#[cfg(target_os = "macos")]
fn write_via_security_stdin(command: &str) -> io::Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(command.as_bytes())?;
    let out = child.wait_with_output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "security add-generic-password failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// A mode-0600 file per label under a directory. The store off macOS, and the
/// one every test uses — hence unused only in a non-test macOS build.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct FileStore {
    dir: PathBuf,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl FileStore {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, label: &str) -> PathBuf {
        self.dir.join(format!("{label}.token"))
    }
}

impl Store for FileStore {
    fn get(&self, label: &str) -> io::Result<Option<String>> {
        match std::fs::read_to_string(self.path(label)) {
            Ok(s) => {
                let t = s.trim().to_string();
                Ok((!t.is_empty()).then_some(t))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn set(&self, label: &str, token: &str) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.path(label);
        // Write 0600 before any bytes land, so the token is never briefly
        // world-readable.
        write_private(&path, token.as_bytes())
    }

    fn delete(&self, label: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(label)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::at(dir.path().to_path_buf());

        assert_eq!(store.get("gmail").unwrap(), None);
        store.set("gmail", "sk-ant-oat01-example").unwrap();
        assert_eq!(
            store.get("gmail").unwrap().as_deref(),
            Some("sk-ant-oat01-example")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("gmail.token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the token file must be owner-only");
        }

        store.delete("gmail").unwrap();
        assert_eq!(store.get("gmail").unwrap(), None);
        // Deleting an absent label is a no-op, not an error.
        store.delete("gmail").unwrap();
    }

    #[test]
    fn get_trims_and_treats_empty_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blank.token"), "   \n").unwrap();
        let store = FileStore::at(dir.path().to_path_buf());
        assert_eq!(store.get("blank").unwrap(), None);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod keychain_tests {
    use super::*;

    #[test]
    fn compose_quotes_bounds_and_refuses_newlines() {
        let c = compose_write_command("me", "claude-proxy-x", "sk-ant-oat01-tok").unwrap();
        assert!(c.starts_with(
            "add-generic-password -U -a \"me\" -s \"claude-proxy-x\" -w \"sk-ant-oat01-tok\""
        ));
        assert!(c.ends_with('\n'));
        // A newline in the secret is refused rather than splitting the command.
        assert!(compose_write_command("me", "svc", "a\nb").is_none());
        // Quotes and backslashes in the secret are escaped for the tokenizer.
        let c2 = compose_write_command("me", "svc", "a\"b\\c").unwrap();
        assert!(c2.contains("-w \"a\\\"b\\\\c\""));
    }
}
