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
/// Shown with the findings but never fails `--fail-on-findings`.
pub(crate) const INFO_CATEGORY: &str = "landing-drought-info";

/// The findings that fail the job: everything but the informational rows.
pub(crate) fn failing(findings: &[DoctorFinding]) -> usize {
    findings
        .iter()
        .filter(|f| f.category != INFO_CATEGORY)
        .count()
}

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
    pub lock_hold_minutes: i64,
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
            lock_hold_minutes: minutes("lock_hold_minutes", 20)?,
            action,
        })
    }

    /// Why `--fail-on-findings` fails: only `investigate` routes the findings.
    pub(crate) fn fail_reason(&self, findings: usize) -> Option<String> {
        (self.action == Action::Investigate && findings > 0)
            .then(|| format!("{findings} landing-drought findings to investigate"))
    }
}

/// One approved (or green, unreviewed) PR's landing evidence, read cheaply
/// from the open-PR listing.
#[derive(Debug, Clone)]
pub(crate) struct Ready {
    pub number: u64,
    /// When the PR became ready: approval, or later green CI.
    pub since: DateTime<Utc>,
    /// An open approval at the current head; otherwise no verdict there yet.
    pub approved: bool,
    /// A review of the current head is queued or running.
    pub review_in_flight: bool,
    /// The forge reports a merge conflict with the base.
    pub conflict: bool,
    /// A rework lane (queued work or a retained implementer lease) owns it.
    pub repairing: bool,
    pub spec: Option<String>,
    pub ci: Ci,
    /// Full-tier CI at the head is missing or cancelled in a tiered repo.
    pub full_tier_gap: Option<&'static str>,
    pub supervised: bool,
    pub on_train: bool,
}

/// The live train-lock driver. trace:STORY-3131 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockHolder {
    pub pid: Option<u32>,
    pub held_minutes: Option<i64>,
    pub command: Option<String>,
}

impl LockHolder {
    /// Held past `lock_hold_minutes`, or held at all by a dead or
    /// unidentified driver: an orphaned lock (BUG-2757's shape) is stale at
    /// any age.
    fn stale(&self, policy: &Policy) -> bool {
        self.pid.is_none()
            || self
                .held_minutes
                .is_some_and(|m| m > policy.lock_hold_minutes)
    }
}

