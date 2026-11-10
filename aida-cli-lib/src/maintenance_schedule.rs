//! Native no-daemon job registry: `aida schedule` (alias `aida cron`).
//!
//! STORY-1047 shipped this as a small allow-list maintenance runner
//! (`[schedule]` tasks with an `interval`, run by `aida schedule tick` from
//! the per-turn hook, ledgered in `.aida/schedule-state.json`). STORY-1226
//! grows the same command — not a third scheduler — into a **per-seat job
//! registry** (ADR-46):
//!
//! - **Two job kinds.** A *substrate* job (`command = "session reap"`) needs no
//!   LLM: the tick runs it through the existing allow-list runner. A *seat*
//!   job (`prompt = "triage the mailbox"`, `seats = ["advisor"]`) needs the
//!   seat's judgment: the scheduler NEVER executes it — it only becomes *due*
//!   and is delivered as text to whoever holds the seat (`aida awaiting
//!   --notice`, the pickup prompt, `aida agent new`'s launch context,
//!   `aida schedule due`). The seat reports back with `aida schedule done`.
//! - **Three schedule kinds** on one entry shape: `every = "30m"` (interval
//!   heartbeat), `on = ["PrMerged", "MailReceived"]` (events already emitted to
//!   `.aida/events.jsonl`), `when = "mail.oldest_unread_age > 15m"` (a typed
//!   predicate over the substrate snapshot, see `schedule_predicate`).
//!   `every` + `on` combine (event fast path + interval fallback). A `when`
//!   job fires once when its predicate turns true and not again until it has
//!   been false (once-until-cleared), recorded as an episode.
//! - **Registry** = project `[schedule]` in `.aida/config.toml` plus the
//!   machine-global `~/.aida/schedule.toml`, merged by job name, project wins.
//! - **Ledger** = one store object per job (`schedule/<job>.yaml` on the
//!   `aida-store` branch, see `schedule_ledger`) so "who last triaged the
//!   mailbox and when" has one answer on every clone. The local
//!   `.aida/schedule-state.json` keeps only per-clone state: the tick
//!   min-gap clock, a mirror of the last run for offline reads, and each
//!   job's `on` event cursor (events.jsonl is per clone).
//!
//! The STORY-262 cadence-fires-a-TASK `schedules.toml` entries surface in
//! `aida schedule list` as kind `fires_task` (deprecated in place; `aida pull`
//! still fires them). Cold-boot of a headless seat for an overdue seat job
//! and the installer belong to STORY-1218's tick, which consults this same
//! registry.
//!
//! trace:STORY-1047 | ai:codex
//! trace:STORY-1226 | ai:claude

use aida_core::DatabaseBackend;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

// trace:STORY-1728 | ai:claude
#[path = "maintenance_schedule_args.rs"]
pub(crate) mod maintenance_schedule_args;
pub(crate) use maintenance_schedule_args::*;

use crate::cli::MaintenanceScheduleCommand;
use crate::events::{self, Event, EventKind};
use crate::schedule_ledger::{self, EpisodeTransition, JobLedger, LastBy};
use crate::schedule_predicate::{self, Expr, Field, Snapshot};

const DEFAULT_MIN_GAP: &str = "60s";

/// Machine-global registry layer: `~/.aida/schedule.toml`.
pub(crate) const GLOBAL_SCHEDULE_FILE: &str = "schedule.toml";

#[derive(Debug, Clone, Deserialize)]
struct ConfigFile {
    schedule: Option<ScheduleConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ScheduleConfig {
    #[serde(default = "default_min_gap")]
    min_gap: String,
    /// Legacy STORY-1047 spelling (`[[schedule.tasks]]`) — still accepted.
    #[serde(default)]
    tasks: Vec<JobConfig>,
    /// STORY-1226 spelling (`[[schedule.jobs]]`). Both lists concatenate.
    #[serde(default)]
    jobs: Vec<JobConfig>,
}

fn default_min_gap() -> String {
    DEFAULT_MIN_GAP.to_string()
}

/// One `[[schedule.jobs]]` / `[[schedule.tasks]]` entry as written in TOML.
/// Back-compat: every STORY-1047 field keeps its name and meaning;
/// `interval` gains the alias `every`.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Deserialize)]
struct JobConfig {
    name: String,
    /// Substrate job: an allow-listed `aida` subcommand.
    #[serde(default)]
    command: Option<String>,
    /// Seat job: the instruction delivered to the seat when due.
    #[serde(default)]
    prompt: Option<String>,
    /// Interval heartbeat (`30m`, `2h`, `1d`). `every` is the new spelling.
    #[serde(default, alias = "every")]
    interval: Option<String>,
    /// Event kinds (serialized `event` tag names) that make the job due.
    /// A job-carrying event may name its source job after a colon —
    /// `"CronJobFailed:disk-headroom-guard"` fires only on that job's
    /// failure; a bare `"CronJobFailed"` still matches every job's failure.
    #[serde(default)]
    on: Vec<String>,
    /// Typed predicate over the substrate snapshot.
    #[serde(default)]
    when: Option<String>,
    /// Gate on the `every` cadence: an interval run fires only while this
    /// predicate holds (checked on every tick, no episode). Unlike `when`,
    /// which is a separate once-until-cleared trigger, `while` never fires
    /// a job by itself.
    // trace:STORY-1917 | ai:claude
    #[serde(default, rename = "while")]
    while_: Option<String>,
    #[serde(default, alias = "quiet-hours")]
    quiet_hours: Option<String>,
    #[serde(default)]
    enabled: bool,
    /// Seats the job applies to; `["*"]` = any seat (the substrate default).
    #[serde(default)]
    seats: Vec<String>,
    /// `substrate` | `seat`; inferred from `command` / `prompt` when absent.
    #[serde(default)]
    kind: Option<JobKind>,
}

/// What a job needs to run.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobKind {
    /// No LLM: the tick runs an allow-listed `aida` command.
    Substrate,
    /// Needs the seat's judgment: delivered as text, never executed here.
    Seat,
    /// Legacy STORY-262 cadence entry from `.aida/schedules.toml` — files a
    /// TASK on `aida pull`. Deprecated; shown for completeness only.
    FiresTask,
}

impl JobKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            JobKind::Substrate => "substrate",
            JobKind::Seat => "seat",
            JobKind::FiresTask => "fires_task",
        }
    }
}

/// Which registry layer a job came from (project wins on a name clash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobSource {
    Project,
    Global,
    Legacy,
    /// Synthesized from this clone's machine-local `~/.aida/store-local.toml`
    /// (the hub-bridge reconcile and its route).
    // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
    Local,
}

impl JobSource {
    fn as_str(self) -> &'static str {
        match self {
            JobSource::Project => "project",
            JobSource::Global => "global",
            JobSource::Legacy => "legacy",
            JobSource::Local => "local",
        }
    }
}

/// One validated `on` entry: an event kind, optionally bound to the job that
/// produced the event (`CronJobFailed:<job>`). Unbound keeps the original
/// meaning — any event of that kind — so existing configs behave as before.
// trace:BUG-1655 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventTrigger {
    kind: String,
    job: Option<String>,
}

/// Event kinds that carry a `job` field and so accept a `:<job>` binding.
// trace:BUG-1655 | ai:claude
const JOB_BOUND_EVENTS: &[&str] = &["CronJobFailed", "CronJobFired"];

impl EventTrigger {
    fn parse(job_name: &str, raw: &str) -> Result<Self> {
        let (kind, job) = match raw.split_once(':') {
            Some((kind, job)) => (kind.trim(), Some(job.trim())),
            None => (raw.trim(), None),
        };
        if !EventKind::known_names().contains(&kind) {
            anyhow::bail!(
                "scheduled job '{job_name}': unknown event '{kind}' in `on`; valid events: {}",
                EventKind::known_names().join(", ")
            );
        }
        let job = match job {
            None => None,
            Some("") => anyhow::bail!(
                "scheduled job '{job_name}': `on = [\"{raw}\"]` names no job after the colon; \
                 write `{kind}:<job-name>` or drop the colon to match every job"
            ),
            Some(j) if !JOB_BOUND_EVENTS.contains(&kind) => anyhow::bail!(
                "scheduled job '{job_name}': event '{kind}' has no source job, so `{kind}:{j}` \
                 cannot be bound; only {} accept `:<job-name>`",
                JOB_BOUND_EVENTS.join(", ")
            ),
            Some(j) => Some(j.to_string()),
        };
        Ok(Self {
            kind: kind.to_string(),
            job,
        })
    }

    /// Does `ev` satisfy this trigger? The kind must match and, when bound,
    /// the event's source job must be the bound job.
    fn matches(&self, ev: &EventKind) -> bool {
        if self.kind != ev.name() {
            return false;
        }
        let Some(want) = &self.job else {
            return true;
        };
        match ev {
            EventKind::CronJobFailed { job, .. } | EventKind::CronJobFired { job, .. } => {
                job == want
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for EventTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.job {
            Some(job) => write!(f, "{}:{job}", self.kind),
            None => f.write_str(&self.kind),
        }
    }
}

/// A validated registry entry.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub name: String,
    pub kind: JobKind,
    pub seats: Vec<String>,
    command: Option<ScheduledCommand>,
    pub prompt: Option<String>,
    interval: Option<Duration>,
    on: Vec<EventTrigger>,
    when: Option<Expr>,
    when_raw: Option<String>,
    /// `while` gate on the interval trigger.
    // trace:STORY-1917 | ai:claude
    while_: Option<Expr>,
    while_raw: Option<String>,
    quiet_hours: Option<QuietHours>,
    enabled: bool,
    pub source: JobSource,
    /// Why this job cannot run (e.g. its `on` binds to a job that is not in
    /// the registry). Such a job is loaded disabled so every other job keeps
    /// running; tick and `schedule status` surface the reason.
    // trace:BUG-1655 | ai:claude
    problem: Option<String>,
    /// A substrate job whose `command` this binary does not know (a job
    /// registered by a newer `aida`, or a typo). The job stays in the
    /// registry: the tick fails THAT job (ledger + `CronJobFailed`) and keeps
    /// running every other due job, instead of the whole registry failing
    /// to load and silently stopping them all.
    // trace:BUG-2142 | ai:claude
    invalid_command: Option<InvalidCommand>,
}

/// The raw `command` string of a substrate job and why it was rejected.
// trace:BUG-2142 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
struct InvalidCommand {
    raw: String,
    error: String,
}

impl Task {
    /// Does this job apply to `seat`? `*` matches every seat; names compare
    /// case-insensitively after the `dialog` → `advisor` fold.
    pub(crate) fn applies_to_seat(&self, seat: &str) -> bool {
        let want = crate::canonical_role_name(seat);
        self.seats
            .iter()
            .any(|s| s == "*" || crate::canonical_role_name(s) == want)
    }

    /// Human schedule summary, e.g. `every 30m + on MailReceived`.
    pub(crate) fn schedule_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(i) = self.interval {
            parts.push(format!("every {}", format_duration(i)));
        }
        if !self.on.is_empty() {
            let on: Vec<String> = self.on.iter().map(ToString::to_string).collect();
            parts.push(format!("on {}", on.join(",")));
        }
        if let Some(w) = &self.when_raw {
            parts.push(format!("when {w}"));
        }
        let summary = parts.join(" + ");
        match &self.while_raw {
            Some(w) => format!("{summary} while {w}"),
            None => summary,
        }
    }

    fn what(&self) -> String {
        if let Some(bad) = &self.invalid_command {
            return format!("aida {} (unsupported)", bad.raw.trim());
        }
        match (&self.command, &self.prompt) {
            (Some(c), _) => format!("aida {}", c.display),
            (None, Some(p)) => p.clone(),
            (None, None) => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledCommand {
    display: &'static str,
    args: &'static [&'static str],
    hook_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuietHours {
    start: NaiveTime,
    end: NaiveTime,
}

impl QuietHours {
    fn contains(&self, now: NaiveTime) -> bool {
        if self.start <= self.end {
            now >= self.start && now < self.end
        } else {
            now >= self.start || now < self.end
        }
    }
}

/// Per-clone runtime state (`.aida/schedule-state.json`). The cross-clone
/// truth is the store ledger; this file keeps the tick clock, an offline
/// mirror of last runs, and each job's `on` event cursor.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ScheduleState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_tick_at: Option<DateTime<Utc>>,
    #[serde(default)]
    tasks: BTreeMap<String, TaskState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TaskState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_success_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_status: Option<i32>,
    /// Newest `events.jsonl` timestamp this job's `on` trigger has consumed.
    // trace:STORY-1226 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_event_ts: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskOutcome {
    status: i32,
    stdout: String,
    stderr: String,
}

/// Why a job is due right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Trigger {
    Every(Duration),
    Event(String),
    Condition,
}

impl Trigger {
    fn label(&self) -> String {
        match self {
            Trigger::Every(d) => format!("every {}", format_duration(*d)),
            Trigger::Event(k) => format!("on {k}"),
            Trigger::Condition => "when".to_string(),
        }
    }
}

/// A due seat job as delivered to a seat — the one shape every surface
/// (notice, pickup prompt, launch context, `aida schedule due`) renders.
// trace:STORY-1226 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DueJob {
    pub name: String,
    pub kind: JobKind,
    pub seats: Vec<String>,
    pub schedule: String,
    pub reason: String,
    pub prompt: Option<String>,
    pub command: Option<String>,
    pub last_run: Option<DateTime<Utc>>,
    pub last_by: Option<String>,
    pub due_since: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<schedule_ledger::RoutedFailure>,
}

impl DueJob {
    /// One line: `mailbox-triage (every 30m, last 47m ago) → triage the mailbox`.
    pub(crate) fn line(&self, now: DateTime<Utc>) -> String {
        let last = match self.last_run {
            Some(t) => format!("last {} ago", human_age(now - t)),
            None => "never run".to_string(),
        };
        let what = self
            .prompt
            .clone()
            .or_else(|| self.command.as_ref().map(|c| format!("run: {c}")))
            .unwrap_or_default();
        let mut line = format!("{} ({}, {}) → {}", self.name, self.reason, last, what);
        if let Some(failure) = &self.failure {
            line.push_str(&format!("\n  trip evidence: {}", failure.trip_id));
            // trace:BUG-1655 | ai:claude
            if let Some(error) = &failure.error {
                let source = failure.job.as_deref().unwrap_or("the job");
                if error.starts_with(TIMEOUT_EXIT_PREFIX) {
                    line.push_str(&format!(
                        "\n  {source} timed out before reporting — this is not a finding: {error}"
                    ));
                } else if error.starts_with(SPAWN_FAILED_EXIT_PREFIX) {
                    // trace:TASK-1535 | ai:codex
                    line.push_str(&format!(
                        "\n  {source} could not be spawned — this is not a finding: {error}"
                    ));
                } else {
                    line.push_str(&format!("\n  {source} failed: {error}"));
                }
            }
            for audit in &failure.performance {
                let ceiling = audit.ceiling_ms.map_or_else(
                    || "n/a".to_string(),
                    |c| format!("{c} ms (breached={})", audit.ceiling_breached),
                );
                line.push_str(&format!(
                    "\n    aida {}: {:.3}% over {} ms ({} of {} calls; tolerance {:.3}%; worst {}; ceiling {}; window {}h; excluded {}; lineage_scoped={})",
                    audit.command,
                    audit.proportion_millipercent as f64 / 1000.0,
                    audit.budget_ms,
                    audit.over_budget,
                    audit.denominator,
                    audit.tolerated_millipercent as f64 / 1000.0,
                    audit.worst_ms.map_or_else(|| "n/a".into(), |v| format!("{v} ms")),
                    ceiling,
                    audit.window_hours,
                    audit.excluded_samples,
                    audit.lineage_scoped,
                ));
            }
        }
        line
    }
}

