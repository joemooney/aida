// trace:TASK-3428 | ai:codex
use crate::{canonical_role_name, collect_open_prs, current_user_id, list_leases};

// ─── STORY-127: Scope-B runtime anti-pattern detectors ──────────────────
//
// Each surface the user TYPES (`aida pull`, `git pull` (via the code leg),
// `aida dev release`, `aida session end`) detects when its precondition is
// not met and emits a one-line WARNING (never a block — the user/agent
// always proceeds). The detection is a single git/queue/lease query.
//
// The detection LOGIC is a set of pure, unit-tested predicates so the
// warning conditions are testable without process/git/forge I/O. The call
// sites do the cheap query and feed the result to the predicate.
// trace:STORY-127 | ai:claude

/// Detector (1) + (2): `aida pull` / `git pull` (code leg) no-op.
///
/// `pull_was_noop` — the code-leg `git pull --ff-only` advanced nothing
/// (local branch already at origin). When that coincides with the user
/// holding an active **reviewer** lease on an **unmerged** PR, the no-op is
/// almost certainly the PR-27-style mistake: running catch-up commands
/// BEFORE the merge has happened. Warn so the user waits for the merge.
///
/// When there is no such reviewer lease the no-op is unremarkable — a plain
/// one-line note still helps ("Already up to date — nothing to pull") but is
/// not the same alarm. The two callers distinguish via the second arg.
/// trace:STORY-127 | ai:claude
pub(crate) fn pull_noop_warning(
    pull_was_noop: bool,
    reviewer_unmerged_pr: Option<&str>,
) -> Option<String> {
    if !pull_was_noop {
        return None;
    }
    match reviewer_unmerged_pr {
        Some(pr) => Some(format!(
            "No new commits on origin. You hold a reviewer lease on {pr}, which is \
             not merged yet — did you mean to wait for {pr} to merge first? \
             (catch-up before the merge is a no-op)"
        )),
        None => Some("Already up to date — origin had no new commits.".to_string()),
    }
}

/// Detector (2): cheap "local main already at origin/main" check, expressed
/// as a pure predicate over the two SHAs so it is testable. `None` for
/// either SHA (couldn't resolve a ref) means "can't tell" → no warning.
/// trace:STORY-127 | ai:claude
pub(crate) fn local_main_already_at_origin(
    local_sha: Option<&str>,
    origin_sha: Option<&str>,
) -> bool {
    match (local_sha, origin_sha) {
        (Some(a), Some(b)) => !a.is_empty() && a == b,
        _ => false,
    }
}

