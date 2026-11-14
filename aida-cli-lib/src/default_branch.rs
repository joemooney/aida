// trace:TASK-3428 | ai:codex
use crate::{command_output_with_timeout_detail, BoundedCommandOutput};

// ============================================================================
// `aida plan verify` — lint a docs/plans/ file against the structured
// template (TASK-92). Reports drifted line refs, missing files, and absent
// required sections; exits non-zero on any hard failure so it can run as a
// pre-commit hook. trace:TASK-93 | ai:claude
// ============================================================================

// `aida skill lint` — the forward-guard for skills that reference an
// implementation plan. A skill under `.claude/skills/` that points at a
// `docs/plans/*.md` file can rot when that plan drifts or is removed; this
// scans each skill for plan references, runs the same checks `aida plan
// verify` runs on each referenced plan (drifted refs / missing files / absent
// sections), and raw-glyph-checks the skill body. Read-only; never rewrites a
// plan or a skill. trace:TASK-927 | ai:claude

// ============================================================================
// TASK-94 — `aida plan helpers <spec>`. Derive a "Reusable helpers" section
// by walking the requirement graph (sibling / same-feature / same-tag specs)
// and harvesting those specs' `// trace:` comments from the codebase, so the
// implementer reuses existing helpers instead of re-inventing them.
// trace:TASK-94 | ai:claude
// ============================================================================

/// TASK-241: surface git linkage for a spec inside `aida show` — the
/// commits that reference it (AIDA commit format puts `(SPEC-ID)` in
/// the message), the files carrying its `trace:` comments, the branch +
/// worktree the work lives on, and PR state. Every sub-probe is
/// best-effort: a missing `gh`, an absent `origin/main`, or zero
/// matches degrades to a dim note rather than failing the show.
/// `--no-git` skips the whole section; `--verbose` un-caps the commit
/// list and adds per-commit diff stats. trace:TASK-241 | ai:claude
/// Resolve a refname for the default branch that is **known to exist**
/// in `project_root`, so callers can pass it to `git merge-base
/// --is-ancestor` without risking a `fatal: Not a valid object name <X>`
/// stderr splice when the repo's default branch is not `main`.
///
/// Resolution order:
///   1. `origin/HEAD` symbolic ref (the actual default branch on origin)
///   2. `init.defaultBranch` config — `origin/<name>` then bare `<name>`
///   3. The `"main"` / `"master"` literals — `origin/<name>` then bare
///
/// Returns `None` when no candidate ref exists; callers should then
/// skip the ancestor check rather than guess (the prior code passed
/// `"main"` as a literal, which leaked stderr on master-default repos).
/// trace:BUG-380 | ai:claude
pub(crate) fn resolve_default_branch_ref(project_root: &std::path::Path) -> Option<String> {
    use std::process::Command as PCmd;
    let git = |args: &[&str]| -> std::result::Result<Option<String>, std::convert::Infallible> {
        Ok(PCmd::new("git")
            .arg("-C")
            .arg(project_root)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
            .filter(|s| !s.is_empty()))
    };
    match walk_default_branch_candidates(git) {
        Ok(resolved) => resolved,
        Err(never) => match never {},
    }
}

/// The one default-branch candidate walk behind [`resolve_default_branch_ref`]
/// and [`resolve_default_branch_ref_until`]: `origin/HEAD`, then
/// `init.defaultBranch`, then `main` / `master`, each tried as `origin/<name>`
/// before `<name>`. `git` runs one git call and yields its trimmed, non-empty
/// stdout (`Ok(None)` on failure); an `Err` from it (the bounded twin's
/// deadline) stops the walk. Keeping the order here keeps show and the gates
/// agreeing on the default branch.
// trace:TASK-2000 trace:TASK-1999 | ai:claude
fn walk_default_branch_candidates<E>(
    mut git: impl FnMut(&[&str]) -> std::result::Result<Option<String>, E>,
) -> std::result::Result<Option<String>, E> {
    if let Some(short) = git(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])? {
        return Ok(Some(short));
    }

    let mut candidates: Vec<String> = Vec::new();
    if let Some(cfg) = git(&["config", "--get", "init.defaultBranch"])? {
        candidates.push(cfg);
    }
    for lit in ["main", "master"] {
        if !candidates.iter().any(|c| c == lit) {
            candidates.push(lit.to_string());
        }
    }

    for name in candidates {
        let origin = format!("origin/{}", name);
        if git(&["rev-parse", "--verify", "--quiet", &origin])?.is_some() {
            return Ok(Some(origin));
        }
        if git(&["rev-parse", "--verify", "--quiet", &name])?.is_some() {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// BUG-1993: resolving the default branch hit the caller's deadline before git
/// finished. Distinct from `Ok(None)` (no candidate ref exists in the repo),
/// so show/linkage callers render "unknown" instead of "not shipped" or "no
/// default branch".
// trace:BUG-1993 | ai:antigravity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DefaultBranchResolveTimedOut;

/// [`resolve_default_branch_ref`] bounded by `deadline`, for `aida show`
/// linkage callers only (non-show gate callers use the unbounded
/// [`resolve_default_branch_ref`] so a slow git call never fails open as
/// "no default branch"). Returns `Err` when the deadline expires before or
/// during any git subprocess.
// trace:BUG-1993 | ai:antigravity
pub(crate) fn resolve_default_branch_ref_until(
    project_root: &std::path::Path,
    deadline: std::time::Instant,
) -> std::result::Result<Option<String>, DefaultBranchResolveTimedOut> {
    use std::process::Command as PCmd;
    let git = |args: &[&str]| -> std::result::Result<Option<String>, DefaultBranchResolveTimedOut> {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(DefaultBranchResolveTimedOut);
        }
        let mut cmd = PCmd::new("git");
        cmd.arg("-C").arg(project_root).args(args);
        match command_output_with_timeout_detail(cmd, remaining) {
            BoundedCommandOutput::Completed(o) if o.status.success() => Ok(Some(
                String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
            )
            .filter(|s| !s.is_empty())),
            BoundedCommandOutput::TimedOut => Err(DefaultBranchResolveTimedOut),
            _ => Ok(None),
        }
    };

    walk_default_branch_candidates(git)
}
