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
//! - `.claude/skills/<name>.md` references are rewritten to the workflow
//!   embedded in the rendered Codex prompt, so Codex does not try to invoke a
//!   local skill that may not exist in its advertised skill set;
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
    // Codex custom prompts are the runnable surface. Pointing them at "the
    // AIDA skill" can trigger progressive-disclosure lookup for a skill that
    // is not installed, even though the command template already carries the
    // needed workflow below. trace:TASK-150 | ai:codex
    let mut out = claude_skill_ref
        .replace_all(body, |caps: &regex::Captures<'_>| {
            format!(
                "the Codex-adapted AIDA workflow below (from `{}`)",
                &caps[1]
            )
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

/// Render one embedded command master as a launch-ready Codex prompt: the
/// BUG-731 conversion (frontmatter stripped, Claude-only prompt language
/// adapted) with the invocation arguments substituted for `$ARGUMENTS` — the
/// same expansion Codex itself performs when the prompt is invoked as
/// `/<name> <args>` from `~/.codex/prompts/`, but usable as the initial
/// prompt of a directly-launched session (e.g. the guided-mode dispatch).
/// `None` when the command has no embedded master or sits in the
/// non-portable set.
// trace:TASK-1162 | ai:claude
pub fn render_codex_command_prompt(name: &str, args: &str) -> Option<String> {
    if CODEX_NONPORTABLE_COMMANDS.iter().any(|(n, _)| *n == name) {
        return None;
    }
    let key = format!("commands/{name}.md");
    let body = crate::templates::EMBEDDED_TEMPLATES.get(key.as_str())?;
    Some(convert_command_to_codex_prompt(body).replace("$ARGUMENTS", args))
}

/// The on-disk form of one Codex custom prompt: the converted body stamped
/// with the same `AIDA Generated: v… | checksum:…` header every other
/// scaffolded file carries. The marker is what makes the edit-preserving
/// `aida init --refresh` / `aida scaffold refresh` pass able to tell a
/// pristine prompt (safe to overlay with a fixed template) from one the user
/// has edited (never overwritten) — before it, the only way to deliver a
/// template fix to an existing `~/.codex/prompts` was `--force`.
///
/// The header is an HTML comment, so it is invisible in rendered markdown and
/// inert as prompt text. The *inline* rendering used to launch a session
/// directly (`render_codex_command_prompt`) deliberately does NOT carry it.
// trace:TASK-1170 | ai:claude
pub fn codex_prompt_file_content(name: &str, converted_body: &str) -> String {
    crate::scaffolding::wrap_with_aida_header(
        std::path::Path::new(&format!("{name}.md")),
        converted_body,
    )
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
        let content = codex_prompt_file_content(&name, &convert_command_to_codex_prompt(body));
        out.push((name, content));
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
        // BUG-718: never write through a symlink — a downstream project may
        // link its prompt at a source-of-truth master. trace:TASK-1170
        if crate::scaffolding::symlink_target(&dest).is_some() {
            outcome.skipped_existing.push(name);
            continue;
        }
        let body = EMBEDDED_TEMPLATES
            .get(*key)
            .expect("key came from the same map");
        let prompt = codex_prompt_file_content(&name, &convert_command_to_codex_prompt(body));
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
    fn convert_rewrites_claude_skill_refs_to_embedded_workflow_refs() {
        let body = "---\ndescription: d\n---\nFollow the workflow in `.claude/skills/aida-commit.md`:\nand `.claude/skills/aida-noncodex.md` stays.";
        let out = convert_command_to_codex_prompt(body);
        assert!(
            out.contains("the Codex-adapted AIDA workflow below (from `aida-commit`)"),
            "{out}"
        );
        assert!(
            out.contains("the Codex-adapted AIDA workflow below (from `aida-noncodex`)"),
            "{out}"
        );
        assert!(!out.contains(".claude/skills/"), "{out}");
        assert!(!out.contains("the AIDA skill `"), "{out}");
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

    // trace:TASK-1162 — the guided-mode dispatch launches Codex with this
    // rendering directly; the arguments must be substituted (no `$ARGUMENTS`
    // left for Codex's own expansion, which never runs on a direct launch).
    #[test]
    fn render_codex_command_prompt_substitutes_arguments() {
        let out = render_codex_command_prompt("aida-guided-implement", "TASK-9").unwrap();
        assert!(out.contains("TASK-9"), "{out}");
        assert!(!out.contains("$ARGUMENTS"), "{out}");
        assert!(!out.contains("AskUserQuestion"), "{out}");
        assert!(!out.starts_with("---"), "{out}");
        // Non-portable and unknown commands render nothing.
        assert!(render_codex_command_prompt("aida-burndown", "x").is_none());
        assert!(render_codex_command_prompt("no-such-command", "x").is_none());
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

    /// BUG-797: the first live codex-reviewed drain shelved at phase 3 because
    /// the rendered /aida-review prompt had dropped the verdict-file handshake
    /// — the reviewer wrote a correct prose verdict and the orchestrator,
    /// which polls `.aida/review-verdicts/PR-N.json`, saw nothing. On Claude
    /// the skill carries this contract; on Codex the rendered command body IS
    /// the whole prompt, so the command master must carry it too.
    // trace:BUG-797 | ai:claude
    #[test]
    fn codex_review_prompt_carries_the_verdict_file_handshake() {
        let prompts = expected_codex_prompts();
        let (_, body) = prompts
            .iter()
            .find(|(n, _)| n == "aida-review")
            .expect("aida-review must be in the portable set");
        assert!(
            body.contains(".aida/review-verdicts/PR-N.json"),
            "the phase-3 → phase-4 handshake artifact must survive conversion"
        );
        for v in ["Approved", "RequestChanges", "Rejected"] {
            assert!(
                body.contains(v),
                "verdict vocabulary `{v}` must be stated — the orchestrator parses it"
            );
        }
        assert!(
            body.contains("escalated-to-human"),
            "the headless merge-escalation escape hatch must be documented"
        );
        // BUG-802: the cwd-robust verb is the recommended path — a reviewer
        // that checked the PR out elsewhere hand-writes into the wrong tree.
        assert!(
            body.contains("aida review record"),
            "the rendered prompt must instruct the drive-root-anchored verb"
        );
        // The ordering is the contract: file BEFORE the PR comment (BUG-280).
        let file_pos = body.find("review-verdicts").unwrap();
        let comment_pos = body
            .find("consolidated review comment")
            .expect("comment step present");
        assert!(
            file_pos < comment_pos,
            "verdict file must be instructed BEFORE the PR comment"
        );
    }

    /// BUG-804: with BUG-799 the codex implementer receives the pickup body
    /// and follows it literally — a bare "confirm with the user" step ended a
    /// headless session with "Awaiting your confirmation" and no work done.
    /// The argument-as-consent rule (TASK-86/548) must be IN the rendered body.
    // trace:BUG-804 | ai:claude
    #[test]
    fn codex_pickup_prompt_carries_argument_as_consent() {
        let prompts = expected_codex_prompts();
        let (_, body) = prompts
            .iter()
            .find(|(n, _)| n == "aida-pickup")
            .expect("aida-pickup must be in the portable set");
        assert!(
            body.contains("IS the consent"),
            "argument-as-consent must survive into the rendered body"
        );
        assert!(
            body.contains("AIDA_HEADLESS"),
            "the never-ask-headless rule must be stated"
        );
        assert!(
            body.contains("the Codex-adapted AIDA workflow below (from `aida-pickup`)"),
            "Codex pickup must not require an unavailable local aida-pickup skill"
        );
        assert!(
            !body.contains("the AIDA skill `aida-pickup`"),
            "Codex pickup must not instruct the agent to resolve a local skill"
        );
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

    // trace:BUG-800 | ai:codex
    #[test]
    fn expected_codex_prompts_do_not_suggest_bare_rust_test_names() {
        for (name, body) in expected_codex_prompts() {
            assert!(
                !body.contains("cargo test <test_name>"),
                "{name} suggests a bare Rust test name instead of `cargo test -p <crate> <test_name>`"
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

    // trace:TASK-1170 — every written prompt carries the scaffold-checksum
    // marker, so `aida init --refresh` / `aida scaffold refresh` can tell a
    // pristine prompt from one the user edited and deliver template fixes
    // without --force. The inline launch rendering stays marker-free.
    #[test]
    fn written_prompts_carry_a_refreshable_scaffold_marker() {
        use crate::scaffolding::refresh::{refresh_disposition, RefreshDisposition};

        let tmp = tempfile::tempdir().unwrap();
        scaffold_codex_prompts(tmp.path(), false).unwrap();
        let deployed =
            std::fs::read_to_string(tmp.path().join("aida-guided-implement.md")).unwrap();
        assert!(
            deployed.starts_with("<!-- AIDA Generated: v"),
            "prompt must carry the scaffold marker: {}",
            &deployed[..deployed.len().min(120)]
        );
        assert_eq!(
            refresh_disposition(&deployed),
            RefreshDisposition::Pristine,
            "a freshly-written prompt must read as pristine"
        );
        // The acceptance case: the guided prompt is arg-substitutable and
        // vendor-neutral once delivered.
        assert!(deployed.contains("$ARGUMENTS"), "{deployed}");
        assert!(!deployed.contains("AskUserQuestion"), "{deployed}");

        // A user edit flips it to Edited so refresh keeps their version.
        let edited = deployed.replace("$ARGUMENTS", "$ARGUMENTS (my note)");
        assert_eq!(refresh_disposition(&edited), RefreshDisposition::Edited);

        // The inline launch rendering must NOT carry the file marker.
        let inline = render_codex_command_prompt("aida-guided-implement", "TASK-9").unwrap();
        assert!(!inline.contains("AIDA Generated:"), "{inline}");
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