pub(crate) fn handle_schedule_command(
    cmd: &MaintenanceScheduleCommand,
    store_path: &Path,
    backend: Option<&aida_core::CachedGitBackend>,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
    match cmd {
        MaintenanceScheduleCommand::Tick { hook } => {
            let run = run_tick(project_root, *hook, backend)?;
            if !run.lines.is_empty() {
                println!("{}", run.lines.join("\n"));
            }
            // BUG-2142: every other due job has already run; the tick still
            // exits nonzero so the driver (systemd/cron log) shows the
            // registry needs a binary that knows these commands.
            // TASK-1769: this is the intended signal, not a side effect. While
            // an enabled job's command is unsupported, EVERY full tick exits
            // nonzero, so the systemd service stays `failed` (and each cron
            // run logs the error) until the binary is updated or the job is
            // fixed or disabled. Hook ticks never report it.
            // trace:BUG-2142 | ai:claude
            // trace:TASK-1769 | ai:claude
            if !run.unsupported.is_empty() {
                anyhow::bail!(
                    "schedule tick: {} scheduled job(s) use a command this aida binary does not \
                     support and were recorded as failed: {}. Every other due job ran. Update \
                     the binary the scheduler runs (`aida schedule install-systemd` or \
                     `install-cron` from the current aida) or fix the job's `command`.",
                    run.unsupported.len(),
                    run.unsupported.join(", ")
                );
            }
        }
        MaintenanceScheduleCommand::Run { name } => {
            let outcome = run_now(project_root, name.as_deref())?;
            if outcome.is_empty() {
                println!("No scheduled jobs configured.");
            } else {
                println!("{}", outcome.join("\n"));
            }
        }
        MaintenanceScheduleCommand::Status { json } => status(project_root, *json)?,
        MaintenanceScheduleCommand::List { seat, json } => {
            list(project_root, seat.as_deref(), *json)?
        }
        MaintenanceScheduleCommand::Due { seat, json } => {
            due(project_root, seat.as_deref(), *json)?
        }
        MaintenanceScheduleCommand::Done { job, note } => done(project_root, job, note.as_deref())?,
        MaintenanceScheduleCommand::EmitCron => emit_cron(project_root)?,
        MaintenanceScheduleCommand::InstallCron => {
            install_driver_command(project_root, crate::schedule_driver::Driver::Cron)?
        }
        MaintenanceScheduleCommand::UninstallCron => uninstall_cron_command(project_root)?,
        // trace:TASK-1491 | ai:claude
        MaintenanceScheduleCommand::InstallSystemd => {
            install_driver_command(project_root, crate::schedule_driver::Driver::Systemd)?
        }
        MaintenanceScheduleCommand::UninstallSystemd => {
            crate::schedule_driver::uninstall_systemd_command(project_root)?
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registry loading + merge
// ---------------------------------------------------------------------------

type LoadedScheduleConfig = ScheduleConfigLoaded;

#[derive(Debug, Clone)]
struct ScheduleConfigLoaded {
    min_gap: String,
    tasks: Vec<Task>,
}

/// Project layer: `[schedule]` in `<project>/.aida/config.toml`.
fn load_config(project_root: &Path) -> Result<Option<LoadedScheduleConfig>> {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let parsed: ConfigFile =
        toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?;
    let Some(raw) = parsed.schedule else {
        return Ok(None);
    };
    build_config(raw, JobSource::Project)
}

/// Global layer: `<home>/.aida/schedule.toml`. Accepts the same `[schedule]`
/// table as the project config, or the bare `[[jobs]]` / `min_gap` form.
// trace:STORY-1226 | ai:claude
fn load_global_config(home: &Path) -> Result<Option<LoadedScheduleConfig>> {
    let path = home.join(".aida").join(GLOBAL_SCHEDULE_FILE);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    // Advisor review round 1 (#1946): fall back to the bare ScheduleConfig
    // shape ONLY when the body has no `schedule` table; a typed error inside
    // `[schedule]` must surface, not silently drop every global job.
    // trace:STORY-1226 | ai:claude
    let has_schedule_table = toml::from_str::<toml::Value>(&body)
        .ok()
        .is_some_and(|v| v.get("schedule").is_some());
    let raw: ScheduleConfig = if has_schedule_table {
        toml::from_str::<ConfigFile>(&body)
            .with_context(|| format!("failed to parse {}", path.display()))?
            .schedule
            .unwrap_or_default()
    } else {
        toml::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))?
    };
    build_config(raw, JobSource::Global)
}

fn build_config(raw: ScheduleConfig, source: JobSource) -> Result<Option<LoadedScheduleConfig>> {
    let mut tasks = Vec::new();
    for job in raw.tasks.into_iter().chain(raw.jobs) {
        tasks.push(build_task(job, source)?);
    }
    Ok(Some(ScheduleConfigLoaded {
        min_gap: raw.min_gap,
        tasks,
    }))
}

/// Validate one entry: infer the kind, require the fields that kind needs,
/// require at least one schedule (`every` / `on` / `when`), and type-check
/// the predicate + event names at load time.
// trace:STORY-1226 | ai:claude
fn build_task(job: JobConfig, source: JobSource) -> Result<Task> {
    let name = job.name.clone();
    let kind = match job.kind {
        Some(JobKind::FiresTask) => anyhow::bail!(
            "scheduled job '{name}': kind = \"fires_task\" is reserved for legacy \
             .aida/schedules.toml entries (`aida advisor schedule add`); use \
             kind = \"substrate\" (command) or kind = \"seat\" (prompt)"
        ),
        Some(k) => k,
        None if job.command.is_some() => JobKind::Substrate,
        None if job.prompt.is_some() => JobKind::Seat,
        None => anyhow::bail!(
            "scheduled job '{name}' needs a `command` (substrate job) or a `prompt` (seat job)"
        ),
    };
    // BUG-2142: an unknown command fails only this job at tick time; it no
    // longer fails the whole registry (which stopped every substrate job).
    // trace:BUG-2142 | ai:claude
    let (command, invalid_command) = match kind {
        JobKind::Substrate => {
            let raw = job
                .command
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("substrate job '{name}' needs a `command`"))?;
            match parse_scheduled_command(raw) {
                // A tracked or machine-global job would run the reconcile on
                // every clone, including one that must never push to origin.
                // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
                Ok(c)
                    if c.display == crate::store_local::BRIDGE_COMMAND
                        && source != JobSource::Local =>
                {
                    (
                        None,
                        Some(InvalidCommand {
                            raw: raw.to_string(),
                            error: format!(
                                "scheduled job '{name}': `{}` runs only on the clone marked \
                                 as the hub bridge — remove this job and run `aida remote \
                                 bridge --enable` on that clone",
                                crate::store_local::BRIDGE_COMMAND
                            ),
                        }),
                    )
                }
                Ok(c) => (Some(c), None),
                Err(e) => (
                    None,
                    Some(InvalidCommand {
                        raw: raw.to_string(),
                        error: format!(
                            "this aida binary does not support the command of scheduled job \
                             '{name}': {e:#}"
                        ),
                    }),
                ),
            }
        }
        _ => (None, None),
    };
    if kind == JobKind::Seat && job.prompt.as_deref().unwrap_or("").trim().is_empty() {
        anyhow::bail!(
            "seat job '{name}' needs a `prompt` (what the seat should do when it is due)"
        );
    }
    let interval = job
        .interval
        .as_deref()
        .map(parse_duration)
        .transpose()
        .with_context(|| format!("invalid interval for scheduled job '{name}'"))?;
    // trace:BUG-1655 | ai:claude
    let on = job
        .on
        .iter()
        .map(|raw| EventTrigger::parse(&name, raw))
        .collect::<Result<Vec<_>>>()?;
    let when = job
        .when
        .as_deref()
        .map(|w| {
            schedule_predicate::parse(w)
                .with_context(|| format!("scheduled job '{name}': invalid `when` predicate"))
        })
        .transpose()?;
    // trace:STORY-1917 | ai:claude
    let while_ = job
        .while_
        .as_deref()
        .map(|w| {
            schedule_predicate::parse(w)
                .with_context(|| format!("scheduled job '{name}': invalid `while` predicate"))
        })
        .transpose()?;
    if while_.is_some() && interval.is_none() {
        anyhow::bail!(
            "scheduled job '{name}': `while` gates the `every` cadence, so it needs `every = \"...\"`"
        );
    }
    if interval.is_none() && job.on.is_empty() && when.is_none() {
        anyhow::bail!(
            "scheduled job '{name}' needs a schedule: `every = \"30m\"`, `on = [\"PrMerged\"]`, \
             or `when = \"mail.unread > 0\"`"
        );
    }
    let seats = if job.seats.is_empty() {
        vec!["*".to_string()]
    } else {
        job.seats
    };
    Ok(Task {
        name,
        kind,
        seats,
        command,
        prompt: job.prompt,
        interval,
        on,
        when,
        when_raw: job.when,
        while_,
        while_raw: job.while_,
        quiet_hours: job
            .quiet_hours
            .as_deref()
            .map(parse_quiet_hours)
            .transpose()?,
        enabled: job.enabled,
        source,
        problem: None,
        invalid_command,
    })
}

/// Merge the two registry layers by job name — the project layer wins on a
/// clash; `min_gap` comes from the project layer when it has one.
// trace:STORY-1226 | ai:claude
fn merge_registries(
    project: Option<LoadedScheduleConfig>,
    global: Option<LoadedScheduleConfig>,
) -> Option<LoadedScheduleConfig> {
    match (project, global) {
        (None, None) => None,
        (Some(p), None) => Some(p),
        (None, Some(g)) => Some(g),
        (Some(p), Some(g)) => {
            let names: BTreeSet<String> = p.tasks.iter().map(|t| t.name.clone()).collect();
            let mut tasks = p.tasks;
            tasks.extend(g.tasks.into_iter().filter(|t| !names.contains(&t.name)));
            Some(ScheduleConfigLoaded {
                min_gap: p.min_gap,
                tasks,
            })
        }
    }
}

pub(crate) fn global_home() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AIDA_HOME") {
        if !p.is_empty() {
            let p = PathBuf::from(p);
            // trace:BUG-1642 | ai:claude
            #[cfg(test)]
            crate::test_home::assert_hermetic(&p);
            return Some(p);
        }
    }
    crate::home_dir()
}

/// Cap on `~/.aida/schedule-tick.log`, the machine-global log the installed
/// cron entry redirects its stdout/stderr into (`>> ~/.aida/schedule-tick.log
/// 2>&1`). Half a megabyte is generous for a plain-text tick log (a tick
/// prints at most a few short lines) while bounding the worst case: a
/// misconfigured entry failing identically every 15 minutes, forever.
// trace:BUG-1600 | ai:claude
const GLOBAL_SCHEDULE_LOG_MAX_BYTES: u64 = 512 * 1024;

fn global_schedule_log_path() -> Option<PathBuf> {
    global_home().map(|h| h.join(".aida").join("schedule-tick.log"))
}

/// Truncate `~/.aida/schedule-tick.log` in place once it exceeds
/// [`GLOBAL_SCHEDULE_LOG_MAX_BYTES`], keeping its newest half (from a line
/// boundary) and dropping the rest. Best-effort: any I/O error is swallowed,
/// same as the rest of this module's logging — a scheduler tick must never
/// fail because its own housekeeping couldn't run.
///
/// Truncates the SAME inode with `File::set_len` rather than replacing the
/// path (e.g. `aida_core::write_atomic`'s temp-file-plus-rename). The
/// installed cron entry has this exact file open for append (`>>`,
/// `O_APPEND`) for the lifetime of the `aida` process this function runs
/// inside — its own stdout/stderr ARE that fd. `O_APPEND` recomputes the
/// write offset from the file's current size on every write, so truncating
/// the same inode here is safely picked up by that fd's next write. A
/// rename would instead point the path at a new inode while the inherited
/// fd kept writing into the old, now-unlinked one — this process's own
/// output would silently vanish from the path anyone else reads.
// trace:BUG-1600 | ai:claude
fn bound_global_schedule_log() {
    let Some(path) = global_schedule_log_path() else {
        return;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return;
    };
    if meta.len() <= GLOBAL_SCHEDULE_LOG_MAX_BYTES {
        return;
    }
    let Ok(body) = std::fs::read_to_string(&path) else {
        return;
    };
    let keep_bytes = (GLOBAL_SCHEDULE_LOG_MAX_BYTES / 2) as usize;
    let keep_from = body.len().saturating_sub(keep_bytes);
    let tail = match body.as_bytes()[keep_from..]
        .iter()
        .position(|&b| b == b'\n')
    {
        Some(idx) => &body[keep_from + idx + 1..],
        None => "",
    };
    let dropped = body.len() - tail.len();
    let new_body = format!(
        "[{}] --- schedule-tick.log truncated: dropped {dropped} older byte(s), cap is {GLOBAL_SCHEDULE_LOG_MAX_BYTES} bytes (a repeated tick failure may be flooding this file — see `aida schedule status` / `aida doctor`) ---\n{tail}",
        Utc::now().to_rfc3339(),
    );
    if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(&path) {
        use std::io::{Seek, SeekFrom, Write};
        if file.set_len(0).is_ok() && file.seek(SeekFrom::Start(0)).is_ok() {
            let _ = file.write_all(new_body.as_bytes());
        }
    }
}

/// The merged registry (project + global). `None` when neither layer
/// declares a `[schedule]`.
// trace:STORY-1226 | ai:claude
fn load_registry(project_root: &Path) -> Result<Option<LoadedScheduleConfig>> {
    let project = load_config(project_root)?;
    let global = match global_home() {
        Some(home) => load_global_config(&home)?,
        None => None,
    };
    let mut merged = merge_registries(project, global);
    // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
    // A malformed local layer drops only the bridge jobs, never the registry.
    let local = bridge_jobs(project_root).unwrap_or_else(|e| {
        eprintln!("warning: hub-bridge job not loaded: {e:#}");
        Vec::new()
    });
    if !local.is_empty() {
        let config = merged.get_or_insert_with(|| ScheduleConfigLoaded {
            min_gap: default_min_gap(),
            tasks: Vec::new(),
        });
        config
            .tasks
            .retain(|t| local.iter().all(|l| l.name != t.name));
        config.tasks.extend(local);
    }
    if let Some(config) = merged.as_mut() {
        disable_unknown_bindings(config);
    }
    Ok(merged)
}

/// The machine-local layer's jobs: the hub-bridge reconcile and its route,
/// only when `~/.aida/store-local.toml` marks this clone `bridge = true` and
/// `project_root` is the clone's primary checkout (a seat worktree of the
/// bridge clone never runs it). They replace a same-named registry job.
// trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
fn bridge_jobs(project_root: &Path) -> Result<Vec<Task>> {
    let Some(path) = crate::store_local::layer_path() else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let entry = crate::store_local::load_from(&path, &crate::store_local::clone_key(project_root))?;
    if !entry.bridge || !crate::store_local::is_primary_checkout(project_root) {
        return Ok(Vec::new());
    }
    bridge_tasks(entry.bridge_every.as_deref())
}

/// Build the bridge job pair. Pure, so a test can check the exact shape.
// trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
fn bridge_tasks(every: Option<&str>) -> Result<Vec<Task>> {
    use crate::store_local::{BRIDGE_COMMAND, BRIDGE_JOB, BRIDGE_ROUTE_JOB, DEFAULT_BRIDGE_EVERY};
    let every = every.unwrap_or(DEFAULT_BRIDGE_EVERY).to_string();
    let job = build_task(
        JobConfig {
            name: BRIDGE_JOB.to_string(),
            command: Some(BRIDGE_COMMAND.to_string()),
            prompt: None,
            interval: Some(every),
            on: Vec::new(),
            when: None,
            while_: None,
            quiet_hours: None,
            enabled: true,
            seats: Vec::new(),
            kind: Some(JobKind::Substrate),
        },
        JobSource::Local,
    )
    .context("invalid bridge_every in ~/.aida/store-local.toml")?;
    let route = build_task(
        JobConfig {
            name: BRIDGE_ROUTE_JOB.to_string(),
            command: None,
            prompt: Some(format!(
                "The hub-bridge reconcile (`aida {BRIDGE_COMMAND}`) stopped on this clone. Read the \
                 routed trip evidence: identity-bearing content waits for your consent \
                 (`aida remote reconcile --execute --yes` only after `aida store scrub-preview` \
                 shows it is safe to publish); a conflict the union cannot resolve needs the \
                 manual path. Never force-push a shared branch. Then `aida schedule done {BRIDGE_ROUTE_JOB}`."
            )),
            interval: None,
            on: vec![format!("CronJobFailed:{BRIDGE_JOB}")],
            when: None,
            while_: None,
            quiet_hours: None,
            enabled: true,
            seats: vec!["advisor".to_string()],
            kind: Some(JobKind::Seat),
        },
        JobSource::Local,
    )?;
    Ok(vec![job, route])
}

/// A `CronJobFailed:<job>` binding must name a job in the merged registry
/// (enabled or not). A route whose binding names no such job (a typo, or a
/// guard removed) is loaded DISABLED with a `problem`, rather than failing
/// the whole registry: one bad route must not stop the watchdog or the other
/// guards. Malformed syntax is still a hard load error (see `EventTrigger`).
// trace:BUG-1655 | ai:claude
fn disable_unknown_bindings(config: &mut LoadedScheduleConfig) {
    let names: BTreeSet<String> = config.tasks.iter().map(|t| t.name.clone()).collect();
    for task in &mut config.tasks {
        // trace:TASK-1535 | ai:codex
        if !task.enabled {
            continue;
        }
        let missing: Vec<String> = task
            .on
            .iter()
            .filter(|t| t.job.as_ref().is_some_and(|j| !names.contains(j)))
            .map(ToString::to_string)
            .collect();
        if missing.is_empty() {
            continue;
        }
        task.enabled = false;
        task.problem = Some(format!(
            "`on` names a job that is not in the schedule registry ({}); \
             fix the job name after the colon — this job will not run until then",
            missing.join(", ")
        ));
    }
}

fn store_root(project_root: &Path) -> PathBuf {
    project_root.join(".aida-store")
}

// ---------------------------------------------------------------------------
// tick / run
// ---------------------------------------------------------------------------

/// What one `aida schedule tick` did: its report lines, and the enabled
/// substrate jobs whose command this binary does not support (BUG-2142;
/// always empty for a hook tick, which never runs them).
// trace:BUG-2142 | ai:claude
struct TickRun {
    lines: Vec<String>,
    unsupported: Vec<String>,
}

#[cfg(test)]
fn tick(
    project_root: &Path,
    hook: bool,
    backend: Option<&aida_core::CachedGitBackend>,
) -> Result<Vec<String>> {
    run_tick(project_root, hook, backend).map(|run| run.lines)
}

fn run_tick(
    project_root: &Path,
    hook: bool,
    backend: Option<&aida_core::CachedGitBackend>,
) -> Result<TickRun> {
    // Hooks from two sessions can arrive together. Only one may inspect and
    // advance schedule cursors at a time; a hook never waits on its peer.
    // trace:TASK-1281 | ai:codex
    let Some(_tick_lock) = try_tick_lock(project_root)? else {
        return Ok(TickRun {
            lines: vec!["schedule tick: another tick is already running".to_string()],
            unsupported: Vec::new(),
        });
    };
    // BUG-1600: cap the machine-global `~/.aida/schedule-tick.log` before
    // this process's own stdout/stderr add to it. That file is shared by
    // every project's installed cron entry, appended via shell `>>`, and
    // this codebase's only periodic (non-hook) driver — a persistently
    // failing entry (the `--format json` bug this fix removes, or any
    // future misconfiguration) would otherwise flood it forever. Skipped
    // for hook ticks: the hook redirects its own output to `/dev/null` and
    // is meant to stay minimal/network-free.
    // trace:BUG-1600 | ai:claude
    if !hook {
        bound_global_schedule_log();
    }
    // BUG-1291: the full scheduler tick owns the bounded orphan-review
    // backstop. Hook ticks stay network-free; explicit/timer ticks inspect the
    // bounded forge list even when no user schedule registry exists.
    let Some(config) = load_registry(project_root)? else {
        return Ok(TickRun {
            lines: if hook {
                Vec::new()
            } else {
                crate::sweep_orphaned_reviews(project_root)
            },
            unsupported: Vec::new(),
        });
    };
    let unsupported = if hook {
        Vec::new()
    } else {
        unsupported_substrate_jobs(&config)
    };
    let mut state = load_state(project_root)?;
    let now = Utc::now();
    if let Some(last_tick) = state.last_tick_at {
        let min_gap = parse_duration(&config.min_gap)
            .with_context(|| format!("invalid [schedule] min_gap '{}'", config.min_gap))?;
        if now.signed_duration_since(last_tick) < min_gap {
            return Ok(TickRun {
                lines: vec![format!(
                    "schedule tick: suppressed by min-gap {}",
                    config.min_gap
                )],
                unsupported,
            });
        }
    }
    let mut recovery_lines = if hook {
        Vec::new()
    } else {
        crate::sweep_orphaned_reviews(project_root)
    };
    let needs_events = config.tasks.iter().any(|t| t.enabled && !t.on.is_empty());
    let events = if needs_events {
        events::read_all(project_root)
    } else {
        Vec::new()
    };
    recovery_lines.extend(tick_core(
        project_root,
        config,
        &mut state,
        now,
        hook,
        run_aida_command,
        |fields| collect_snapshot(project_root, fields, backend),
        &events,
    )?);
    Ok(TickRun {
        lines: recovery_lines,
        unsupported,
    })
}

