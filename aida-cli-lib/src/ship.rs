//! `aida ship [<spec>]` — the one-shot HUMAN-implementer finish (STORY-720).
//!
//! The human-implementer counterpart to the `--auto-complete` orchestrator.
//! From inside a worktree where a human has implemented a spec, `aida ship`
//! runs the finish ceremony in ONE command:
//!
//!   commit any uncommitted work with the `(SPEC-ID)` trailer
//!     → rebase onto current `origin/main`
//!     → push
//!     → open the PR
//!     → wait for CI green
//!     → squash-merge
//!     → `aida pull` (Done → Completed auto-bump)
//!     → remove the worktree
//!
//! It is exactly the orchestrator's finish phases (CI-wait, merge, pull,
//! worktree-cleanup, rebase-on-divergence) with phase-1 (implement) done by
//! the HUMAN instead of a spawned agent. The finish half is NOT reimplemented
//! here — the handler in `main.rs::run_human_finish_ceremony` reuses
//! `pr_ship_handler` (TASK-458) for the PR/CI/merge/pull/cleanup tail, the
//! same machinery `aida pr ship` already drives.
//!
//! `run_human_finish_ceremony` is the **shared** finish ceremony: `aida zen`
//! (STORY-721) and `aida integrate` (STORY-718) call the same fn so the
//! commit→rebase→PR→CI→merge→pull→cleanup sequence has exactly one home.
//!
//! This module owns the **pure pieces** so they're unit-testable without
//! `git`/`gh`: spec resolution from the worktree's branch/lease, the commit
//! subject derivation (with the `(SPEC-ID)` trailer), the finish-mode
//! selection from the flags, and the dry-run plan formatting.
//!
//! trace:STORY-720

/// The three finish shapes `aida ship` can take, selected from `--no-pr` /
/// `--no-merge`. Computing this once keeps the handler a flat match on intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishMode {
    /// `--no-pr` — commit + rebase + push, then stop. No PR is opened.
    RebasePushOnly,
    /// `--no-merge` — commit + rebase + push + open-the-PR, then stop. CI is
    /// not watched and the PR is not merged.
    StopAtPr,
    /// Default — the full one-shot finish: commit → rebase → push → PR →
    /// CI → squash-merge → pull → worktree-cleanup.
    FullShip,
}

/// Resolve the finish mode from the two narrowing flags. `--no-pr` is the
/// most restrictive and wins when both are set (a no-PR ship cannot also
/// stop-at-PR).
pub(crate) fn finish_mode(no_pr: bool, no_merge: bool) -> FinishMode {
    if no_pr {
        FinishMode::RebasePushOnly
    } else if no_merge {
        FinishMode::StopAtPr
    } else {
        FinishMode::FullShip
    }
}

/// Resolve the spec id `aida ship` is finishing. Precedence:
///   1. an explicit `<spec>` argument (normalized to upper-case),
///   2. the SPEC-ID encoded in the current branch name
///      (`story-720-ship` → `STORY-720`),
///   3. the scope of the active session lease covering this branch.
///
/// Pure: the caller supplies the branch and the lease scope (read via git /
/// the lease store) so this stays unit-testable. Returns `None` when no spec
/// can be determined — the handler then asks the user for an explicit id.
pub(crate) fn resolve_spec(
    explicit: Option<&str>,
    branch: &str,
    lease_scope: Option<&str>,
) -> Option<String> {
    if let Some(e) = explicit {
        let e = e.trim();
        if !e.is_empty() {
            return Some(e.to_ascii_uppercase());
        }
    }
    if let Some(from_branch) = crate::worktree_lease::spec_id_from_branch(branch) {
        return Some(from_branch);
    }
    // A lease scope is only useful when it is itself a SPEC-ID (the generic
    // harness-worktree scope is not a spec). spec_id_from_branch's recognizer
    // accepts the `TYPE-NUM` shape directly when lower-cased.
    if let Some(scope) = lease_scope {
        let scope = scope.trim();
        if let Some(s) = spec_id_from_scope(scope) {
            return Some(s);
        }
    }
    None
}

/// Recognize a bare SPEC-ID scope (`STORY-720`, `task-688`) and normalize it.
/// Returns `None` for the generic harness-worktree scope or anything that is
/// not a `TYPE-NUM` pair.
fn spec_id_from_scope(scope: &str) -> Option<String> {
    // A lease scope of the form `STORY-720` is recognized by the same branch
    // recognizer (which keys off the `<type>-<number>` head).
    crate::worktree_lease::spec_id_from_branch(scope)
}

/// Build the commit subject used when `aida ship` commits the human's
/// uncommitted work. Guarantees the `(SPEC-ID)` trailer is present so the
/// merge auto-bumps the spec (and the client-side trailer guard passes).
///
///   * a `custom` subject is used verbatim, with ` (SPEC-ID)` appended unless
///     it already names the spec;
///   * absent a custom subject, a sensible conventional default is produced.
pub(crate) fn commit_subject(spec: &str, custom: Option<&str>) -> String {
    let base = custom
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("feat: implement {spec}"));
    ensure_spec_trailer(&base, spec)
}

