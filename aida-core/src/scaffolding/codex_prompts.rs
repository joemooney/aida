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
use regex::Regex;
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
    out = adapt_claude_only_prompt_language(&out);
    out = ensure_codex_argument_placeholder(&out);
    out
}

fn adapt_claude_only_prompt_language(body: &str) -> String {
    // trace:BUG-731 | ai:codex
    let claude_skill_ref = Regex::new(r"`?\.claude/skills/(aida-[A-Za-z0-9_-]+)(?:/SKILL)?\.md`?")
        .expect("valid Claude skill ref regex");
    let mut out = claude_skill_ref
        .replace_all(body, |caps: &regex::Captures<'_>| {
            format!("the AIDA skill `{}`", &caps[1])
        })
        .into_owned();

    let ask_user_question =
        Regex::new(r"`?AskUserQuestion`?").expect("valid AskUserQuestion regex");
    out = ask_user_question
        .replace_all(
            &out,
            "plain-text numbered question with concrete options and a recommendation",
        )
        .into_owned();
    out = out.replace(
        "via a structured plain-text numbered question",
        "via a plain-text numbered question",
    );
    out = out.replace("ai:claude", "ai:codex");

    let slash_command = Regex::new(r"(^|[^A-Za-z0-9_.-])/aida-([A-Za-z0-9_-]+)")
        .expect("valid slash command regex");
    slash_command
        .replace_all(&out, |caps: &regex::Captures<'_>| {
            format!("{}aida {}", &caps[1], caps[2].replace('-', " "))
        })
        .into_owned()
}

fn ensure_codex_argument_placeholder(body: &str) -> String {
    // trace:BUG-731 | ai:codex
    format!(
        "Codex prompt arguments: `$ARGUMENTS`\n\n{}",
        body.trim_start()
    )
}

#[cfg(test)]
fn command_template_advertises_arguments(name: &str, body: &str) -> bool {
    let command = format!("/{}", name);
    body.lines().any(|line| {
        let Some(pos) = line.find(&command) else {
            return false;
        };
        let tail = &line[pos + command.len()..];
        tail.contains('<')
            || tail.contains('[')
            || tail.contains("$ARGUMENTS")
            || tail.contains("$1")
    })
}

#[cfg(test)]
fn contains_codex_argument_placeholder(body: &str) -> bool {
    body.contains("$ARGUMENTS") || body.contains("$1")
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
    fn convert_rewrites_claude_skill_refs_to_vendor_neutral_names() {
        let body = "---\ndescription: d\n---\nFollow the workflow in `.claude/skills/aida-commit.md`:\nand `.claude/skills/aida-noncodex.md` stays.";
        let out = convert_command_to_codex_prompt(body);
        assert!(out.contains("the AIDA skill `aida-commit`"), "{out}");
        assert!(out.contains("the AIDA skill `aida-noncodex`"), "{out}");
        assert!(!out.contains(".claude/skills/"), "{out}");
        assert!(out.contains("Codex prompt arguments: `$ARGUMENTS`"));
        assert!(
            !out.starts_with("---"),
            "frontmatter must be stripped: {out}"
        );
    }

    #[test]
    fn convert_adapts_claude_only_prompt_language() {
        let body =
            "Ask via `AskUserQuestion`, then run `/aida-pr` and add `// trace:<SPEC> | ai:claude`.";
        let out = convert_command_to_codex_prompt(body);
        assert!(!out.contains("AskUserQuestion"), "{out}");
        assert!(!out.contains("/aida-"), "{out}");
        assert!(!out.contains("ai:claude"), "{out}");
        assert!(out.contains("plain-text numbered question"), "{out}");
        assert!(out.contains("aida pr"), "{out}");
        assert!(out.contains("ai:codex"), "{out}");
    }

    #[test]
    fn codex_prompts_with_advertised_arguments_reference_codex_arguments() {
        use crate::templates::EMBEDDED_TEMPLATES;

        for (name, rendered) in expected_codex_prompts() {
            let key = format!("commands/{name}.md");
            let command = EMBEDDED_TEMPLATES
                .get(key.as_str())
                .expect("expected prompt came from an embedded command");
            if command_template_advertises_arguments(&name, command) {
                assert!(
                    contains_codex_argument_placeholder(&rendered),
                    "{name} advertises arguments but its Codex prompt has no $ARGUMENTS/$1 placeholder"
                );
            }
        }
    }

    #[test]
    fn expected_codex_prompts_do_not_leak_claude_only_invocations() {
        for (name, body) in expected_codex_prompts() {
            assert!(
                !body.contains("AskUserQuestion"),
                "{name} leaks the Claude-only AskUserQuestion tool"
            );
            assert!(
                !body.contains("/aida-"),
                "{name} leaks Claude slash-command syntax"
            );
        }
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
