//! Codex hook wiring — `.codex/hooks.json` plus the handler scripts.
//!
//! AIDA wires seven Claude Code hook events. Codex supports all seven (its
//! `hooks` feature is stable and on by default), uses the same
//! `matcher`/`hooks`/`hookSpecificOutput` schema, and — verified empirically
//! against codex-cli 0.144.6 — normalizes tool names to Claude's vocabulary,
//! so a `"Bash"` matcher matches a Codex shell call. The wiring therefore
//! ports almost mechanically.
//!
//! ONE SET OF SCRIPTS, TWO WIRINGS. The handlers are NOT forked per vendor.
//! Forking would double every future hook fix and guarantee drift; instead the
//! scripts resolve the project root through a vendor-neutral chain and are
//! copied from the same embedded masters into each vendor's tree. The only
//! per-vendor difference is the wiring file, and `codex_hooks_match_settings`
//! pins it against `settings.json` so the two cannot silently diverge.
//!
//! What differs from the Claude wiring, and why:
//!   - `$CLAUDE_PROJECT_DIR` has no Codex equivalent, so the command uses
//!     `$(git rev-parse --show-toplevel)`, which is what the porting doc
//!     prescribes and which an empirical probe confirmed works.
//!   - handler paths point at `.codex/hooks/` so a codex-only machine needs no
//!     `.claude/` directory at all.
//!
//! trace:TASK-1181 | ai:claude

use std::path::Path;

/// Where the wiring lives, relative to the project root.
pub const HOOKS_JSON_REL: &str = ".codex/hooks.json";
/// Where the handler scripts live, relative to the project root.
pub const HOOKS_DIR_REL: &str = ".codex/hooks";

/// The handlers referenced by the Codex wiring.
///
/// Deliberately a subset of `templates/hooks/`: the git hooks
/// (`aida-commit-msg`, `aida-pre-commit.sh`, `aida-post-commit.sh`) are
/// git-level and vendor-independent — they are installed into `.git/hooks`
/// and must NOT be duplicated here.
pub fn handler_names() -> Vec<String> {
    let text = crate::templates::EMBEDDED_TEMPLATES
        .get("codex-hooks.json")
        .copied()
        .unwrap_or("{}");
    let mut names: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(i) = line.find("/.codex/hooks/") {
            let rest = &line[i + "/.codex/hooks/".len()..];
            if let Some(name) = rest.split('"').next() {
                let n = name.trim().to_string();
                if !n.is_empty() && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    names.sort();
    names
}

/// What a scaffold pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CodexHooksOutcome {
    pub written: Vec<String>,
    pub skipped_existing: Vec<String>,
}

/// Write the Codex hook wiring and handlers under `project_root`.
///
/// Idempotent: an existing file is left alone unless `force`, matching every
/// other AIDA scaffold. A user who edited their wiring keeps it.
pub fn scaffold_codex_hooks(
    project_root: &Path,
    force: bool,
) -> std::io::Result<CodexHooksOutcome> {
    let mut out = CodexHooksOutcome::default();
    let dir = project_root.join(HOOKS_DIR_REL);
    std::fs::create_dir_all(&dir)?;

    // Handlers first, so the wiring never points at a script that is not there.
    for name in handler_names() {
        let Some(body) = crate::templates::EMBEDDED_TEMPLATES.get(&format!("hooks/{name}") as &str)
        else {
            continue;
        };
        let dest = dir.join(&name);
        if dest.exists() && !force {
            out.skipped_existing.push(name);
            continue;
        }
        std::fs::write(&dest, format!("{}\n", body.trim_end()))?;
        set_executable(&dest);
        out.written.push(name);
    }

    let wiring = project_root.join(HOOKS_JSON_REL);
    if wiring.exists() && !force {
        out.skipped_existing.push(HOOKS_JSON_REL.to_string());
    } else {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get("codex-hooks.json")
            .copied()
            .unwrap_or("{}");
        std::fs::write(&wiring, format!("{}\n", body.trim_end()))?;
        out.written.push(HOOKS_JSON_REL.to_string());
    }
    Ok(out)
}

#[cfg(unix)]
fn set_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(md) = std::fs::metadata(p) {
        let mut perms = md.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(p, perms);
    }
}

