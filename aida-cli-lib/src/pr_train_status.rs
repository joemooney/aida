//! `aida pr train --status`: what the train is doing, from local state only.
//!
//! Reads the journals under `.aida/train`, the step clock beside each one and
//! the driver record in the run lock. It takes no train lock, runs no gate and
//! changes nothing; the one forge call is a bounded read of the train PR's CI
//! rows.
// trace:STORY-2823 | ai:claude
use super::*;
use aida_core::proc_identity::{HolderRecord, Verdict};
use serde::Serialize;

/// Exit code when no train is journaled and nothing drives one.
pub(crate) const NO_TRAIN_EXIT_CODE: i32 = 40;
/// Exit code when a journal remains but no live process drives it.
pub(crate) const ORPHANED_EXIT_CODE: i32 = 41;
/// Shared budget for forge classification and the CI read for one train PR.
const CI_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Carries the status exit code past the top-level error printer.
#[derive(Debug)]
pub(crate) struct TrainStatusExit(pub(crate) i32);
impl std::fmt::Display for TrainStatusExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "train status exit {}", self.0)
    }
}
impl std::error::Error for TrainStatusExit {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum State {
    None,
    Running,
    Orphaned,
}

impl State {
    pub(super) fn exit_code(self) -> i32 {
        match self {
            State::Running => 0,
            State::None => NO_TRAIN_EXIT_CODE,
            State::Orphaned => ORPHANED_EXIT_CODE,
        }
    }
}

/// The live process holding the run lock. `pid` is absent when a process
/// holds the lock but its identity cannot be judged from here (another node,
/// boot or PID namespace, or a driver older than the driver record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DriverView {
    pub(super) pid: Option<u32>,
    pub(super) running_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct MemberView {
    pub(super) pr: u64,
    pub(super) head: String,
    /// `riding`, `landed` or `rejected`.
    pub(super) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct CiView {
    pub(super) checks: usize,
    pub(super) passed: usize,
    pub(super) failed: usize,
    pub(super) pending: usize,
    pub(super) shards: usize,
    pub(super) shards_passed: usize,
    /// One line per workflow run, as the train prints them while it waits.
    pub(super) runs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TrainView {
    pub(super) journal: String,
    pub(super) branch: String,
    pub(super) train_pr: Option<u64>,
    pub(super) base: String,
    pub(super) tip: String,
    pub(super) step: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) retire_reason: Option<String>,
    /// Absent for a journal written before the step clock existed.
    pub(super) time_in_step_secs: Option<u64>,
    pub(super) last_call: String,
    pub(super) last_checkpoint_secs: Option<u64>,
    pub(super) members: Vec<MemberView>,
    pub(super) ci: Option<CiView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ci_unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TrainStatus {
    pub(super) state: State,
    pub(super) exit_code: i32,
    pub(super) driver: Option<DriverView>,
    pub(super) trains: Vec<TrainView>,
}

/// What the status reads from outside the journal directory, so tests script it.
pub(super) struct Probes<'a> {
    pub(super) now: chrono::DateTime<chrono::Utc>,
    /// The verdict on a recorded driver; only an alive identity supplies a PID.
    pub(super) observe: &'a dyn Fn(&HolderRecord) -> Option<Verdict>,
    /// The CI rows of a train PR, or why they could not be read.
    pub(super) ci: &'a dyn Fn(u64) -> std::result::Result<Vec<crate::ci_gate::CheckRow>, String>,
}

fn secs_since(now: chrono::DateTime<chrono::Utc>, then: &str) -> Option<u64> {
    let then = chrono::DateTime::parse_from_rfc3339(then).ok()?;
    u64::try_from((now - then.with_timezone(&chrono::Utc)).num_seconds()).ok()
}

/// Lock occupancy decides whether a train is running. The record only names
/// the driver when its process identity is still alive.
// trace:STORY-2823 | ai:codex
fn live_driver(dir: &Path, probes: &Probes<'_>) -> Result<Option<DriverView>> {
    if !journal::lock_held(dir)? {
        return Ok(None);
    }
    if let Some(record) = journal::driver_record(dir) {
        if matches!((probes.observe)(&record.holder), Some(Verdict::Alive)) {
            return Ok(Some(DriverView {
                pid: record.holder.identity().map(|id| id.pid),
                running_secs: secs_since(probes.now, &record.since),
            }));
        }
    }
    Ok(Some(DriverView {
        pid: None,
        running_secs: None,
    }))
}

fn ci_view(rows: &[crate::ci_gate::CheckRow]) -> CiView {
    let count = |bucket: &[&str]| {
        rows.iter()
            .filter(|r| bucket.contains(&r.bucket.as_str()))
            .count()
    };
    let shards: Vec<_> = rows.iter().filter(|r| train_ci_is_shard(r)).collect();
    CiView {
        checks: rows.len(),
        passed: count(&["pass"]),
        failed: count(&["fail", "cancel"]),
        pending: count(&["pending"]),
        shards: shards.len(),
        shards_passed: shards.iter().filter(|r| r.bucket == "pass").count(),
        runs: train_ci_wait_details(rows),
    }
}

fn train_view(path: &Path, entry: &journal::Entry, probes: &Probes<'_>) -> TrainView {
    let step = entry.step.label();
    let members = entry
        .train
        .cars
        .iter()
        .map(|car| {
            let pr = car.member.pr;
            let rejected = entry.rejected.iter().find(|r| r.pr == pr);
            MemberView {
                pr,
                head: car.member.head.clone(),
                state: if entry.completed_members.contains(&pr) {
                    "landed"
                } else if rejected.is_some() {
                    "rejected"
                } else {
                    "riding"
                },
                reason: rejected.map(|r| r.reason.clone()),
            }
        })
        .collect();
    let (ci, ci_unavailable) = match entry.change_id.map(probes.ci) {
        None => (None, None),
        Some(Ok(rows)) => (Some(ci_view(&rows)), None),
        Some(Err(why)) => (None, Some(why)),
    };
    TrainView {
        journal: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        branch: entry.branch.clone(),
        train_pr: entry.change_id,
        base: entry.train.base.clone(),
        tip: entry.train.tip.clone(),
        step,
        retire_reason: match &entry.step {
            journal::Step::Retire { why } => Some(why.clone()),
            _ => None,
        },
        time_in_step_secs: journal::step_clock(path)
            .filter(|clock| clock.step == step)
            .and_then(|clock| secs_since(probes.now, &clock.since)),
        last_call: entry.call.clone(),
        last_checkpoint_secs: std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| {
                let at: chrono::DateTime<chrono::Utc> = at.into();
                u64::try_from((probes.now - at).num_seconds()).ok()
            }),
        members,
        ci,
        ci_unavailable,
    }
}

pub(super) fn collect(dir: &Path, probes: &Probes<'_>) -> Result<TrainStatus> {
    let journals = journal::peek(dir)?;
    let driver = live_driver(dir, probes)?;
    let state = match (&driver, journals.is_empty()) {
        (Some(_), _) => State::Running,
        (None, true) => State::None,
        (None, false) => State::Orphaned,
    };
    Ok(TrainStatus {
        state,
        exit_code: state.exit_code(),
        driver,
        trains: journals
            .iter()
            .map(|(path, entry)| train_view(path, entry, probes))
            .collect(),
    })
}

fn span(secs: u64) -> String {
    match (secs / 3600, secs % 3600 / 60, secs % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, m, _) => format!("{h}h {m}m"),
    }
}

