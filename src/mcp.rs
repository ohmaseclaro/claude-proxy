//! MCP servers for profiles. Claude keeps user- and local-scope MCP servers in
//! `~/.claude.json`, which holds the login too and so cannot be shared; a
//! profile would start with none. Each launch passes the primary's servers to
//! Claude with `--mcp-config` instead.

use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::paths::{config_dir, home};

/// The primary profile's user-scope servers plus the local-scope ones of `cwd`.
pub fn user_servers(cwd: &Path) -> Map<String, Value> {
    servers_in(&home().join(".claude.json"), &cwd.to_string_lossy())
}

fn servers_in(claude_json: &Path, cwd: &str) -> Map<String, Value> {
    let Some(v) = fs::read(claude_json)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return Map::new();
    };
    let mut servers = v["mcpServers"].as_object().cloned().unwrap_or_default();
    if let Some(local) = v["projects"][cwd]["mcpServers"].as_object() {
        servers.extend(local.clone());
    }
    servers
}

/// `--mcp-config=<file>` for these servers, written owner-only (server entries
/// can carry API keys). The `=` form matters: Claude's flag is variadic and
/// would swallow a following subcommand or argument.
pub fn flag(file: &Path) -> String {
    format!("--mcp-config={}", file.display())
}

pub fn write(file: &Path, servers: Map<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(".tmp");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options.open(&tmp)?;
    f.write_all(&serde_json::to_vec(&json!({"mcpServers": servers}))?)?;
    fs::rename(tmp, file)
}

/// A config file for an interactive launch, named by its contents so launches
/// from different directories never overwrite each other's.
pub fn shared_file(servers: &Map<String, Value>) -> PathBuf {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    Value::Object(servers.clone()).to_string().hash(&mut h);
    config_dir()
        .join("mcp")
        .join(format!("{:016x}.json", h.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_and_local_scope_servers_are_merged() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join(".claude.json");
        fs::write(
            &file,
            r#"{"mcpServers":{"a":{"command":"x"}},
                "projects":{"/repo":{"mcpServers":{"b":{"command":"y"}}},
                            "/other":{"mcpServers":{"c":{"command":"z"}}}}}"#,
        )
        .unwrap();
        let mut names: Vec<_> = servers_in(&file, "/repo").keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["a", "b"]);
        assert!(servers_in(&t.path().join("missing"), "/repo").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("mcp/x.json");
        write(&file, Map::new()).unwrap();
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
