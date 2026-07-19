//! Vendor-agnostic substrate gate for the advisor-no-code-write invariant
//! (STORY-684).
//!
//! ## The gap this closes
//!
//! The invariant "a session wearing the ADVISOR hat must not write code /
//! implement" (STORY-670) was, until this module, enforced *only* by a Claude
//! Code `PreToolUse` hook (`aida-advisor-code-guard.sh`) on `Edit|Write|
//! MultiEdit`. That hook is a Claude-vendor mechanism: it never fires for a
//! different vendor's agent (Codex, etc.) — so the invariant evaporated the
//! moment a non-Claude agent wore the advisor hat. That is exactly the failure
//! mode the substrate-as-bouncer principle warns about: a load-bearing
//! invariant lived in a vendor-specific prompt/hook layer, not in the substrate.
//!
//! ## The enforcement point chosen (and why it is vendor-agnostic)
//!
//! The vendor-neutral boundary that EVERY agent — Claude, Codex, a human, a
//! headless `claude -p` child — crosses when it ships code is **`git commit`**.
//! AIDA already scaffolds a git `pre-commit` hook (`aida-pre-commit.sh`) and
//! owns a CLI commit path (`aida commit`). This module is the single shared
//! decision both call:
//!
//!   1. `aida commit` (the CLI commit path) calls [`enforce_at_commit`] before
//!      invoking `git commit`.
//!   2. The scaffolded git `pre-commit` hook shells out to the hidden
//!      `aida internal advisor-code-gate` subcommand, which calls the same
//!      [`enforce_at_commit`]. A git `pre-commit` hook runs no matter which
//!      vendor (or none) drove the `git commit` — Codex's shell-out, a raw
//!      terminal commit, a headless agent — so the gate binds them all.
//!
//! This is the symmetric counterpart to TASK-647's queue-add gate (which
//! refuses a non-advisor session the *approve / queue-for-work* affordance):
//! here we refuse an *advisor* session the *commit-code* affordance.
//!
//! ## What it refuses
//!
//! A commit is refused (exit non-zero / `bail!`) when ALL of:
//!   - the effective role is `advisor` (resolved via the ADR-2 resolver, which
//!     reads roster → `AIDA_SESSION_ROLE` → default), AND
//!   - the staged changes contain at least one recognized CODE file (the same
//!     extension allow-list the Claude hook uses; specs/plans/docs/config are
//!     legitimate advisor work and never trip it), AND
//!   - no sanctioned-coding context is active.
//!
//! ## Sanctioned-coding contexts (the gate stays silent)
//!
//!   - `AIDA_AUTO_COMPLETE` set — an orchestrator / drain implementer child is a
//!     sanctioned coder even if it inherited an advisor role env.
//!   - solo mode active (`~/.aida/solo.toml`, TTL-honored) — the operator's
//!     explicit opt-in to code+integrate from the advisor seat.
//!   - **agent-work context (BUG-622)** — a fanned agent / child session is
//!     structurally NEVER the human advisor seat the gate protects, even when it
//!     inherited `AIDA_SESSION_ROLE=advisor` from the parent advisor session. The
//!     advisor *seat* is the human at the keyboard in the top-level session of the
//!     main checkout; a worktree-isolated subagent fanned off it is a sanctioned
//!     implementer. Detected by positive agent-context signals (see
//!     [`in_agent_work_context`]). Without this carve-out the STORY-684 gate
//!     false-blocked every legitimate fanned-implementer commit, forcing
//!     `--no-verify` (which bypasses ALL hooks, defeating the gate) or a manual
//!     `AIDA_SESSION_ROLE=implementer`. trace:BUG-622
//!   - the explicit escape hatch `AIDA_ALLOW_ADVISOR_CODE=1` — see below.
//!
//! ## Escape hatch (FLAGGED for review)
//!
//! `AIDA_ALLOW_ADVISOR_CODE=1` (env) bypasses the gate for the current process.
//! Rationale: this gate is a *correctness guardrail on a shared store*, not a
//! security boundary against a malicious actor — a determined advisor session
//! can always `--no-verify` the pre-commit hook or `git commit` directly. The
//! honest move is one explicit, auditable, uniform opt-in rather than a maze of
//! special cases. It is intentionally NOT a `--force`-style silent default; it
//! must be deliberately set. (Mirrors the `AIDA_ALLOW_INTERMEDIATE` /
//! `--allow-intermediate` pattern already in the pre-commit hook.) The
//! pre-commit hook's own `--no-verify` is the lower-level git-native escape.
//!
//! trace:STORY-684 | ai:claude

