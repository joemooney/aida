//! `aida commit` — assemble a conventional-format-compliant commit message and
//! run `git commit` with it, so users never trip the commit-msg hook.
//!
//! The operator hit the hook rejecting `git commit -am "aida updates"` because
//! it isn't conventional-format. This is the CLI-native path that authors the
//! `[AI:tool]? type(scope): description (REQ-ID)` shape for plain-terminal /
//! Codex use (the `/aida-commit` skill is the Claude-session counterpart).
//!
//! The assembled message is validated against the SAME rules the commit-msg
//! hook enforces (mirrored from `aida-core/templates/hooks/aida-commit-msg`)
//! BEFORE committing, so `aida commit` output always passes the hook.
//!
//! trace:STORY-663 | ai:claude

use anyhow::{bail, Context, Result};
use colored::Colorize;
use regex::Regex;
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

/// The conventional-commit types the hook accepts.
const VALID_TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// Inputs for assembling a commit message. Mirrors the `aida commit` flags.
pub(crate) struct CommitArgs {
    pub commit_type: String,
    pub scope: Option<String>,
    pub message: String,
    pub spec: Option<String>,
    pub ai: Option<String>,
    pub no_ai: bool,
    pub all: bool,
    pub dry_run: bool,
}

/// Entry point for `Command::Commit`. Self-contained: needs only git + the
/// staged diff (for REQ-ID inference), no requirements store.
pub(crate) fn handle_commit_command(args: &CommitArgs) -> Result<()> {
    let root =
        crate::find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    // Validate the type up front with a friendly message rather than letting
    // the assembled-message validation produce an opaque error.
    let ctype = args.commit_type.trim().to_lowercase();
    if !VALID_TYPES.contains(&ctype.as_str()) {
        bail!(
            "Unknown commit type '{}'. Valid types: {}",
            args.commit_type,
            VALID_TYPES.join(", ")
        );
    }

    // Gather the staged-trace context once: it drives both REQ-ID inference and
    // the AI-prefix decision (the hook only requires `[AI:tool]` when an
    // AI-authored trace is staged).
    let staged_specs = staged_trace_specs(&root, args.all);
    let has_ai_trace = staged_trace_has_ai(&root, args.all);

    // Resolve the REQ-ID: explicit --spec wins; else infer from staged traces
    // only when exactly one distinct spec is present (ambiguous → omit).
    let req_id = match &args.spec {
        Some(s) => Some(normalize_spec(s)),
        None => {
            if staged_specs.len() == 1 {
                staged_specs.into_iter().next()
            } else {
                None
            }
        }
    };

    // Resolve the AI tag. --no-ai forces it off. An explicit --ai forces it on.
    // Otherwise default to including `[AI:claude]` only when an AI-authored
    // trace is staged (matches the hook's rule), so a pure-human/chore commit
    // doesn't get an unwanted prefix.
    let ai_tool: Option<String> = if args.no_ai {
        None
    } else if let Some(tool) = &args.ai {
        Some(tool.trim().to_string())
    } else if has_ai_trace {
        Some("claude".to_string())
    } else {
        None
    };

    // feat/fix REQUIRE a REQ-ID. Catch the user-fixable case (no --spec and no
    // single inferable spec) with actionable guidance rather than the
    // internal-bug self-check below.
    if (ctype == "feat" || ctype == "fix") && req_id.is_none() {
        bail!(
            "`{ctype}` commits require a (REQ-ID), but none was given and one could not be \
             inferred from staged trace comments.\n  \
             Pass --spec <SPEC-ID> (e.g. --spec BUG-577), or use a non-feat/fix type \
             (chore/docs/refactor/...) if there is no requirement."
        );
    }

    let message = assemble_message(
        &ctype,
        args.scope.as_deref(),
        &args.message,
        &ai_id_pair(req_id.as_deref(), ai_tool.as_deref()),
    );

    // Self-check against the hook rules before committing. After the friendly
    // checks above, any failure here is genuinely unexpected.
    if let Err(reason) = validate_message(&message, has_ai_trace) {
        bail!(
            "Internal: assembled message would fail the commit-msg hook ({reason}).\n  \
             Message: {message}\n  This is a bug — please report."
        );
    }

    if args.dry_run {
        println!("{message}");
        return Ok(());
    }

    // Refuse a no-op: without -a there must be something staged.
    if !args.all && !has_staged_changes(&root) {
        bail!(
            "Nothing staged to commit. Stage changes with `git add <paths>` first, \
             or pass -a/--all to commit all tracked changes."
        );
    }

    // STORY-684: vendor-agnostic advisor-no-code-write gate. The `aida commit`
    // CLI path is one of the two enforcement points (the scaffolded git
    // pre-commit hook is the other, so even a raw `git commit` from any vendor
    // hits it). Refuse an advisor-seat commit that stages code; the gate is
    // silent for non-advisor roles and the sanctioned-coding carve-outs.
    // trace:STORY-684
    crate::advisor_code_gate::enforce_at_commit(&root, args.all)?;

    // STORY-711 slice 2: automatic advisor-lock gate. Same two-enforcement-
    // point pattern as the advisor-code gate above (the scaffolded git
    // pre-commit hook is the other). Silent no-op under the default
    // `[locking] posture = "off"`.
    // trace:TASK-1140 | ai:claude
    crate::locking_gate::enforce_at_commit(&root)?;

    run_git_commit(&root, &message, args.all)?;
    println!(
        "{} committed: {}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        message
    );
    Ok(())
}

