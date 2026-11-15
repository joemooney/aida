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
    /// The repository's CI is not tiered and the commit has checks: there is
    /// no separate full run to require.
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

/// Whether the repository's CI is tiered: one of its workflow files defines
/// the job that reports `ci-tier: <tier>`. Read from the checkout, never from
/// one commit's checks, so a head whose tier check is absent (CI not started,
/// a cancelled run, a workflow edited on the branch) cannot pass as untiered.
// trace:STORY-2809 | ai:codex
pub(crate) fn repo_is_tiered(project_root: &Path) -> std::io::Result<bool> {
    let dir = match std::fs::read_dir(project_root.join(".github/workflows")) {
        Ok(dir) => dir,
        // Absent configuration is untiered; errors reading existing evidence
        // (including missing entries or files below) must still refuse.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    repo_is_tiered_with(
        dir.map(|entry| entry.map(|entry| entry.path())),
        std::fs::read_to_string,
    )
}

fn repo_is_tiered_with(
    entries: impl IntoIterator<Item = std::io::Result<std::path::PathBuf>>,
    mut read: impl FnMut(std::path::PathBuf) -> std::io::Result<String>,
) -> std::io::Result<bool> {
    let mut tiered = false;
    for entry in entries {
        let path = entry?;
        if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        ) {
            tiered |= read(path)?.contains(TIER_PREFIX);
        }
    }
    Ok(tiered)
}

/// Judge the full tier from every check on one commit; `tiered` is
/// [`repo_is_tiered`]. A run counts only when its own tier marker says
/// `full`; checks from a FAST run on the same commit never contribute. A
/// green run outranks a pending one, which outranks a failed one, so a rerun
/// at the same head can still admit it. In a tiered repository a commit with
/// no full-tier run is `Missing` whatever else it carries.
pub(crate) fn judge(checks: &CommitChecks, tiered: bool) -> FullTier {
    let is_marker = |name: &str| name.starts_with(TIER_PREFIX);
    if !tiered && !checks.checks.iter().any(|c| is_marker(&c.name)) {
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

/// What a full-tier request does to PR labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelPlan {
    /// The PR does not carry the label: adding it starts the run.
    Add,
    /// The label is left over from an earlier head and nothing is running on
    /// this one: remove it and add it again, because adding a label the PR
    /// already carries fires no event.
    Reapply,
    /// Change nothing: any label event cancels the PR's CI run in flight.
    Leave,
}

/// Decide the label edits from `gh pr view --json labels,statusCheckRollup`.
/// A head that already has a full-tier run, or a labelled head with a check
/// still running, is left alone: relabelling would cancel that run.
pub(crate) fn label_plan(view: &serde_json::Value) -> Result<LabelPlan, String> {
    let list = |key: &str| {
        view.get(key)
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("the PR view has no `{key}` list"))
    };
    let text = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let labelled = list("labels")?
        .iter()
        .any(|l| text(l, "name") == FULL_LABEL);
    // A head with no check yet reports no rollup at all.
    let none = Vec::new();
    let rollup = match view.get("statusCheckRollup") {
        None | Some(serde_json::Value::Null) => &none,
        Some(_) => list("statusCheckRollup")?,
    };
    if rollup.iter().any(|c| text(c, "name") == FULL_MARKER) {
        return Ok(LabelPlan::Leave);
    }
    if !labelled {
        return Ok(LabelPlan::Add);
    }
    // A check run reports `status`, a commit status reports `state`.
    let running = rollup.iter().any(|c| {
        let status = text(c, "status");
        (!status.is_empty() && status != "COMPLETED")
            || matches!(text(c, "state").as_str(), "PENDING" | "EXPECTED")
    });
    Ok(if running {
        LabelPlan::Leave
    } else {
        LabelPlan::Reapply
    })
}