use std::path::Path;
use std::process::Command;

/// Env var: explicit, auditable opt-out of the advisor code-commit gate for the
/// current process. See the module-level "Escape hatch" note.
// trace:STORY-684
pub(crate) const ALLOW_ENV: &str = "AIDA_ALLOW_ADVISOR_CODE";

/// File extensions treated as CODE for the purposes of this gate. Kept in lock
/// step with the Claude `aida-advisor-code-guard.sh` hook's extension list so
/// the two enforcement points refuse exactly the same set. Specs, plans, docs,
/// and config (`.md`, `.toml`, `.yaml`, `.json`, …) are deliberately ABSENT —
/// editing those is legitimate advisor work.
// trace:STORY-684
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "rb", "c", "h", "hpp", "cc", "cpp", "cs",
    "kt", "swift", "php", "sh",
];

/// PURE: is this path a code file by the gate's extension allow-list?
///
/// Mirrors the Claude hook: only the recognized source extensions count;
/// everything else (docs/specs/config/unknown) is treated as advisor-legitimate
/// and does NOT trip the gate. Case-insensitive on the extension.
// trace:STORY-684
pub(crate) fn is_code_file(path: &str) -> bool {
    // Extract the final extension; the diff lists are already UTF-8 strings.
    let ext = match path.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext,
        _ => return false,
    };
    let ext = ext.to_ascii_lowercase();
    CODE_EXTENSIONS.contains(&ext.as_str())
}

/// PURE: the gate decision, free of all I/O so it is exhaustively unit-testable.
///
/// Returns `Some(message)` when the commit must be REFUSED (the advisor seat is
/// trying to ship code with no sanctioned context), or `None` when the commit
/// is allowed. The inputs are everything the decision depends on:
///   - `role`: the resolved effective role (already canonicalized).
///   - `staged_paths`: the staged file paths (classification happens here).
///   - `auto_complete`: an orchestrator/drain child (sanctioned coder).
///   - `solo_active`: solo mode is on (operator opt-in to advisor coding).
///   - `agent_context`: a fanned agent / child session (BUG-622) — structurally
///     not the human advisor seat, even with an inherited `AIDA_SESSION_ROLE=advisor`.
///   - `override_set`: the `AIDA_ALLOW_ADVISOR_CODE` escape hatch is set.
// trace:STORY-684 trace:BUG-622
pub(crate) fn refusal<'a, I>(
    role: &str,
    staged_paths: I,
    auto_complete: bool,
    solo_active: bool,
    agent_context: bool,
    override_set: bool,
) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    // Only advisor sessions are gated; every other role codes freely.
    if role != "advisor" {
        return None;
    }
    // Sanctioned-coding contexts and the explicit escape hatch pass silently.
    // BUG-622: a fanned agent / child session is a sanctioned implementer even
    // when it inherited the parent advisor session's role env.
    if auto_complete || solo_active || agent_context || override_set {
        return None;
    }
    // Collect the staged code files; no code → nothing to refuse (an advisor
    // committing specs/plans/docs/config is doing legitimate advisor work).
    let code_files: Vec<&str> = staged_paths
        .into_iter()
        .filter(|p| is_code_file(p))
        .collect();
    if code_files.is_empty() {
        return None;
    }
    Some(build_refusal_message(&code_files))
}

