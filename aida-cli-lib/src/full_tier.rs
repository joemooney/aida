//! Full-tier CI evidence for one exact commit, and the request that starts a
//! full-tier run on an approved PR head.
//!
//! PR CI runs the FAST tier by default, so the full suite used to run for the
//! first time inside a merge train. A train now admits only a member whose
//! exact head already carries a green full-tier run: the run that reports
//! `ci-tier: full` and whose `Build (ubuntu-latest)` aggregate passed. The
//! evidence is read by commit SHA, never through the PR, so a head that moved
//! cannot borrow an older head's run.
// trace:STORY-2809 | ai:claude
use crate::forge::{CheckOutcome, CommitChecks};
use std::path::Path;

/// The PR label that selects the full tier in the CI workflow.
pub(crate) const FULL_LABEL: &str = "ci:full";
const TIER_PREFIX: &str = "ci-tier: ";
const FULL_MARKER: &str = "ci-tier: full";
const AGGREGATE: &str = "Build (ubuntu-latest)";

/// Where full-tier CI stands on one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullTier {
    /// The commit has checks but none advertises a CI tier: the repository's
    /// CI is not tiered, so there is no separate full run to require.
    Untiered,
    /// Full-tier run `run` passed its aggregate on this commit.
    Green { run: String },
    /// Full-tier run `run` exists on this commit and has not finished.
    Pending { run: String, waiting: Vec<String> },
    /// Full-tier run `run` on this commit failed or was cancelled.
    Failed { run: String, failing: Vec<String> },
    /// No full-tier run has started on this commit.
    Missing,
}

/// Judge the full tier from every check on one commit. A run counts only when
/// its own tier marker says `full`; checks from a FAST run on the same commit
/// never contribute. A green run outranks a pending one, which outranks a
/// failed one, so a rerun at the same head can still admit it.
pub(crate) fn judge(checks: &CommitChecks) -> FullTier {
    let is_marker = |name: &str| name.starts_with(TIER_PREFIX);
    if !checks.checks.iter().any(|c| is_marker(&c.name)) {
        return if checks.checks.is_empty() {
            FullTier::Missing
        } else {
            FullTier::Untiered
        };
    }
    let mut runs: Vec<&str> = checks
        .checks
        .iter()
        .filter(|c| c.name == FULL_MARKER)
        .filter_map(|c| c.run_id.as_deref())
        .collect();
    // Newest run first: it is the one a pending or failed report names.
    runs.sort_by_key(|run| std::cmp::Reverse(run.parse::<u64>().unwrap_or(0)));
    runs.dedup();
    let mut pending = None;
    let mut failed = None;
    for run in runs {
        let rows: Vec<_> = checks
            .checks
            .iter()
            .filter(|c| c.run_id.as_deref() == Some(run))
            .collect();
        // A run that also reports another tier is not a full run.
        if rows
            .iter()
            .any(|c| is_marker(&c.name) && c.name != FULL_MARKER)
        {
            continue;
        }
        let named = |outcome: CheckOutcome| -> Vec<String> {
            rows.iter()
                .filter(|c| c.outcome == outcome)
                .map(|c| c.name.clone())
                .collect()
        };
        let failing = named(CheckOutcome::Failed);
        let waiting = named(CheckOutcome::Pending);
        let passed = |name: &str| {
            rows.iter()
                .any(|c| c.name == name && c.outcome == CheckOutcome::Passed)
        };
        if !failing.is_empty() {
            failed.get_or_insert(FullTier::Failed {
                run: run.to_string(),
                failing,
            });
        } else if waiting.is_empty() && passed(FULL_MARKER) && passed(AGGREGATE) {
            return FullTier::Green {
                run: run.to_string(),
            };
        } else {
            // The aggregate registers only after every lane has finished.
            pending.get_or_insert(FullTier::Pending {
                run: run.to_string(),
                waiting: if waiting.is_empty() {
                    vec![AGGREGATE.to_string()]
                } else {
                    waiting
                },
            });
        }
    }
    pending.or(failed).unwrap_or(FullTier::Missing)
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Why a train does not admit a member whose head stands at `tier`; `None`
/// when the head is admissible. Names the missing, running or red run.
pub(crate) fn admission_refusal(tier: &FullTier, head: &str) -> Option<String> {
    let head = short(head);
    match tier {
        FullTier::Untiered | FullTier::Green { .. } => None,
        FullTier::Missing => Some(format!(
            "no full-tier CI run exists at its head {head}: a train admits a PR only after a \
             `{FULL_MARKER}` run with a green `{AGGREGATE}` on that exact commit; approving the \
             head or adding the `{FULL_LABEL}` label starts one"
        )),
        FullTier::Pending { run, waiting } => Some(format!(
            "full-tier CI run {run} at its head {head} has not finished (waiting on {})",
            waiting.join(", ")
        )),
        FullTier::Failed { run, failing } => Some(format!(
            "full-tier CI run {run} at its head {head} is red (failing: {})",
            failing.join(", ")
        )),
    }
}

/// The activity-log line for a `--skip-full-check` admission, written before
/// the train is built so the override is on record even if the train fails.
pub(crate) fn skip_event(now_iso: &str, pr: u64, head: &str, detail: &str) -> String {
    serde_json::json!({
        "ts": now_iso,
        "command": "aida pr train",
        "step": "train-admit-skip-full-check",
        "status": "overridden",
        "pr": pr,
        "head_sha": head,
        "detail": detail,
    })
    .to_string()
}

/// The two label edits that start a full-tier run on PR `pr`'s current head.
/// A label left over from an earlier head is removed first: adding a label
/// the PR already carries fires no event, so no run would start.
pub(crate) fn request_commands(repo: &str, pr: u64) -> [Vec<String>; 2] {
    ["--remove-label", "--add-label"].map(|flag| {
        vec![
            "pr".to_string(),
            "edit".to_string(),
            pr.to_string(),
            "-R".to_string(),
            repo.to_string(),
            flag.to_string(),
            FULL_LABEL.to_string(),
        ]
    })
}

/// Start a full-tier CI run on PR `pr`'s current head by (re)applying the
/// `ci:full` label. `Ok(false)` when the project has no GitHub forge to ask.
pub(crate) fn request_full_ci(project_root: &Path, pr: u64) -> Result<bool, String> {
    request_full_ci_with(project_root, pr, crate::merge_hold::run_forge_cli)
}

/// [`request_full_ci`] with the forge CLI injected.
pub(crate) fn request_full_ci_with(
    project_root: &Path,
    pr: u64,
    runner: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    if kind != crate::forge::ForgeKind::GitHub {
        return Ok(false);
    }
    let Some(pin) = crate::merge_hold::resolve_pinned_repo(project_root, kind)? else {
        return Ok(false);
    };
    let [remove, add] = request_commands(&pin.repo_arg(), pr);
    // Removing a label the PR does not carry is not an error worth reporting.
    let _ = runner(project_root, "gh", &remove);
    match runner(project_root, "gh", &add)? {
        (true, _) => Ok(true),
        (false, stderr) => Err(format!("`gh {}` failed: {}", add.join(" "), stderr.trim())),
    }
}

#[cfg(test)]
#[path = "tests/story_2809_full_tier_tests.rs"]
mod tests;
