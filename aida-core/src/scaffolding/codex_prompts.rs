//! Codex custom prompts (`~/.codex/prompts/*.md`) generated from the same
//! embedded command masters that back `.claude/commands/` — the slash-command
//! UX parity piece for codex-first machines (STORY-763).
//!
//! `.codex/skills/` (project-local skill bodies) and `.codex/config.toml`
//! (MCP registration) already ship at init; what they don't give a Codex
//! session is the `/aida-…` invocation surface. Codex CLI reads custom
//! prompts from the user-global `~/.codex/prompts/` directory — one markdown
//! file per prompt, invoked by filename. This module converts each embedded
//! command template into that shape:
//!
//! - the YAML frontmatter (a Claude Code convention) is stripped;
//! - `.claude/skills/<name>.md` references are rewritten to the project's
//!   `.codex/skills/<name>/SKILL.md` copy when that skill ships in the codex
//!   set (both are plain repo files an agent can read — the rewrite just
//!   points Codex at its own copy first);
//! - commands that structurally depend on Claude-only mechanics (hooks, the
//!   Agent-tool subagent fan-out, session resume) are excluded with a stated
//!   reason instead of silently dropped.
//!
//! Writes are conservative: an existing prompt file is never overwritten
//! unless `force` — the user may have edited it.
// trace:STORY-763 | ai:claude

use anyhow::{Context, Result};
use std::path::Path;

/// Commands that do not port to Codex custom prompts, with the reason shown
/// to the user. Curated from the migration inventory rather than guessed:
/// these bodies drive Claude Code hooks, the Agent-tool subagent fan-out, or
/// orchestrator-internal session-resume semantics Codex has no analogue for.
pub const CODEX_NONPORTABLE_COMMANDS: &[(&str, &str)] = &[
    (
        "aida-solo",
        "drives Claude Code hooks and the Agent-tool subagent fan-out",
    ),
    (
        "aida-burndown",
        "fans out worktree-isolated subagents via the Claude Code Agent tool",
    ),
    (
        "aida-insights",
        "reads Claude-session telemetry surfaces (hooks/statusline)",
    ),
    (
        "aida-advise",
        "orchestrator-internal headless advisor tier — spawned by the drain, not run by hand",
    ),
];

/// The skills `aida init` scaffolds into `.codex/skills/` (mirrors the
/// `codex_skill_defs` set in `mod.rs` — keep in sync when that list grows).
/// Used to rewrite `.claude/skills/<name>.md` references to the Codex-side
/// copy so a Codex prompt points at its own skill body first.
const CODEX_SKILL_SET: &[&str] = &[
    "aida-req",
    "aida-plan",
    "aida-implement",
    "aida-capture",
    "aida-docs",
    "aida-docs-review",
    "aida-release",
    "aida-evaluate",
    "aida-commit",
    "aida-sync",
    "aida-test",
    "aida-review",
    "aida-onboard",
    "aida-sprint",
    "aida-search",
    "aida-standup",
    "aida-import-plan",
    "aida-digest",
    "aida-backlog-groom",
];

/// Outcome of a prompts scaffold run — everything the CLI needs to report
/// honestly, including what was deliberately NOT written and why.
#[derive(Debug, Default)]
pub struct CodexPromptsOutcome {
    pub written: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub excluded: Vec<(String, String)>,
}

/// Strip a leading YAML frontmatter block (`---\n…\n---\n`) if present.
///
/// Shared with the non-Claude skill-invocation materializer (TASK-1045), which
/// inlines an embedded skill body into a headless prompt for a vendor that
/// can't expand `/aida-<skill>`; the Claude Code frontmatter is noise there.
pub fn strip_frontmatter(body: &str) -> &str {
    let Some(rest) = body.strip_prefix("---\n") else {
        return body;
    };
    match rest.find("\n---\n") {
        Some(end) => &rest[end + 5..],
        // Unterminated frontmatter — leave the body untouched rather than eat it.
        None => body,
    }
}

/// Convert one embedded command template into a Codex custom prompt body.
pub fn convert_command_to_codex_prompt(body: &str) -> String {
    let mut out = strip_frontmatter(body).trim_start().to_string();
    for name in CODEX_SKILL_SET {
        let claude_ref = format!(".claude/skills/{name}.md");
        let codex_ref = format!(".codex/skills/{name}/SKILL.md");
        out = out.replace(&claude_ref, &codex_ref);
    }
    out
}

/// The expected Codex custom-prompt set as `(name, body)` pairs — the same
/// portable enumeration `scaffold_codex_prompts` writes, but pure (no I/O).
/// Lets a drift check (e.g. `aida doctor --category scaffold-drift`, TASK-1124)
/// compare a deployed `~/.codex/prompts` against the current source templates
/// without re-deriving the conversion.
// trace:TASK-1124 | ai:claude
pub fn expected_codex_prompts() -> Vec<(String, String)> {
    use crate::templates::EMBEDDED_TEMPLATES;

    let mut keys: Vec<&&str> = EMBEDDED_TEMPLATES
        .keys()
        .filter(|k| k.starts_with("commands/") && k.ends_with(".md"))
        .collect();
    keys.sort();

    let mut out = Vec::new();
    for key in keys {
        let name = key
            .trim_start_matches("commands/")
            .trim_end_matches(".md")
            .to_string();
        if CODEX_NONPORTABLE_COMMANDS.iter().any(|(n, _)| *n == name) {
            continue;
        }
        let body = EMBEDDED_TEMPLATES
            .get(*key)
            .expect("key came from the same map");
        out.push((name, convert_command_to_codex_prompt(body)));
    }
    out
}