/// Build the operator-facing refusal text. Kept separate so the wording is
/// unit-testable and the decision logic stays terse.
// trace:STORY-684
fn build_refusal_message(code_files: &[&str]) -> String {
    let mut shown = code_files.to_vec();
    shown.sort_unstable();
    shown.dedup();
    let preview: Vec<String> = shown.iter().take(5).map(|p| format!("  - {p}")).collect();
    let more = if shown.len() > 5 {
        format!("\n  ... and {} more", shown.len() - 5)
    } else {
        String::new()
    };
    // Plain ASCII on purpose: this is an operator-facing stderr refusal, so it
    // stays legible under any terminal profile without routing through the glyph
    // registry. trace:STORY-684
    format!(
        "Refusing commit: you are in the ADVISOR seat (AIDA_SESSION_ROLE=advisor) and \
this commit stages code.\nThe advisor seat does specs, routing, and review -- not \
implementation. Staged code:\n{}{}\nPick one:\n  \
- aida role enter implementer              : switch hats and implement it yourself\n  \
- aida queue add <SPEC> --for implementer  : route the work to an implementer\n  \
- aida solo                                : operator-sanctioned, code + integrate from this seat\n\
This is a vendor-agnostic substrate gate (STORY-684), not a Claude-only hook. To \
override deliberately for this process: AIDA_ALLOW_ADVISOR_CODE=1 (audited escape \
hatch), or git commit --no-verify.",
        preview.join("\n"),
        more
    )
}

/// True when solo mode is effectively active right now (flag set AND within its
/// TTL). Thin wrapper over the presence module.
// trace:STORY-684
fn solo_active_now() -> bool {
    crate::presence::current_solo(chrono::Utc::now())
}

/// PURE: is the commit happening in a fanned-agent / child-session context?
///
/// The advisor *seat* the gate protects is the human at the keyboard in the
/// top-level session of the main checkout. A worktree-isolated subagent fanned
/// off an advisor session inherits `AIDA_SESSION_ROLE=advisor` from the parent's
/// env, but it is structurally a sanctioned implementer — never the advisor seat.
/// This detector returns true on any positive agent-context signal so the gate
/// stops false-blocking those commits (BUG-622). It is conservative: a real
/// advisor-seat human commit (top-level session, main checkout) carries NONE of
/// these signals, so it stays gated.
///
/// Pure over its inputs (the env values + cwd) so it is unit-testable without
/// touching the process env. Signals, vendor-neutral first:
///   - `AIDA_AGENT_TYPE` set — an AIDA-launched agent (`aida agent new …`,
///     `run_tracked_agent` / the bg-dispatch path stamp it). Vendor-neutral.
///   - `CLAUDE_CODE_CHILD_SESSION` set — a Claude `Agent`-tool fanned subagent
///     (the exact leak BUG-622 reports). Claude-specific, but additive: it only
///     ever widens the carve-out for a genuine child session.
///   - the commit's cwd is inside an agent-managed isolation worktree
///     (`.claude/worktrees/agent-<id>`), the path stamp every fan-out launcher
///     uses — covers a child session whose env was scrubbed but still runs in an
///     agent worktree.
// trace:BUG-622 | ai:claude
pub(crate) fn agent_work_context(
    aida_agent_type: Option<&str>,
    claude_child_session: Option<&str>,
    cwd: &Path,
) -> bool {
    let env_signal = |v: Option<&str>| v.map(|s| !s.trim().is_empty()).unwrap_or(false);
    if env_signal(aida_agent_type) || env_signal(claude_child_session) {
        return true;
    }
    let cwd_str = cwd.to_string_lossy().replace('\\', "/");
    cwd_str.contains("/.claude/worktrees/agent-") || cwd_str.contains(".claude/worktrees/agent-")
}

/// Live wrapper over [`agent_work_context`]: reads the process env + cwd. Kept
/// thin so the decision stays pure/testable.
// trace:BUG-622
fn in_agent_work_context(root: &Path) -> bool {
    let agent_type = std::env::var("AIDA_AGENT_TYPE").ok();
    let child_session = std::env::var("CLAUDE_CODE_CHILD_SESSION").ok();
    // Prefer the commit root (the worktree being committed into); fall back to
    // the process cwd. Either being an agent worktree is a positive signal.
    let cwd = std::env::current_dir().unwrap_or_else(|_| root.to_path_buf());
    agent_work_context(agent_type.as_deref(), child_session.as_deref(), root)
        || agent_work_context(agent_type.as_deref(), child_session.as_deref(), &cwd)
}