/// Bundle the (req_id, ai_tool) so `assemble_message` can stay a pure helper
/// that's trivially unit-testable.
fn ai_id_pair<'a>(
    req_id: Option<&'a str>,
    ai_tool: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>) {
    (ai_tool, req_id)
}

/// Pure assembly of `[AI:tool] type(scope): description (REQ-ID)`. Kept free of
/// I/O so it can be unit-tested directly.
fn assemble_message(
    ctype: &str,
    scope: Option<&str>,
    description: &str,
    ai_and_req: &(Option<&str>, Option<&str>),
) -> String {
    let (ai_tool, req_id) = ai_and_req;
    let mut out = String::new();
    if let Some(tool) = ai_tool {
        out.push_str(&format!("[AI:{tool}] "));
    }
    out.push_str(ctype);
    if let Some(s) = scope {
        let s = s.trim();
        if !s.is_empty() {
            out.push_str(&format!("({s})"));
        }
    }
    out.push_str(": ");
    out.push_str(description.trim());
    if let Some(id) = req_id {
        out.push_str(&format!(" ({id})"));
    }
    out
}

/// Validate the assembled first line against the same rules the commit-msg hook
/// enforces. Returns Err(reason) when the hook would reject it (strict-mode
/// semantics: feat/fix without a REQ-ID is rejected; an AI-traced commit without
/// the `[AI:tool]` prefix is rejected). Mirrors
/// `aida-core/templates/hooks/aida-commit-msg`. trace:STORY-663
pub(crate) fn validate_message(message: &str, has_ai_trace: bool) -> Result<(), String> {
    let first_line = message.lines().next().unwrap_or("");

    // Patterns mirrored from the hook.
    let ai_tool = r"[a-zA-Z]+(\+[a-zA-Z]+)*";
    let conventional = Regex::new(&format!(
        r"^(\[AI:{ai_tool}(:(high|med|low))?\] )?(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-zA-Z0-9_,/[:space:]-]+\))?: .+"
    ))
    .expect("valid conventional regex");
    let feat_fix = Regex::new(&format!(
        r"^(\[AI:{ai_tool}(:(high|med|low))?\] )?(feat|fix)"
    ))
    .expect("valid feat/fix regex");
    let ai_tag =
        Regex::new(&format!(r"^\[AI:{ai_tool}(:(high|med|low))?\]")).expect("valid ai-tag regex");
    // REQ-ID atom: <PREFIX>(-<NODE>)?-<SEQ>(..<N>)? at end of line, optionally a
    // comma/space-separated list with a trailing non-`)` suffix.
    let id_atom = r"[A-Za-z]+(-[A-Za-z0-9_]+)?-[0-9]+(\.\.[0-9]+)?";
    let req_id = Regex::new(&format!(r"\({id_atom}([,[:space:]]+{id_atom})*[^)]*\)$"))
        .expect("valid req-id regex");

    // Validation 1: conventional format.
    if !conventional.is_match(first_line) {
        return Err("not conventional format".to_string());
    }

    // Validation 2: feat/fix require a REQ-ID (strict-mode).
    if feat_fix.is_match(first_line) && !req_id.is_match(first_line) {
        return Err("feat/fix commit missing (REQ-ID)".to_string());
    }

    // Validation 3: AI-traced files require the [AI:tool] prefix (strict-mode).
    if has_ai_trace && !ai_tag.is_match(first_line) {
        return Err("AI-traced commit missing [AI:tool] prefix".to_string());
    }

    Ok(())
}

