//! Sharing your Claude setup with every profile while the login stays apart.
//!
//! A profile is its own `CLAUDE_CONFIG_DIR`, so on its own it would see none of
//! the skills, plugins, hooks, settings, or `CLAUDE.md` in `~/.claude`. Each
//! allowlisted entry is symlinked from `~/.claude` into the profile instead.
//! Transcripts (`projects`) are shared too, so every account's sessions land in
//! `~/.claude/projects` and `--resume` sees them all.
//!
//! An allowlist, not a denylist: anything account-specific (the login, the
//! identity in `.claude.json`, org policy, MCP auth state, caches) stays in the
//! profile by default, including entries a future Claude version adds.

use std::path::Path;

use crate::paths::home;

const SHARED: &[&str] = &[
    // configuration
    "settings.json",
    "CLAUDE.md",
    "keybindings.json",
    "agents",
    "commands",
    "skills",
    "output-styles",
    "plugins",
    "hooks",
    "scripts",
    // session data: transcripts + per-project memory, checkpoints, todos, plans
    "projects",
    "file-history",
    "todos",
    "plans",
    "history.jsonl",
    // get-shit-done installs under the config dir
    "get-shit-done",
    "gsd-core",
    "gsd-file-manifest.json",
    "gsd-install-state.json",
    ".gsd-profile",
    ".gsd-source",
];

/// Shared entries whose profile-local contents are worth keeping: they are
/// merged into `~/.claude` before the link replaces them (a log file is
/// appended).
const MERGE: &[&str] = &[
    "projects",
    "file-history",
    "todos",
    "plans",
    "history.jsonl",
];

/// Link every shared entry into `profile`. Idempotent and best-effort: a link
/// that cannot be made leaves the profile working, just without that entry.
/// A profile's own copy of a shared entry (one Claude wrote before the entry
/// was linked) gives way to the link: merged when it holds history, otherwise
/// kept as `<name>.pre-shared`, never deleted.
pub fn link_shared(profile: &Path) {
    link_shared_from(&home().join(".claude"), profile);
}

fn link_shared_from(global: &Path, profile: &Path) {
    if global == profile {
        return;
    }
    for name in SHARED {
        let src = global.join(name);
        if !src.exists() {
            continue;
        }
        let dst = profile.join(name);
        match std::fs::symlink_metadata(&dst) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = symlink(&src, &dst);
            }
            Ok(m) if m.is_dir() && src.is_dir() => {
                if MERGE.contains(name) {
                    merge_into(&dst, &src);
                }
                // Whatever is left (or a config dir Claude seeded on first run)
                // is kept as a backup, never deleted.
                let cleared = std::fs::remove_dir(&dst).is_ok()
                    || std::fs::rename(&dst, backup_path(&dst)).is_ok();
                if cleared {
                    let _ = symlink(&src, &dst);
                }
            }
            Ok(m) if m.is_file() && src.is_file() => {
                let merged = MERGE.contains(name) && append_into(&dst, &src).is_ok();
                let cleared = (merged && std::fs::remove_file(&dst).is_ok())
                    || std::fs::rename(&dst, backup_path(&dst)).is_ok();
                if cleared {
                    let _ = symlink(&src, &dst);
                }
            }
            // Already a link, or unreadable: leave it.
            _ => {}
        }
    }
}

/// Move everything in `from` into `to` that does not already exist there,
/// recursing into directories present on both sides.
fn merge_into(from: &Path, to: &Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        match std::fs::symlink_metadata(&target) {
            Err(_) => {
                let _ = std::fs::rename(entry.path(), &target);
            }
            Ok(m) if m.is_dir() && entry.path().is_dir() => {
                merge_into(&entry.path(), &target);
                let _ = std::fs::remove_dir(entry.path());
            }
            Ok(_) => {}
        }
    }
}