fn run_now(project_root: &Path, only: Option<&str>) -> Result<Vec<String>> {
    let Some(config) = load_registry(project_root)? else {
        return Ok(vec![]);
    };
    if let Some(name) = only {
        if !config.tasks.iter().any(|task| task.name == name) {
            anyhow::bail!("no scheduled job named '{name}'");
        }
    }
    let mut state = load_state(project_root)?;
    let now = Utc::now();
    run_with_executor(
        project_root,
        config,
        &mut state,
        now,
        only,
        run_aida_command,
    )
}

/// Test seam kept from STORY-1047: a tick with a fixed clock and executor,
/// no events and an empty snapshot.
#[cfg(test)]
fn tick_with_executor<F>(
    project_root: &Path,
    config: LoadedScheduleConfig,
    state: &mut ScheduleState,
    now: DateTime<Utc>,
    hook: bool,
    exec: F,
) -> Result<Vec<String>>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
{
    tick_core(
        project_root,
        config,
        state,
        now,
        hook,
        exec,
        |_| Snapshot::default(),
        &[],
    )
}

/// STORY-1917 R5 test seam: one tick over the project's real registry with
/// the real snapshot collector, at a fixed clock, where each substrate job
/// runs in-process through `run` (given the job's command) instead of a
/// child `aida`.
#[cfg(test)]
pub(crate) fn tick_project_registry_for_test(
    project_root: &Path,
    now: DateTime<Utc>,
    run: &dyn Fn(&str) -> Result<i32>,
) -> Result<Vec<String>> {
    let config = load_registry(project_root)?.context("no schedule registry")?;
    let mut state = load_state(project_root)?;
    tick_core(
        project_root,
        config,
        &mut state,
        now,
        false,
        |_, command| {
            Ok(TaskOutcome {
                status: run(command.display)?,
                stdout: String::new(),
                stderr: String::new(),
            })
        },
        |fields| collect_snapshot(project_root, fields, None),
        &[],
    )
}

/// The tick: evaluate every enabled job's triggers (`every`, `on`, `when`),
/// run substrate jobs through `exec`, and mark seat jobs due — never running
/// them. Pure over its inputs except for the ledger + local-state writes.
// trace:STORY-1226 | ai:claude
#[allow(clippy::too_many_arguments)]
fn tick_core<F, S>(
    project_root: &Path,
    config: LoadedScheduleConfig,
    state: &mut ScheduleState,
    now: DateTime<Utc>,
    hook: bool,
    exec: F,
    snapshot: S,
    events: &[Event],
) -> Result<Vec<String>>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
    S: Fn(&BTreeSet<Field>) -> Snapshot,
{
    let store = store_root(project_root);
    let ledgers = schedule_ledger::load_all(&store);
    let mut staged_ledgers = ledgers.clone();
    let mut touched = false;
    let mut lines = Vec::new();

    // One snapshot for every `when` job, sized to the fields they reference.
    let wanted: BTreeSet<Field> = config
        .tasks
        .iter()
        .filter(|t| t.enabled)
        .flat_map(|t| t.when.iter().chain(t.while_.iter()))
        .flat_map(|e| e.fields())
        .collect();
    let snap = if wanted.is_empty() {
        None
    } else {
        Some(snapshot(&wanted))
    };

    for task in &config.tasks {
        if !task.enabled || task.kind == JobKind::FiresTask {
            // trace:TASK-1535 | ai:codex
            if let Some(problem) = &task.problem {
                lines.push(format!("schedule tick: {} skipped: {problem}", task.name));
            }
            continue;
        }
        // A hook tick runs only hook-allowed substrate commands; an
        // unsupported command (BUG-2142) is left to the full tick.
        // trace:BUG-2142 | ai:claude
        if hook
            && task.kind == JobKind::Substrate
            && !task.command.as_ref().is_some_and(|c| c.hook_allowed)
        {
            continue;
        }
        if quiet_now(task) {
            continue;
        }
        let ledger = ledgers.get(&task.name);
        let local = state.tasks.get(&task.name);
        let last_run = effective_last_run(ledger, local);
        let mut triggers: Vec<Trigger> = Vec::new();

        if let Some(interval) = task.interval {
            // A `while` gate holds the cadence back while it is false; an
            // unevaluable gate is false (and reported), never a free run.
            // trace:STORY-1917 | ai:claude
            let gate_open = match (&task.while_, snap.as_ref()) {
                (None, _) => true,
                (Some(expr), Some(snap)) => match schedule_predicate::eval(expr, snap) {
                    Ok(v) => v,
                    Err(e) => {
                        lines.push(format!(
                            "schedule tick: {} `while` predicate error: {e}",
                            task.name
                        ));
                        false
                    }
                },
                (Some(_), None) => false,
            };
            if gate_open && interval_due(last_run, interval, now) {
                triggers.push(Trigger::Every(interval));
            }
        }

        let mut continue_on = false;
        if !task.on.is_empty() {
            let cursor = local.and_then(|s| s.last_seen_event_ts).or(last_run);
            // Advisor review round 1 (#1946): a job seen for the first time
            // (no cursor, no last_run) must not replay every historical
            // matching event — initialise the cursor to now and fire only on
            // events after this tick. trace:STORY-1226 | ai:claude
            if cursor.is_none() {
                state
                    .tasks
                    .entry(task.name.clone())
                    .or_default()
                    .last_seen_event_ts = Some(now);
                touched = true;
                continue_on = true;
            }
            let mut newest: Option<DateTime<Utc>> = None;
            let mut kind_hit: Option<String> = None;
            let mut due_failure: Option<schedule_ledger::RoutedFailure> = None;
            let scan: &[Event] = if continue_on { &[] } else { events };
            for ev in scan {
                // trace:BUG-1655 | ai:claude
                let Some(trigger) = task.on.iter().find(|t| t.matches(&ev.kind)) else {
                    continue;
                };
                if cursor.is_some_and(|c| ev.ts <= c) {
                    continue;
                }
                if newest.is_none_or(|n| ev.ts > n) {
                    newest = Some(ev.ts);
                    kind_hit = Some(trigger.to_string());
                    due_failure = match &ev.kind {
                        EventKind::CronJobFailed {
                            job,
                            error,
                            trip_id: Some(trip_id),
                            performance,
                            ..
                        } => Some(schedule_ledger::RoutedFailure {
                            trip_id: trip_id.clone(),
                            performance: performance
                                .iter()
                                .take(schedule_ledger::MAX_PERFORMANCE_AUDITS)
                                .cloned()
                                .collect(),
                            // trace:BUG-1655 | ai:claude
                            job: Some(job.clone()),
                            error: Some(error.clone()).filter(|e| !e.is_empty()),
                        }),
                        _ => None,
                    };
                }
            }
            if let (Some(ts), Some(kind)) = (newest, kind_hit) {
                state
                    .tasks
                    .entry(task.name.clone())
                    .or_default()
                    .last_seen_event_ts = Some(ts);
                touched = true;
                triggers.push(Trigger::Event(kind));
                // The matched event is retained until `schedule done`; this is
                // the actual pickup artifact, not a pointer to mutable latest.
                staged_ledgers
                    .entry(task.name.clone())
                    .or_insert_with(|| JobLedger::new(&task.name))
                    .due_failure = due_failure;
            }
        }

        if let (Some(expr), Some(snap)) = (&task.when, snap.as_ref()) {
            let is_true = match schedule_predicate::eval(expr, snap) {
                Ok(v) => v,
                Err(e) => {
                    lines.push(format!("schedule tick: {} predicate error: {e}", task.name));
                    false
                }
            };
            let staged = staged_ledgers
                .entry(task.name.clone())
                .or_insert_with(|| JobLedger::new(&task.name));
            let transition = schedule_ledger::apply_condition(staged, is_true, now);
            if transition == EpisodeTransition::Fired {
                triggers.push(Trigger::Condition);
            }
        }

        if triggers.is_empty() {
            continue;
        }
        let reason = triggers
            .iter()
            .map(Trigger::label)
            .collect::<Vec<_>>()
            .join(" + ");

        match task.kind {
            JobKind::Substrate => {
                let Some(outcome) = run_substrate_task(project_root, task, &exec) else {
                    continue;
                };
                touched = true;
                let trip = failure_trip(task, now, &outcome);
                record_outcome_local(project_root, state, task, now, &outcome, trip.as_ref());
                apply_outcome_ledger(
                    staged_ledgers
                        .entry(task.name.clone())
                        .or_insert_with(|| JobLedger::new(&task.name)),
                    now,
                    &outcome,
                    trip.as_ref(),
                );
                lines.push(format!(
                    "schedule tick: {} {}",
                    task.name,
                    if outcome.status == 0 { "ok" } else { "failed" }
                ));
            }
            JobKind::Seat => {
                // Deliver, never execute: mark due once per episode; the seat
                // clears it with `aida schedule done`.
                let already_due = ledger.is_some_and(|l| l.due_since.is_some());
                if already_due {
                    continue;
                }
                let seat_label = task.seats.join(",");
                let staged = staged_ledgers
                    .entry(task.name.clone())
                    .or_insert_with(|| JobLedger::new(&task.name));
                staged.due_since = Some(now);
                staged.due_reason = Some(reason.clone());
                events::emit(
                    project_root,
                    &Event::new(
                        None,
                        "",
                        EventKind::CronJobFired {
                            job: task.name.clone(),
                            seat: seat_label.clone(),
                        },
                    ),
                );
                touched = true;
                lines.push(format!(
                    "schedule tick: {} due ({reason}) → seat {seat_label}",
                    task.name
                ));
            }
            JobKind::FiresTask => {}
        }
    }
    if let Err(err) =
        schedule_ledger::write_batch_cas_opts(&store, &ledgers, &staged_ledgers, !hook)
    {
        eprintln!(
            "warning: schedule ledgers were not written: {err}; local run state was retained"
        );
    }
    if touched {
        state.last_tick_at = Some(now);
        save_state(project_root, state)?;
    }
    Ok(lines)
}

/// Exit status recorded for a substrate job whose command this binary does
/// not support (the shell's "command not found").
// trace:BUG-2142 | ai:claude
const UNSUPPORTED_COMMAND_STATUS: i32 = 127;

/// Run one substrate job through `exec`. A job whose command this binary
/// does not support fails as itself (status 127, the reason on stderr), so
/// the caller ledgers it and fires its `CronJobFailed` route like any other
/// failure. `None` only for a substrate job with neither (never built).
// trace:BUG-2142 | ai:claude
fn run_substrate_task<F>(project_root: &Path, task: &Task, exec: &F) -> Option<TaskOutcome>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
{
    if let Some(bad) = &task.invalid_command {
        return Some(TaskOutcome {
            status: UNSUPPORTED_COMMAND_STATUS,
            stdout: String::new(),
            stderr: bad.error.clone(),
        });
    }
    let command = task.command.as_ref()?;
    Some(exec(project_root, command).unwrap_or_else(|e| TaskOutcome {
        status: 1,
        stdout: String::new(),
        stderr: e.to_string(),
    }))
}

/// Enabled substrate jobs in `project_root`'s registry whose command this
/// binary does not support, as `name (aida <raw>)`. A full tick that finds
/// any exits nonzero naming them, after running every other due job.
// trace:BUG-2142 | ai:claude
fn unsupported_substrate_jobs(config: &LoadedScheduleConfig) -> Vec<String> {
    config
        .tasks
        .iter()
        .filter(|t| t.enabled && t.kind == JobKind::Substrate)
        .filter_map(|t| {
            t.invalid_command
                .as_ref()
                .map(|bad| format!("{} (`aida {}`)", t.name, bad.raw.trim()))
        })
        .collect()
}

/// `aida schedule run [<job>]`: force a substrate job now (all enabled ones
/// without a name); for a seat job, print its prompt + the report-back line —
/// the scheduler still does not act on the seat's behalf.
// trace:STORY-1226 | ai:claude
fn run_with_executor<F>(
    project_root: &Path,
    config: LoadedScheduleConfig,
    state: &mut ScheduleState,
    now: DateTime<Utc>,
    only: Option<&str>,
    exec: F,
) -> Result<Vec<String>>
where
    F: Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome>,
{
    let mut ran_any = false;
    let mut lines = Vec::new();
    for task in &config.tasks {
        if task.kind == JobKind::FiresTask {
            continue;
        }
        if only.is_some_and(|name| task.name != name) {
            continue;
        }
        if only.is_none() && (!task.enabled || task.kind != JobKind::Substrate) {
            continue;
        }
        match task.kind {
            JobKind::Substrate => {
                let Some(outcome) = run_substrate_task(project_root, task, &exec) else {
                    continue;
                };
                ran_any = true;
                let trip = failure_trip(task, now, &outcome);
                record_outcome_local(project_root, state, task, now, &outcome, trip.as_ref());
                if let Err(err) =
                    schedule_ledger::write_cas(&store_root(project_root), &task.name, |ledger| {
                        apply_outcome_ledger(ledger, now, &outcome, trip.as_ref());
                    })
                {
                    eprintln!(
                        "warning: schedule ledger for '{}' was not written: {err}; local run state was retained",
                        task.name
                    );
                }
                lines.push(format!(
                    "schedule run: {} {}",
                    task.name,
                    if outcome.status == 0 { "ok" } else { "failed" }
                ));
            }
            JobKind::Seat => {
                lines.push(format!(
                    "schedule run: {} is a seat job for {} — not executed by the scheduler.\n\
                     Do this now:\n  {}\nThen report it: `aida schedule done {}`",
                    task.name,
                    task.seats.join(","),
                    task.prompt.as_deref().unwrap_or(""),
                    task.name
                ));
            }
            JobKind::FiresTask => unreachable!("legacy tasks are excluded by load_registry"),
        }
    }
    if ran_any {
        state.last_tick_at = Some(now);
        save_state(project_root, state)?;
    }
    Ok(lines)
}

/// `aida schedule done <job>`: the seat reports a run. Ledgers who/when,
/// clears the due flag, and mirrors the clock locally.
// trace:STORY-1226 | ai:claude
pub(crate) fn done(project_root: &Path, job: &str, note: Option<&str>) -> Result<()> {
    let seat = resolve_done_seat(
        project_root,
        crate::queue_role_fallback::session_role_env(),
        std::env::current_dir().ok().as_deref(),
    );
    done_as(
        project_root,
        job,
        note,
        seat.as_ref().map(|s| s.as_deref()).map_err(String::as_str),
        crate::autopilot::current_headless(),
    )
}

/// The reporting session's seat, resolved exactly as the awaiting notice
/// resolves it ([`crate::session_seat`]: env, else the lease covering cwd).
/// `Err` when the seat cannot be determined — no cwd, or an unreadable lease
/// directory — so the operator-escalation gate fails closed.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
fn resolve_done_seat(
    project_root: &Path,
    env_role: Option<String>,
    cwd: Option<&Path>,
) -> std::result::Result<Option<String>, String> {
    let leases = if env_role.is_some() {
        Vec::new()
    } else {
        let Some(_) = cwd else {
            return Err("the working directory is unknown".to_string());
        };
        crate::leases::list_leases_strict(project_root, &[])?
    };
    let (_, role) = crate::session_seat(env_role, &leases, cwd);
    Ok(role.map(|r| crate::canonical_role_name(&r)))
}

/// [`done`] with the caller's seat and headless flag explicit. An operator
/// escalation may only be cleared from a session a human reads; `seat` is
/// `Err` when it could not be resolved, which refuses an operator job.
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
fn done_as(
    project_root: &Path,
    job: &str,
    note: Option<&str>,
    seat: std::result::Result<Option<&str>, &str>,
    headless: bool,
) -> Result<()> {
    // The mandate-renewal job is synthesized from the live mandate, not the
    // registry, and clears itself once the mandate is renewed; reporting it
    // is an acknowledgement with nothing to ledger.
    // trace:TASK-2805 | ai:claude
    if job == MANDATE_RENEWAL_JOB {
        println!("✓ {job} acknowledged — it clears once the operator runs `aida mandate renew`");
        return Ok(());
    }
    let config = load_registry(project_root)?;
    let store = store_root(project_root);
    let task = config
        .as_ref()
        .and_then(|c| c.tasks.iter().find(|t| t.name == job))
        .cloned()
        .or_else(|| builtin_routed_seat_task(&store, job));
    let Some(found) = task.as_ref() else {
        anyhow::bail!("no scheduled job named '{job}' (see `aida schedule list`)");
    };
    if crate::mail_backlog::is_operator_job(&found.seats) {
        let seat = match seat {
            Ok(seat) => seat,
            Err(why) => anyhow::bail!(
                "'{job}' is an operator escalation, and this session's seat could not be \
                 resolved ({why}); run it from the operator terminal"
            ),
        };
        if !crate::mail_backlog::operator_escalation_visible(seat, headless) {
            anyhow::bail!(
                "'{job}' is an operator escalation for the human at the terminal; \
             only the operator terminal or an interactive advisor session can mark it done"
            );
        }
    }
    let now = Utc::now();
    let by = reporter(task.as_ref());
    schedule_ledger::write_cas(&store, job, |l| {
        l.last_run = Some(now);
        l.last_by = Some(by.clone());
        l.result = Some("done".to_string());
        l.note = note.map(str::to_string);
        l.due_since = None;
        l.due_reason = None;
        l.due_failure = None;
    })?;
    let mut state = load_state(project_root)?;
    let entry = state.tasks.entry(job.to_string()).or_default();
    entry.last_run_at = Some(now);
    entry.last_success_at = Some(now);
    entry.last_status = Some(0);
    save_state(project_root, &state)?;
    let next = task
        .as_ref()
        .and_then(|t| t.interval)
        .map(|i| format!(" — next due in {}", format_duration(i)))
        .unwrap_or_default();
    println!("✓ {job} done by {}{next}", by.seat);
    Ok(())
}

