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
            // A healthy train holds the lock through its whole CI wait.
            lock_hold_minutes: minutes("lock_hold_minutes", 45)?,
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
    /// The watcher's latest attempt at the current head; `None` when it has
    /// not admitted this head yet (waiting for capacity: backlog).
    pub review: Option<crate::review_watch::ReviewWatch>,
    /// The watcher's retained-checkout block for this PR (BUG-2776), as its
    /// `why; fix: action` line: it names the held review's actual cause.
    pub retained: Option<String>,
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

/// The live train-lock driver. trace:STORY-3131 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockHolder {
    pub pid: Option<u32>,
    pub held_minutes: Option<i64>,
    pub command: Option<String>,
    /// Since the train last changed step or checkpointed; `None` with no
    /// journal.
    pub idle_minutes: Option<i64>,
    /// Train CI jobs still pending: `None` when not read or unreadable.
    pub ci_live: Option<bool>,
    /// The hold past which the train needs a look: see [`lock_threshold`].
    pub threshold_minutes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockState {
    /// Within its threshold, or showing progress: PRs wait for it.
    Busy,
    /// Past its threshold with no positive sign of being stuck.
    Slow,
    /// No step change, checkpoint or CI activity for the threshold.
    Stuck,
    /// A dead or unidentified driver that shows no progress (BUG-2757).
    Orphaned,
}

impl LockHolder {
    /// "Stop" needs positive evidence; progress or live CI always means busy.
    pub(crate) fn state(&self) -> LockState {
        let t = self.threshold_minutes;
        if self.idle_minutes.is_some_and(|m| m < t) || self.ci_live == Some(true) {
            return LockState::Busy;
        }
        if self.pid.is_none() {
            return LockState::Orphaned;
        }
        if self.held_minutes.is_none_or(|m| m <= t) {
            return LockState::Busy;
        }
        if self.idle_minutes.is_some() && self.ci_live == Some(false) {
            LockState::Stuck
        } else {
            LockState::Slow
        }
    }
}

/// `max(lock_hold_minutes, 2 x p90)` of past train runs (nearest rank).
pub(crate) fn lock_threshold(policy: &Policy, history_secs: &[u64]) -> i64 {
    let mut runs: Vec<u64> = history_secs.to_vec();
    runs.sort_unstable();
    let p90 = match runs.len() {
        0 => 0,
        n => runs[(n * 9).div_ceil(10) - 1],
    };
    let doubled = i64::try_from(p90.saturating_mul(2) / 60).unwrap_or(i64::MAX);
    policy.lock_hold_minutes.max(doubled)
}