/// Detector (3): `aida dev release` (→ `scripts/release.sh`) with unmerged
/// PRs. Returns a one-line warning naming every open PR — a release tag cut
/// now will NOT include their changes. Pure over the already-collected
/// open-PR numbers. trace:STORY-127 | ai:claude
pub(crate) fn release_unmerged_pr_warning(open_pr_numbers: &[u64]) -> Option<String> {
    if open_pr_numbers.is_empty() {
        return None;
    }
    let mut nums: Vec<u64> = open_pr_numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    let list = nums
        .iter()
        .map(|n| format!("PR-{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let (verb, subj) = if nums.len() == 1 {
        ("is", "PR")
    } else {
        ("are", "PRs")
    };
    Some(format!(
        "{} open {} ({}) — the release {} not merged, so the tag will NOT include \
         their changes. Merge first if they belong in this release.",
        nums.len(),
        subj,
        list,
        verb,
    ))
}

/// Detector (4): `aida session end` while a DIFFERENT role has queued work
/// waiting. `ending_role` is the role of the lease being torn down;
/// `role_counts` is the per-role queue depth for the current user. Returns
/// the `(role, count)` pairs for every OTHER role with at least one waiting
/// item, sorted by role name for stable output. Pure over the inputs.
/// trace:STORY-127 | ai:claude
pub(crate) fn cross_role_queue_waiting(
    ending_role: Option<&str>,
    role_counts: &std::collections::HashMap<String, usize>,
) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = role_counts
        .iter()
        .filter(|(role, &count)| {
            count > 0 && ending_role.map(|er| er != role.as_str()).unwrap_or(true)
        })
        .map(|(role, &count)| (role.clone(), count))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Render the detector-(4) warning body from the cross-role pairs. `None`
/// when nothing is waiting elsewhere. trace:STORY-127 | ai:claude
pub(crate) fn session_end_cross_role_warning(waiting: &[(String, usize)]) -> Option<String> {
    if waiting.is_empty() {
        return None;
    }
    let list = waiting
        .iter()
        .map(|(role, count)| {
            format!(
                "{} has {} item{}",
                role,
                count,
                if *count == 1 { "" } else { "s" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "About to end this session, but another role's queue has work waiting \
         ({list}). Switch role first (`aida role enter <role>`) if you meant to \
         pick that up."
    ))
}

/// STORY-127: per-role queue depth for the current user. Reads the same
/// queue YAML `aida queue list` / `read_queue_depth` consult (keyed off
/// `current_user_id`). Returns an empty map on any read/parse failure —
/// the detectors degrade to silent. trace:STORY-127 | ai:claude
pub(crate) fn read_queue_role_counts(
    project_root: &std::path::Path,
) -> std::collections::HashMap<String, usize> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let user = current_user_id(None);
    let queue_path = project_root
        .join(".aida-store/registry/queues")
        .join(format!("{}.yaml", user));
    let Ok(content) = std::fs::read_to_string(&queue_path) else {
        return counts;
    };
    let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&content) else {
        return counts;
    };
    for e in &entries {
        let role = e
            .get("for_role")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("implementer");
        *counts.entry(role.to_string()).or_insert(0) += 1;
    }
    counts
}

/// STORY-127: does the current user hold an active **reviewer** lease whose
/// scope is a PR (`PR-N` / `MR-N`) that is still open (unmerged)? Returns
/// the PR label for the warning, else `None`. Cheap: walks the local lease
/// set, then a single forge open-PR snapshot keyed by number. The PR is
/// "unmerged" iff its number appears in the open-PR snapshot.
/// trace:STORY-127 | ai:claude
pub(crate) fn active_reviewer_unmerged_pr(project_root: &std::path::Path) -> Option<String> {
    let leases = list_leases(project_root);
    // Reviewer leases scope to a PR/MR; the scope string is e.g. "PR-27".
    let reviewer_pr_scopes: Vec<String> = leases
        .iter()
        .filter(|l| {
            l.role
                .as_deref()
                .map(|r| canonical_role_name(r) == "reviewer")
                .unwrap_or(false)
        })
        .filter(|l| pr_number_from_scope(&l.scope).is_some())
        .map(|l| l.scope.clone())
        .collect();
    if reviewer_pr_scopes.is_empty() {
        return None;
    }
    let open_numbers: std::collections::HashSet<u64> = collect_open_prs(project_root)
        .by_branch
        .values()
        .map(|p| p.number)
        .collect();
    for scope in reviewer_pr_scopes {
        if let Some(n) = pr_number_from_scope(&scope) {
            if open_numbers.contains(&n) {
                return Some(format!("PR-{n}"));
            }
        }
    }
    None
}

/// Parse a PR/MR number out of a lease scope like `PR-27` / `MR-3` / `pr-27`.
/// trace:STORY-127 | ai:claude
pub(crate) fn pr_number_from_scope(scope: &str) -> Option<u64> {
    let s = scope.trim();
    let rest = s
        .strip_prefix("PR-")
        .or_else(|| s.strip_prefix("pr-"))
        .or_else(|| s.strip_prefix("MR-"))
        .or_else(|| s.strip_prefix("mr-"))?;
    rest.parse::<u64>().ok()
}