fn append_into(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut lines = std::fs::read(from)?;
    if !lines.is_empty() && !lines.ends_with(b"\n") {
        lines.push(b'\n');
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(to)?
        .write_all(&lines)
}

fn backup_path(dst: &Path) -> std::path::PathBuf {
    let name = dst.file_name().unwrap_or_default().to_string_lossy();
    let mut candidate = dst.with_file_name(format!("{name}.pre-shared"));
    let mut n = 1;
    while candidate.exists() {
        candidate = dst.with_file_name(format!("{name}.pre-shared.{n}"));
        n += 1;
    }
    candidate
}

#[cfg(unix)]
fn symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

// Windows symlinks need extra privileges; profiles there stay unshared.
#[cfg(not(unix))]
fn symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn links_missing_entries_and_skips_absent_ones() {
        let t = tempfile::tempdir().unwrap();
        let (global, profile) = (t.path().join("g"), t.path().join("p"));
        std::fs::create_dir_all(global.join("skills")).unwrap();
        std::fs::write(global.join("settings.json"), "{}").unwrap();
        std::fs::create_dir_all(&profile).unwrap();

        link_shared_from(&global, &profile);

        for name in ["skills", "settings.json"] {
            let m = std::fs::symlink_metadata(profile.join(name)).unwrap();
            assert!(m.file_type().is_symlink(), "{name} should be linked");
        }
        assert!(
            !profile.join("agents").exists(),
            "absent globally → not linked"
        );
        // Idempotent.
        link_shared_from(&global, &profile);
    }

    #[test]
    fn merges_profile_transcripts_into_the_shared_dir_then_links() {
        let t = tempfile::tempdir().unwrap();
        let (global, profile) = (t.path().join("g"), t.path().join("p"));
        std::fs::create_dir_all(global.join("projects/slug")).unwrap();
        std::fs::write(global.join("projects/slug/old.jsonl"), "old").unwrap();
        std::fs::create_dir_all(profile.join("projects/slug")).unwrap();
        std::fs::write(profile.join("projects/slug/new.jsonl"), "new").unwrap();

        link_shared_from(&global, &profile);

        assert_eq!(
            std::fs::read_to_string(global.join("projects/slug/new.jsonl")).unwrap(),
            "new"
        );
        assert!(global.join("projects/slug/old.jsonl").exists());
        let m = std::fs::symlink_metadata(profile.join("projects")).unwrap();
        assert!(m.file_type().is_symlink());
    }

    #[test]
    fn a_seeded_config_dir_is_backed_up_not_deleted() {
        let t = tempfile::tempdir().unwrap();
        let (global, profile) = (t.path().join("g"), t.path().join("p"));
        std::fs::create_dir_all(global.join("plugins")).unwrap();
        std::fs::create_dir_all(profile.join("plugins")).unwrap();
        std::fs::write(profile.join("plugins/local.json"), "x").unwrap();

        link_shared_from(&global, &profile);

        assert!(std::fs::symlink_metadata(profile.join("plugins"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(profile.join("plugins.pre-shared/local.json")).unwrap(),
            "x"
        );
    }

    #[test]
    fn a_profile_owned_file_gives_way_to_the_shared_one() {
        let t = tempfile::tempdir().unwrap();
        let (global, profile) = (t.path().join("g"), t.path().join("p"));
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(global.join("settings.json"), "global").unwrap();
        std::fs::write(profile.join("settings.json"), "mine").unwrap();
        std::fs::write(global.join("history.jsonl"), "a\n").unwrap();
        std::fs::write(profile.join("history.jsonl"), "b").unwrap();

        link_shared_from(&global, &profile);

        assert_eq!(
            std::fs::read_to_string(profile.join("settings.json")).unwrap(),
            "global"
        );
        assert_eq!(
            std::fs::read_to_string(profile.join("settings.json.pre-shared")).unwrap(),
            "mine"
        );
        assert_eq!(
            std::fs::read_to_string(global.join("history.jsonl")).unwrap(),
            "a\nb\n"
        );
        assert!(!profile.join("history.jsonl.pre-shared").exists());
        assert!(std::fs::symlink_metadata(profile.join("history.jsonl"))
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