pub(super) fn render(status: &TrainStatus) -> String {
    let mut out = Vec::new();
    match (&status.driver, status.state) {
        (Some(driver), _) => {
            let who = match driver.pid {
                Some(pid) => format!("driver PID {pid}"),
                None => "a live driver whose PID is not visible from here".into(),
            };
            let since = driver
                .running_secs
                .map(|s| format!(", running {}", span(s)))
                .unwrap_or_default();
            out.push(format!("Train: running ({who}{since})"));
            if status.trains.is_empty() {
                out.push("  no train built yet: the driver is gating or building".into());
            }
        }
        (None, State::Orphaned) => {
            out.push("Train: orphaned, no live driver (orphaned journal)".into());
        }
        (None, _) => out.push("No train: no journal and no live driver.".into()),
    }
    for train in &status.trains {
        let pr = match train.train_pr {
            Some(n) => format!("train PR #{n}"),
            None => "train PR not opened yet".into(),
        };
        let in_step = match train.time_in_step_secs {
            Some(s) => format!("for {}", span(s)),
            None => "for an unknown time".into(),
        };
        out.push(format!(
            "{pr} ({})  step {} {in_step}",
            train.branch, train.step
        ));
        if let Some(why) = &train.retire_reason {
            out.push(format!("  retiring: {why}"));
        }
        if !train.last_call.is_empty() {
            let ago = train
                .last_checkpoint_secs
                .map(|s| format!(", {} ago", span(s)))
                .unwrap_or_default();
            out.push(format!("  last call: {}{ago}", train.last_call));
        }
        out.push(format!(
            "  base {}  tip {}",
            short(&train.base),
            short(&train.tip)
        ));
        out.push("  members:".into());
        for m in &train.members {
            let reason = m
                .reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default();
            out.push(format!(
                "    #{} @ {}  {}{reason}",
                m.pr,
                short(&m.head),
                m.state
            ));
        }
        match (&train.ci, &train.ci_unavailable) {
            (Some(ci), _) if ci.checks == 0 => out.push("  CI: no checks registered yet".into()),
            (Some(ci), _) => {
                out.push(format!(
                    "  CI: {}/{} shards passed; {} of {} checks passed, {} failed, {} pending",
                    ci.shards_passed, ci.shards, ci.passed, ci.checks, ci.failed, ci.pending
                ));
                out.extend(ci.runs.iter().map(|run| format!("    {run}")));
            }
            (None, Some(why)) => out.push(format!("  CI: unavailable ({why})")),
            (None, None) => out.push("  CI: none, the train PR is not open".into()),
        }
        if status.state == State::Orphaned {
            let members: Vec<_> = train.members.iter().map(|m| m.pr.to_string()).collect();
            out.push(format!(
                "  resume with `aida pr train {}`, or discard with `aida pr train --abandon {}`",
                members.join(" "),
                train.journal
            ));
        }
    }
    out.join("\n")
}