/// Only inspect a single, settled full run. Ambiguous or live rollups retain
/// the existing no-relabel policy; the run's conclusion is checked separately.
// trace:TASK-3105 | ai:codex
fn retry_candidate(view: &serde_json::Value) -> Option<String> {
    let rows = view.get("statusCheckRollup")?.as_array()?;
    if rows.iter().any(|row| {
        row.get("status")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s != "COMPLETED")
            || matches!(
                row.get("state").and_then(|v| v.as_str()),
                Some("PENDING" | "EXPECTED")
            )
    }) {
        return None;
    }
    let mut markers = rows
        .iter()
        .filter(|row| row.get("name").and_then(|v| v.as_str()) == Some(FULL_MARKER));
    let marker = markers.next()?;
    if markers.next().is_some() || marker.get("status")?.as_str()? != "COMPLETED" {
        return None;
    }
    crate::ci_gate::run_id_from_check_link(marker.get("detailsUrl")?.as_str()?)
}

fn gh_pr(repo: &str, pr: u64, verb: &str, tail: &[&str]) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        verb.to_string(),
        pr.to_string(),
        "-R".to_string(),
        repo.to_string(),
    ];
    args.extend(tail.iter().map(|s| s.to_string()));
    args
}

const VIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Start a full-tier CI run on PR `pr`'s current head through the `ci:full`
/// label. `Ok(true)` when a label was edited; `Ok(false)` when the project has
/// no GitHub forge to ask, or the head already has a run that an edit would
/// cancel.
pub(crate) fn request_full_ci(project_root: &Path, pr: u64) -> Result<bool, String> {
    request_full_ci_with(
        project_root,
        pr,
        |root, cli, args| {
            crate::merge_hold::run_forge_cli_stdout_bounded_with(root, cli, args, VIEW_TIMEOUT)
        },
        crate::merge_hold::run_forge_cli,
    )
}

/// [`request_full_ci`] with the forge CLI injected: `view` returns stdout on
/// success, `edit` returns stderr; both return stderr on failure.
pub(crate) fn request_full_ci_with(
    project_root: &Path,
    pr: u64,
    view: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
    edit: impl Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
) -> Result<bool, String> {
    let kind = crate::forge::resolve_forge_kind(project_root);
    if kind != crate::forge::ForgeKind::GitHub {
        return Ok(false);
    }
    let Some(pin) = crate::merge_hold::resolve_pinned_repo(project_root, kind)? else {
        return Ok(false);
    };
    let repo = pin.repo_arg();
    let run = |runner: &dyn Fn(&Path, &str, &[String]) -> Result<(bool, String), String>,
               args: Vec<String>| match runner(project_root, "gh", &args)? {
        (true, out) => Ok(out),
        (false, err) => Err(format!("`gh {}` failed: {}", args.join(" "), err.trim())),
    };
    let viewed = run(
        &view,
        gh_pr(&repo, pr, "view", &["--json", "labels,statusCheckRollup"]),
    )?;
    let viewed = serde_json::from_str(&viewed)
        .map_err(|e| format!("`gh pr view {pr}` returned invalid JSON: {e}"))?;
    let mut plan = label_plan(&viewed)?;
    // trace:TASK-3105 | ai:codex
    if plan == LabelPlan::Leave {
        if let Some(id) = retry_candidate(&viewed) {
            let evidence = run(
                &view,
                vec![
                    "run".into(),
                    "view".into(),
                    id,
                    "-R".into(),
                    repo.clone(),
                    "--json".into(),
                    "status,conclusion".into(),
                ],
            )?;
            let evidence: serde_json::Value = serde_json::from_str(&evidence)
                .map_err(|e| format!("`gh run view` returned invalid JSON: {e}"))?;
            if evidence["status"] == "completed" && evidence["conclusion"] == "cancelled" {
                plan = if viewed["labels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|label| label["name"] == FULL_LABEL)
                {
                    LabelPlan::Reapply
                } else {
                    LabelPlan::Add
                };
            }
        }
    }
    if plan == LabelPlan::Leave {
        return Ok(false);
    }
    if plan == LabelPlan::Reapply {
        run(
            &edit,
            gh_pr(&repo, pr, "edit", &["--remove-label", FULL_LABEL]),
        )?;
    }
    run(
        &edit,
        gh_pr(&repo, pr, "edit", &["--add-label", FULL_LABEL]),
    )?;
    Ok(true)
}

#[cfg(test)]
#[path = "tests/story_2809_full_tier_tests.rs"]
mod tests;