/// List the file paths that this commit will include (Added/Copied/Modified/
/// Renamed). With `include_unstaged` false (the default / pre-commit-hook case)
/// it reads `git diff --cached`, exactly what's staged. With it true (the
/// `aida commit -a` case, where git stages tracked changes at commit time) it
/// reads `git diff HEAD`, so the gate sees what `-a` will pull in. Best-effort:
/// a git failure yields an empty list (fail-open — see the module note: this is
/// a guardrail, not a security wall).
// trace:STORY-684
fn staged_paths(root: &Path, include_unstaged: bool) -> Vec<String> {
    let args: &[&str] = if include_unstaged {
        &["diff", "HEAD", "--name-only", "--diff-filter=ACMR"]
    } else {
        &["diff", "--cached", "--name-only", "--diff-filter=ACMR"]
    };
    let out = Command::new("git").current_dir(root).args(args).output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Enforce the advisor code-commit gate at a real commit boundary.
///
/// Gathers the live context (effective role, staged paths, sanctioned-context
/// flags) and returns `Err` with the refusal message when the commit must be
/// blocked, or `Ok(())` when it may proceed. Both the `aida commit` CLI path
/// and the `aida internal advisor-code-gate` subcommand (called by the git
/// pre-commit hook) funnel through here, so the decision is identical no matter
/// which vendor drove the commit.
///
/// `include_unstaged` should be true only for the `aida commit -a` path (where
/// git stages tracked changes at commit time); the pre-commit-hook path passes
/// false because git has already staged everything by the time the hook runs.
// trace:STORY-684
pub(crate) fn enforce_at_commit(root: &Path, include_unstaged: bool) -> anyhow::Result<()> {
    let role = crate::effective_role_with_roster().0;
    let auto_complete = std::env::var("AIDA_AUTO_COMPLETE")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let override_set = std::env::var(ALLOW_ENV)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let solo = solo_active_now();
    // BUG-622: a fanned agent / child session that inherited the advisor role
    // env is a sanctioned implementer, not the human advisor seat.
    let agent_context = in_agent_work_context(root);
    let paths = staged_paths(root, include_unstaged);
    if let Some(msg) = refusal(
        &role,
        paths.iter().map(String::as_str),
        auto_complete,
        solo,
        agent_context,
        override_set,
    ) {
        anyhow::bail!(msg);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refuse(role: &str, paths: &[&str]) -> Option<String> {
        refusal(role, paths.iter().copied(), false, false, false, false)
    }

    // ── file classification ──

    #[test]
    fn rust_and_common_sources_are_code() {
        for p in [
            "src/main.rs",
            "a/b/foo.ts",
            "x.tsx",
            "y.py",
            "z.go",
            "k.java",
            "m.cpp",
            "n.sh",
        ] {
            assert!(is_code_file(p), "{p} should be code");
        }
    }

    #[test]
    fn docs_specs_config_are_not_code() {
        for p in [
            "README.md",
            "docs/plans/x.md",
            ".aida/config.toml",
            "Cargo.toml",
            "data.json",
            "x.yaml",
            "x.yml",
            "Cargo.lock",
            "notes.txt",
            "Makefile",
            "no_extension",
        ] {
            assert!(!is_code_file(p), "{p} should NOT be code");
        }
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        assert!(is_code_file("Foo.RS"));
        assert!(is_code_file("Bar.Py"));
    }

    // ── the gate decision (the security-relevant truth table) ──

    #[test]
    fn advisor_committing_code_is_refused() {
        let msg = refuse("advisor", &["src/main.rs"]);
        assert!(msg.is_some(), "advisor + code must refuse");
        let m = msg.unwrap();
        assert!(m.contains("ADVISOR seat"), "got: {m}");
        assert!(m.contains("src/main.rs"), "lists the offending file: {m}");
    }

    #[test]
    fn advisor_committing_only_docs_is_allowed() {
        assert!(
            refuse("advisor", &["README.md", "docs/x.md", ".aida/config.toml"]).is_none(),
            "advisor editing docs/specs/config is legitimate"
        );
    }

    #[test]
    fn advisor_mixed_docs_and_code_is_refused() {
        // One code file among docs is still a code commit.
        assert!(refuse("advisor", &["README.md", "src/lib.rs"]).is_some());
    }

    #[test]
    fn implementer_committing_code_is_allowed() {
        assert!(
            refuse("implementer", &["src/main.rs"]).is_none(),
            "implementer is the sanctioned coder"
        );
    }

    #[test]
    fn reviewer_and_default_roles_commit_code_freely() {
        assert!(refuse("reviewer", &["src/main.rs"]).is_none());
        assert!(refuse("", &["src/main.rs"]).is_none());
        assert!(refuse("operator", &["src/main.rs"]).is_none());
    }

    #[test]
    fn empty_stage_is_allowed_even_for_advisor() {
        assert!(refuse("advisor", &[]).is_none());
    }

    // ── sanctioned-context carve-outs ──

    #[test]
    fn auto_complete_lets_advisor_commit_code() {
        // An orchestrator/drain child is a sanctioned coder.
        assert!(
            refusal(
                "advisor",
                ["src/main.rs"].into_iter(),
                true,
                false,
                false,
                false
            )
            .is_none(),
            "AIDA_AUTO_COMPLETE must bypass"
        );
    }

    #[test]
    fn solo_mode_lets_advisor_commit_code() {
        assert!(
            refusal(
                "advisor",
                ["src/main.rs"].into_iter(),
                false,
                true,
                false,
                false
            )
            .is_none(),
            "solo mode must bypass"
        );
    }

    #[test]
    fn explicit_override_lets_advisor_commit_code() {
        assert!(
            refusal(
                "advisor",
                ["src/main.rs"].into_iter(),
                false,
                false,
                false,
                true
            )
            .is_none(),
            "AIDA_ALLOW_ADVISOR_CODE must bypass"
        );
    }

    // ── BUG-622: fanned-agent / child-session carve-out ──

    #[test]
    fn agent_context_lets_inherited_advisor_commit_code() {
        // The exact BUG-622 case: a worktree implementer agent fanned from an
        // advisor session inherits AIDA_SESSION_ROLE=advisor, but is a sanctioned
        // implementer — the gate must NOT false-block its code commit.
        assert!(
            refusal(
                "advisor",
                ["src/main.rs"].into_iter(),
                false,
                false,
                true,
                false
            )
            .is_none(),
            "a fanned-agent / child-session context must bypass (BUG-622)"
        );
    }

    #[test]
    fn human_advisor_seat_without_agent_context_is_still_refused() {
        // The gate must NOT weaken for a genuine advisor-seat human commit: no
        // agent context, no other carve-out → still refused. (BUG-622 must not
        // open a trivial bypass.)
        assert!(
            refusal(
                "advisor",
                ["src/main.rs"].into_iter(),
                false,
                false,
                false,
                false
            )
            .is_some(),
            "human advisor seat (no agent context) still gated"
        );
    }

    #[test]
    fn agent_type_env_signals_agent_context() {
        // AIDA-launched agent: AIDA_AGENT_TYPE set is a positive signal.
        assert!(agent_work_context(Some("claude"), None, Path::new("/repo")));
        // Empty / whitespace env value is NOT a signal.
        assert!(!agent_work_context(Some(""), None, Path::new("/repo")));
        assert!(!agent_work_context(Some("  "), None, Path::new("/repo")));
    }

    #[test]
    fn claude_child_session_env_signals_agent_context() {
        // A Claude `Agent`-tool fanned subagent — the literal BUG-622 leak.
        assert!(agent_work_context(None, Some("1"), Path::new("/repo")));
    }

    #[test]
    fn agent_worktree_cwd_signals_agent_context() {
        // A commit inside an agent-managed isolation worktree, even with the env
        // scrubbed, is an agent context.
        assert!(agent_work_context(
            None,
            None,
            Path::new("/home/u/proj/.claude/worktrees/agent-abc123")
        ));
    }

    #[test]
    fn plain_main_checkout_is_not_agent_context() {
        // The human advisor seat: top-level session, main checkout, no agent env.
        assert!(!agent_work_context(None, None, Path::new("/home/u/proj")));
    }

    #[test]
    fn refusal_message_dedups_and_truncates_file_list() {
        let files = [
            "a.rs", "a.rs", "b.rs", "c.rs", "d.rs", "e.rs", "f.rs", "g.rs",
        ];
        let msg = refuse("advisor", &files).unwrap();
        // 7 distinct files: preview caps at 5 + "... and 2 more".
        assert!(msg.contains("... and 2 more"), "got: {msg}");
    }
}
