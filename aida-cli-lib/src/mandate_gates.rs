//! `aida mandate status` display and cache helpers for human gates.
// trace:STORY-1700 trace:STORY-1889 | ai:claude

use crate::gate::mandate_gates::*;
use std::path::Path;

const UNREADABLE: &str = "unreadable (every action gated)";
const DESCRIBE_CACHE: &str = "mandate-gates-cache.json";

/// The `origin/main` commit id, read from the ref files without spawning
/// git when it can be (the per-turn notice's latency budget), else through
/// one `git rev-parse`.
// trace:STORY-1700 | ai:claude
pub(crate) fn origin_main_oid(project_root: &Path) -> Option<String> {
    let dot_git = project_root.join(".git");
    let git_dir = if dot_git.is_dir() {
        Some(dot_git)
    } else {
        std::fs::read_to_string(&dot_git)
            .ok()
            .and_then(|t| {
                t.trim()
                    .strip_prefix("gitdir:")
                    .map(|p| p.trim().to_string())
            })
            .map(|p| project_root.join(p))
    };
    let common = git_dir.map(|d| match std::fs::read_to_string(d.join("commondir")) {
        Ok(rel) => d.join(rel.trim()),
        Err(_) => d,
    });
    let is_oid = |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(common) = common {
        if let Ok(text) = std::fs::read_to_string(common.join("refs/remotes/origin/main")) {
            let oid = text.trim();
            if is_oid(oid) {
                return Some(oid.to_string());
            }
        }
        if let Ok(packed) = std::fs::read_to_string(common.join("packed-refs")) {
            if let Some(oid) = packed.lines().find_map(|l| {
                l.strip_suffix(" refs/remotes/origin/main")
                    .filter(|o| is_oid(o))
            }) {
                return Some(oid.to_string());
            }
        }
    }
    let out = aida_core::git_ops::host_git_command(project_root)
        .args(["rev-parse", "--verify", "--quiet", "origin/main^{commit}"])
        .output()
        .ok()?;
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && is_oid(&oid)).then_some(oid)
}

/// What a cached description is keyed on: the committed layer's commit and
/// the operator layer's modification time.
fn describe_key(project_root: &Path) -> Option<String> {
    let oid = origin_main_oid(project_root)?;
    let home = crate::aida_home_dir()?.join(".aida").join("config.toml");
    let mtime = match std::fs::metadata(&home) {
        Ok(meta) => meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
            .to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(_) => return None,
    };
    Some(format!("{oid}:{mtime}"))
}

/// The human gates in force, for display. Cached in
/// `.aida/mandate-gates-cache.json` keyed on [`describe_key`], so the
/// per-turn notice spawns no git while neither layer changes; an unreadable
/// layer is never cached.
// trace:STORY-1700 | ai:claude
pub(crate) fn describe(main_worktree: Option<&Path>) -> String {
    let Some(root) = main_worktree else {
        return UNREADABLE.into();
    };
    let key = describe_key(root);
    let cache = root.join(".aida").join(DESCRIBE_CACHE);
    if let Some(key) = &key {
        let hit = std::fs::read(&cache)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .filter(|v| v["key"].as_str() == Some(key.as_str()))
            .and_then(|v| v["describe"].as_str().map(str::to_string));
        if let Some(text) = hit {
            return text;
        }
    }
    match load_human_gates(root) {
        Ok(g) => {
            let text = g.describe();
            if let Some(key) = key {
                let body = serde_json::json!({ "key": key, "describe": text });
                let tmp = cache.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
                if std::fs::write(&tmp, body.to_string()).is_ok() {
                    let _ = std::fs::rename(&tmp, &cache);
                }
            }
            text
        }
        Err(_) => UNREADABLE.into(),
    }
}

/// `aida mandate status --json` `protected_paths`: the effective set with
/// each entry's source, or `{"error": ...}` (then only the floor is known
/// to hold, and every op is gated).
// trace:STORY-1888 | ai:claude
pub(crate) fn protected_paths_json(project_root: &Path) -> serde_json::Value {
    match load_human_gates(project_root) {
        Ok(g) => g.protected.to_json(),
        Err(e) => serde_json::json!({
            "error": format!("{e:#}"),
            "floor": ProtectedPaths::floor().to_json(),
        }),
    }
}

/// The `aida mandate status` block listing the effective protected set.
// trace:STORY-1888 | ai:claude
pub(crate) fn protected_paths_text(project_root: &Path) -> String {
    let (set, note) = match load_human_gates(project_root) {
        Ok(g) => (g.protected, String::new()),
        Err(e) => (
            ProtectedPaths::floor(),
            format!(" (gate config unreadable, built-in floor only: {e:#})"),
        ),
    };
    let mut out = format!("  protected paths{note}:\n");
    for e in set.entries() {
        out.push_str(&format!("    {}  [{}]\n", e.pattern, e.source.as_str()));
    }
    out
}