/// Who is reporting: the session role (or the substrate for a tick), the
/// session id when one is exported, and the vendor.
fn reporter(task: Option<&Task>) -> LastBy {
    let seat = std::env::var("AIDA_SESSION_ROLE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| crate::canonical_role_name(&s))
        .or_else(|| task.and_then(|t| t.seats.iter().find(|s| *s != "*").cloned()))
        .unwrap_or_else(|| crate::current_user_id(None));
    let session = std::env::var("AIDA_SESSION_ID")
        .ok()
        .or_else(|| std::env::var("CLAUDE_CODE_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty());
    let vendor = match crate::agent_registry::detect_agent_type().as_str() {
        "other" | "" => None,
        v => Some(v.to_string()),
    };
    LastBy {
        seat,
        session,
        vendor,
    }
}

// ---------------------------------------------------------------------------
// Read surfaces: status / list / due / emit-cron
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct StatusRow {
    name: String,
    kind: &'static str,
    seats: Vec<String>,
    schedule: String,
    interval: String,
    command: String,
    enabled: bool,
    status: String,
    last_run: String,
    last_by: Option<String>,
    next_due: String,
    due_reason: Option<String>,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<String>,
    /// Job-specific outcome of the last run (the docs sweep's findings count
    /// and mode). trace:STORY-2220 | ai:claude
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

fn effective_last_run(
    ledger: Option<&JobLedger>,
    local: Option<&TaskState>,
) -> Option<DateTime<Utc>> {
    match (
        ledger.and_then(|l| l.last_run),
        local.and_then(|s| s.last_run_at),
    ) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn interval_due(last_run: Option<DateTime<Utc>>, interval: Duration, now: DateTime<Utc>) -> bool {
    match last_run {
        None => true,
        Some(last) => now.signed_duration_since(last) >= interval,
    }
}

/// Is a job due at `now`, and why? Pure over the ledger + local mirror:
/// `every` is derived from the last run; `on` / `when` show as due once the
/// tick flagged them (`due_since`) until the seat reports done.
// trace:STORY-1226 | ai:claude
fn due_reason(
    task: &Task,
    ledger: Option<&JobLedger>,
    local: Option<&TaskState>,
    now: DateTime<Utc>,
) -> Option<String> {
    if !task.enabled {
        return None;
    }
    if let Some(reason) = ledger.and_then(|l| l.due_since.map(|_| l.due_reason.clone())) {
        return Some(reason.unwrap_or_else(|| "due".to_string()));
    }
    let interval = task.interval?;
    interval_due(effective_last_run(ledger, local), interval, now)
        .then(|| Trigger::Every(interval).label())
}

fn status_row(
    task: &Task,
    ledger: Option<&JobLedger>,
    local: Option<&TaskState>,
    now: DateTime<Utc>,
) -> StatusRow {
    let last_run_at = effective_last_run(ledger, local);
    let next_due = match (last_run_at, task.interval) {
        (Some(last), Some(i)) => Some(last + i),
        _ => None,
    };
    let reason = due_reason(task, ledger, local, now);
    // trace:BUG-2142 | ai:claude
    let problem = task.problem.clone().or_else(|| {
        task.invalid_command.as_ref().map(|bad| {
            format!(
                "{}; each due tick records it as failed until the scheduler runs an aida that \
                 supports it",
                bad.error
            )
        })
    });
    let status = if task.problem.is_some() {
        // trace:BUG-1655 | ai:claude
        "invalid"
    } else if !task.enabled {
        "disabled"
    } else if task.invalid_command.is_some() {
        "invalid"
    } else if task.kind == JobKind::FiresTask {
        "legacy"
    } else if quiet_now(task) {
        "quiet"
    } else if reason.is_some() {
        "due"
    } else {
        "pending"
    };
    StatusRow {
        name: task.name.clone(),
        kind: task.kind.as_str(),
        seats: task.seats.clone(),
        schedule: task.schedule_summary(),
        interval: task
            .interval
            .map(format_duration)
            .unwrap_or_else(|| "-".to_string()),
        command: task.what(),
        enabled: task.enabled,
        status: status.to_string(),
        last_run: last_run_at
            .map(|t| t.to_rfc3339())
            .unwrap_or_else(|| "(never)".to_string()),
        last_by: ledger.and_then(|l| l.last_by.as_ref()).map(last_by_label),
        next_due: next_due.map(|t| t.to_rfc3339()).unwrap_or_else(|| {
            if task.interval.is_some() {
                "now".to_string()
            } else {
                "-".to_string()
            }
        }),
        due_reason: reason,
        source: task.source.as_str(),
        problem,
        detail: None,
    }
}

/// Attach a job's own last-run outcome to its status row.
// trace:STORY-2220 | ai:claude
fn with_detail(mut row: StatusRow, project_root: &Path) -> StatusRow {
    if row.command == "aida docs sweep" {
        row.detail = crate::docs_sweep::status_detail(project_root);
    }
    row
}

/// The `status` / `list` rows: each task's schedule state plus its own
/// last-run detail.
// trace:STORY-2220 | ai:claude
fn status_rows(project_root: &Path, tasks: &[Task], now: DateTime<Utc>) -> Result<Vec<StatusRow>> {
    let state = load_state(project_root)?;
    let ledgers = schedule_ledger::load_all(&store_root(project_root));
    Ok(tasks
        .iter()
        .map(|t| {
            with_detail(
                status_row(t, ledgers.get(&t.name), state.tasks.get(&t.name), now),
                project_root,
            )
        })
        .collect())
}

/// One `<job>: <detail>` paragraph per row that carries a detail.
// trace:STORY-2220 | ai:claude
fn detail_lines(rows: &[StatusRow]) -> String {
    rows.iter()
        .filter_map(|row| Some(format!("\n{}: {}\n", row.name, row.detail.as_ref()?)))
        .collect()
}

fn last_by_label(by: &LastBy) -> String {
    match &by.vendor {
        Some(v) => format!("{}/{v}", by.seat),
        None => by.seat.clone(),
    }
}

/// Registry + the deprecated STORY-262 cadence entries, for `list`/`status`.
fn all_tasks(project_root: &Path) -> Result<Vec<Task>> {
    let mut tasks = load_registry(project_root)?
        .map(|c| c.tasks)
        .unwrap_or_default();
    tasks.extend(legacy_tasks(project_root));
    Ok(tasks)
}

/// STORY-262 `.aida/schedules.toml` entries folded in as kind `fires_task`.
// trace:STORY-1226 | ai:claude
fn legacy_tasks(project_root: &Path) -> Vec<Task> {
    crate::schedule::load(project_root)
        .schedules
        .into_iter()
        .map(|s| Task {
            name: s.name,
            kind: JobKind::FiresTask,
            seats: vec![s.for_role],
            command: None,
            prompt: Some(format!("files TASK \"{}\" on `aida pull`", s.title)),
            interval: crate::schedule::parse_cadence(&s.cadence).ok(),
            on: Vec::new(),
            when: None,
            when_raw: None,
            while_: None,
            while_raw: None,
            quiet_hours: None,
            enabled: s.enabled,
            source: JobSource::Legacy,
            problem: None,
            invalid_command: None,
        })
        .collect()
}

fn status(project_root: &Path, json: bool) -> Result<()> {
    let tasks = all_tasks(project_root)?;
    if tasks.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No scheduled jobs configured.");
        }
        return Ok(());
    }
    let rows = status_rows(project_root, &tasks, Utc::now())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<22} {:<10} {:<12} {:<26} {:<10} {:<22} {:<22} What",
        "Name", "Kind", "Seats", "Schedule", "Status", "Last run", "Next due"
    );
    for row in &rows {
        println!(
            "{:<22} {:<10} {:<12} {:<26} {:<10} {:<22} {:<22} {}",
            row.name,
            row.kind,
            row.seats.join(","),
            row.schedule,
            row.status,
            row.last_run,
            row.next_due,
            row.command
        );
    }
    print!("{}", detail_lines(&rows)); // trace:STORY-2220 | ai:claude
                                       // trace:BUG-1655 | ai:claude
    for row in rows.iter().filter(|r| r.problem.is_some()) {
        println!(
            "\nwarning: {} is not running: {}",
            row.name,
            row.problem.as_deref().unwrap_or_default()
        );
    }
    if rows.iter().any(|r| r.kind == "fires_task") {
        println!(
            "\nnote: `fires_task` rows come from .aida/schedules.toml (deprecated); \
             `aida pull` still fires them. Move them to a seat job in [schedule]."
        );
    }
    Ok(())
}

/// `aida schedule list [--seat <role>] [--json]`: the registry as configured,
/// with source layer, kind, and the last reporter.
// trace:STORY-1226 | ai:claude
fn list(project_root: &Path, seat: Option<&str>, json: bool) -> Result<()> {
    // trace:BUG-1289 | ai:claude
    let json = json || crate::output_format_is_json();
    let tasks: Vec<Task> = all_tasks(project_root)?
        .into_iter()
        .filter(|t| seat.is_none_or(|s| t.applies_to_seat(s)))
        .collect();
    if tasks.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No scheduled jobs configured. Add `[[schedule.jobs]]` entries to .aida/config.toml (project) or ~/.aida/schedule.toml (global).");
        }
        return Ok(());
    }
    let rows = status_rows(project_root, &tasks, Utc::now())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<22} {:<10} {:<12} {:<26} {:<8} {:<10} {:<18} {:<8} What",
        "Name", "Kind", "Seats", "Schedule", "Enabled", "Status", "Last by", "Source"
    );
    for row in &rows {
        println!(
            "{:<22} {:<10} {:<12} {:<26} {:<8} {:<10} {:<18} {:<8} {}",
            row.name,
            row.kind,
            row.seats.join(","),
            row.schedule,
            if row.enabled { "yes" } else { "no" },
            row.status,
            row.last_by.as_deref().unwrap_or("-"),
            row.source,
            row.command
        );
    }
    print!("{}", detail_lines(&rows)); // trace:STORY-2220 | ai:claude
    if rows.iter().any(|r| r.kind == "fires_task") {
        println!(
            "\nnote: `fires_task` rows come from .aida/schedules.toml (deprecated); \
             `aida pull` still fires them. Move them to a seat job in [schedule]."
        );
    }
    Ok(())
}

/// The `lane-liveness-<seat>` seat jobs the lane-liveness substrate job
/// routes findings to, built from its single prompt table so they appear in
/// `due` / `done` with no config entry.
// trace:STORY-1688 | ai:claude
fn builtin_routed_seat_task(store: &Path, name: &str) -> Option<Task> {
    // STORY-HOME-128: `mail-backlog-guard-route-<seat>` routes to whatever
    // seat the overdue mail was addressed to — only a route the guard raised
    // (TASK-134). trace:STORY-HOME-128 | ai:claude
    // trace:TASK-B6B1278890774EF993D2EBC327862470-134 | ai:claude
    let (seat, prompt) = match crate::lane_liveness::route_for_job(name) {
        Some(route) => (route.seat.to_string(), route.prompt.to_string()),
        None => {
            let seat = crate::mail_backlog::known_route_seat(store, name)?;
            (seat.to_string(), crate::mail_backlog::route_prompt(seat))
        }
    };
    Some(Task {
        name: name.to_string(),
        kind: JobKind::Seat,
        seats: vec![seat],
        command: None,
        prompt: Some(prompt),
        interval: None,
        on: vec![EventTrigger {
            kind: "CronJobFired".to_string(),
            job: Some(name.to_string()),
        }],
        when: None,
        when_raw: None,
        while_: None,
        while_raw: None,
        quiet_hours: None,
        enabled: true,
        source: JobSource::Project,
        problem: None,
        invalid_command: None,
    })
}

/// Due jobs, file-only (config parse + ledger/state reads; no git, no
/// network). Seat jobs are filtered to `seat` when given; substrate jobs are
/// included only when no seat filter is given (`include_substrate`).
// trace:STORY-1226 | ai:claude
fn scheduled_due(project_root: &Path, seat: Option<&str>, include_substrate: bool) -> Vec<DueJob> {
    let Ok(config) = load_registry(project_root) else {
        return Vec::new();
    };
    let store = store_root(project_root);
    let ledgers = schedule_ledger::load_all(&store);
    let mut tasks = config.map(|c| c.tasks).unwrap_or_default();
    // trace:STORY-1688 | ai:claude
    // trace:STORY-HOME-128 | ai:claude
    let builtin_names = crate::lane_liveness::ROUTES
        .iter()
        .map(|route| crate::lane_liveness::route_job_name(route.seat))
        .chain(
            ledgers
                .keys()
                .filter(|name| crate::mail_backlog::seat_for_job(name).is_some())
                .cloned(),
        )
        .collect::<Vec<_>>();
    for builtin_name in builtin_names {
        if ledgers
            .get(&builtin_name)
            .and_then(|l| l.due_since)
            .is_some()
            && !tasks.iter().any(|t| t.name == builtin_name)
        {
            if let Some(task) = builtin_routed_seat_task(&store, &builtin_name) {
                tasks.push(task);
            }
        }
    }
    if tasks.is_empty() {
        return Vec::new();
    }
    let state = load_state(project_root).unwrap_or_default();
    let now = Utc::now();
    let mut out = Vec::new();
    for task in &tasks {
        match task.kind {
            JobKind::Seat => {
                if seat.is_some_and(|s| !task.applies_to_seat(s)) {
                    continue;
                }
            }
            JobKind::Substrate => {
                if !include_substrate {
                    continue;
                }
            }
            JobKind::FiresTask => continue,
        }
        if quiet_now(task) {
            continue;
        }
        let ledger = ledgers.get(&task.name);
        let local = state.tasks.get(&task.name);
        let Some(reason) = due_reason(task, ledger, local, now) else {
            continue;
        };
        out.push(DueJob {
            name: task.name.clone(),
            kind: task.kind,
            seats: task.seats.clone(),
            schedule: task.schedule_summary(),
            reason,
            prompt: task.prompt.clone(),
            command: task.command.as_ref().map(|c| format!("aida {}", c.display)),
            last_run: effective_last_run(ledger, local),
            last_by: ledger.and_then(|l| l.last_by.as_ref()).map(last_by_label),
            due_since: ledger.and_then(|l| l.due_since),
            failure: ledger.and_then(|l| l.due_failure.clone()),
        });
    }
    out
}

/// Due SEAT jobs for `seat` (every seat when `None`). The one read every
/// delivery surface shares: awaiting `--notice`, the pickup prompt, the
/// launch context. File-only and fail-open (no registry → empty).
// trace:STORY-1226 | ai:claude
pub(crate) fn due_seat_jobs(project_root: &Path, seat: Option<&str>) -> Vec<DueJob> {
    collect_due(project_root, seat, false)
}

// trace:STORY-2346 | ai:codex
fn collect_due(project_root: &Path, seat: Option<&str>, include_substrate: bool) -> Vec<DueJob> {
    let mut due = scheduled_due(project_root, seat, include_substrate);
    // trace:STORY-2346 | ai:codex
    let now = Utc::now();
    let state = crate::gate::mandate::current_state();
    if let Some(job) = mandate_renewal_job(&state, now, seat) {
        if let Ok(dir) = crate::gate::mandate::mandate_dir() {
            crate::mandate::notify_expiry(&dir, project_root, &state, now);
        }
        due.push(job);
    }
    due
}

/// Name of the synthetic due job raised before the mandate expires.
// trace:TASK-2805 | ai:claude
const MANDATE_RENEWAL_JOB: &str = "mandate-renewal";

// trace:STORY-2346 | ai:codex
pub(crate) fn mandate_renewal_job(
    state: &crate::gate::mandate::MandateState,
    now: DateTime<Utc>,
    seat: Option<&str>,
) -> Option<DueJob> {
    if seat.is_some_and(|s| !matches!(s, "advisor" | "operator")) {
        return None;
    }
    let prompt = crate::mandate::expiry_warning(state, now)?;
    Some(DueJob {
        name: MANDATE_RENEWAL_JOB.into(),
        kind: JobKind::Seat,
        seats: vec!["advisor".into(), "operator".into()],
        schedule: "before mandate expiry".into(),
        reason: "expires within 60m".into(),
        prompt: Some(format!("Ask the operator at a human terminal: {prompt}")),
        command: None,
        last_run: None,
        last_by: None,
        due_since: None,
        failure: None,
    })
}

/// The due seat jobs `aida awaiting` surfaces: `seat`'s own, plus the
/// operator escalations (seat `operator`) — but only in sessions a human
/// reads (the operator terminal, interactive advisor). Headless lanes and
/// doer seats never see them, so they can never become `cron.next` there.
// trace:TASK-B6B1278890774EF993D2EBC327862470-135 | ai:claude
// trace:TASK-B6B1278890774EF993D2EBC327862470-142 | ai:claude
pub(crate) fn awaiting_seat_jobs(
    project_root: &Path,
    seat: Option<&str>,
    headless: bool,
) -> Vec<DueJob> {
    let mut due = due_seat_jobs(project_root, seat);
    if !crate::mail_backlog::operator_escalation_visible(seat, headless) {
        due.retain(|j| !crate::mail_backlog::is_operator_job(&j.seats));
        return due;
    }
    if seat.is_some_and(|s| s != crate::mail_backlog::OPERATOR_SEAT) {
        for job in due_seat_jobs(project_root, Some(crate::mail_backlog::OPERATOR_SEAT)) {
            if !due.iter().any(|d| d.name == job.name) {
                due.push(job);
            }
        }
    }
    due
}

/// The `DUE JOBS` block prepended to a seat's pickup prompt / launch
/// context. Empty string when nothing is due.
// trace:STORY-1226 | ai:claude
pub(crate) fn render_due_jobs_block(due: &[DueJob], seat: &str) -> String {
    if due.is_empty() {
        return String::new();
    }
    let now = Utc::now();
    let mut out = format!("DUE JOBS (seat: {seat}):\n");
    for job in due {
        out.push_str(&format!("- {}\n", job.line(now)));
    }
    out.push_str(
        "Each is advice to the seat, not an automatic action. When you finish one, report it: \
         `aida schedule done <job> [--note \"...\"]`.\n",
    );
    out
}

/// `aida schedule due [--seat <role>] [--json]`.
// trace:STORY-1226 | ai:claude
fn due(project_root: &Path, seat: Option<&str>, json: bool) -> Result<()> {
    let due = collect_due(project_root, seat, seat.is_none());
    if json {
        println!("{}", serde_json::to_string_pretty(&due)?);
        return Ok(());
    }
    if due.is_empty() {
        println!(
            "Nothing due{}.",
            seat.map(|s| format!(" for seat {s}")).unwrap_or_default()
        );
        return Ok(());
    }
    let now = Utc::now();
    for job in &due {
        let tag = match job.kind {
            JobKind::Seat => format!("[seat {}]", job.seats.join(",")),
            JobKind::Substrate => {
                "[substrate — the tick runs it; `aida schedule run <job>` now]".to_string()
            }
            JobKind::FiresTask => "[legacy]".to_string(),
        };
        println!("{} {}", job.line(now), tag);
    }
    if due.iter().any(|j| j.kind == JobKind::Seat) {
        println!("\nReport a finished seat job with `aida schedule done <job>`.");
    }
    Ok(())
}