fn cause(ready: &Ready, stale_lock: Option<&LockHolder>) -> Option<(String, String)> {
    let n = ready.number;
    // A conflict blocks CI and landing alike, so it outranks every CI state.
    if ready.conflict && !ready.repairing {
        let fix = ready.spec.as_ref().map_or_else(
            || format!("rebase PR {n} onto main"),
            |s| format!("`aida rework {s}`"),
        );
        return Some((
            "merge conflict with no repair lane".into(),
            format!("route the conflict repair: {fix}"),
        ));
    }
    Some(match &ready.ci {
        Ci::Green if !ready.approved => (
            "review verdict missing or held at the current head".into(),
            format!(
                "check `aida review watch status`; clear an exited-attempt hold with `aida review watch --release PR-{n}`"
            ),
        ),
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
        Ci::Green if stale_lock.is_some() => {
            let lock = stale_lock?;
            let pid = lock.pid.map_or("an unidentified process".into(), |p| format!("pid {p}"));
            let command = lock.command.as_ref().map_or(String::new(), |c| format!(" ({c})"));
            let age = lock.held_minutes.map_or(String::new(), |m| format!(" {m}m"));
            (
                format!("train lock held{age} by {pid}{command}"),
                format!("inspect `aida pr train --status`; stop {pid} if it is hung, then train PR {n}"),
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
    lock: Option<&LockHolder>,
) -> Vec<DoctorFinding> {
    if !policy.enabled {
        return Vec::new();
    }
    let stale_lock = lock.filter(|l| l.stale(policy));
    let dwell = TimeDelta::minutes(policy.ready_dwell_minutes);
    let mut findings: Vec<DoctorFinding> = prs
        .iter()
        .filter(|r| !r.on_train && now.signed_duration_since(r.since) > dwell)
        .filter_map(|r| {
            let (why, action) = cause(r, stale_lock)?;
            let state = if r.approved {
                "approved and ready"
            } else {
                "green"
            };
            // A queued or running review is the normal backlog: shown, not
            // failing. trace:STORY-3131 | ai:claude
            let backlog = !r.approved && r.ci == Ci::Green && r.review_in_flight;
            let (category, why, action) = if backlog {
                (
                    INFO_CATEGORY,
                    "review queued or running at the current head".into(),
                    "none: wait for the review".into(),
                )
            } else {
                (CATEGORY, why, action)
            };
            Some(DoctorFinding {
                category: category.into(),
                id: format!("PR-{}", r.number),
                summary: format!(
                    "PR-{} {state} {}m, not on a train: {why}",
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
    // Only landable PRs (approved, green, no full-tier gap, no conflict)
    // make main's idleness a drought.
    let landable = prs
        .iter()
        .filter(|r| !r.on_train && now.signed_duration_since(r.since) > dwell)
        .filter(|r| r.approved && r.ci == Ci::Green && r.full_tier_gap.is_none() && !r.conflict)
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

/// The latest ledger record as seen from `head`.
#[derive(Debug, PartialEq, Eq)]
enum AtHead {
    /// An open approval at `head`, recorded then.
    Approved(DateTime<Utc>),
    /// No record, or the latest one reviewed an older head.
    Missing,
    /// A closed verdict, or a non-approval at `head` (stalled-reviews' case).
    Settled,
}

fn ledger_at_head(
    records: Vec<(String, crate::gate::review_verdict::JsonObj)>,
    head: &str,
) -> Result<AtHead> {
    let mut latest = None;
    for (at, record) in records {
        let at = DateTime::parse_from_rfc3339(&at)?.with_timezone(&Utc);
        if latest.as_ref().is_none_or(|(t, _)| at > *t) {
            latest = Some((at, record));
        }
    }
    let Some((at, record)) = latest else {
        return Ok(AtHead::Missing);
    };
    let verdict = parse_ledger_record(&record).context("invalid ledger verdict")?;
    let at_head = verdict
        .reviewed_sha
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(head));
    Ok(if verdict.is_closed() {
        AtHead::Settled
    } else if !at_head {
        AtHead::Missing
    } else if verdict.kind == VerdictKind::Approved {
        AtHead::Approved(at)
    } else {
        AtHead::Settled
    })
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
    let lanes = crate::stalled_reviews::ReworkLanes::load(root, &storage, &store)?;
    // One clock for PR ages and the lock's age. trace:STORY-3131 | ai:claude
    let now = DateTime::<Utc>::from(exec.clock().wall_now());
    let reviewing = crate::review_watch::reviewing_heads(root);
    let mut ready = Vec::new();
    for pr in crate::stalled_reviews::open_prs(root)? {
        // Drafts and other bases never land on main. trace:STORY-3131 | ai:claude
        if pr["isDraft"] == true || pr["baseRefName"] != "main" {
            continue;
        }
        let number = pr["number"].as_u64().context("PR number missing")?;
        let head = pr["headRefOid"].as_str().context("invalid PR head")?;
        let records = project::change_verdicts_checked(root, ChangeKey::pr(number))
            .map_err(anyhow::Error::msg)?;
        let rollup = &pr["statusCheckRollup"];
        let ci = crate::land::ci_of_rollup(rollup);
        let green_at = crate::land::green_rollup_at(rollup).map(|t| t.with_timezone(&Utc));
        // An unreviewed head counts once its CI is green: the review is the wait.
        let (approved, since) = match (ledger_at_head(records, head)?, green_at) {
            (AtHead::Approved(at), green) => (true, green.map_or(at, |ci| at.max(ci))),
            (AtHead::Missing, Some(green)) if ci == Ci::Green => (false, green),
            _ => continue,
        };
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
            since,
            approved,
            review_in_flight: reviewing
                .iter()
                .any(|(key, sha)| *key == format!("PR-{number}") && sha.eq_ignore_ascii_case(head)),
            conflict: pr["mergeable"] == "CONFLICTING",
            repairing: spec_ids.iter().any(|s| lanes.covers(&store, s, branch)),
            spec: spec_ids.first().cloned(),
            ci,
            full_tier_gap: tiered.then(|| full_tier_gap(rollup)).flatten(),
            supervised,
            on_train: !crate::pr_train::pr_head_pins(root, number)?.is_empty(),
        });
    }
    let lock = crate::pr_train::lock_driver(root, now)?.map(|(pid, secs)| LockHolder {
        pid,
        held_minutes: secs.map(|s| i64::try_from(s / 60).unwrap_or(i64::MAX)),
        command: pid.and_then(command_line),
    });
    let findings = evaluate(&ready, &policy, now, last_landing(root)?, lock.as_ref());
    Ok((policy, findings))
}

/// A process's command line, shortened for a finding.
fn command_line(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let line = String::from_utf8_lossy(&raw).replace('\0', " ");
    let line = line.trim();
    (!line.is_empty()).then(|| line.chars().take(80).collect())
}

#[cfg(test)]
#[path = "tests/story_3131_landing_drought_tests.rs"]
mod tests;