#[cfg(not(unix))]
fn set_executable(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wiring must cover exactly the events `settings.json` wires, with the
    /// same matchers and the same handlers. Without this, editing the Claude
    /// settings silently leaves Codex behind — the drift this whole design is
    /// trying to make impossible.
    #[test]
    fn codex_hooks_match_settings() {
        let settings: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get("settings.json")
                .expect("settings.json embedded"),
        )
        .expect("settings.json parses");
        let codex: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get("codex-hooks.json")
                .expect("codex-hooks.json embedded"),
        )
        .expect("codex-hooks.json parses");

        let sh = settings.get("hooks").and_then(|h| h.as_object()).unwrap();
        let ch = codex.get("hooks").and_then(|h| h.as_object()).unwrap();

        // Every Claude event Codex supports must be present.
        const CODEX_EVENTS: &[&str] = &[
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "UserPromptSubmit",
            "SessionStart",
            "Stop",
            "SubagentStart",
            "SubagentStop",
        ];
        for ev in sh.keys() {
            if !CODEX_EVENTS.contains(&ev.as_str()) {
                continue; // no Codex equivalent; legitimately dropped
            }
            assert!(
                ch.contains_key(ev),
                "settings.json wires `{ev}` but the Codex wiring does not — \
                 regenerate templates/codex-hooks.json"
            );
            let s_entries = sh[ev].as_array().unwrap();
            let c_entries = ch[ev].as_array().unwrap();
            assert_eq!(
                s_entries.len(),
                c_entries.len(),
                "`{ev}` has {} Claude entries but {} Codex entries",
                s_entries.len(),
                c_entries.len()
            );
            for (s, c) in s_entries.iter().zip(c_entries) {
                assert_eq!(
                    s.get("matcher"),
                    c.get("matcher"),
                    "`{ev}` matcher differs — Codex normalizes tool names to \
                     Claude's vocabulary, so matchers must be identical"
                );
                let sn: Vec<&str> = s["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h["command"].as_str().unwrap().rsplit('/').next().unwrap())
                    .collect();
                let cn: Vec<&str> = c["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h["command"].as_str().unwrap().rsplit('/').next().unwrap())
                    .collect();
                assert_eq!(sn, cn, "`{ev}` wires different handlers");
            }
        }
    }

    #[test]
    fn the_wiring_carries_no_claude_only_variable() {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get("codex-hooks.json")
            .unwrap();
        assert!(
            !body.contains("CLAUDE_PROJECT_DIR"),
            "Codex has no CLAUDE_PROJECT_DIR; use $(git rev-parse --show-toplevel)"
        );
        assert!(
            !body.contains("/.claude/hooks/"),
            "a codex-only machine must not need a .claude/ directory"
        );
        assert!(body.contains("git rev-parse --show-toplevel"));
    }

    #[test]
    fn git_hooks_are_not_duplicated_into_the_codex_tree() {
        // commit-msg / pre-commit / post-commit are git-level and vendor
        // independent; installing them here would double-fire them.
        let names = handler_names();
        for git_hook in [
            "aida-commit-msg",
            "aida-pre-commit.sh",
            "aida-post-commit.sh",
        ] {
            assert!(
                !names.iter().any(|n| n == git_hook),
                "{git_hook} is a git hook and must not be wired into Codex"
            );
        }
        assert!(!names.is_empty(), "expected some handlers");
    }

    #[test]
    fn scaffolding_writes_wiring_and_handlers() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert!(dir.join(HOOKS_JSON_REL).is_file(), "wiring must exist");
        assert!(
            out.written.iter().any(|w| w == HOOKS_JSON_REL),
            "wiring should be reported as written"
        );
        for name in handler_names() {
            assert!(
                dir.join(HOOKS_DIR_REL).join(&name).is_file(),
                "handler {name} must be written — the wiring points at it"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffolding_is_idempotent_and_preserves_edits() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        scaffold_codex_hooks(&dir, false).unwrap();
        let wiring = dir.join(HOOKS_JSON_REL);
        std::fs::write(&wiring, "{\"hooks\":{}}\n").unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&wiring).unwrap(),
            "{\"hooks\":{}}\n",
            "a user's edited wiring must survive"
        );
        assert!(out.skipped_existing.iter().any(|s| s == HOOKS_JSON_REL));

        let forced = scaffold_codex_hooks(&dir, true).unwrap();
        assert!(forced.written.iter().any(|w| w == HOOKS_JSON_REL));
        std::fs::remove_dir_all(&dir).ok();
    }
}