fn emit_cron(project_root: &Path) -> Result<()> {
    let Some(config) = load_registry(project_root)? else {
        println!("# No scheduled jobs configured.");
        return Ok(());
    };
    println!("# AIDA scheduled maintenance. Paste into crontab with `crontab -e`.");
    println!("# Run from: {}", project_root.display());
    println!("# Seat jobs are delivered to the seat, not cron'd; only substrate jobs with `every` appear.");
    for task in config
        .tasks
        .iter()
        .filter(|task| task.enabled && task.kind == JobKind::Substrate)
    {
        // A `while`-gated job needs the tick to evaluate its gate; a raw cron
        // line would run it ungated. trace:STORY-1917 | ai:claude
        let (Some(interval), None) = (task.interval, &task.while_) else {
            continue;
        };
        println!(
            "{} cd {} && aida schedule run {}",
            cron_interval(interval),
            shell_quote(&project_root.display().to_string()),
            shell_quote(&task.name)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cron driver install / uninstall / doctor evidence (STORY-1463)
//
// STORY-1226 registered jobs only ever RUN when something invokes
// `aida schedule tick`; nothing installs that invoker. This section adds the
// one supported auto-installer (a crontab entry, Linux/macOS) plus the
// read-only evidence `aida doctor` uses to flag a repo where nothing drives
// the tick — a registered job that silently never runs.
// ---------------------------------------------------------------------------

/// Marker embedded (as a trailing `#` comment) in the installed crontab
/// line, keyed by this repo's canonical path. Makes install idempotent
/// (re-running `aida init`/`install-cron` never duplicates the entry) and
/// lets `uninstall-cron` find exactly the line that belongs to this repo
/// without touching a different repo's entry.
// trace:STORY-1463 | ai:claude
pub(crate) fn tick_cron_marker(project_root: &Path) -> String {
    let canon = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    format!("aida-schedule-tick:{}", canon.display())
}

/// The one `aida schedule tick` invocation every driver runs, built once and
/// rendered by both [`build_tick_cron_line`] and
/// `schedule_driver::build_systemd_units`, so the argv, the PATH, the working
/// directory and the marker cannot drift between the two drivers (BUG-1600:
/// the drift that shipped was an unsupported `--format json`). The invoker
/// tag (`AIDA_SCHEDULE_INVOKER=cron|systemd`) is the only per-driver part.
// trace:TASK-1491 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TickInvocation {
    /// Canonical repo path: the `cd` target / `WorkingDirectory=`.
    pub repo: String,
    /// Absolute `aida` binary.
    pub exe: String,
    /// `PATH` value: the binary's directory first, then the system dirs.
    pub path_env: String,
    /// Arguments after the binary. Exactly `schedule tick`, never a format flag.
    pub args: [&'static str; 2],
    /// `aida-schedule-tick:<canon repo>`, carried by every driver artifact.
    pub marker: String,
}

/// Build the shared [`TickInvocation`]. Errors when any path contains `%`:
/// cron's OWN parser (before `/bin/sh` ever sees the line, and regardless of
/// shell quoting) turns an unescaped `%` in the command field into a literal
/// newline plus stdin redirection — see crontab(5) — and systemd expands `%`
/// as a unit specifier in `ExecStart=`/`WorkingDirectory=`/`Environment=`.
/// Either would silently corrupt the entry and every later read-back
/// comparison against the marker. A line break is refused for the same
/// reason: both formats are line-oriented. Rejecting outright is simpler and
/// auditable for characters that should never appear in an install path.
// trace:STORY-1463 | ai:claude
// trace:TASK-1491 | ai:claude
pub(crate) fn tick_invocation(repo: &Path, aida_exe: &Path) -> Result<TickInvocation> {
    let repo_str = repo.display().to_string();
    let aida_exe_str = aida_exe.display().to_string();
    let bin_dir = aida_exe
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    for (label, value) in [
        ("repo path", repo_str.as_str()),
        ("aida binary path", aida_exe_str.as_str()),
        ("aida binary directory", bin_dir.as_str()),
    ] {
        if value.contains('%') {
            anyhow::bail!(
                "cannot build a scheduler-tick driver entry: {label} '{value}' contains '%', \
                 which cron's own parser treats as a literal newline in the command field \
                 (crontab(5)) and systemd expands as a unit specifier — rename the path to \
                 avoid '%' and retry"
            );
        }
        if value.contains(['\n', '\r']) {
            anyhow::bail!(
                "cannot build a scheduler-tick driver entry: {label} {value:?} contains a line \
                 break — rename the path and retry"
            );
        }
    }
    // systemd strips trailing whitespace from a unit value (the tick would
    // `cd` into a sibling path, and our marker would never match again) and
    // a trailing backslash continues the line into the next directive.
    for (label, value) in [
        ("repo path", repo_str.as_str()),
        ("aida binary path", aida_exe_str.as_str()),
    ] {
        if value.ends_with(char::is_whitespace) || value.ends_with('\\') {
            anyhow::bail!(
                "cannot build a scheduler-tick driver entry: {label} {value:?} ends in \
                 whitespace or a backslash, which a systemd unit cannot hold — rename the path \
                 and retry"
            );
        }
    }
    Ok(TickInvocation {
        path_env: format!("{bin_dir}:/usr/local/bin:/usr/bin:/bin"),
        repo: repo_str,
        exe: aida_exe_str,
        // BUG-1600: `schedule tick` has no `--json`/`--format json` projection
        // (see docs/internal/cli-format-json-audit.md, BUG-1502's capability gate) — it
        // only ever prints human/TOON lines. A prior version of the cron
        // builder added `--format json` believing cron needed a
        // machine-readable log; nothing ever parsed it, and every tick from an
        // installed entry failed before dispatch.
        // trace:BUG-1600 | ai:claude
        args: ["schedule", "tick"],
        marker: tick_cron_marker(repo),
    })
}

/// Pure line-builder, split out from [`tick_cron_line`] so the exact shape
/// is unit-testable without touching `std::env::current_exe`. Renders the
/// shared [`tick_invocation`]; see there for the `%` refusal.
// trace:STORY-1463 | ai:claude
// trace:TASK-1491 | ai:claude
pub(crate) fn build_tick_cron_line(repo: &Path, aida_exe: &Path) -> Result<String> {
    Ok(render_tick_cron_line(&tick_invocation(repo, aida_exe)?))
}

/// Render the crontab line for an already-built [`TickInvocation`].
// trace:TASK-1491 | ai:claude
pub(crate) fn render_tick_cron_line(inv: &TickInvocation) -> String {
    // `AIDA_SCHEDULE_INVOKER=cron` tags every event this invocation records
    // so scheduler telemetry can tell a cron-driven tick apart from the
    // per-turn hook (`--hook`, which is self-identifying), a systemd timer,
    // or a manual run. Plain `schedule tick` output goes to
    // `schedule-tick.log`, which a human or `aida doctor` reads.
    // trace:BUG-1600 | ai:claude
    format!(
        "*/15 * * * * cd {} && PATH={} AIDA_SCHEDULE_INVOKER=cron {} {} >> ~/.aida/schedule-tick.log 2>&1 # {}",
        shell_quote(&inv.repo),
        shell_quote(&inv.path_env),
        shell_quote(&inv.exe),
        inv.args.join(" "),
        inv.marker,
    )
}

/// The crontab entry that drives `aida schedule tick` for `project_root`
/// every 15 minutes: an absolute `aida` path and an explicit `PATH` (cron's
/// own PATH is minimal), `cd`'d into the repo, logging to
/// `~/.aida/schedule-tick.log`. This is the reference shape from STORY-1463
/// (the line an operator had to hand-install before this existed). No
/// `--format`/`--json` flag — `schedule tick` has no JSON projection.
// trace:STORY-1463 | ai:claude
// trace:BUG-1600 | ai:claude
pub(crate) fn tick_cron_line(project_root: &Path) -> Result<String> {
    let repo = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    // trace:BUG-2142 | ai:claude
    build_tick_cron_line(&repo, &crate::schedule_driver::real_stable_tick_exe())
}

/// Read the current user's crontab. `Ok(None)` means no crontab exists yet
/// for this user — a common, legitimate state (`crontab -l` exits non-zero
/// with "no crontab for <user>"), not an error. Any other failure (the
/// `crontab` binary missing, a permission error, …) is `Err` so the caller
/// can report "unknown" rather than misreading it as "no entry installed".
// trace:STORY-1463 | ai:claude
pub(crate) fn read_crontab() -> Result<Option<String>> {
    let output = match ProcessCommand::new("crontab").arg("-l").output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("`crontab` is not installed or not on PATH");
        }
        Err(e) => return Err(e).context("failed to run `crontab -l`"),
    };
    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("no crontab") {
        return Ok(None);
    }
    anyhow::bail!(
        "crontab -l failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

pub(crate) fn write_crontab(body: &str) -> Result<()> {
    use std::io::Write;
    let mut child = ProcessCommand::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn `crontab -`")?;
    child
        .stdin
        .as_mut()
        .context("no stdin for `crontab -`")?
        .write_all(body.as_bytes())
        .context("failed to write the new crontab")?;
    let status = child.wait().context("failed waiting on `crontab -`")?;
    if !status.success() {
        anyhow::bail!("`crontab -` exited with {status}");
    }
    Ok(())
}

/// Whether `line` is entirely a crontab comment: its first non-whitespace
/// character is `#`. Cron treats such a line as inert — it never runs —
/// so neither the marker match nor the legacy-line match may fire on it: a
/// user who deliberately commented out their tick entry (marked or legacy)
/// must never have it silently reactivated by `install-cron`.
// trace:BUG-1605 | ai:claude
fn line_is_commented_out(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Whether `line` carries exactly this marker as its trailing `# <marker>`
/// comment — never a bare substring match. A substring match on the marker
/// (or on the whole crontab body) wrongly matches a repo whose path is a
/// strict PREFIX of another's: `aida-schedule-tick:/x/aida` is a substring
/// of `aida-schedule-tick:/x/aida-web`, so `/x/aida`'s install/uninstall
/// must never touch `/x/aida-web`'s entry (or vice versa). Anchoring on the
/// trailing `# marker` token — the exact shape `build_tick_cron_line`
/// writes — rules that out.
///
/// BUG-1605: a commented-out line (`# */15 * * * * cd ... # <marker>`) is
/// NOT a marker match even though its trailing bytes still end with
/// `# <marker>` — the whole line is inert, and matching it would let
/// `crontab_after_install` "repair" a deliberately-disabled entry back to
/// active.
// trace:STORY-1463 | ai:claude
// trace:BUG-1605 | ai:claude
pub(crate) fn line_has_marker(line: &str, marker: &str) -> bool {
    !line_is_commented_out(line) && line.trim_end().ends_with(&format!("# {marker}"))
}

/// The repo path a marker was built for (`tick_cron_marker`'s inverse):
/// strips the fixed `aida-schedule-tick:` prefix. `None` for a malformed
/// marker, which a caller treats as "no legacy repair target" — the marker
/// shape is this module's own invariant, not user input.
// trace:BUG-1605 | ai:claude
fn repo_from_marker(marker: &str) -> Option<&str> {
    marker.strip_prefix("aida-schedule-tick:")
}

/// Whether `tok` (whitespace-split, optionally `'`/`"`-quoted) looks like an
/// `aida` binary invocation: the bare name, or a path ending in `/aida`.
// trace:BUG-1605 | ai:claude
fn is_aida_binary_token(tok: &str) -> bool {
    let bare = tok.trim_matches(|c| c == '\'' || c == '"');
    bare == "aida" || bare.ends_with("/aida")
}

/// Whether `line` is a legacy, pre-marker AIDA tick line for `repo`: older
/// installs (before STORY-1463's marker) wrote `cd <repo> && ... aida
/// schedule tick ...` with no trailing `# <marker>` comment, so
/// `line_has_marker` never recognised them as this repo's own entry and
/// `install-cron` appended a second, correctly-marked line instead of
/// repairing the first — both then ran (BUG-1605).
///
/// Requires, in order: `cd <repo>` — unquoted or `'`/`"`-quoted, with a
/// leading space and a trailing ` &&` so a repo whose path is a strict
/// PREFIX of another's can never match (same guard `line_has_marker`
/// documents for the marker itself: `/x/aida` must not match `/x/aida-web`'s
/// line) — then, anywhere after it, an `aida` binary token immediately
/// followed by `schedule tick` (further flags, e.g. the old `--format
/// json`, may follow). Callers check `line_has_marker` first; a line that
/// already carries this repo's marker is the MARKED case, not legacy.
///
/// BUG-1605: a commented-out legacy line (`# */15 * * * * cd <repo> && ...`)
/// is NOT a legacy match — same reasoning as `line_has_marker`'s comment
/// guard: the line is inert, and a user who disabled it deliberately must
/// not have it silently reactivated.
// trace:BUG-1605 | ai:claude
fn line_is_legacy_tick_line(line: &str, repo: &str) -> bool {
    if line_is_commented_out(line) {
        return false;
    }
    let rest = ["", "'", "\""].iter().find_map(|q| {
        let needle = format!(" cd {q}{repo}{q} &&");
        line.find(&needle).map(|pos| &line[pos + needle.len()..])
    });
    let Some(rest) = rest else {
        return false;
    };
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    tokens
        .windows(3)
        .any(|w| is_aida_binary_token(w[0]) && w[1] == "schedule" && w[2] == "tick")
}

/// Every line index in `existing` that is an ACTIVE (not commented-out)
/// legacy tick line for `repo`, and is not already the marked line at
/// `marked_pos` — in document order. There can be more than one: a repo
/// may have accumulated several unmarked entries across old installs before
/// the marker convention existed.
// trace:BUG-1605 | ai:claude
fn legacy_tick_line_positions(lines: &[&str], repo: &str, marked_pos: Option<usize>) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|&(i, l)| Some(i) != marked_pos && line_is_legacy_tick_line(l, repo))
        .map(|(i, _)| i)
        .collect()
}

/// Whether `existing` already carries a tick entry for this repo — the
/// current marked shape or a legacy pre-marker line — so
/// `crontab_after_install` returning `Some(...)` is a repair rather than a
/// fresh append.
// trace:BUG-1605 | ai:claude
pub(crate) fn crontab_has_repair_target(existing: &str, marker: &str) -> bool {
    let repo = repo_from_marker(marker);
    existing
        .lines()
        .any(|l| line_has_marker(l, marker) || repo.is_some_and(|r| line_is_legacy_tick_line(l, r)))
}

/// Pure: the new crontab body after installing `line` (marked by `marker`).
/// `None` when `marker` is already present with byte-identical content —
/// true idempotent no-op. When the marker is present but the line's content
/// has drifted (BUG-1600: an old build installed `schedule tick --format
/// json`, which fails every tick), the stale line is REPLACED in place
/// rather than left alone — `aida init`'s offer and `aida schedule
/// install-cron` are the "run this again to pick up a fix" affordance, so a
/// previously-installed entry must self-repair the next time either runs.
///
/// BUG-1605: every ACTIVE (not commented-out) legacy, pre-marker line for
/// this repo (`legacy_tick_line_positions`) is repaired the same way —
/// collapsed into the current marked `line`, not appended alongside. A repo
/// can carry more than one such line (several old installs stacked up
/// before the marker convention existed); ALL of them are folded into the
/// one kept line, so exactly one correct, active entry remains. A
/// commented-out line — marked or legacy — is inert and is never touched:
/// a user who deliberately disabled their tick entry keeps it disabled.
///
/// Never reorders whatever `crontab -l` already printed; a repo with
/// neither an active marked nor an active legacy line is still appended,
/// never inserted elsewhere (this covers "no entry at all" and "every
/// candidate line is commented out" alike). Every other line — another
/// repo's, or an unrelated user line, commented or not — passes through
/// byte-for-byte.
// trace:STORY-1463 | ai:claude
// trace:BUG-1600 | ai:claude
// trace:BUG-1605 | ai:claude
pub(crate) fn crontab_after_install(existing: &str, marker: &str, line: &str) -> Option<String> {
    let lines: Vec<&str> = existing.lines().collect();
    let marked_pos = lines.iter().position(|l| line_has_marker(l, marker));
    let legacy_positions = match repo_from_marker(marker) {
        Some(repo) => legacy_tick_line_positions(&lines, repo, marked_pos),
        None => Vec::new(),
    };

    if marked_pos.is_none() && legacy_positions.is_empty() {
        let mut body = existing.to_string();
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(line);
        body.push('\n');
        return Some(body);
    }
    if legacy_positions.is_empty() {
        if let Some(pos) = marked_pos {
            if lines[pos] == line {
                return None;
            }
        }
    }

    // Keep exactly one line: the marked line's slot when there was one,
    // else the first legacy line's slot. Drop every other legacy line.
    // Order of every other (unrelated) line is preserved.
    let keep_pos = marked_pos.unwrap_or_else(|| legacy_positions[0]);
    let drop: BTreeSet<usize> = legacy_positions
        .iter()
        .copied()
        .filter(|&i| i != keep_pos)
        .collect();
    let mut repaired = Vec::with_capacity(lines.len());
    for (i, l) in lines.into_iter().enumerate() {
        if drop.contains(&i) {
            continue;
        }
        repaired.push(if i == keep_pos { line } else { l });
    }
    let mut body = repaired.join("\n");
    body.push('\n');
    Some(body)
}

/// Pure: the new crontab body with every line carrying `marker` removed, or
/// `None` when `marker` was not present (idempotent no-op).
// trace:STORY-1463 | ai:claude
pub(crate) fn crontab_after_uninstall(existing: &str, marker: &str) -> Option<String> {
    if !existing.lines().any(|l| line_has_marker(l, marker)) {
        return None;
    }
    let mut body = existing
        .lines()
        .filter(|line| !line_has_marker(line, marker))
        .collect::<Vec<_>>()
        .join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    Some(body)
}

/// Pure: the crontab body after switching this repo to another driver
/// (advisor A13b). Removes every ACTIVE line that is this repo's marked
/// entry ([`line_has_marker`]) or an active legacy, pre-marker tick line for
/// it ([`line_is_legacy_tick_line`], the BUG-1605 predicates) — and nothing
/// else. Commented-out lines (marked or legacy), another repo's lines
/// (including a repo whose path is a strict prefix of this one) and
/// unrelated user lines come through byte-for-byte, line endings and a
/// missing final newline included. Returns the new body and every removed
/// line (for the caller to print), or `None` when nothing matched.
///
/// Differs from [`crontab_after_uninstall`], which removes only MARKED
/// lines: a driver switch must not leave an old unmarked entry ticking
/// alongside the new driver.
// trace:TASK-1491 | ai:claude
pub(crate) fn crontab_after_driver_switch(
    existing: &str,
    marker: &str,
) -> Option<(String, Vec<String>)> {
    let repo = repo_from_marker(marker);
    let mut body = String::with_capacity(existing.len());
    let mut removed = Vec::new();
    for raw in existing.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let ours = line_has_marker(line, marker)
            || repo.is_some_and(|r| line_is_legacy_tick_line(line, r));
        if ours {
            removed.push(line.to_string());
        } else {
            body.push_str(raw);
        }
    }
    if removed.is_empty() {
        None
    } else {
        Some((body, removed))
    }
}

/// What [`install_tick_cron`] did. BUG-1600: a plain bool collapsed "wasn't
/// there, now is" and "was there but stale (e.g. the old `--format json`
/// flag), now fixed" into the same `Ok(true)` — the CLI printed "Installed"
/// for a repair too, which reads as a no-op to an operator re-running the
/// command to pick up a fix. Distinguishing the three lets the caller say so.
// trace:BUG-1600 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DriverInstallOutcome {
    /// No entry existed for this repo; one was appended.
    Installed,
    /// An entry existed and already matched the current reference shape.
    AlreadyUpToDate,
    /// An entry existed with stale content (e.g. an old unsupported flag)
    /// and was rewritten in place.
    Repaired,
}