/// Normalize a user-supplied spec id to upper-case (the storage canonical form
/// the hook matches against). Strips a leading `(`/trailing `)` if pasted.
fn normalize_spec(s: &str) -> String {
    s.trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim()
        .to_uppercase()
}

/// Collect the distinct SPEC-IDs referenced by `trace:` comments in the staged
/// diff. With `all`, also considers unstaged tracked changes (since `-a` will
/// commit them). Returns upper-cased ids.
fn staged_trace_specs(root: &Path, all: bool) -> BTreeSet<String> {
    let diff = diff_text(root, all);
    let re = Regex::new(r"(?i)trace:([A-Z]+(-[A-Z0-9_]+)?-[0-9]+)").expect("valid trace regex");
    let mut out = BTreeSet::new();
    for line in diff.lines() {
        // Only added lines (`+`) carry the committer's new traces; but the hook
        // greps whole files, so we accept any trace in the diff body. Keeping it
        // diff-scoped (added or context) is close enough and avoids a full
        // working-tree scan.
        for cap in re.captures_iter(line) {
            if let Some(m) = cap.get(1) {
                out.insert(m.as_str().to_uppercase());
            }
        }
    }
    out
}

/// True when the staged diff contains an AI-authored trace (`trace:ID | ai:...`),
/// which is what the hook keys the `[AI:tool]` requirement off of.
fn staged_trace_has_ai(root: &Path, all: bool) -> bool {
    let diff = diff_text(root, all);
    let re = Regex::new(r"(?i)trace:[A-Z]+(-[A-Z0-9_]+)?-[0-9]+\s*\|\s*ai:")
        .expect("valid ai-trace regex");
    diff.lines().any(|l| re.is_match(l))
}

/// The diff text to scan for traces. Staged-only by default; staged+unstaged
/// (tracked) when `-a` is set.
fn diff_text(root: &Path, all: bool) -> String {
    let args: &[&str] = if all {
        &["diff", "HEAD"]
    } else {
        &["diff", "--cached"]
    };
    git_stdout(root, args).unwrap_or_default()
}

/// True if there is anything in the staging area.
fn has_staged_changes(root: &Path) -> bool {
    // `git diff --cached --quiet` exits 1 when there ARE staged changes.
    Command::new("git")
        .current_dir(root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false)
}

/// Run `git commit -m <message>` (adding `-a` when requested). Inherits stdio so
/// the commit-msg hook output is visible.
fn run_git_commit(root: &Path, message: &str, all: bool) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.current_dir(root).arg("commit");
    if all {
        cmd.arg("-a");
    }
    cmd.args(["-m", message]);
    let status = cmd.status().context("failed to spawn `git commit`")?;
    if !status.success() {
        bail!("`git commit` failed (the commit-msg hook may have rejected the message)");
    }
    Ok(())
}

