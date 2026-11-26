//! Landing-drought watchdog: approved PRs that sit ready without landing, and
//! a main branch that has not moved while they wait. Read-only; the scheduled
//! `landing-drought` job fails on findings so its route seat investigates.
// trace:STORY-3131 | ai:claude
use crate::doctor_root::DoctorFinding;
use crate::gate::ledger::{project, ChangeKey};
use crate::gate::review_verdict::{parse_ledger_record, VerdictKind};
use crate::land::Ci;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, TimeDelta, Utc};
use std::path::Path;

pub(crate) const CATEGORY: &str = "landing-drought";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Report,
    Investigate,
}

/// `[landing_watchdog]` in `.aida/config.toml`; defaults apply when absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub enabled: bool,
    pub drought_minutes: i64,
    pub ready_dwell_minutes: i64,
    pub action: Action,
}

impl Policy {
    pub(crate) fn read(config: Option<&toml::Value>) -> Result<Self> {
        let cfg = config.and_then(|v| v.get("landing_watchdog"));
        let get = |key: &str| cfg.and_then(|v| v.get(key));
        let minutes = |key: &str, default: i64| -> Result<i64> {
            let n = match get(key) {
                None => default,
                Some(v) => v
                    .as_integer()
                    .with_context(|| format!("landing_watchdog.{key} must be an integer"))?,
            };
            anyhow::ensure!(n > 0, "landing_watchdog.{key} must be positive");
            Ok(n)
        };
        let enabled = match get("enabled") {
            None => true,
            Some(v) => v
                .as_bool()
                .context("landing_watchdog.enabled must be a boolean")?,
        };
        let action = match get("action").map(|v| v.as_str()) {
            None | Some(Some("investigate")) => Action::Investigate,
            Some(Some("report")) => Action::Report,
            Some(_) => bail!("landing_watchdog.action must be \"report\" or \"investigate\""),
        };
        Ok(Self {
            enabled,
            drought_minutes: minutes("drought_minutes", 60)?,
            ready_dwell_minutes: minutes("ready_dwell_minutes", 30)?,
            action,
        })
    }

    /// Why `--fail-on-findings` fails: only `investigate` routes the findings.
    pub(crate) fn fail_reason(&self, findings: usize) -> Option<String> {
        (self.action == Action::Investigate && findings > 0)
            .then(|| format!("{findings} landing-drought findings to investigate"))
    }
}

/// One approved PR's landing evidence, read cheaply from the open-PR listing.
#[derive(Debug, Clone)]
pub(crate) struct Ready {
    pub number: u64,
    /// When the PR became ready: approval, or later green CI.
    pub since: DateTime<Utc>,
    pub ci: Ci,
    /// Full-tier CI at the head is missing or cancelled in a tiered repo.
    pub full_tier_gap: Option<&'static str>,
    pub supervised: bool,
    pub on_train: bool,
    /// A train ejected this head for stacked-diff drift: when, and the route.
    // trace:BUG-2777 | ai:claude
    pub drift: Option<(DateTime<Utc>, String)>,
}

/// A stacked-diff-drift ejection older than this is its own cause.
// trace:BUG-2777 | ai:claude
const DRIFT_MINUTES: i64 = 30;

fn drift_cause(ready: &Ready, now: DateTime<Utc>) -> Option<(String, String)> {
    let (at, route) = ready.drift.as_ref()?;
    let age = now.signed_duration_since(*at);
    (age > TimeDelta::minutes(DRIFT_MINUTES)).then(|| {
        let fix = if route.is_empty() {
            format!("update PR {}'s branch from main and review the new head", ready.number)
        } else {
            route.clone()
        };
        (
            format!(
                "ejected for stacked-diff drift {}m ago (the approval does not cover what would land)",
                age.num_minutes()
            ),
            fix,
        )
    })
}

