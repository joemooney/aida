// trace:TASK-3428 | ai:codex
use crate::git_arg_guard;

/// A behind-base warning. Workflow content identities determine whether CI's
/// definition changed; changed paths explain the difference and ordinary test
/// changes remain advisory. The head and base are pinned for the entire read.
/// STORY-1690's independent behind-base gate still refuses any behind head.
// trace:BUG-1468 trace:STORY-1690 trace:STORY-2401 trace:TASK-3429 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleCheckWarning {
    pub(crate) behind_commits: u64,
    // trace:STORY-2401 | ai:codex
    pub(crate) head_digest: String,
    pub(crate) base_digest: String,
    pub(crate) definition_files: Vec<String>,
    pub(crate) test_files: Vec<String>,
}

/// Pure classifier for the two tiers above. `workflow_is_pr_triggered`
/// decides whether a changed `.github/workflows/...` file's `on:` triggers
/// include `pull_request` (a non-PR-triggered workflow — cron/nightly,
/// release, manual dispatch — does NOT count as a definition change at
/// all: it lands in neither tier). `script_is_pr_workflow_invoked` decides
/// whether a `scripts/...` path is one a PR-triggered workflow calls
/// directly. Callers resolve both via git on `base_ref` (tests fake them
/// directly, no git needed).
// trace:BUG-1468 | ai:claude
pub(crate) fn classify_changed_files(
    changed_files: &[String],
    workflow_is_pr_triggered: &dyn Fn(&str) -> bool,
    script_is_pr_workflow_invoked: &dyn Fn(&str) -> bool,
) -> (Vec<String>, Vec<String>) {
    let mut definition = Vec::new();
    let mut test_only = Vec::new();
    for f in changed_files {
        if is_workflow_path(f) {
            if workflow_is_pr_triggered(f) {
                definition.push(f.clone());
            }
            // else: a nightly/cron/dispatch/release-only workflow — its
            // green never covered this PR's check to begin with, so it
            // doesn't count in either tier.
        } else if is_script_path(f) && script_is_pr_workflow_invoked(f) {
            definition.push(f.clone());
        } else if is_test_path(f) {
            test_only.push(f.clone());
        }
    }
    (definition, test_only)
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_workflow_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/") || lower.contains("/.github/workflows/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_script_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("scripts/") || lower.contains("/scripts/")
}

// trace:BUG-1468 | ai:claude
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.sh")
}

/// True when a `.github/workflows/...` file's content, AT `base_ref`,
/// mentions `pull_request` — a plain substring check (no YAML parsing),
/// standing in for "this workflow's `on:` triggers include `pull_request`"
/// (BUG-1468 follow-up 2). A nightly/cron, release, or manual-dispatch-only
/// workflow never gates a PR's own check, so it must not count as a
/// definition change even though it lives under `.github/workflows/`.
/// False on any git error (fail-open — a git hiccup demotes a definition
/// change to "doesn't count" rather than blocking a ship on its own).
// trace:BUG-1468 | ai:claude
pub(crate) fn workflow_is_pr_triggered(
    repo: &std::path::Path,
    base_ref: &str,
    workflow_path: &str,
) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &format!("{base_ref}:{workflow_path}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("pull_request"))
        .unwrap_or(false)
}

/// The `.github/workflows/*` files, AT `base_ref`, whose content mentions
/// `pull_request` — the PR-triggered subset a `scripts/` change must be
/// invoked by to count as a definition change. Empty on any git error
/// (fail-open — see [`workflow_is_pr_triggered`]).
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_triggered_workflow_files(repo: &std::path::Path, base_ref: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "grep",
            "-l",
            "-F",
            "pull_request",
            base_ref,
            "--",
            ".github/workflows",
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    // `git grep -l <rev> -- <pathspec>` prints "<rev>:<path>" per match.
    let prefix = format!("{base_ref}:");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix(prefix.as_str()).map(|p| p.to_string()))
        .collect()
}

/// True when `script_path` (a `scripts/...` file that changed) is invoked
/// directly by one of the PR-TRIGGERED workflows on `base_ref` — a literal
/// substring match of the path inside those tracked `.github/workflows/*`
/// blobs. A workflow that invokes the same script but isn't itself
/// PR-triggered (nightly, release, dispatch-only) does not count (BUG-1468
/// follow-up 2). False on any git error (fail-open).
// trace:BUG-1468 | ai:claude
pub(crate) fn script_referenced_by_pr_workflows(
    repo: &std::path::Path,
    base_ref: &str,
    script_path: &str,
) -> bool {
    pr_triggered_workflow_files(repo, base_ref)
        .iter()
        .any(|wf| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["show", &format!("{base_ref}:{wf}")])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).contains(script_path))
                .unwrap_or(false)
        })
}