/// Write the Codex custom-prompt set into `dest_dir` (normally
/// `~/.codex/prompts`). Existing files are skipped unless `force`; the
/// non-portable set is excluded with reasons. Idempotent.
pub fn scaffold_codex_prompts(dest_dir: &Path, force: bool) -> Result<CodexPromptsOutcome> {
    use crate::templates::EMBEDDED_TEMPLATES;

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating codex prompts dir {}", dest_dir.display()))?;

    let mut outcome = CodexPromptsOutcome::default();
    let mut keys: Vec<&&str> = EMBEDDED_TEMPLATES
        .keys()
        .filter(|k| k.starts_with("commands/") && k.ends_with(".md"))
        .collect();
    keys.sort();

    for key in keys {
        let name = key
            .trim_start_matches("commands/")
            .trim_end_matches(".md")
            .to_string();
        if let Some((_, reason)) = CODEX_NONPORTABLE_COMMANDS.iter().find(|(n, _)| *n == name) {
            outcome.excluded.push((name, (*reason).to_string()));
            continue;
        }
        let dest = dest_dir.join(format!("{name}.md"));
        if dest.exists() && !force {
            outcome.skipped_existing.push(name);
            continue;
        }
        let body = EMBEDDED_TEMPLATES
            .get(*key)
            .expect("key came from the same map");
        let prompt = convert_command_to_codex_prompt(body);
        std::fs::write(&dest, prompt)
            .with_context(|| format!("writing codex prompt {}", dest.display()))?;
        outcome.written.push(name);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_removes_leading_yaml_block_only() {
        let body = "---\ndescription: \"x\"\n---\n# Title\n\nBody --- dashes inline\n";
        assert_eq!(
            strip_frontmatter(body),
            "# Title\n\nBody --- dashes inline\n"
        );
        // No frontmatter → unchanged.
        assert_eq!(strip_frontmatter("# Plain\n"), "# Plain\n");
        // Unterminated frontmatter → untouched, never eaten.
        let broken = "---\ndescription: x\n# no close\n";
        assert_eq!(strip_frontmatter(broken), broken);
    }

    #[test]
    fn convert_rewrites_skill_refs_to_codex_copies() {
        let body = "---\ndescription: d\n---\nFollow the workflow in `.claude/skills/aida-commit.md`:\nand `.claude/skills/aida-noncodex.md` stays.";
        let out = convert_command_to_codex_prompt(body);
        assert!(out.contains(".codex/skills/aida-commit/SKILL.md"), "{out}");
        // A skill NOT in the codex set keeps its .claude path (still a readable repo file).
        assert!(out.contains(".claude/skills/aida-noncodex.md"), "{out}");
        assert!(
            !out.starts_with("---"),
            "frontmatter must be stripped: {out}"
        );
    }

    // trace:TASK-1124 — the pure enumeration matches what scaffold_codex_prompts
    // would write: the portable set (nonportable excluded), stripped bodies.
    #[test]
    fn expected_codex_prompts_is_the_portable_set_with_converted_bodies() {
        let expected = expected_codex_prompts();
        assert!(
            expected.len() > 30,
            "expected the bulk of the command set, got {}",
            expected.len()
        );
        // Nonportable commands are excluded.
        for (name, _) in CODEX_NONPORTABLE_COMMANDS {
            assert!(
                !expected.iter().any(|(n, _)| n == name),
                "nonportable command {name} must be excluded"
            );
        }
        // Bodies are converted (frontmatter stripped).
        for (name, body) in &expected {
            assert!(!body.starts_with("---"), "{name} keeps frontmatter");
        }
        // It agrees with the writer: writing to a temp dir yields the same set.
        let tmp = tempfile::tempdir().unwrap();
        let written = scaffold_codex_prompts(tmp.path(), false).unwrap();
        let mut want: Vec<&String> = expected.iter().map(|(n, _)| n).collect();
        want.sort();
        let mut got: Vec<&String> = written.written.iter().collect();
        got.sort();
        assert_eq!(want, got, "expected set must match the written set");
    }

    #[test]
    fn scaffold_writes_prompts_skips_existing_and_excludes_nonportable() {
        let tmp = tempfile::tempdir().unwrap();
        let out1 = scaffold_codex_prompts(tmp.path(), false).unwrap();
        assert!(
            out1.written.len() > 30,
            "expected the bulk of the command set, got {}",
            out1.written.len()
        );
        assert_eq!(out1.excluded.len(), CODEX_NONPORTABLE_COMMANDS.len());
        for (name, _) in CODEX_NONPORTABLE_COMMANDS {
            assert!(
                !tmp.path().join(format!("{name}.md")).exists(),
                "{name} must be excluded"
            );
        }
        // Second run: everything already present is skipped, nothing rewritten.
        let out2 = scaffold_codex_prompts(tmp.path(), false).unwrap();
        assert!(out2.written.is_empty());
        assert_eq!(out2.skipped_existing.len(), out1.written.len());

        // User edit survives a non-force rerun; --force overwrites.
        let target = tmp.path().join("aida-commit.md");
        std::fs::write(&target, "user edited").unwrap();
        scaffold_codex_prompts(tmp.path(), false).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "user edited");
        scaffold_codex_prompts(tmp.path(), true).unwrap();
        assert_ne!(std::fs::read_to_string(&target).unwrap(), "user edited");
    }
}