fn cause(ready: &Ready) -> Option<(String, String)> {
    let n = ready.number;
    Some(match &ready.ci {
        Ci::Red(failed) => (
            format!("red required check ({failed})"),
            format!("inspect `gh pr checks {n}`; rerun a flake or route rework"),
        ),
        // A cancelled full tier holds CI pending, but nothing is running;
        // a missing one only matters once the fast tier is green.
        Ci::Green | Ci::Pending
            if ready.full_tier_gap == Some("cancelled")
                || (ready.ci == Ci::Green && ready.full_tier_gap.is_some()) =>
        {
            (
                format!("full CI tier {} at the approved head", ready.full_tier_gap?),
                format!("add the `ci:full` label to PR {n} to run full-tier CI at its head"),
            )
        }
        Ci::Green if ready.supervised => (
            "supervised, not drain-eligible".into(),
            format!("needs operator train: `aida pr train {n}`"),
        ),
        Ci::Green => (
            "unknown".into(),
            "investigate: `aida land stalls`, train admission and the schedule log".into(),
        ),
        // CI still running at the head: not ready yet.
        _ => return None,
    })
}

pub(crate) fn evaluate(
    prs: &[Ready],
    policy: &Policy,
    now: DateTime<Utc>,
    last_landing: Option<DateTime<Utc>>,
) -> Vec<DoctorFinding> {
    if !policy.enabled {
        return Vec::new();
    }
    let dwell = TimeDelta::minutes(policy.ready_dwell_minutes);
    let mut findings: Vec<DoctorFinding> = prs
        .iter()
        .filter(|r| !r.on_train)
        .filter_map(|r| {
            let drifted = drift_cause(r, now);
            if drifted.is_none() && now.signed_duration_since(r.since) <= dwell {
                return None;
            }
            let (why, action) = drifted.or_else(|| cause(r))?;
            Some(DoctorFinding {
                category: CATEGORY.into(),
                id: format!("PR-{}", r.number),
                summary: format!(
                    "PR-{} approved and ready {}m, not on a train: {why}",
                    r.number,
                    now.signed_duration_since(r.since).num_minutes()
                ),
                action,
                safe_heal: false,
            })
        })
        .collect();
    let drought = TimeDelta::minutes(policy.drought_minutes);
    let dry = last_landing.is_none_or(|at| now.signed_duration_since(at) > drought);
    // Only landable PRs (green, no full-tier gap) make main's idleness a drought.
    let landable = prs
        .iter()
        .filter(|r| !r.on_train && now.signed_duration_since(r.since) > dwell)
        .filter(|r| r.ci == Ci::Green && r.full_tier_gap.is_none() && r.drift.is_none())
        .count();
    if dry && landable > 0 {
        let since = last_landing.map_or("ever".into(), |at| {
            format!("{}m", now.signed_duration_since(at).num_minutes())
        });
        findings.push(DoctorFinding {
            category: CATEGORY.into(),
            id: "main".into(),
            summary: format!("no landing on main for {since} while {landable} landable PRs wait"),
            action: "investigate the stuck PRs above, or page the operator".into(),
            safe_heal: false,
        });
    }
    findings
}

/// Full-tier CI at the head as the PR listing shows it.
fn full_tier_gap(rollup: &serde_json::Value) -> Option<&'static str> {
    let rows: Vec<_> = rollup
        .as_array()?
        .iter()
        .filter(|c| c["name"] == crate::full_tier::FULL_MARKER)
        .collect();
    let any = |outcome: &str| rows.iter().any(|c| c["conclusion"] == outcome);
    if rows.is_empty() {
        Some("missing")
    } else if any("SUCCESS") {
        None
    } else if any("CANCELLED") {
        Some("cancelled")
    } else {
        None
    }
}

fn last_landing(root: &Path) -> Result<Option<DateTime<Utc>>> {
    for rev in ["origin/main", "main"] {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%cI", rev, "--"])
            .current_dir(root)
            .output()
            .context("running git log for the last landing")?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            return Ok(DateTime::parse_from_rfc3339(text.trim())
                .ok()
                .map(|t| t.with_timezone(&Utc)));
        }
    }
    bail!("cannot read the last landing on main")
}