/// Append a ` (SPEC-ID)` trailer to `subject` unless the parenthesized trailer
/// is already present (so `feat: x (STORY-720)` is left untouched and a double
/// trailer is never produced). Keys off the `(SPEC)` form — the shape the
/// merge auto-bump and the client-side trailer guard recognize — not a bare
/// substring, so a default subject that names the spec in prose still gets its
/// trailer.
fn ensure_spec_trailer(subject: &str, spec: &str) -> String {
    let subject = subject.trim();
    if subject.contains(&format!("({spec})")) {
        subject.to_string()
    } else {
        format!("{subject} ({spec})")
    }
}

/// Render the dry-run / preview plan for a `aida ship` invocation. Mirrors the
/// step lines the handler prints at run time so `--dry-run` is a faithful
/// preview. `dirty` reflects whether there is uncommitted work to commit.
pub(crate) fn format_ship_plan(spec: &str, branch: &str, mode: FinishMode, dirty: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("aida ship — {spec} on branch {branch}\n"));
    let mut n = 1;
    let mut step = |out: &mut String, text: &str| {
        out.push_str(&format!("  {n}. {text}\n"));
        n += 1;
    };
    if dirty {
        step(
            &mut out,
            "commit uncommitted work with the (SPEC-ID) trailer",
        );
    } else {
        step(&mut out, "no uncommitted work — skip commit");
    }
    step(&mut out, "rebase onto current origin/main");
    step(&mut out, "push branch (force-with-lease)");
    match mode {
        FinishMode::RebasePushOnly => {
            step(&mut out, "stop (--no-pr): rebased + pushed, no PR opened");
        }
        FinishMode::StopAtPr => {
            step(&mut out, "open the PR");
            step(&mut out, "stop (--no-merge): PR open, not merged");
        }
        FinishMode::FullShip => {
            step(&mut out, "open the PR");
            step(&mut out, "wait for CI green");
            step(&mut out, "squash-merge the PR");
            step(&mut out, "aida pull (Done → Completed auto-bump)");
            step(&mut out, "remove the worktree");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_mode_selects_from_flags() {
        assert_eq!(finish_mode(false, false), FinishMode::FullShip);
        assert_eq!(finish_mode(false, true), FinishMode::StopAtPr);
        assert_eq!(finish_mode(true, false), FinishMode::RebasePushOnly);
        // --no-pr is the most restrictive and wins over --no-merge.
        assert_eq!(finish_mode(true, true), FinishMode::RebasePushOnly);
    }

    #[test]
    fn explicit_spec_wins_and_is_uppercased() {
        assert_eq!(
            resolve_spec(Some("story-720"), "some-branch", None).as_deref(),
            Some("STORY-720")
        );
        // Whitespace-only explicit arg falls through to the branch.
        assert_eq!(
            resolve_spec(Some("   "), "task-688-x", None).as_deref(),
            Some("TASK-688")
        );
    }

    #[test]
    fn resolves_spec_from_branch_name() {
        assert_eq!(
            resolve_spec(None, "story-720-ship", None).as_deref(),
            Some("STORY-720")
        );
        assert_eq!(
            resolve_spec(None, "bug-42-fix-thing", None).as_deref(),
            Some("BUG-42")
        );
    }

    #[test]
    fn falls_back_to_lease_scope_when_branch_unrecognized() {
        assert_eq!(
            resolve_spec(None, "worktree-agent-deadbeef", Some("STORY-720")).as_deref(),
            Some("STORY-720")
        );
        // A non-spec harness scope yields nothing.
        assert_eq!(
            resolve_spec(None, "worktree-agent-deadbeef", Some("harness-worktree")),
            None
        );
    }

    #[test]
    fn no_spec_anywhere_is_none() {
        assert_eq!(resolve_spec(None, "feature-foo", None), None);
        assert_eq!(resolve_spec(Some(""), "main", None), None);
    }

    #[test]
    fn commit_subject_appends_trailer_to_default() {
        assert_eq!(
            commit_subject("STORY-720", None),
            "feat: implement STORY-720 (STORY-720)"
        );
    }

    #[test]
    fn commit_subject_uses_custom_and_appends_trailer() {
        assert_eq!(
            commit_subject("BUG-42", Some("fix(api): handle null response")),
            "fix(api): handle null response (BUG-42)"
        );
    }

    #[test]
    fn commit_subject_does_not_double_trailer() {
        assert_eq!(
            commit_subject("BUG-42", Some("fix(api): handle null (BUG-42)")),
            "fix(api): handle null (BUG-42)"
        );
    }

    #[test]
    fn plan_full_ship_lists_every_phase() {
        let plan = format_ship_plan("STORY-720", "story-720-ship", FinishMode::FullShip, true);
        assert!(plan.contains("commit uncommitted work"));
        assert!(plan.contains("rebase onto current origin/main"));
        assert!(plan.contains("squash-merge the PR"));
        assert!(plan.contains("auto-bump"));
        assert!(plan.contains("remove the worktree"));
    }

    #[test]
    fn plan_no_pr_stops_after_push() {
        let plan = format_ship_plan("STORY-720", "b", FinishMode::RebasePushOnly, false);
        assert!(plan.contains("no uncommitted work"));
        assert!(plan.contains("no PR opened"));
        assert!(!plan.contains("squash-merge"));
    }

    #[test]
    fn plan_no_merge_stops_at_pr() {
        let plan = format_ship_plan("STORY-720", "b", FinishMode::StopAtPr, true);
        assert!(plan.contains("open the PR"));
        assert!(plan.contains("not merged"));
        assert!(!plan.contains("squash-merge"));
    }
}