/// Capture stdout of a git command, returning None on failure.
fn git_stdout(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(
        ctype: &str,
        scope: Option<&str>,
        desc: &str,
        ai: Option<&str>,
        req: Option<&str>,
    ) -> String {
        assemble_message(ctype, scope, desc, &(ai, req))
    }

    #[test]
    fn assembles_full_shape() {
        let m = assemble(
            "fix",
            Some("rel"),
            "accept --spec alias",
            Some("claude"),
            Some("BUG-577"),
        );
        assert_eq!(m, "[AI:claude] fix(rel): accept --spec alias (BUG-577)");
    }

    #[test]
    fn assembles_without_ai_or_scope_or_req() {
        let m = assemble("chore", None, "update deps", None, None);
        assert_eq!(m, "chore: update deps");
    }

    #[test]
    fn assembles_without_scope_with_req() {
        let m = assemble("feat", None, "add thing", Some("claude"), Some("FR-1-042"));
        assert_eq!(m, "[AI:claude] feat: add thing (FR-1-042)");
    }

    #[test]
    fn trims_whitespace_in_fields() {
        let m = assemble(
            "fix",
            Some("  api  "),
            "  handle null  ",
            None,
            Some("BUG-23"),
        );
        assert_eq!(m, "fix(api): handle null (BUG-23)");
    }

    #[test]
    fn empty_scope_is_omitted() {
        let m = assemble("docs", Some("   "), "tidy readme", None, None);
        assert_eq!(m, "docs: tidy readme");
    }

    // --- validation mirrors the hook ---

    #[test]
    fn valid_full_message_passes() {
        assert!(
            validate_message("[AI:claude] fix(rel): accept --spec alias (BUG-577)", true).is_ok()
        );
    }

    #[test]
    fn chore_without_req_passes() {
        assert!(validate_message("chore: update deps", false).is_ok());
    }

    #[test]
    fn docs_without_req_passes() {
        assert!(validate_message("docs: update README", false).is_ok());
    }

    #[test]
    fn feat_without_req_is_rejected() {
        let e = validate_message("[AI:claude] feat(x): add y", true).unwrap_err();
        assert!(e.contains("REQ-ID"), "got: {e}");
    }

    #[test]
    fn fix_without_req_is_rejected() {
        let e = validate_message("fix(x): bug", false).unwrap_err();
        assert!(e.contains("REQ-ID"), "got: {e}");
    }

    #[test]
    fn non_conventional_is_rejected() {
        let e = validate_message("aida updates", false).unwrap_err();
        assert!(e.contains("conventional"), "got: {e}");
    }

    #[test]
    fn ai_traced_without_prefix_is_rejected() {
        // Valid chore shape, but an AI trace is staged and the prefix is missing.
        let e = validate_message("chore: scaffold", true).unwrap_err();
        assert!(e.contains("[AI:tool]"), "got: {e}");
    }

    #[test]
    fn no_ai_trace_without_prefix_passes() {
        assert!(validate_message("chore: scaffold", false).is_ok());
    }

    #[test]
    fn multi_agent_prefix_accepted() {
        assert!(validate_message(
            "[AI:codex+claude] test(hooks): accept mixed authorship (TASK-509)",
            true
        )
        .is_ok());
    }

    #[test]
    fn confidence_suffix_accepted() {
        assert!(validate_message("[AI:claude:med] fix(api): handle null (BUG-23)", true).is_ok());
    }

    #[test]
    fn comma_list_req_id_accepted() {
        assert!(validate_message("feat(api): two specs (TASK-20, TASK-27)", false).is_ok());
    }

    #[test]
    fn distributed_node_id_accepted() {
        assert!(validate_message("fix(x): y (FR-JPM-42)", false).is_ok());
    }

    #[test]
    fn normalize_spec_uppercases_and_strips_parens() {
        assert_eq!(normalize_spec("(bug-577)"), "BUG-577");
        assert_eq!(normalize_spec("  fr-1-042 "), "FR-1-042");
    }

    // End-to-end: assemble then validate. Every assembled message must pass.
    #[test]
    fn assembled_messages_always_validate() {
        let cases = [
            ("fix", Some("rel"), "x", Some("claude"), Some("BUG-1"), true),
            ("feat", None, "y", Some("claude"), Some("FR-1-2"), true),
            ("chore", None, "z", None, None, false),
            ("docs", Some("readme"), "w", None, None, false),
        ];
        for (ctype, scope, desc, ai, req, has_ai) in cases {
            let m = assemble(ctype, scope, desc, ai, req);
            assert!(
                validate_message(&m, has_ai).is_ok(),
                "assembled message failed validation: {m}"
            );
        }
    }
}