/// Remove this repo's tick entry from the user's crontab (found via its
/// marker). Idempotent (`Ok(false)` when nothing matched); never touches an
/// entry belonging to a different repo.
// trace:STORY-1463 | ai:claude
pub(crate) fn uninstall_tick_cron(project_root: &Path) -> Result<bool> {
    if cfg!(windows) {
        anyhow::bail!("cron is not available on Windows");
    }
    let marker = tick_cron_marker(project_root);
    let Some(existing) = read_crontab()? else {
        return Ok(false);
    };
    match crontab_after_uninstall(&existing, &marker) {
        None => Ok(false),
        Some(body) => {
            write_crontab(&body)?;
            Ok(true)
        }
    }
}

/// Whether a scheduler driver is installed for this repo, from the one
/// signal we can positively check: our own crontab marker.
// trace:STORY-1463 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CronDriverStatus {
    /// The marker line is present in the user's crontab.
    Installed,
    /// crontab is readable and the marker is absent.
    Missing,
    /// Could not determine (no `crontab` on PATH, unsupported platform,
    /// permission error, …). PRIN-5: never collapse this into "ok".
    Unknown(String),
}

/// Pure classifier over an already-resolved crontab read, so the doctor
/// logic (`build_scheduler_driver_findings`) is unit-testable without
/// shelling out. `Err(reason)` mirrors a `read_crontab` failure.
// trace:STORY-1463 | ai:claude
pub(crate) fn classify_cron_driver(
    crontab: Result<Option<String>, String>,
    marker: &str,
) -> CronDriverStatus {
    match crontab {
        Ok(Some(body)) if body.lines().any(|l| line_has_marker(l, marker)) => {
            CronDriverStatus::Installed
        }
        Ok(_) => CronDriverStatus::Missing,
        Err(reason) => CronDriverStatus::Unknown(reason),
    }
}

/// How many enabled SUBSTRATE jobs this repo's merged registry declares
/// (project `.aida/config.toml` + machine-global `~/.aida/schedule.toml`).
/// `0` when there is no registry at all, or every entry is a seat job / disabled.
// trace:STORY-1463 | ai:claude
pub(crate) fn enabled_substrate_job_count(project_root: &Path) -> Result<usize> {
    let Some(config) = load_registry(project_root)? else {
        return Ok(0);
    };
    Ok(config
        .tasks
        .iter()
        .filter(|t| t.enabled && t.kind == JobKind::Substrate)
        .count())
}

/// An enabled substrate job whose effective last run (ledger, cross-clone;
/// or local mirror) is more than 2x its own interval in the past — the
/// observable symptom of a broken driver even when an entry IS installed
/// (wrong PATH, wrong `aida` binary, cron daemon disabled, …).
// trace:STORY-1463 | ai:claude
pub(crate) struct OverdueSubstrateJob {
    pub name: String,
    pub interval: Duration,
    pub last_run: DateTime<Utc>,
}

/// Enabled substrate jobs overdue by more than 2x their interval. A job
/// that has NEVER run (no ledger, no local mirror) is deliberately excluded
/// here — that is "no evidence", not "overdue" — the missing-driver finding
/// (`enabled_substrate_job_count` + `cron_driver_status`) covers that case.
// trace:STORY-1463 | ai:claude
pub(crate) fn overdue_substrate_jobs(project_root: &Path) -> Result<Vec<OverdueSubstrateJob>> {
    let Some(config) = load_registry(project_root)? else {
        return Ok(Vec::new());
    };
    let ledgers = schedule_ledger::load_all(&store_root(project_root));
    let state = load_state(project_root)?;
    let now = Utc::now();
    let mut out = Vec::new();
    for task in &config.tasks {
        if !task.enabled || task.kind != JobKind::Substrate {
            continue;
        }
        // A `while`-gated job idles by design while its gate is false.
        // trace:STORY-1917 | ai:claude
        let (Some(interval), None) = (task.interval, &task.while_) else {
            continue;
        };
        let Some(last_run) =
            effective_last_run(ledgers.get(&task.name), state.tasks.get(&task.name))
        else {
            continue;
        };
        if now.signed_duration_since(last_run) > interval * 2 {
            out.push(OverdueSubstrateJob {
                name: task.name.clone(),
                interval,
                last_run,
            });
        }
    }
    Ok(out)
}

/// Pure assembly of the `scheduler-driver` doctor findings from
/// already-computed evidence — no I/O, fully unit-testable. Systemd-aware
/// (TASK-1491): a repo driven only by its systemd timer is not driverless,
/// and a repo with BOTH drivers installed is flagged — harmless under the
/// tick lock, but each driver ticks, and one should be removed.
// trace:STORY-1463 | ai:claude
// trace:TASK-1491 | ai:claude
pub(crate) fn build_scheduler_driver_findings(
    enabled_substrate_jobs: usize,
    status: &crate::schedule_driver::DriverStatus,
    overdue: &[OverdueSubstrateJob],
    now: DateTime<Utc>,
) -> Vec<crate::DoctorFinding> {
    use crate::schedule_driver::SystemdDriverStatus;
    let mut out = Vec::new();
    if status.both_installed() {
        out.push(crate::DoctorFinding {
            category: "scheduler-driver".to_string(),
            id: "scheduler-tick-dual-driver".to_string(),
            summary: "both a crontab entry and a systemd user timer run `aida schedule tick` for \
                      this repo; the tick lock keeps them from overlapping, but keep only one"
                .to_string(),
            action: "aida schedule uninstall-cron".to_string(),
            safe_heal: false,
        });
    }
    let install_action = match status.systemd {
        SystemdDriverStatus::Unsupported => "aida schedule install-cron",
        _ => "aida schedule install-systemd",
    };
    if enabled_substrate_jobs > 0 && !status.any_installed() {
        // PRIN-5: when either driver's state is unreadable and none is
        // confirmed installed, the answer is "unknown", never "none".
        let unknown = status.unknown_reasons();
        if unknown.is_empty() {
            let systemd_note = match &status.systemd {
                SystemdDriverStatus::Disabled => {
                    " (this repo's systemd timer exists but is disabled)"
                }
                SystemdDriverStatus::Stopped => {
                    " (this repo's systemd timer is enabled but not running)"
                }
                _ => "",
            };
            out.push(crate::DoctorFinding {
                category: "scheduler-driver".to_string(),
                id: "scheduler-tick-not-installed".to_string(),
                summary: format!(
                    "{enabled_substrate_jobs} enabled substrate scheduler job(s) registered, but nothing invokes `aida schedule tick` for this repo — they will never run{systemd_note}"
                ),
                action: install_action.to_string(),
                safe_heal: false,
            });
        } else {
            out.push(crate::DoctorFinding {
                category: "scheduler-driver".to_string(),
                id: "scheduler-tick-driver-unknown".to_string(),
                summary: format!(
                    "cannot confirm whether a scheduler driver is installed for this repo ({}) — status unknown, not ok",
                    unknown.join("; ")
                ),
                action: install_action.to_string(),
                safe_heal: false,
            });
        }
    }
    if !overdue.is_empty() {
        let detail = overdue
            .iter()
            .map(|o| {
                format!(
                    "{} (last run {} ago, interval {})",
                    o.name,
                    human_age(now.signed_duration_since(o.last_run)),
                    format_duration(o.interval)
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        out.push(crate::DoctorFinding {
            category: "scheduler-driver".to_string(),
            id: "scheduler-job-overdue".to_string(),
            summary: format!(
                "{} substrate job(s) overdue by more than 2x their interval — a driver may be installed but not actually ticking: {detail}",
                overdue.len()
            ),
            action: "aida schedule status".to_string(),
            safe_heal: false,
        });
    }
    out
}

/// PURE: whether doctor reads the drivers at all. A repo with no registered
/// jobs, nothing overdue and no timer file of ours never shells out to
/// `crontab` or `systemctl`; a timer file alone (a file read) is enough
/// evidence to look, so a dual-driver setup is flagged even with no jobs.
// trace:TASK-1491 | ai:claude
pub(crate) fn scheduler_driver_check_needed(
    enabled_substrate_jobs: usize,
    any_overdue: bool,
    systemd_timer_file_present: bool,
) -> bool {
    enabled_substrate_jobs > 0 || any_overdue || systemd_timer_file_present
}

/// Doctor: an enabled `*-route` seat job whose `on` holds a bare
/// `CronJobFailed` fires on EVERY substrate job's failure, so one guard trip
/// wakes every route with the wrong evidence. Hint to bind it to its guard.
// trace:BUG-1655 | ai:claude
pub(crate) fn build_unbound_route_findings(tasks: &[Task]) -> Vec<crate::DoctorFinding> {
    let names: BTreeSet<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    tasks
        .iter()
        .filter(|t| t.enabled && t.name.ends_with("-route"))
        .filter(|t| {
            t.on.iter()
                .any(|trigger| trigger.kind == "CronJobFailed" && trigger.job.is_none())
        })
        .map(|t| {
            let guard = t.name.trim_end_matches("-route");
            // trace:TASK-1535 | ai:codex
            let action = if names.contains(guard) {
                format!(
                    "bind it to its guard in .aida/config.toml: on = [\"CronJobFailed:{guard}\"]"
                )
            } else {
                "bind it to the job it guards in .aida/config.toml".to_string()
            };
            crate::DoctorFinding {
                category: "scheduler-driver".to_string(),
                id: format!("schedule-route-unbound:{}", t.name),
                summary: format!(
                    "scheduled job '{}' fires on ANY job's failure (`on = [\"CronJobFailed\"]`), \
                     so another guard's trip wakes it with the wrong evidence",
                    t.name
                ),
                action,
                safe_heal: false,
            }
        })
        .collect()
}

// trace:TASK-1535 | ai:codex
pub(crate) fn build_invalid_route_findings(tasks: &[Task]) -> Vec<crate::DoctorFinding> {
    tasks
        .iter()
        .filter(|t| t.problem.is_some())
        .map(|t| crate::DoctorFinding {
            category: "scheduler-driver".to_string(),
            id: format!("schedule-route-invalid:{}", t.name),
            summary: format!(
                "scheduled job '{}' is disabled because its bound job is missing from the schedule registry: {}",
                t.name,
                t.problem.as_deref().unwrap_or_default()
            ),
            action: "correct the bound job name or add the job it guards in .aida/config.toml"
                .to_string(),
            safe_heal: false,
        })
        .collect()
}

/// Doctor: an enabled substrate job whose `command` this binary does not
/// support. The tick fails it every time it is due (BUG-2142).
// trace:BUG-2142 | ai:claude
pub(crate) fn build_unsupported_command_findings(tasks: &[Task]) -> Vec<crate::DoctorFinding> {
    tasks
        .iter()
        .filter(|t| t.enabled && t.kind == JobKind::Substrate)
        .filter_map(|t| t.invalid_command.as_ref().map(|bad| (t, bad)))
        .map(|(t, bad)| crate::DoctorFinding {
            category: "scheduler-driver".to_string(),
            id: format!("schedule-command-unsupported:{}", t.name),
            summary: format!(
                "scheduled job '{}' runs `aida {}`, which this aida binary does not support; \
                 every due tick records it as failed",
                t.name,
                bad.raw.trim()
            ),
            action: "update aida and reinstall the tick driver, or fix the job's `command` \
                     (`aida schedule list` shows its source)"
                .to_string(),
            safe_heal: false,
        })
        .collect()
}

/// `aida doctor` entry point, evidence-gated by
/// [`scheduler_driver_check_needed`].
// trace:STORY-1463 | ai:claude
// trace:TASK-1491 | ai:claude
pub(crate) fn scheduler_driver_doctor_findings(
    project_root: &Path,
) -> Result<Vec<crate::DoctorFinding>> {
    let enabled = enabled_substrate_job_count(project_root)?;
    let overdue = overdue_substrate_jobs(project_root)?;
    // Only read the timer file when nothing else already asks for a look.
    let timer_file = enabled == 0
        && overdue.is_empty()
        && crate::schedule_driver::systemd_timer_file_present_for(project_root);
    // trace:BUG-1655 | ai:claude
    let mut out = load_registry(project_root)?
        .map(|c| {
            // trace:TASK-1535 | ai:codex
            let mut findings = build_unbound_route_findings(&c.tasks);
            findings.extend(build_invalid_route_findings(&c.tasks));
            // trace:BUG-2142 | ai:claude
            findings.extend(build_unsupported_command_findings(&c.tasks));
            findings
        })
        .unwrap_or_default();
    if !scheduler_driver_check_needed(enabled, !overdue.is_empty(), timer_file) {
        return Ok(out);
    }
    let status = crate::schedule_driver::driver_status(project_root);
    out.extend(build_scheduler_driver_findings(
        enabled,
        &status,
        &overdue,
        Utc::now(),
    ));
    // trace:BUG-2142 | ai:claude
    out.extend(crate::schedule_driver::stale_tick_binary_findings(
        project_root,
    ));
    // TASK-1517: an unattended wave's memory ceiling only exists if the user
    // manager can enforce it. Read only where the systemd timer of this repo
    // actually exists, so a cron-driven or timer-less repo never looks; the
    // finding is report-only and never stops a wave.
    // trace:TASK-1517 | ai:claude
    if wave_limit_delegation_check_needed(&status.systemd) {
        out.extend(crate::schedule_driver::build_wave_limit_findings(
            &crate::schedule_driver::real_wave_limit_delegation(),
        ));
    }
    Ok(out)
}

/// PURE: whether the wave-limit delegation check reads anything at all. Only
/// a repo whose systemd timer exists can launch a wave in a transient unit,
/// so nowhere else is there a limit to enforce.
// trace:TASK-1517 | ai:claude
pub(crate) fn wave_limit_delegation_check_needed(
    systemd: &crate::schedule_driver::SystemdDriverStatus,
) -> bool {
    use crate::schedule_driver::SystemdDriverStatus;
    matches!(
        systemd,
        SystemdDriverStatus::Installed
            | SystemdDriverStatus::Disabled
            | SystemdDriverStatus::Stopped
    )
}

/// `aida schedule install-cron` / `install-systemd`. Gated like `aida shift
/// enable` (a human at a TTY answering yes): a driver starts unattended
/// scheduled runs. Installing one driver removes this repo's other one, but
/// only after the new one is verified (A13a).
// trace:STORY-1463 | ai:claude
// trace:TASK-1491 | ai:claude
fn install_driver_command(
    project_root: &Path,
    driver: crate::schedule_driver::Driver,
) -> Result<()> {
    use crate::schedule_driver::Driver;
    if cfg!(windows) {
        println!("Windows has no crontab or systemd. Add this line to Task Scheduler instead:");
        println!("  {}", tick_cron_line(project_root)?);
        return Ok(());
    }
    let command = match driver {
        Driver::Cron => "aida schedule install-cron",
        Driver::Systemd => "aida schedule install-systemd",
    };
    crate::schedule_driver::with_real_operator(|op| {
        crate::schedule_driver::install_driver_command(
            project_root,
            command,
            driver,
            op,
            &mut crate::schedule_driver::RealDriverHost,
        )
    })?;
    retire_stopgap_reconcile_cron(project_root, crate::store_local::remove_stopgap_cron);
    Ok(())
}

/// On the hub-bridge clone, the installed tick now runs the reconcile, so
/// the hand-installed stopgap crontab entry would run it twice: remove it
/// with `remove` (the real crontab edit outside tests). Best-effort — a
/// crontab that cannot be read never fails the install.
// trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
fn retire_stopgap_reconcile_cron(project_root: &Path, remove: impl FnOnce() -> Result<bool>) {
    if !bridge_jobs(project_root).is_ok_and(|jobs| !jobs.is_empty()) {
        return;
    }
    match remove() {
        Ok(true) => println!(
            "Removed the stopgap `{}` crontab entry — `{}` replaces it.",
            crate::store_local::STOPGAP_CRON_MARKER,
            crate::store_local::BRIDGE_JOB
        ),
        Ok(false) => {}
        Err(e) => eprintln!("warning: could not check the crontab for the stopgap entry: {e:#}"),
    }
}

/// `aida schedule uninstall-cron`.
// trace:STORY-1463 | ai:claude
fn uninstall_cron_command(project_root: &Path) -> Result<()> {
    if cfg!(windows) {
        println!("Windows has no crontab entry to remove.");
        return Ok(());
    }
    match uninstall_tick_cron(project_root) {
        Ok(true) => println!("Removed this repo's scheduler tick crontab entry."),
        Ok(false) => println!("No crontab entry found for this repo."),
        Err(e) => return Err(e),
    }
    Ok(())
}

/// PURE: whether `aida init` may offer the crontab install. The same floor
/// as `aida schedule install-cron` (`driver_gate_refusal`: a human at an
/// interactive stdin, outside agent output mode), plus a terminal stdout to
/// show the question on.
// trace:TASK-1491 | ai:claude
pub(crate) fn init_tick_offer_allowed(stdin_tty: bool, stdout_tty: bool, agent_mode: bool) -> bool {
    stdout_tty
        && crate::schedule_driver::driver_gate_refusal("aida init", stdin_tty, agent_mode).is_none()
}

/// STORY-1463: at a TTY, offer to install the crontab entry that drives
/// `aida schedule tick` for this repo. Default answer is **no** — writing to
/// the operator's crontab is a real system side effect that should be an
/// explicit yes, not an assumed one. Non-interactive `aida init` never
/// prompts and never installs anything.
// trace:STORY-1463 | ai:claude
pub(crate) fn maybe_offer_tick_install(project_root: &Path) {
    use std::io::IsTerminal;
    if !init_tick_offer_allowed(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        crate::agent_output_mode(),
    ) {
        return;
    }
    let line = match tick_cron_line(project_root) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("  Note: scheduler tick install skipped: {e}");
            return;
        }
    };
    println!();
    println!("Scheduler tick");
    println!("  Registered scheduler jobs only run when something invokes `aida schedule tick`.");
    println!("  This line would drive it every 15 minutes:");
    println!("    {line}");
    if cfg!(windows) {
        println!(
            "  Windows has no crontab — add the line above to Task Scheduler yourself if you want it driven."
        );
        return;
    }
    let install = crate::prompt_yes_no("  Install now? [y/N] ", false).unwrap_or(false);
    if !install {
        println!(
            "  Skipped. Install later with `aida schedule install-cron` (undo with `aida schedule uninstall-cron`)."
        );
        return;
    }
    // TASK-1491: the same verified switch as `install-cron`, so a systemd
    // timer for this repo is removed once the entry is confirmed.
    let result = crate::schedule_driver::real_tick_invocation(project_root).and_then(|inv| {
        crate::schedule_driver::switch_driver(
            &mut crate::schedule_driver::RealDriverHost,
            &inv,
            crate::schedule_driver::Driver::Cron,
        )
    });
    match result.map(|r| r.outcome) {
        Ok(DriverInstallOutcome::Installed) => println!("  Installed."),
        Ok(DriverInstallOutcome::AlreadyUpToDate) => println!("  Already installed."),
        // trace:BUG-1600 | ai:claude
        Ok(DriverInstallOutcome::Repaired) => {
            println!(
                "  Repaired — the installed entry was running an older, unsupported invocation."
            )
        }
        Err(e) => eprintln!("  Note: scheduler tick was not installed: {e:#}"),
    }
}

// ---------------------------------------------------------------------------
// Snapshot for `when` predicates
// ---------------------------------------------------------------------------

/// Strict project-wide idle evidence for opportunistic docs work. Existing
/// seat-local queue meters retain their display semantics.
// trace:STORY-2220 | ai:codex
fn project_work_idle(project_root: &Path) -> Result<bool> {
    let store = crate::store_sync::detect_distributed_store_from(project_root)
        .context("cannot resolve project queues")?;
    let queues = store.join("registry/queues");
    let entries = match std::fs::read_dir(&queues) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("cannot enumerate project queues"),
    };
    let mut busy = false;
    if let Some(entries) = entries {
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read queue {}", path.display()))?;
            let document: serde_yaml::Value = serde_yaml::from_str(&content)
                .with_context(|| format!("cannot parse queue {}", path.display()))?;
            // Empty/null YAML is invalid evidence; only an explicit sequence
            // can establish that an existing queue contains no work.
            // trace:STORY-2220 | ai:codex
            anyhow::ensure!(
                document.is_sequence(),
                "cannot parse queue {}: expected a YAML sequence",
                path.display()
            );
            let rows: Vec<aida_core::QueueEntry> = serde_yaml::from_value(document)
                .with_context(|| format!("cannot parse queue {}", path.display()))?;
            busy |= rows.iter().any(|row| {
                matches!(
                    row.for_role.as_deref(),
                    None | Some("implementer" | "reviewer")
                )
            });
        }
    }
    let project = project_root.canonicalize()?;
    let home = crate::home_dir().context("cannot resolve global queue home")?;
    for role in ["implementer", "reviewer"] {
        let path = home.join(".aida/queue").join(format!("{role}.yaml"));
        for row in crate::global_queue::read_queue(&path, role)? {
            // Compare canonical store identity too, so linked worktrees count.
            let same = row.project_root == project || row.project_root.canonicalize()? == project;
            let same_store = crate::store_sync::detect_distributed_store_from(&row.project_root)
                .map(|other| other.canonicalize())
                .transpose()?
                .is_some_and(|other| Some(other) == store.canonicalize().ok());
            busy |= same || same_store;
        }
    }
    Ok(!busy)
}