/// Resolve the ref to measure `branch`'s staleness FROM: `origin/<branch>`
/// when that remote-tracking ref exists (what a PR's own CI actually ran
/// against), else the local `branch` ref itself (BUG-1468 follow-up 3 — a
/// local checkout can be ahead or behind what's actually pushed/reviewed).
// trace:BUG-1468 | ai:claude
pub(crate) fn stale_check_branch_ref(repo: &std::path::Path, branch: &str) -> String {
    let remote_ref = format!("origin/{branch}");
    let exists = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "--quiet", &remote_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if exists {
        remote_ref
    } else {
        branch.to_string()
    }
}

/// git-IO wrapper: how far `branch` is behind `base_ref`, and the two-tier
/// classification (see [`StaleCheckWarning`]) of what changed on `base_ref`
/// since divergence. Staleness is measured from `origin/<branch>` when that
/// ref exists, else the local `branch` ref (see [`stale_check_branch_ref`]).
/// `None` when the branch is not behind base (nothing to warn about) or on
/// a git error: advisory callers only. A gate uses [`pr_stale_check`],
/// which fails closed.
// trace:BUG-1468 | ai:claude
pub(crate) fn pr_stale_check_warning(
    repo: &std::path::Path,
    branch: &str,
    base_ref: &str,
) -> Option<StaleCheckWarning> {
    pr_stale_check(repo, branch, base_ref).ok().flatten()
}

/// [`pr_stale_check_warning`] for a gate: `Err` when staleness cannot be
/// computed (a missing ref, a git error), so the caller refuses instead of
/// reading a failure as "up to date".
// trace:BUG-1468 | ai:claude
// trace:STORY-1690 | ai:claude
pub(crate) fn pr_stale_check(
    repo: &std::path::Path,
    branch: &str,
    base_ref: &str,
) -> std::result::Result<Option<StaleCheckWarning>, String> {
    let branch_ref = stale_check_branch_ref(repo, branch);
    // Compare content at pinned commits, not the path history since divergence.
    // A touch followed by a revert must not invalidate the definition.
    // trace:STORY-2401 | ai:codex
    let forge = crate::forge::resolve_forge_kind(repo);
    let head_key = crate::ci_digest::CiEvidenceKey::read(repo, &branch_ref, forge)
        .map_err(|e| e.to_string())?;
    let base_key =
        crate::ci_digest::CiEvidenceKey::read(repo, base_ref, forge).map_err(|e| e.to_string())?;
    let branch_ref = head_key.head_sha.as_str();
    let base_ref = base_key.head_sha.as_str();
    let run = |args: &[&str]| -> std::result::Result<String, String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .map_err(|e| format!("git {}: {e}", args[0]))?;
        if !out.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let behind_commits: u64 = run(&[
        "rev-list",
        "--count",
        // trace:BUG-1622 | ai:claude
        git_arg_guard::END_OF_OPTIONS,
        &format!("{branch_ref}..{base_ref}"),
    ])?
    .trim()
    .parse()
    .map_err(|e| format!("git rev-list --count: {e}"))?;
    if behind_commits == 0 {
        return Ok(None);
    }
    let changed: Vec<String> = run(&[
        "diff",
        "--name-only",
        // trace:BUG-1622 | ai:claude
        git_arg_guard::END_OF_OPTIONS,
        &format!("{branch_ref}...{base_ref}"),
        "--",
    ])?
    .lines()
    .map(|l| l.to_string())
    .collect();
    let (mut definition_files, test_files) = classify_changed_files(
        &changed,
        &|workflow_path| workflow_is_pr_triggered(repo, base_ref, workflow_path),
        &|script_path| script_referenced_by_pr_workflows(repo, base_ref, script_path),
    );
    if head_key.workflow_digest == base_key.workflow_digest {
        definition_files.clear();
    } else if definition_files.is_empty() {
        // Includes removed, newly added and non-PR workflows, and GitLab CI.
        definition_files.push(match forge {
            crate::forge::ForgeKind::GitHub | crate::forge::ForgeKind::None => {
                ".github/workflows".into()
            }
            crate::forge::ForgeKind::GitLab => ".gitlab-ci.yml".into(),
        });
    }
    Ok(Some(StaleCheckWarning {
        behind_commits,
        head_digest: head_key.workflow_digest,
        base_digest: base_key.workflow_digest,
        definition_files,
        test_files,
    }))
}