/// The train PR's CI rows, read once and stopped at [`CI_READ_TIMEOUT`].
fn read_ci_rows(
    main_worktree: &Path,
    pr: u64,
) -> std::result::Result<Vec<crate::ci_gate::CheckRow>, String> {
    let started = std::time::Instant::now();
    if crate::forge::train_status_forge_kind(main_worktree, CI_READ_TIMEOUT)?
        != crate::forge::ForgeKind::GitHub
    {
        return Err("this forge exposes no per-check CI rows".into());
    }
    let remaining = CI_READ_TIMEOUT.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err("CI read budget exhausted during forge classification".into());
    }
    crate::forge::gh_pr_check_rows_bounded(main_worktree, pr, remaining)
}

/// Resolve without falling back to a possibly unrelated journal directory.
// trace:STORY-2823 | ai:codex
fn status_main_worktree(root: &Path) -> Result<PathBuf> {
    match crate::seat_clone::main_for(root) {
        crate::seat_clone::MainFor::Verified(main) => return Ok(main),
        crate::seat_clone::MainFor::Unverified(clone) => {
            return Err(crate::seat_clone::unverified_refusal(&clone));
        }
        crate::seat_clone::MainFor::NotAClone => {}
    }
    let mut git = std::process::Command::new("git");
    git.arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain", "-z"]);
    let out = match crate::bounded_process::command_output_with_timeout_capped(
        git,
        std::time::Duration::from_secs(3),
        1024 * 1024,
    ) {
        crate::bounded_process::BoundedCommandOutput::Completed(out) => out,
        _ => anyhow::bail!("train status worktree lookup failed or exceeded its 3-second budget"),
    };
    anyhow::ensure!(
        out.status.success(),
        "train status worktree lookup failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    anyhow::ensure!(
        out.stdout.len() <= 1024 * 1024,
        "train status worktree lookup exceeded its output budget"
    );
    let first = out.stdout.split(|b| *b == 0).next().unwrap_or_default();
    let path = first
        .strip_prefix(b"worktree ")
        .context("train status worktree lookup returned no main worktree")?;
    use std::os::unix::ffi::OsStrExt;
    let main = PathBuf::from(std::ffi::OsStr::from_bytes(path));
    anyhow::ensure!(
        main.is_absolute(),
        "train status worktree lookup returned an invalid path"
    );
    aida_core::seat_clone_detect::note_host_main(&main);
    Ok(main)
}

/// `aida pr train --status [--json]`.
pub(crate) fn train_status(json: bool) -> Result<()> {
    let root = crate::find_project_root()?;
    let main_worktree = status_main_worktree(&root)?;
    let status = collect(
        &main_worktree.join(".aida/train"),
        &Probes {
            now: chrono::Utc::now(),
            observe: &|holder| holder.observe().ok(),
            ci: &|pr| read_ci_rows(&main_worktree, pr),
        },
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{}", render(&status));
    }
    match status.exit_code {
        0 => Ok(()),
        code => Err(anyhow::Error::new(TrainStatusExit(code)).context(crate::TypedPayloadEmitted)),
    }
}

#[cfg(test)]
#[path = "tests/story_2823_train_status_tests.rs"]
mod tests;