/// Build the predicate snapshot, computing only the `fields` some job
/// references. Every column is a local-file / cache read — never git, never
/// the network (`ci.red_prs` is therefore 0 here; the tick has no gh call).
// trace:STORY-1226 | ai:claude
pub(crate) fn collect_snapshot(
    project_root: &Path,
    fields: &BTreeSet<Field>,
    backend: Option<&aida_core::CachedGitBackend>,
) -> Snapshot {
    let mut snap = Snapshot::default();
    let now = Utc::now();
    if fields.contains(&Field::MailUnread) || fields.contains(&Field::MailOldestUnreadAge) {
        let store = store_root(project_root);
        let local = crate::mailbox_store::read_local_messages(project_root).unwrap_or_default();
        let canonical = crate::mailbox_store::read_canonical_messages(&store).unwrap_or_default();
        let merged = aida_core::mailbox::merge_dedup(&local, &canonical);
        // trace:TASK-1492 trace:STORY-1843 | ai:claude — the same per-recipient
        // read (watermark or receipt) the night shift's mail-latency
        // escalation uses. Fails closed: unreadable read evidence counts the
        // mail unread, never as zero unread.
        let evidence = crate::mailbox_store::ReadEvidence::load_fail_closed(project_root);
        let per_recipient = crate::mailbox_store::unread_by_recipient(&merged, &evidence);
        let unread: i64 = per_recipient.values().map(|u| u.count).sum();
        let oldest: Option<i64> = per_recipient.values().map(|u| u.oldest_ts).min();
        snap.mail_unread = unread;
        snap.mail_oldest_unread_age_secs = oldest
            .map(|ts| ((now.timestamp_millis() - ts) / 1000).max(0))
            .unwrap_or(0);
    }
    if fields.contains(&Field::DrainLockFree) {
        snap.drain_lock_free = crate::drain_lock::read_pid_live_lock(project_root).is_none();
    }
    // trace:STORY-1700 | ai:claude
    if fields.contains(&Field::MandateDriveActive) {
        snap.mandate_drive_active = crate::gate::mandate_drive::drive_record_active(now);
    }
    // trace:STORY-1917 | ai:agy
    if fields.contains(&Field::MandateDispatchActive) {
        snap.mandate_dispatch_active = crate::dispatch::dispatch_record_active(now);
    }
    if fields.contains(&Field::DispatchPending) {
        snap.dispatch_pending = crate::dispatch::pending_request_count(project_root);
    }
    if fields.contains(&Field::ProjectWorkIdle) {
        snap.project_work_idle = project_work_idle(project_root).ok();
    }
    if fields.contains(&Field::QueueDepth) {
        snap.queue_depth = crate::read_queue_depth(project_root, None).unwrap_or(0) as i64;
    }
    if fields.contains(&Field::QueueDrainModeReady) {
        snap.queue_drain_mode_ready = backend
            .map(|b| drain_mode_ready_count(project_root, b))
            .unwrap_or(0);
    }
    if fields.contains(&Field::SessionsFinishedUnreaped) {
        snap.sessions_finished_unreaped = crate::session_reap::scan_reapable(project_root)
            .reapable
            .len() as i64;
    }
    if fields.contains(&Field::FindingsOpen) {
        snap.findings_open = backend
            .and_then(|b| {
                b.list_summaries(&aida_core::ListFilter {
                    status: Some("draft".to_string()),
                    ..Default::default()
                })
                .ok()
            })
            .map(|draft| {
                crate::findings::count_findings(&crate::findings::build_findings_view(
                    &draft,
                    &crate::findings::FindingsFilter::default(),
                )) as i64
            })
            .unwrap_or(0);
    }
    if fields.contains(&Field::EscalationsOpen) {
        snap.escalations_open = backend
            .and_then(|b| {
                b.list_summaries(&aida_core::ListFilter::default())
                    .ok()
                    .map(|rows| {
                        rows.iter()
                            .filter(|s| s.status.eq_ignore_ascii_case("NeedsAttention"))
                            .filter(|s| {
                                let id = s
                                    .agreed_id
                                    .clone()
                                    .or_else(|| s.spec_id.clone())
                                    .unwrap_or_default();
                                !matches!(
                                    b.get_requirement_by_spec_id(&id),
                                    Ok(Some(req)) if req.failure_reason.is_some()
                                )
                            })
                            .count() as i64
                    })
            })
            .unwrap_or(0);
    }
    snap
}

/// Queued specs groomed `execution_mode = drain` that are not in flight.
fn drain_mode_ready_count(project_root: &Path, backend: &aida_core::CachedGitBackend) -> i64 {
    let store = store_root(project_root);
    let user = aida_core::db::resolve_queue_user(&store, &crate::current_user_id(None));
    let path = store.join("registry/queues").join(format!("{user}.yaml"));
    let Ok(text) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let Ok(entries) = serde_yaml::from_str::<Vec<serde_yaml::Value>>(&text) else {
        return 0;
    };
    entries
        .iter()
        .filter_map(|e| e.get("spec_id").and_then(serde_yaml::Value::as_str))
        .filter(|id| {
            matches!(
                backend.get_requirement_by_spec_id(id),
                Ok(Some(req))
                    if req.execution_mode == Some(aida_core::ExecutionMode::Drain)
                        && matches!(
                            req.status,
                            aida_core::RequirementStatus::Approved
                                | aida_core::RequirementStatus::Planned
                        )
            )
        })
        .count() as i64
}

// ---------------------------------------------------------------------------
// Allow-list runner (STORY-1047)
// ---------------------------------------------------------------------------