/// The time of the latest ledger record when it is an open approval at
/// `head`; `None` when the latest record is anything else.
fn approved_at_head(
    records: Vec<(String, crate::gate::review_verdict::JsonObj)>,
    head: &str,
) -> Result<Option<DateTime<Utc>>> {
    let mut latest = None;
    for (at, record) in records {
        let at = DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc);
        if latest.as_ref().is_none_or(|(t, _)| at > *t) {
            latest = Some((at, record));
        }
    }
    let Some((at, record)) = latest else {
        return Ok(None);
    };
    let verdict = parse_ledger_record(&record).context("invalid ledger verdict")?;
    let at_head = verdict
        .reviewed_sha
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(head));
    let open = !verdict.is_closed() && verdict.kind == VerdictKind::Approved;
    Ok((open && at_head).then_some(at))
}

/// Fail closed like the ship gate: a PR needs supervision unless every spec
/// it names resolves and is drain mode. No spec ID, or an unresolved one
/// (`None`), counts as supervised.
fn needs_supervision(
    modes: impl Iterator<Item = Option<Option<aida_core::ExecutionMode>>>,
) -> bool {
    let mut any = false;
    for mode in modes {
        any = true;
        match mode {
            Some(m) if !crate::gate::ship::merge_requires_supervision(m) => {}
            _ => return true,
        }
    }
    !any
}

pub(crate) fn scan(
    exec: &aida_core::exec_context::ExecContext,
) -> Result<(Policy, Vec<DoctorFinding>)> {
    let root = exec.project_root();
    let policy = Policy::read(crate::read_project_config_value(root).as_ref())?;
    if !policy.enabled {
        return Ok((policy, Vec::new()));
    }
    use aida_core::DatabaseBackend;
    let storage = aida_core::GitBackend::open_existing(&root.join(".aida-store"))?;
    let store = storage.load_for_read()?;
    let tiered = crate::full_tier::repo_is_tiered(root)?;
    let mut ready = Vec::new();
    for pr in crate::stalled_reviews::open_prs(root)? {
        let number = pr["number"].as_u64().context("PR number missing")?;
        let head = pr["headRefOid"].as_str().context("invalid PR head")?;
        let records = project::change_verdicts_checked(root, ChangeKey::pr(number))
            .map_err(anyhow::Error::msg)?;
        let Some(at) = approved_at_head(records, head)? else {
            continue;
        };
        let rollup = &pr["statusCheckRollup"];
        let green_at = crate::land::green_rollup_at(rollup).map(|t| t.with_timezone(&Utc));
        let branch = pr["headRefName"].as_str().unwrap_or_default();
        let title = pr["title"].as_str().unwrap_or_default();
        let spec_ids = crate::pr_ship::extract_pr_spec_ids(branch, title);
        let supervised = needs_supervision(spec_ids.iter().map(|s| {
            store
                .get_requirement_by_spec_id(s)
                .map(|r| r.execution_mode)
        }));
        ready.push(Ready {
            number,
            since: green_at.map_or(at, |ci| at.max(ci)),
            ci: crate::land::ci_of_rollup(rollup),
            full_tier_gap: tiered.then(|| full_tier_gap(rollup)).flatten(),
            supervised,
            on_train: !crate::pr_train::pr_head_pins(root, number)?.is_empty(),
            drift: crate::pr_train::culprit::drift(root, number, head),
        });
    }
    let now = DateTime::<Utc>::from(exec.clock().wall_now());
    let mut findings = evaluate(&ready, &policy, now, last_landing(root)?);
    findings.extend(retained_findings(
        crate::review_watch::service::retained_review_blocks(root, now),
    ));
    Ok((policy, findings))
}

/// A review the watcher holds behind a kept reviewer checkout starves the
/// landing pipeline upstream of any approval. trace:BUG-2776 | ai:claude
pub(crate) fn retained_findings(blocks: Vec<(String, String)>) -> Vec<DoctorFinding> {
    blocks
        .into_iter()
        .map(|(id, line)| {
            let (summary, action) = line.split_once("; fix: ").unwrap_or((&line, ""));
            DoctorFinding {
                category: CATEGORY.into(),
                id,
                summary: summary.into(),
                action: action.into(),
                safe_heal: false,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/story_3131_landing_drought_tests.rs"]
mod tests;