/// A finding's (category, why, action) for one waiting PR.
type Cause = (&'static str, String, String);

fn cause(ready: &Ready, lock: Option<&LockHolder>) -> Option<Cause> {
    use crate::review_watch::ReviewWatch;
    let n = ready.number;
    let fail = |why: String, action: String| Some((CATEGORY, why, action));
    // A conflict blocks CI and landing alike, so it outranks every CI state.
    if ready.conflict {
        // A repair lane already owns the conflict: its cause is known and in
        // hand, so it never falls through to "unknown". trace:STORY-3131 | ai:claude
        if ready.repairing {
            let lane = ready.spec.as_ref().map_or_else(
                || "a rework lane".into(),
                |s| format!("the {s} rework lane"),
            );
            return Some((
                INFO_CATEGORY,
                format!("conflict repair in progress: {lane}"),
                "none: wait for the repair lane to push a resolved head".into(),
            ));
        }
        let fix = ready.spec.as_ref().map_or_else(
            || format!("rebase PR {n} onto its base"),
            |s| format!("`aida rework {s}`"),
        );
        return fail(
            "merge conflict with no repair lane".into(),
            format!("route the conflict repair: {fix}"),
        );
    }
    match &ready.ci {
        // Only a held attempt stops the watcher; anything else is the review
        // backlog, queued or waiting for capacity. trace:STORY-3131 | ai:claude
        Ci::Green if !ready.approved => match ready.review {
            // The watcher's own retained-checkout signal, reused as the
            // hold's cause rather than reported twice. trace:STORY-3131 | ai:claude
            _ if ready.retained.is_some() => {
                let (why, action) = split_retained(ready.retained.as_deref()?);
                fail(why.into(), action.into())
            }
            Some(ReviewWatch::Held) => fail(
                "review held at the current head without a verdict".into(),
                format!("check `aida review watch status`; release the hold with `aida review watch --release PR-{n}`"),
            ),
            watch => Some((
                INFO_CATEGORY,
                if watch.is_some() {
                    "review in flight at the current head".into()
                } else {
                    "review waiting for watcher capacity".into()
                },
                "none: wait for the review".into(),
            )),
        },
        Ci::Red(failed) => fail(
            format!("red required check ({failed})"),
            format!("inspect `gh pr checks {n}`; rerun a flake or route rework"),
        ),
        // A cancelled full tier holds CI pending, but nothing is running;
        // a missing one only matters once the fast tier is green.
        Ci::Green | Ci::Pending
            if ready.full_tier_gap == Some("cancelled")
                || (ready.ci == Ci::Green && ready.full_tier_gap.is_some()) =>
        {
            fail(
                format!("full CI tier {} at the approved head", ready.full_tier_gap?),
                format!("add the `ci:full` label to PR {n} to run full-tier CI at its head"),
            )
        }
        // No automatic train picks up a supervised PR, so a held lock is not
        // what it waits for. trace:STORY-3131 | ai:claude
        Ci::Green if ready.supervised => fail(
            "supervised, not drain-eligible".into(),
            format!("needs operator train: `aida pr train {n}`"),
        ),
        Ci::Green if lock.is_some() => lock_cause(n, lock?),
        Ci::Green => fail(
            "unknown".into(),
            "investigate: `aida land stalls`, train admission and the schedule log".into(),
        ),
        // CI still running at the head: not ready yet.
        _ => None,
    }
}

/// A PR waiting on a held train lock. trace:STORY-3131 | ai:claude
fn lock_cause(n: u64, lock: &LockHolder) -> Option<Cause> {
    let pid = lock
        .pid
        .map_or("an unidentified process".into(), |p| format!("pid {p}"));
    let command = lock
        .command
        .as_ref()
        .map_or(String::new(), |c| format!(" ({c})"));
    let age = lock
        .held_minutes
        .map_or(String::new(), |m| format!(" {m}m"));
    let t = lock.threshold_minutes;
    let status = "`aida pr train --status`";
    Some(match lock.state() {
        LockState::Busy => (
            INFO_CATEGORY,
            format!("waiting for the running train (lock held{age} by {pid})"),
            format!("none: the next train picks up PR {n}"),
        ),
        LockState::Slow => (
            CATEGORY,
            format!("train lock held{age} by {pid}{command}, past the {t}m threshold"),
            format!("investigate {status}: no sign yet that it is stuck; then train PR {n}"),
        ),
        LockState::Stuck => (
            CATEGORY,
            format!(
                "train lock held{age} by {pid}{command}: no step change, checkpoint or CI job for {}m",
                lock.idle_minutes.unwrap_or(t)
            ),
            format!("confirm with {status}, stop {pid}: it is stuck; then train PR {n}"),
        ),
        LockState::Orphaned => (
            CATEGORY,
            format!("train lock held{age} by {pid} with no progress"),
            format!("investigate {status} for an orphaned lock; then train PR {n}"),
        ),
    })
}

pub(crate) fn evaluate(
    prs: &[Ready],
    policy: &Policy,
    now: DateTime<Utc>,
    last_landing: Option<DateTime<Utc>>,
    lock: Option<&LockHolder>,
    base: &str,
) -> Vec<DoctorFinding> {
    if !policy.enabled {
        return Vec::new();
    }
    let dwell = TimeDelta::minutes(policy.ready_dwell_minutes);
    let mut findings: Vec<DoctorFinding> = prs
        .iter()
        .filter(|r| !r.on_train)
        .filter_map(|r| {
            // A drift ejection is its own cause, dwell or not. trace:BUG-2777 | ai:claude
            let drifted = drift_cause(r, now).map(|(why, action)| (CATEGORY, why, action));
            if drifted.is_none() && now.signed_duration_since(r.since) <= dwell {
                return None;
            }
            let (category, why, action) = drifted.or_else(|| cause(r, lock))?;
            let state = if r.approved {
                "approved and ready"
            } else {
                "green"
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
        .filter(|r| r.approved && r.ci == Ci::Green && r.full_tier_gap.is_none())
        .filter(|r| !r.conflict && r.drift.is_none())
        .count();
    if dry && landable > 0 {
        // A train still working is the landing in progress, not a drought.
        let busy = lock.is_some_and(|l| l.state() == LockState::Busy);
        let since = last_landing.map_or("ever".into(), |at| {
            format!("{}m", now.signed_duration_since(at).num_minutes())
        });
        findings.push(DoctorFinding {
            category: if busy { INFO_CATEGORY } else { CATEGORY }.into(),
            id: base.into(),
            summary: format!("no landing on {base} for {since} while {landable} landable PRs wait"),
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

fn last_landing(root: &Path, base: &str) -> Result<Option<DateTime<Utc>>> {
    for rev in [format!("origin/{base}"), base.to_owned()] {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%cI", &rev, "--"])
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
    bail!("cannot read the last landing on {base}")
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

/// Whether a train pins `head`, and its drift row, read from the main
/// worktree's `.aida` where the train writes them (`train_root`, resolved by
/// the caller with [`crate::main_worktree_root_from`]). trace:BUG-2777 | ai:claude
pub(crate) fn train_state(
    train_root: &std::path::Path,
    number: u64,
    head: &str,
) -> Result<(bool, Option<(DateTime<Utc>, String)>)> {
    Ok((
        !crate::pr_train::pr_head_pins(train_root, number)?.is_empty(),
        crate::pr_train::culprit::drift(train_root, number, head),
    ))
}

/// The train's pin and drift row for `(number, head)`; see [`train_state`].
type TrainState = (bool, Option<(DateTime<Utc>, String)>);

/// What [`ready_of`] looks up outside the PR listing row, so tests script it.
// trace:STORY-3131 | ai:claude
struct Lookups<'a> {
    base: &'a str,
    tiered: bool,
    watch: &'a [(String, String, crate::review_watch::ReviewWatch)],
    /// The watcher's retained-checkout blocks, `(PR-N, line)`.
    retained: &'a [(String, String)],
    verdict: &'a dyn Fn(u64, &str) -> Result<AtHead>,
    repairing: &'a dyn Fn(&str, &str) -> bool,
    supervised: &'a dyn Fn(&[String]) -> bool,
    train: &'a dyn Fn(u64, &str) -> Result<TrainState>,
}

/// One open-PR listing row as landing evidence; `None` when it is a draft,
/// targets another base, has a settled verdict, or is not ready yet.
fn ready_of(pr: &serde_json::Value, l: &Lookups<'_>) -> Result<Option<Ready>> {
    if pr["isDraft"] == true || pr["baseRefName"].as_str() != Some(l.base) {
        return Ok(None);
    }
    let number = pr["number"].as_u64().context("PR number missing")?;
    let head = pr["headRefOid"].as_str().context("invalid PR head")?;
    let rollup = &pr["statusCheckRollup"];
    let ci = crate::land::ci_of_rollup(rollup);
    let green_at = crate::land::green_rollup_at(rollup).map(|t| t.with_timezone(&Utc));
    // An unreviewed head counts once its CI is green: the review is the wait.
    let (approved, since) = match ((l.verdict)(number, head)?, green_at) {
        (AtHead::Approved(at), green) => (true, green.map_or(at, |ci| at.max(ci))),
        (AtHead::Missing, Some(green)) if ci == Ci::Green => (false, green),
        _ => return Ok(None),
    };
    let branch = pr["headRefName"].as_str().unwrap_or_default();
    let title = pr["title"].as_str().unwrap_or_default();
    let spec_ids = crate::pr_ship::extract_pr_spec_ids(branch, title);
    let key = format!("PR-{number}");
    let (on_train, drift) = (l.train)(number, head)?;
    Ok(Some(Ready {
        number,
        since,
        approved,
        review: l
            .watch
            .iter()
            .find(|(k, sha, _)| *k == key && sha.eq_ignore_ascii_case(head))
            .map(|(_, _, w)| *w),
        retained: l
            .retained
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, line)| line.clone()),
        conflict: pr["mergeable"] == "CONFLICTING",
        repairing: spec_ids.iter().any(|s| (l.repairing)(s, branch)),
        spec: spec_ids.first().cloned(),
        ci,
        full_tier_gap: l.tiered.then(|| full_tier_gap(rollup)).flatten(),
        supervised: (l.supervised)(&spec_ids),
        on_train,
        drift,
    }))
}

/// The train-lock evidence from the MAIN worktree's `.aida/train`, where the
/// train holds its lock: a seat or lane worktree's own `.aida` never has it.
// trace:STORY-3131 | ai:claude
fn train_lock(root: &Path, now: DateTime<Utc>) -> Result<Option<crate::pr_train::LockEvidence>> {
    crate::pr_train::lock_driver(&crate::main_worktree_root_from(root), now)
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
    let lanes = crate::stalled_reviews::ReworkLanes::load(root, &storage, &store)?;
    // One clock for PR ages and the lock's age. trace:STORY-3131 | ai:claude
    let now = DateTime::<Utc>::from(exec.clock().wall_now());
    // The base the train lands on, resolved as the landing gate does.
    let base = crate::trusted_config::local_default_branch_name(root)
        .unwrap_or_else(|| crate::forge::default_branch_of(root));
    let watch = crate::review_watch::review_heads(root);
    let retained = crate::review_watch::service::retained_review_blocks(root, now);
    // trace:BUG-2777 | ai:claude
    let train_root = crate::main_worktree_root_from(root);
    let lookups = Lookups {
        base: &base,
        tiered: crate::full_tier::repo_is_tiered(root)?,
        watch: &watch,
        retained: &retained,
        verdict: &|number, head| {
            let records = project::change_verdicts_checked(root, ChangeKey::pr(number))
                .map_err(anyhow::Error::msg)?;
            ledger_at_head(records, head)
        },
        repairing: &|spec, branch| lanes.covers(&store, spec, branch),
        supervised: &|specs| {
            needs_supervision(specs.iter().map(|s| {
                store
                    .get_requirement_by_spec_id(s)
                    .map(|r| r.execution_mode)
            }))
        },
        train: &|number, head| train_state(&train_root, number, head),
    };
    let mut ready = Vec::new();
    for pr in crate::stalled_reviews::open_prs(root)? {
        ready.extend(ready_of(&pr, &lookups)?);
    }
    let lock = train_lock(&train_root, now)?.map(|e| {
        let minutes = |s: u64| i64::try_from(s / 60).unwrap_or(i64::MAX);
        let mut holder = LockHolder {
            pid: e.pid,
            held_minutes: e.held_secs.map(minutes),
            command: e.pid.and_then(command_line),
            idle_minutes: e.idle_secs.map(minutes),
            ci_live: None,
            threshold_minutes: lock_threshold(&policy, &e.history_secs),
        };
        // The forge CI read only runs when it could change the advice.
        if holder.state() != LockState::Busy {
            let live: Vec<_> = e
                .train_prs
                .iter()
                .map(|pr| crate::pr_train::train_ci_live(&train_root, *pr))
                .collect();
            holder.ci_live = if live.contains(&Some(true)) {
                Some(true)
            } else if !live.is_empty() && live.iter().all(|l| *l == Some(false)) {
                Some(false)
            } else {
                None
            };
        }
        holder
    });
    let mut findings = evaluate(
        &ready,
        &policy,
        now,
        last_landing(root, &base)?,
        lock.as_ref(),
        &base,
    );
    // A retained block a waiting PR's finding already carries is not
    // repeated; the rest (a PR inside its dwell, or not ready) still shows.
    let rest: Vec<_> = retained
        .into_iter()
        .filter(|(id, _)| !findings.iter().any(|f| f.id == *id))
        .collect();
    findings.extend(retained_findings(rest));
    Ok((policy, findings))
}

/// A retained block line as a finding's (summary, action).
fn split_retained(line: &str) -> (&str, &str) {
    line.split_once("; fix: ").unwrap_or((line, ""))
}

/// A review the watcher holds behind a kept reviewer checkout starves the
/// landing pipeline upstream of any approval. trace:BUG-2776 | ai:claude
pub(crate) fn retained_findings(blocks: Vec<(String, String)>) -> Vec<DoctorFinding> {
    blocks
        .into_iter()
        .map(|(id, line)| {
            let (summary, action) = split_retained(&line);
            DoctorFinding {
                category: CATEGORY.into(),
                summary: summary.into(),
                action: action.into(),
                id,
                safe_heal: false,
            }
        })
        .collect()
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