// One entry per runnable substrate command: the input string(s) that select
// it, plus the ScheduledCommand it maps to. This table is the SINGLE source
// of truth for the allowlist — `parse_scheduled_command` matches against it
// and `valid_commands` (used both in the bail message and by the scaffold
// parity test below) is derived from it, so the enumeration cannot drift
// from the match arms the way a hand-typed sibling list can. Same shape as
// the fix requested for the `aida doctor` category list (BUG-1554).
// trace:BUG-1557 | ai:claude
fn command_table() -> &'static [(&'static [&'static str], ScheduledCommand)] {
    &[
        // External condition probes and canonical writes belong only on timer ticks.
        // trace:STORY-2402 | ai:codex
        (
            &["defer resurface"],
            ScheduledCommand {
                display: "defer resurface",
                args: &["defer", "resurface"],
                hook_allowed: false,
            },
        ),
        (
            &["cache verify"],
            ScheduledCommand {
                display: "cache verify",
                args: &["cache", "verify"],
                hook_allowed: true,
            },
        ),
        // Inline history-only work belongs to timer ticks, never prompt hooks.
        // trace:STORY-1661 | ai:codex
        (
            &["cache refresh --history --if-behind"],
            ScheduledCommand {
                display: "cache refresh --history --if-behind",
                args: &["cache", "refresh", "--history", "--if-behind"],
                hook_allowed: false,
            },
        ),
        // TASK-1527: retire an orphaned cache refresh request. A silent no-op
        // without a pending request; when one pends it refreshes the cache
        // inline, which can take seconds — timer-tick territory, not a
        // per-prompt hook's.
        // trace:TASK-1527 | ai:claude
        (
            &["cache refresh --if-requested"],
            ScheduledCommand {
                display: "cache refresh --if-requested",
                args: &["cache", "refresh", "--if-requested"],
                hook_allowed: false,
            },
        ),
        (
            &["session reap"],
            ScheduledCommand {
                display: "session reap",
                args: &["session", "reap", "--yes"],
                hook_allowed: true,
            },
        ),
        // STORY-2351: the hourly hygiene pass (reap + agent gc + idle-cache
        // reclaim). Deletes caches and walks every lease, so never on the
        // per-turn hook path.
        // trace:STORY-2351 | ai:claude
        (
            &["session hygiene"],
            ScheduledCommand {
                display: "session hygiene",
                args: &["session", "hygiene"],
                hook_allowed: false,
            },
        ),
        (
            &["queue gc"],
            ScheduledCommand {
                display: "queue gc",
                args: &["queue", "gc"],
                hook_allowed: true,
            },
        ),
        (
            &["notify check"],
            ScheduledCommand {
                display: "notify check",
                args: &["notify", "check"],
                hook_allowed: true,
            },
        ),
        (
            &["doctor"],
            ScheduledCommand {
                display: "doctor",
                args: &["doctor"],
                hook_allowed: false,
            },
        ),
        // A GATING doctor run: unlike bare `doctor`, this exits non-zero when
        // the category has findings, which is what lets a substrate job carry a
        // failure into CronJobFailed and on to a seat.
        //
        // Spelled out rather than parsed because `args` is a &'static slice, so
        // a category cannot be threaded through without making it owned. That
        // is a real change to this struct and every entry above, and it is the
        // general fix — one entry per gated category is the same hand-
        // enumeration shape already filed against the doctor category list.
        // Recorded here so the next person adding a gated category sees the
        // choice rather than just copying the line.
        // trace:STORY-1422 | ai:claude
        (
            &["doctor check performance --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check performance --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "performance",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        (
            &["fetch --code-only"],
            ScheduledCommand {
                display: "fetch --code-only",
                args: &["fetch", "--code-only", "--quiet"],
                hook_allowed: false,
            },
        ),
        (
            &["store compact", "store gc"],
            ScheduledCommand {
                display: "store compact",
                args: &["store", "compact"],
                hook_allowed: false,
            },
        ),
        // STORY-1367's three first jobs: named gated `doctor check` categories,
        // same GATING shape as `doctor check performance --fail-on-findings`
        // above — a plain `doctor check <category>` stays report-only, so
        // `--fail-on-findings` is what turns a cadence check into a job the
        // tick can actually route (non-zero exit → CronJobFailed → a due seat
        // item). All three are silent on a clean run: `scan_remote_drift`
        // returns nothing with fewer than two configured remotes,
        // `scan_stale_remote_branches` produces no finding for an
        // Excluded-verdict branch, and `scan_disk_headroom` returns nothing
        // above its floor.
        // trace:STORY-1367 | ai:claude
        (
            &["doctor check remote-drift --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check remote-drift --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "remote-drift",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        (
            &["doctor check stale-remote-branches --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check stale-remote-branches --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "stale-remote-branches",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        // STORY-1462: the runaway-seat watchdog, same gating shape.
        // trace:STORY-1462 | ai:claude
        (
            &["doctor check runaway-seats --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check runaway-seats --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "runaway-seats",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        (
            &["doctor check disk-headroom --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check disk-headroom --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "disk-headroom",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        // trace:TASK-2347 | ai:codex — host snapshot reader, never a model timer.
        (
            &["doctor check codex-mail-wake --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check codex-mail-wake --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "codex-mail-wake",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        // trace:TASK-1222 | ai:codex — dry-run only; never allow --apply.
        (
            &["worktree gc --fail-on-findings"],
            ScheduledCommand {
                display: "worktree gc --fail-on-findings",
                args: &["worktree", "gc", "--json", "--fail-on-findings"],
                hook_allowed: false,
            },
        ),
        // STORY-1700: the drive-mandate tick. Model-free and grant-free; a
        // no-op unless a drive mandate is active (the job's `when` gate).
        // trace:STORY-1700 | ai:claude
        (
            &["fleet drive --tick"],
            ScheduledCommand {
                display: "fleet drive --tick",
                args: &["fleet", "drive", "--tick"],
                hook_allowed: false,
            },
        ),
        // STORY-1917: the typed dispatch substrate tick. Executes only `take`
        // and `impl` requests (leaving `review`, `flow`, and `train` Pending
        // for `aida dispatch serve`), never on the per-turn hook path.
        // trace:STORY-1917 | ai:agy
        (
            &["dispatch tick"],
            ScheduledCommand {
                display: "dispatch tick",
                args: &["dispatch", "tick"],
                hook_allowed: false,
            },
        ),
        // STORY-2220: the background docs sweep. Off the per-turn hook path:
        // it walks git history and, in PR mode, calls the forge.
        // trace:STORY-2220 | ai:claude
        (
            &["docs sweep"],
            ScheduledCommand {
                display: "docs sweep",
                args: &["docs", "sweep"],
                hook_allowed: false,
            },
        ),
        // STORY-1699: relay stall guard — store-free, silent unless a relay
        // run is blocked/exited without a `.done` acknowledgement.
        // trace:STORY-1699 | ai:claude
        (
            &["doctor check relay-stall --fail-on-findings"],
            ScheduledCommand {
                display: "doctor check relay-stall --fail-on-findings",
                args: &[
                    "doctor",
                    "check",
                    "relay-stall",
                    "--json",
                    "--fail-on-findings",
                ],
                hook_allowed: false,
            },
        ),
        // STORY-1688: lane liveness substrate job — checks lane process/log
        // liveness, review pickup, gate waits, and orphaned work leases, and
        // routes its own findings, so it exits 0 rather than failing as a gate.
        // Off the per-turn hook path: it calls the forge.
        // trace:STORY-1688 | ai:claude
        (
            &["doctor check lane-liveness", "lane-liveness"],
            ScheduledCommand {
                display: "doctor check lane-liveness",
                args: &["doctor", "check", "lane-liveness", "--json"],
                hook_allowed: false,
            },
        ),
        // STORY-HOME-128: the mail-backlog guard nudges each overdue
        // recipient through its own wake path and records the nudge, so like
        // lane-liveness it exits 0 rather than failing as a gate.
        // trace:STORY-HOME-128 | ai:claude
        (
            &["doctor check mail-backlog", "mail-backlog"],
            ScheduledCommand {
                display: "doctor check mail-backlog",
                args: &["doctor", "check", "mail-backlog", "--json"],
                hook_allowed: false,
            },
        ),
        // BUG-1746: mirror fan-out runs on a substrate cadence, independent of
        // which seat made the store writes; keep it off the per-turn hook path.
        // trace:BUG-1746 | ai:codex
        (
            &["remote mirror-sync"],
            ScheduledCommand {
                display: "remote mirror-sync",
                args: &["remote", "mirror-sync", "--json"],
                hook_allowed: false,
            },
        ),
        // STORY-1218: the night-shift tick. Never on the per-turn hook path:
        // a hook must not be able to launch a drain. The tick itself is a
        // no-op unless this clone's local layer enables it.
        // trace:STORY-1218 | ai:claude
        (
            &["shift tick"],
            ScheduledCommand {
                display: "shift tick",
                args: &["shift", "tick"],
                hook_allowed: false,
            },
        ),
        // STORY-1689: promote the live dev binary when main is green. The
        // tick only decides and launches; the build runs detached, outside
        // this child's timeout. It fetches and reads the forge, so never on
        // the per-turn hook path.
        // trace:STORY-1689 | ai:claude
        (
            &["dev build --if-main-green"],
            ScheduledCommand {
                display: "dev build --if-main-green",
                args: &["dev", "build", "--if-main-green"],
                hook_allowed: false,
            },
        ),
        // The hub-bridge reconcile. Never `--yes`: identity-bearing content
        // stops the run and the failure routes to the advisor. Only a job
        // from this clone's machine-local layer may run it (`build_task`).
        // trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
        (
            &[crate::store_local::BRIDGE_COMMAND],
            ScheduledCommand {
                display: crate::store_local::BRIDGE_COMMAND,
                args: &["remote", "reconcile", "--execute"],
                hook_allowed: false,
            },
        ),
    ]
}

/// STORY-1218: every registered job (project + global layer) whose command
/// is `command`, as `(name, enabled)`. Used by the night-shift guards (is the
/// watchdog job running?) and by `aida shift enable` (is the tick job
/// registered?). A registry that fails to parse reads as no jobs.
// trace:STORY-1218 | ai:claude
pub(crate) fn jobs_running_command(project_root: &Path, command: &str) -> Vec<(String, bool)> {
    let Ok(Some(config)) = load_registry(project_root) else {
        return Vec::new();
    };
    config
        .tasks
        .iter()
        .filter(|t| t.command.as_ref().is_some_and(|c| c.display == command))
        .map(|t| (t.name.clone(), t.enabled))
        .collect()
}

fn parse_scheduled_command(s: &str) -> Result<ScheduledCommand> {
    let normalized = s.split_whitespace().collect::<Vec<_>>().join(" ");
    for (matches, command) in command_table() {
        if matches.contains(&normalized.as_str()) {
            return Ok(command.clone());
        }
    }
    anyhow::bail!(
        "unknown scheduled task command '{}'; valid commands: {}",
        s,
        valid_commands().join(", ")
    )
}

fn valid_commands() -> Vec<&'static str> {
    command_table()
        .iter()
        .flat_map(|(matches, _)| matches.iter().copied())
        .collect()
}

/// Review follow-up: the wall-clock ceiling a single scheduled substrate
/// child (`aida doctor check ...`, `aida fetch --code-only`, …) may run
/// before [`crate::command_output_with_timeout`] gives up on it. `run_now`
/// holds the tick lock (`try_tick_lock`) for the duration of the run, so an
/// unbounded child — a stalled network call inside `doctor check
/// remote-drift`, a hung `git fetch` — would wedge every other job behind it
/// indefinitely. Generous rather than tight: these are periodic maintenance
/// checks, not a latency-sensitive per-turn hook path (that path already
/// skips network-touching jobs — see `tick`'s `hook` flag).
pub(crate) const SCHEDULED_COMMAND_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(120);

/// Exit status a killed-at-timeout (or unspawnable) scheduled child reports.
pub(crate) const TIMEOUT_EXIT_STATUS: i32 = 124;
/// How a timed-out run's `CronJobFailed.error` starts, so a routed failure
/// can say "did not finish" instead of presenting it as a finding.
// trace:BUG-1655 | ai:claude
const TIMEOUT_EXIT_PREFIX: &str = "exit 124: timed out:";
// trace:TASK-1535 | ai:codex
const SPAWN_FAILED_EXIT_PREFIX: &str = "exit 124: could not be spawned:";

/// STORY-2351: `session hygiene` reaps, gcs and reclaims in one child, and a
/// reap scan alone can take most of [`SCHEDULED_COMMAND_TIMEOUT`] on a busy
/// host. It gets a longer ceiling; its own soft budget
/// ([`crate::session_hygiene::SOFT_BUDGET`]) defers the reclaim well before
/// this kill, so a slow run reports instead of routing a timeout.
// trace:STORY-2351 | ai:claude
pub(crate) const HYGIENE_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The kill ceiling for one scheduled child.
// trace:STORY-2351 | ai:claude
fn command_timeout(command: &ScheduledCommand) -> std::time::Duration {
    // trace:TASK-1222 | ai:codex — lifecycle scans emit partial counts before this ceiling.
    if matches!(
        command.display,
        "session hygiene" | "worktree gc --fail-on-findings"
    ) {
        HYGIENE_COMMAND_TIMEOUT
    } else {
        SCHEDULED_COMMAND_TIMEOUT
    }
}

fn run_aida_command(project_root: &Path, command: &ScheduledCommand) -> Result<TaskOutcome> {
    let mut cmd = ProcessCommand::new(crate::aida_exe_path());
    cmd.args(command.args);
    Ok(run_with_kill_timeout(
        cmd,
        command.display,
        project_root,
        command_timeout(command),
    ))
}

/// The timeout-wrapped child run, factored out of [`run_aida_command`] so a
/// test can point `cmd` at a hanging fake binary (e.g. `sleep 30`) through
/// the exact same path production uses, rather than only exercising
/// [`crate::command_output_with_timeout`] in isolation.
fn run_with_kill_timeout(
    mut cmd: ProcessCommand,
    display: &str,
    project_root: &Path,
    timeout: std::time::Duration,
) -> TaskOutcome {
    cmd.current_dir(project_root)
        .env("AIDA_SCHEDULE_CHILD", "1")
        // Never let a scheduled child block on a credential prompt nobody is
        // there to answer — same convention as the other unattended git legs
        // (`fetch --code-only`).
        .env("GIT_TERMINAL_PROMPT", "0");
    // BUG-1288's bounded runner: a portable kill-on-timeout wait, reused
    // rather than reimplemented. Spawn failure and timeout both map to exit
    // 124 (the conventional `timeout(1)` sentinel) —
    // non-zero, so the existing tick machinery records it as a FAILURE
    // (`record_outcome_local`/`failure_trip`), never as ok. PRIN-5: a result
    // we could not observe must never be reported as the ok/success case.
    // trace:TASK-1535 | ai:codex
    match crate::command_output_with_timeout_detail(cmd, timeout) {
        crate::BoundedCommandOutput::Completed(output) => TaskOutcome {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        crate::BoundedCommandOutput::TimedOut => TaskOutcome {
            status: TIMEOUT_EXIT_STATUS,
            stdout: String::new(),
            stderr: format!(
                "timed out: {display} did not complete within {}s (killed)",
                timeout.as_secs()
            ),
        },
        crate::BoundedCommandOutput::SpawnFailed => TaskOutcome {
            status: TIMEOUT_EXIT_STATUS,
            stdout: String::new(),
            stderr: format!("could not be spawned: {display}"),
        },
    }
}

fn quiet_now(task: &Task) -> bool {
    task.quiet_hours
        .is_some_and(|quiet| quiet.contains(Local::now().time()))
}

/// Record the local mirror plus any failure log/event. The caller stages the
/// store ledger separately so a tick can batch every job into one commit.
// trace:TASK-1280 | ai:codex
fn record_outcome_local(
    project_root: &Path,
    state: &mut ScheduleState,
    task: &Task,
    now: DateTime<Utc>,
    outcome: &TaskOutcome,
    trip: Option<&schedule_ledger::FailureTrip>,
) {
    let entry = state.tasks.entry(task.name.clone()).or_default();
    entry.last_run_at = Some(now);
    entry.last_status = Some(outcome.status);
    if outcome.status == 0 {
        entry.last_success_at = Some(now);
    } else {
        if let Err(err) = append_log(project_root, task, outcome, trip) {
            eprintln!("warning: could not append schedule failure log: {err}");
        }
        events::emit(
            project_root,
            &Event::new(
                None,
                "",
                EventKind::CronJobFailed {
                    job: task.name.clone(),
                    seat: task.seats.join(","),
                    error: trip.and_then(|t| t.audit_error.as_deref()).map_or_else(
                        || routed_failure_error(outcome.status, &outcome.stderr),
                        |error| {
                            format!(
                                "exit {}: performance audit unavailable: {error}",
                                outcome.status
                            )
                        },
                    ),
                    trip_id: trip.map(|t| t.trip_id.clone()),
                    performance: trip.map(|t| t.performance.clone()).unwrap_or_default(),
                },
            ),
        );
    }
}

/// The error a failed job's `CronJobFailed` carries: its exit status and
/// its last stderr line.
// trace:STORY-1689 | ai:claude
pub(crate) fn routed_failure_error(status: i32, stderr: &str) -> String {
    format!(
        "exit {status}: {}",
        stderr.trim().lines().last().unwrap_or("")
    )
}

// trace:TASK-1280 | ai:codex
fn apply_outcome_ledger(
    ledger: &mut JobLedger,
    now: DateTime<Utc>,
    outcome: &TaskOutcome,
    trip: Option<&schedule_ledger::FailureTrip>,
) {
    let result = if outcome.status == 0 {
        "ok".to_string()
    } else {
        format!("failed:{}", outcome.status)
    };
    let by = LastBy {
        seat: "substrate".to_string(),
        session: std::env::var("AIDA_SESSION_ID")
            .ok()
            .filter(|s| !s.is_empty()),
        vendor: Some("tick".to_string()),
    };
    ledger.last_run = Some(now);
    ledger.last_by = Some(by);
    ledger.result = Some(result);
    if let Some(trip) = trip {
        ledger.failure_trips.push(trip.clone());
        let excess = ledger
            .failure_trips
            .len()
            .saturating_sub(schedule_ledger::MAX_FAILURE_TRIPS);
        if excess > 0 {
            ledger.failure_trips.drain(..excess);
        }
    }
}

#[derive(Deserialize)]
struct PerformanceAuditEnvelope {
    #[serde(default)]
    performance_audits: Vec<schedule_ledger::PerformanceAudit>,
}

fn failure_trip(
    task: &Task,
    now: DateTime<Utc>,
    outcome: &TaskOutcome,
) -> Option<schedule_ledger::FailureTrip> {
    if outcome.status == 0 {
        return None;
    }
    let trip_id = format!(
        "{}@{}",
        task.name,
        now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
    );
    let is_performance = task
        .command
        .as_ref()
        .is_some_and(|c| c.display == "doctor check performance --fail-on-findings");
    let (performance, audit_error) = if is_performance {
        match serde_json::from_str::<PerformanceAuditEnvelope>(&outcome.stdout) {
            Ok(envelope) if !envelope.performance_audits.is_empty() => (
                envelope
                    .performance_audits
                    .into_iter()
                    .take(schedule_ledger::MAX_PERFORMANCE_AUDITS)
                    .collect(),
                None,
            ),
            Ok(_) => (
                Vec::new(),
                Some("typed doctor output contained no performance_audits".into()),
            ),
            Err(err) => (
                Vec::new(),
                Some(format!("malformed typed doctor output: {err}")),
            ),
        }
    } else {
        (Vec::new(), None)
    };
    Some(schedule_ledger::FailureTrip {
        trip_id,
        at: now,
        status: outcome.status,
        performance,
        audit_error,
    })
}

// trace:BUG-1745 | ai:codex
#[cfg(test)]
pub(super) fn bug_1745_performance_argv() -> &'static [&'static str] {
    command_table()
        .iter()
        .find(|(key, _)| *key == ["doctor check performance --fail-on-findings"])
        .map(|(_, command)| command.args)
        .expect("performance failure command is registered")
}

// trace:BUG-1745 | ai:codex
#[cfg(test)]
pub(super) fn bug_1745_failure_trip(
    stdout: String,
    status: i32,
) -> Option<schedule_ledger::FailureTrip> {
    let command = command_table()
        .iter()
        .find(|(key, _)| *key == ["doctor check performance --fail-on-findings"])
        .map(|(_, command)| command.clone())
        .expect("performance failure command is registered");
    let task = Task {
        name: "bug-1745-test".into(),
        kind: JobKind::Substrate,
        seats: Vec::new(),
        command: Some(command),
        prompt: None,
        interval: None,
        on: Vec::new(),
        when: None,
        when_raw: None,
        while_: None,
        while_raw: None,
        quiet_hours: None,
        enabled: true,
        source: JobSource::Project,
        problem: None,
        invalid_command: None,
    };
    failure_trip(
        &task,
        Utc::now(),
        &TaskOutcome {
            status,
            stdout,
            stderr: String::new(),
        },
    )
}

fn try_tick_lock(project_root: &Path) -> Result<Option<std::fs::File>> {
    use aida_core::file_lock::is_lock_contended;
    use fs2::FileExt;

    let dir = project_root.join(".aida");
    std::fs::create_dir_all(&dir)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join("schedule-tick.lock"))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(err) if is_lock_contended(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("schedule-state.json")
}

fn log_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("schedule.log")
}

fn load_state(project_root: &Path) -> Result<ScheduleState> {
    let path = state_path(project_root);
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Ok(ScheduleState::default());
    };
    serde_json::from_str(&body).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_state(project_root: &Path, state: &ScheduleState) -> Result<()> {
    let path = state_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(state)?;
    aida_core::write_atomic(&path, &body)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn append_log(
    project_root: &Path,
    task: &Task,
    outcome: &TaskOutcome,
    trip: Option<&schedule_ledger::FailureTrip>,
) -> Result<()> {
    use std::io::Write;
    let path = log_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(
        file,
        "[{}] task={} command={} status={}{} stderr={}",
        Utc::now().to_rfc3339(),
        task.name,
        task.command.as_ref().map(|c| c.display).unwrap_or("-"),
        outcome.status,
        trip.map(|trip| {
            let evidence = trip
                .performance
                .iter()
                .map(|p| {
                    format!(
                        "{}:{:.3}%/{},budget={}ms,tolerance={:.3}%,worst={}ms",
                        p.command,
                        p.proportion_millipercent as f64 / 1000.0,
                        p.denominator,
                        p.budget_ms,
                        p.tolerated_millipercent as f64 / 1000.0,
                        p.worst_ms.map_or_else(|| "n/a".into(), |v| v.to_string()),
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            format!(" trip_id={} performance=[{}]", trip.trip_id, evidence)
        })
        .unwrap_or_default(),
        outcome.stderr.trim()
    )?;
    Ok(())
}

/// `30m` / `2h` / `1d` / `1w` / `90s` → chrono duration. Shared with the
/// predicate grammar's duration literals.
// trace:STORY-1226 | ai:claude
pub(crate) fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty duration");
    }
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| anyhow::anyhow!("duration '{s}' has no unit"))?;
    if split == 0 {
        anyhow::bail!("duration '{s}' has no leading number");
    }
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse()?;
    if n <= 0 {
        anyhow::bail!("duration '{s}' must be positive");
    }
    match unit {
        "s" => Ok(Duration::seconds(n)),
        "m" => Ok(Duration::minutes(n)),
        "h" => Ok(Duration::hours(n)),
        "d" => Ok(Duration::days(n)),
        "w" => Ok(Duration::weeks(n)),
        _ => anyhow::bail!("duration '{s}' has unknown unit '{unit}'"),
    }
}

fn parse_quiet_hours(s: &str) -> Result<QuietHours> {
    let (start, end) = s
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("quiet-hours must be HH:MM-HH:MM"))?;
    Ok(QuietHours {
        start: NaiveTime::parse_from_str(start.trim(), "%H:%M")?,
        end: NaiveTime::parse_from_str(end.trim(), "%H:%M")?,
    })
}

fn format_duration(duration: Duration) -> String {
    if duration.num_weeks() > 0 && duration == Duration::weeks(duration.num_weeks()) {
        format!("{}w", duration.num_weeks())
    } else if duration.num_days() > 0 && duration == Duration::days(duration.num_days()) {
        format!("{}d", duration.num_days())
    } else if duration.num_hours() > 0 && duration == Duration::hours(duration.num_hours()) {
        format!("{}h", duration.num_hours())
    } else if duration.num_minutes() > 0 && duration == Duration::minutes(duration.num_minutes()) {
        format!("{}m", duration.num_minutes())
    } else {
        format!("{}s", duration.num_seconds())
    }
}

/// Coarse human age (`47m`, `3h`, `2d`) for due-lines.
fn human_age(d: Duration) -> String {
    let d = if d < Duration::zero() {
        Duration::zero()
    } else {
        d
    };
    if d.num_days() >= 1 {
        format!("{}d", d.num_days())
    } else if d.num_hours() >= 1 {
        format!("{}h", d.num_hours())
    } else if d.num_minutes() >= 1 {
        format!("{}m", d.num_minutes())
    } else {
        format!("{}s", d.num_seconds())
    }
}

fn cron_interval(duration: Duration) -> &'static str {
    if duration <= Duration::hours(1) {
        "0 * * * *"
    } else if duration <= Duration::days(1) {
        "0 3 * * *"
    } else if duration <= Duration::weeks(1) {
        "0 3 * * 0"
    } else {
        "0 3 1 * *"
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
#[path = "tests/maintenance_schedule_tests.rs"]
mod tests;

/// Per-seat job registry: substrate jobs run by the tick, seat jobs
/// delivered to whoever holds the seat (alias: `aida cron`).
// trace:STORY-1047 | ai:codex
// trace:STORY-1226 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "schedule")]
#[clap(visible_alias = "cron")]
pub(crate) struct ScheduleArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::MaintenanceScheduleCommand,
}

fn run_schedule(
    matches: &clap::ArgMatches,
    ctx: &crate::cmdkit::DispatchCtx,
) -> anyhow::Result<crate::cmdkit::Flow> {
    use crate::*;
    let args = <ScheduleArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    match &ctx.store {
        None => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Distributed { .. }) => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match &args {
                ScheduleArgs {
                    inner: schedule_cmd,
                } => {
                    return crate::cmdkit::returned(self::handle_schedule_command(
                        schedule_cmd,
                        store_path,
                        Some(backend),
                    ));
                }
            }
            Ok(crate::cmdkit::Flow::Continue)
        }
        Some(crate::cmdkit::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match &args {
                ScheduleArgs { inner: _ } => {
                    anyhow::bail!(
                        "`aida schedule` is available in the default (distributed) mode."
                    );
                }
            }
            Ok(crate::cmdkit::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static SCHEDULE: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "schedule",
    help_after: "store",
    command: <ScheduleArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Store(run_schedule),
};
