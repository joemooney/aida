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
    #[serde(default)]
    on: Vec<String>,
    /// Typed predicate over the substrate snapshot.
    #[serde(default)]
    when: Option<String>,
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
}

impl JobSource {
    fn as_str(self) -> &'static str {
        match self {
            JobSource::Project => "project",
            JobSource::Global => "global",
            JobSource::Legacy => "legacy",
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
    on: Vec<String>,
    when: Option<Expr>,
    when_raw: Option<String>,
    quiet_hours: Option<QuietHours>,
    enabled: bool,
    pub source: JobSource,
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
            parts.push(format!("on {}", self.on.join(",")));
        }
        if let Some(w) = &self.when_raw {
            parts.push(format!("when {w}"));
        }
        parts.join(" + ")
    }

    fn what(&self) -> String {
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
            let outcome = tick(project_root, *hook, backend)?;
            if !outcome.is_empty() {
                println!("{}", outcome.join("\n"));
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
    let command = match kind {
        JobKind::Substrate => {
            let raw = job
                .command
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("substrate job '{name}' needs a `command`"))?;
            Some(parse_scheduled_command(raw)?)
        }
        _ => None,
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
    for kind_name in &job.on {
        if !EventKind::known_names().contains(&kind_name.as_str()) {
            anyhow::bail!(
                "scheduled job '{name}': unknown event '{kind_name}' in `on`; valid events: {}",
                EventKind::known_names().join(", ")
            );
        }
    }
    let when = job
        .when
        .as_deref()
        .map(|w| {
            schedule_predicate::parse(w)
                .with_context(|| format!("scheduled job '{name}': invalid `when` predicate"))
        })
        .transpose()?;
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
        on: job.on,
        when,
        when_raw: job.when,
        quiet_hours: job
            .quiet_hours
            .as_deref()
            .map(parse_quiet_hours)
            .transpose()?,
        enabled: job.enabled,
        source,
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
            return Some(PathBuf::from(p));
        }
    }
    dirs::home_dir()
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
    Ok(merge_registries(project, global))
}

fn store_root(project_root: &Path) -> PathBuf {
    project_root.join(".aida-store")
}

// ---------------------------------------------------------------------------
// tick / run
// ---------------------------------------------------------------------------

fn tick(
    project_root: &Path,
    hook: bool,
    backend: Option<&aida_core::CachedGitBackend>,
) -> Result<Vec<String>> {
    // Hooks from two sessions can arrive together. Only one may inspect and
    // advance schedule cursors at a time; a hook never waits on its peer.
    // trace:TASK-1281 | ai:codex
    let Some(_tick_lock) = try_tick_lock(project_root)? else {
        return Ok(vec![
            "schedule tick: another tick is already running".to_string()
        ]);
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
        return Ok(if hook {
            Vec::new()
        } else {
            crate::sweep_orphaned_reviews(project_root)
        });
    };
    let mut state = load_state(project_root)?;
    let now = Utc::now();
    if let Some(last_tick) = state.last_tick_at {
        let min_gap = parse_duration(&config.min_gap)
            .with_context(|| format!("invalid [schedule] min_gap '{}'", config.min_gap))?;
        if now.signed_duration_since(last_tick) < min_gap {
            return Ok(vec![format!(
                "schedule tick: suppressed by min-gap {}",
                config.min_gap
            )]);
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
    Ok(recovery_lines)
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
        .filter_map(|t| t.when.as_ref())
        .flat_map(|e| e.fields())
        .collect();
    let snap = if wanted.is_empty() {
        None
    } else {
        Some(snapshot(&wanted))
    };

    for task in &config.tasks {
        if !task.enabled || task.kind == JobKind::FiresTask {
            continue;
        }
        if hook && task.command.as_ref().is_some_and(|c| !c.hook_allowed) {
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
            if interval_due(last_run, interval, now) {
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
                if !task.on.iter().any(|k| k == ev.kind.name()) {
                    continue;
                }
                if cursor.is_some_and(|c| ev.ts <= c) {
                    continue;
                }
                if newest.is_none_or(|n| ev.ts > n) {
                    newest = Some(ev.ts);
                    kind_hit = Some(ev.kind.name().to_string());
                    due_failure = match &ev.kind {
                        EventKind::CronJobFailed {
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
                let Some(command) = &task.command else {
                    continue;
                };
                let outcome = exec(project_root, command).unwrap_or_else(|e| TaskOutcome {
                    status: 1,
                    stdout: String::new(),
                    stderr: e.to_string(),
                });
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
                let Some(command) = &task.command else {
                    continue;
                };
                let outcome = exec(project_root, command).unwrap_or_else(|e| TaskOutcome {
                    status: 1,
                    stdout: String::new(),
                    stderr: e.to_string(),
                });
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
fn done(project_root: &Path, job: &str, note: Option<&str>) -> Result<()> {
    let config = load_registry(project_root)?;
    let task = config
        .as_ref()
        .and_then(|c| c.tasks.iter().find(|t| t.name == job))
        .cloned();
    if task.is_none() {
        anyhow::bail!("no scheduled job named '{job}' (see `aida schedule list`)");
    }
    let now = Utc::now();
    let by = reporter(task.as_ref());
    let store = store_root(project_root);
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
    let status = if !task.enabled {
        "disabled"
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
    }
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
            quiet_hours: None,
            enabled: s.enabled,
            source: JobSource::Legacy,
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
    let state = load_state(project_root)?;
    let ledgers = schedule_ledger::load_all(&store_root(project_root));
    let now = Utc::now();
    let rows: Vec<StatusRow> = tasks
        .iter()
        .map(|t| status_row(t, ledgers.get(&t.name), state.tasks.get(&t.name), now))
        .collect();
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
    let state = load_state(project_root)?;
    let ledgers = schedule_ledger::load_all(&store_root(project_root));
    let now = Utc::now();
    let rows: Vec<StatusRow> = tasks
        .iter()
        .map(|t| status_row(t, ledgers.get(&t.name), state.tasks.get(&t.name), now))
        .collect();
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
    if rows.iter().any(|r| r.kind == "fires_task") {
        println!(
            "\nnote: `fires_task` rows come from .aida/schedules.toml (deprecated); \
             `aida pull` still fires them. Move them to a seat job in [schedule]."
        );
    }
    Ok(())
}

/// Due jobs, file-only (config parse + ledger/state reads; no git, no
/// network). Seat jobs are filtered to `seat` when given; substrate jobs are
/// included only when no seat filter is given (`include_substrate`).
// trace:STORY-1226 | ai:claude
fn collect_due(project_root: &Path, seat: Option<&str>, include_substrate: bool) -> Vec<DueJob> {
    let Ok(Some(config)) = load_registry(project_root) else {
        return Vec::new();
    };
    let state = load_state(project_root).unwrap_or_default();
    let ledgers = schedule_ledger::load_all(&store_root(project_root));
    let now = Utc::now();
    let mut out = Vec::new();
    for task in &config.tasks {
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
        let Some(interval) = task.interval else {
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
        // (see docs/cli-format-json-audit.md, BUG-1502's capability gate) — it
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
    let aida_exe = crate::aida_exe_path();
    let aida_exe = aida_exe.canonicalize().unwrap_or(aida_exe);
    build_tick_cron_line(&repo, &aida_exe)
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
        let Some(interval) = task.interval else {
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
    if !scheduler_driver_check_needed(enabled, !overdue.is_empty(), timer_file) {
        return Ok(Vec::new());
    }
    let status = crate::schedule_driver::driver_status(project_root);
    Ok(build_scheduler_driver_findings(
        enabled,
        &status,
        &overdue,
        Utc::now(),
    ))
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
    Ok(())
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
        let watermarks =
            crate::mailbox_store::read_all_watermarks(project_root).unwrap_or_default();
        // trace:TASK-1492 | ai:claude — the same per-recipient read the night
        // shift's mail-latency escalation uses.
        let per_recipient = crate::mailbox_store::unread_by_recipient(&merged, &watermarks);
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
        (
            &["cache verify"],
            ScheduledCommand {
                display: "cache verify",
                args: &["cache", "verify"],
                hook_allowed: true,
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
const SCHEDULED_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

fn run_aida_command(project_root: &Path, command: &ScheduledCommand) -> Result<TaskOutcome> {
    let mut cmd = ProcessCommand::new(crate::aida_exe_path());
    cmd.args(command.args);
    Ok(run_with_kill_timeout(
        cmd,
        command.display,
        project_root,
        SCHEDULED_COMMAND_TIMEOUT,
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
    // BUG-1288's `command_output_with_timeout`: a portable kill-on-timeout
    // wait, reused rather than reimplemented. `None` (spawn failure OR
    // timeout) maps to exit 124 (the conventional `timeout(1)` sentinel) —
    // non-zero, so the existing tick machinery records it as a FAILURE
    // (`record_outcome_local`/`failure_trip`), never as ok. PRIN-5: a result
    // we could not observe must never be reported as the ok/success case.
    match crate::command_output_with_timeout(cmd, timeout) {
        Some(output) => TaskOutcome {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        None => TaskOutcome {
            status: 124,
            stdout: String::new(),
            stderr: format!(
                "{display} did not complete within {}s (killed) or could not be spawned",
                timeout.as_secs()
            ),
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
                        || {
                            format!(
                                "exit {}: {}",
                                outcome.status,
                                outcome.stderr.trim().lines().last().unwrap_or("")
                            )
                        },
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
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 11, hour, 0, 0).unwrap()
    }

    fn task(name: &str, interval: &str, command: &str) -> Task {
        Task {
            name: name.to_string(),
            kind: JobKind::Substrate,
            seats: vec!["*".to_string()],
            command: Some(parse_scheduled_command(command).unwrap()),
            prompt: None,
            interval: Some(parse_duration(interval).unwrap()),
            on: Vec::new(),
            when: None,
            when_raw: None,
            quiet_hours: None,
            enabled: true,
            source: JobSource::Project,
        }
    }

    fn seat_task(name: &str, seats: &[&str], interval: Option<&str>, on: &[&str]) -> Task {
        Task {
            name: name.to_string(),
            kind: JobKind::Seat,
            seats: seats.iter().map(|s| s.to_string()).collect(),
            command: None,
            prompt: Some(format!("do {name}")),
            interval: interval.map(|i| parse_duration(i).unwrap()),
            on: on.iter().map(|s| s.to_string()).collect(),
            when: None,
            when_raw: None,
            quiet_hours: None,
            enabled: true,
            source: JobSource::Project,
        }
    }

    fn config(tasks: Vec<Task>) -> LoadedScheduleConfig {
        LoadedScheduleConfig {
            min_gap: DEFAULT_MIN_GAP.to_string(),
            tasks,
        }
    }

    fn ok_exec(
        seen: Rc<RefCell<Vec<String>>>,
    ) -> impl Fn(&Path, &ScheduledCommand) -> Result<TaskOutcome> {
        move |_root, cmd| {
            seen.borrow_mut().push(cmd.display.to_string());
            Ok(TaskOutcome {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        }
    }

    fn parse_project(toml_body: &str) -> Result<Option<LoadedScheduleConfig>> {
        let parsed: ConfigFile = toml::from_str(toml_body)?;
        match parsed.schedule {
            Some(raw) => build_config(raw, JobSource::Project),
            None => Ok(None),
        }
    }

    // trace:STORY-1218 | ai:claude
    #[test]
    fn schedule_command_table_shift_tick_not_hook_allowed() {
        let cmd = parse_scheduled_command("shift tick").unwrap();
        assert_eq!(cmd.args, &["shift", "tick"]);
        assert!(
            !cmd.hook_allowed,
            "a per-turn hook tick must never be able to launch a drain"
        );
    }

    #[test]
    fn rejects_unknown_commands_with_valid_set() {
        let err = parse_scheduled_command("cache veryfy")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown scheduled task command"));
        assert!(err.contains("cache verify"));
    }

    // Review follow-up: a scheduled child that never exits (a stalled network
    // call inside a `doctor check` substrate job) must be killed at the
    // timeout ceiling rather than left to run `run_now`/`try_tick_lock`
    // indefinitely, and the outcome it produces must read as a FAILURE
    // (PRIN-5: an unobserved result is never reported as ok), never as a
    // silent success. Drives a real hanging `sleep` child through
    // `run_with_kill_timeout` — the exact function `run_aida_command` calls
    // in production — with a timeout far shorter than the sleep duration.
    //
    // unix-only: `sleep`/`sh` as fixtures, and the process-group kill this
    // pins is itself a unix-only mechanism (see `kill_process_group` in
    // lib.rs) — Windows keeps the pre-existing direct-child-only kill.
    #[cfg(unix)]
    #[test]
    fn hanging_command_is_killed_at_the_timeout_and_reported_as_a_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cmd = ProcessCommand::new("sleep");
        cmd.arg("30");
        let started = std::time::Instant::now();
        let outcome = run_with_kill_timeout(
            cmd,
            "sleep 30",
            tmp.path(),
            std::time::Duration::from_millis(300),
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "the 30s sleep must be killed near the 300ms timeout, not waited out; took {elapsed:?}"
        );
        assert_ne!(outcome.status, 0, "a timed-out child must never read as ok");
        assert_eq!(outcome.status, 124, "the conventional timeout(1) sentinel");
        assert!(
            outcome.stderr.contains("did not complete within"),
            "{}",
            outcome.stderr
        );

        // Wire the same outcome through `failure_trip` / `record_outcome_local`
        // the way the real tick does, to pin that a timeout actually trips the
        // job (non-zero status is all `failure_trip` requires) rather than
        // merely LOOKING like a failure in isolation.
        let task = task(
            "hang-guard",
            "1h",
            "doctor check remote-drift --fail-on-findings",
        );
        let trip = failure_trip(&task, at(12), &outcome);
        assert!(trip.is_some(), "a 124 exit must mint a failure trip");
    }

    // Review follow-up: the GRANDCHILD case — a direct child that
    // backgrounds a long-running descendant and then waits on it (`sh -c
    // 'sleep 30 & wait'`, the same shape a credential-manager helper or a
    // backgrounded git op takes) inherits the pipe write ends too. Killing
    // only the direct child (old `Child::kill`-only behavior) would leave
    // that grandchild alive, still holding stdout/stderr open, and the
    // reader threads' `read_to_end` blocked on them — this is exactly the
    // gap `kill_process_group`'s `killpg` closes: `sh` and `sleep` share one
    // process group (`process_group(0)` at spawn), so one SIGKILL reaps
    // both. Must return within about the ceiling plus slack, not anywhere
    // near the 30s the grandchild alone would otherwise run.
    #[cfg(unix)]
    #[test]
    fn hanging_grandchild_is_reaped_via_process_group_kill() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cmd = ProcessCommand::new("sh");
        cmd.arg("-c").arg("sleep 30 & wait");
        let ceiling = std::time::Duration::from_secs(2);
        let started = std::time::Instant::now();
        let outcome = run_with_kill_timeout(cmd, "sh -c 'sleep 30 & wait'", tmp.path(), ceiling);
        let elapsed = started.elapsed();
        assert!(
            elapsed < ceiling + std::time::Duration::from_secs(3),
            "the backgrounded grandchild must be reaped with the parent via killpg, not \
             outlive it and wedge the read; ceiling {ceiling:?}, took {elapsed:?}"
        );
        assert_eq!(
            outcome.status, 124,
            "killed-on-timeout must read as failed, never ok"
        );
    }

    // BUG-1557: every `command = "..."` string the scaffolded config template
    // ships (commented-out examples included — those are exactly the ones a
    // project owner un-comments) must parse through `parse_scheduled_command`,
    // or the job errors on every tick forever while sitting in config looking
    // like a guard.
    //
    // Deliberately ONE-DIRECTIONAL: the allowlist may (and does) contain
    // entries with no scaffolded example — `fetch --code-only`, `store
    // compact`, `store gc` — and that is correct, not a defect. A set-equality
    // check would fail today for those three false reasons and invite the
    // check to be weakened or deleted. Only "scaffolded but unparseable" is a
    // bug, so that is the only direction this test asserts.
    //
    // The extraction is scoped to `[[schedule.jobs]]` blocks specifically
    // (tracking block boundaries line-by-line), NOT a blanket
    // `command *= *"` grep. That is a deliberate exclusion, recorded here
    // rather than discovered as a failure: the leading doc comment in
    // `init_schedule_config_section` contains a literal placeholder,
    // `command = "<allow-listed aida subcommand>"`, that documents the
    // convention rather than declaring a job, and a naive extractor would
    // pick it up and it would never parse.
    // trace:BUG-1557 | ai:claude
    fn scaffolded_job_commands(section: &str) -> Vec<String> {
        let mut commands = Vec::new();
        let mut in_job_block = false;
        for line in section.lines() {
            let stripped = line.trim().trim_start_matches('#').trim();
            if stripped.starts_with("[[") {
                in_job_block = stripped == "[[schedule.jobs]]";
                continue;
            }
            if stripped.starts_with('[') {
                in_job_block = false;
                continue;
            }
            if !in_job_block {
                continue;
            }
            if let Some((key, value)) = stripped.split_once('=') {
                if key.trim() == "command" {
                    if let Some(inner) = value
                        .trim()
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                    {
                        commands.push(inner.to_string());
                    }
                }
            }
        }
        commands
    }

    #[test]
    fn scaffold_extraction_excludes_the_documentation_placeholder() {
        let section = crate::init_cmd::init_schedule_config_section();
        assert!(
            section.contains("<allow-listed aida subcommand>"),
            "the doc placeholder this test guards against moved or was removed; \
             update this test rather than deleting it silently"
        );
        let commands = scaffolded_job_commands(section);
        assert!(
            !commands.iter().any(|c| c.contains("allow-listed")),
            "extraction picked up the documentation placeholder, not a real job: {commands:?}"
        );
    }

    #[test]
    fn every_scaffolded_job_command_parses() {
        let section = crate::init_cmd::init_schedule_config_section();
        let commands = scaffolded_job_commands(section);
        // Sanity floor so a broken extractor (e.g. one that stops matching
        // `[[schedule.jobs]]` blocks entirely) can't silently pass by finding
        // zero commands.
        assert!(
            commands.len() >= 5,
            "expected at least 5 scaffolded `command = ...` examples, found {}: {commands:?}",
            commands.len()
        );
        for command in &commands {
            parse_scheduled_command(command).unwrap_or_else(|e| {
                panic!(
                    "scaffolded example `command = \"{command}\"` does not parse \
                     as an allowlisted scheduled command: {e}"
                )
            });
        }
    }

    #[test]
    fn tick_runs_only_due_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        state.tasks.insert(
            "recent".to_string(),
            TaskState {
                last_run_at: Some(at(11)),
                ..TaskState::default()
            },
        );
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = tick_with_executor(
            tmp.path(),
            config(vec![
                task("due", "1h", "cache verify"),
                task("recent", "24h", "queue gc"),
            ]),
            &mut state,
            at(12),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();
        assert_eq!(&*seen.borrow(), &["cache verify"]);
        assert_eq!(out, vec!["schedule tick: due ok"]);
        // The run is ledgered per job under the store root.
        let ledger = schedule_ledger::load(&store_root(tmp.path()), "due").unwrap();
        assert_eq!(ledger.last_run, Some(at(12)));
        assert_eq!(ledger.result.as_deref(), Some("ok"));
        assert_eq!(
            ledger.last_by.as_ref().map(|b| b.seat.as_str()),
            Some("substrate")
        );
    }

    // trace:TASK-1281 | ai:codex
    #[test]
    fn ledger_failure_does_not_lose_local_run_suppression() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(store_root(tmp.path()), "not a directory").unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = tick_with_executor(
            tmp.path(),
            config(vec![task("cache", "1h", "cache verify")]),
            &mut state,
            at(12),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();
        assert_eq!(out, vec!["schedule tick: cache ok"]);
        assert_eq!(state.tasks["cache"].last_run_at, Some(at(12)));
        assert_eq!(
            load_state(tmp.path()).unwrap().tasks["cache"].last_run_at,
            Some(at(12))
        );
    }

    // trace:TASK-1281 | ai:codex
    #[test]
    fn legacy_fires_task_has_no_run_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = Task {
            name: "legacy".into(),
            kind: JobKind::FiresTask,
            seats: vec!["*".into()],
            command: None,
            prompt: None,
            interval: Some(Duration::hours(1)),
            on: Vec::new(),
            when: None,
            when_raw: None,
            quiet_hours: None,
            enabled: true,
            source: JobSource::Project,
        };
        let mut state = ScheduleState::default();
        let lines = run_with_executor(
            tmp.path(),
            config(vec![legacy]),
            &mut state,
            at(12),
            Some("legacy"),
            |_root, _command| unreachable!(),
        )
        .unwrap();
        assert!(lines.is_empty());
    }

    // trace:TASK-1281 | ai:codex
    // trace:BUG-1595 | ai:claude
    #[test]
    fn tick_lock_is_nonblocking_and_reusable() {
        let tmp = tempfile::tempdir().unwrap();
        let first = try_tick_lock(tmp.path()).unwrap().unwrap();
        assert!(try_tick_lock(tmp.path()).unwrap().is_none());
        drop(first);

        // BUG-1595 (recurrence of BUG-1303): re-locking right after drop can
        // still observe contention under `--test-threads` parallelism, even
        // though this process's own fd for the lock file is closed.
        //
        // Root cause, not a test artifact: `try_tick_lock` takes an
        // fs2 `flock`(2)-style advisory lock, which is owned by the OPEN
        // FILE DESCRIPTION, not by this process's fd. `std::fs::File`
        // already opens with `O_CLOEXEC` (confirmed via strace: the
        // `openat` call carries `O_CLOEXEC`), so that is not the gap.
        // The gap is that `O_CLOEXEC` only takes effect at `execve()` — it
        // does nothing about `fork()` itself. `aida-cli-lib`'s test binary
        // shells out to git constantly (`std::process::Command`, 700+
        // call sites), and every such spawn forks the *whole* process,
        // which briefly duplicates every open fd — including this test's
        // lock fd, if it happens to be open at that instant — into the
        // child. For the short fork()..execve() window the child holds
        // its own reference to the same open file description, so the
        // advisory lock stays held even after `drop(first)` closes our
        // fd. Switching to Linux OFD locks (`fcntl(F_OFD_SETLK)`) would
        // not close this gap either: POSIX defines OFD locks as
        // inherited across `fork()` exactly like `flock()` (a copy of
        // the fd from `fork()` refers to the same open file description
        // and shares its OFD lock). So this window is not a bug in the
        // lock; it is real, unavoidable, and cannot be raced out of the
        // implementation by picking a different lock primitive. The
        // bounded retry below is what BUG-1595 calls for in that case.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let reacquired = loop {
            match try_tick_lock(tmp.path()).unwrap() {
                Some(guard) => break Some(guard),
                None if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                None => break None,
            }
        };
        assert!(
            reacquired.is_some(),
            "lock was not reacquired within 2s of dropping the first guard"
        );
    }

    // A chatty tick advances the store once, regardless of job count.
    // trace:TASK-1280 | ai:codex
    #[test]
    fn tick_batches_three_job_ledgers_into_one_store_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_root(tmp.path());
        std::fs::create_dir_all(&store).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&store)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "AIDA Test"]);
        git(&["config", "user.email", "aida@example.invalid"]);

        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = tick_with_executor(
            tmp.path(),
            config(vec![
                task("one", "1h", "cache verify"),
                task("two", "1h", "queue gc"),
                task("three", "1h", "session reap"),
            ]),
            &mut state,
            at(12),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();

        assert_eq!(out.len(), 3);
        assert_eq!(git(&["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(schedule_ledger::load_all(&store).len(), 3);
    }

    #[test]
    fn min_gap_suppresses_recent_tick() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::env_lock();
        std::env::set_var("AIDA_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            r#"
[schedule]
min_gap = "60s"

[[schedule.tasks]]
name = "cache"
command = "cache verify"
interval = "1h"
enabled = true
"#,
        )
        .unwrap();
        let state = ScheduleState {
            last_tick_at: Some(Utc::now()),
            tasks: BTreeMap::new(),
        };
        save_state(tmp.path(), &state).unwrap();
        let lines = tick(tmp.path(), false, None).unwrap();
        std::env::remove_var("AIDA_HOME");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("suppressed by min-gap"));
    }

    // BUG-1600: `~/.aida/schedule-tick.log` must never grow without bound —
    // a stale/misconfigured cron entry that fails identically every 15
    // minutes must not flood it forever.
    // trace:BUG-1600 | ai:claude
    #[test]
    fn bound_global_schedule_log_leaves_small_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::env_lock();
        std::env::set_var("AIDA_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        let log = tmp.path().join(".aida").join("schedule-tick.log");
        std::fs::write(&log, "small content\n").unwrap();

        bound_global_schedule_log();

        let body = std::fs::read_to_string(&log).unwrap();
        std::env::remove_var("AIDA_HOME");
        assert_eq!(body, "small content\n", "well under the cap → untouched");
    }

    #[test]
    fn bound_global_schedule_log_truncates_when_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::env_lock();
        std::env::set_var("AIDA_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        let log = tmp.path().join(".aida").join("schedule-tick.log");
        // The exact repeated-failure shape BUG-1600 produced: the same
        // error line, over and over, forever.
        let line = "error: `aida schedule tick --format json` is unsupported\n";
        let repeats = (GLOBAL_SCHEDULE_LOG_MAX_BYTES as usize / line.len()) + 100;
        let big = line.repeat(repeats);
        std::fs::write(&log, &big).unwrap();
        let before_len = std::fs::metadata(&log).unwrap().len();
        assert!(
            before_len > GLOBAL_SCHEDULE_LOG_MAX_BYTES,
            "test setup sanity"
        );

        bound_global_schedule_log();

        let after = std::fs::read_to_string(&log).unwrap();
        std::env::remove_var("AIDA_HOME");
        assert!(
            (after.len() as u64) <= GLOBAL_SCHEDULE_LOG_MAX_BYTES,
            "must be at/under the cap after truncation: {} bytes",
            after.len()
        );
        assert!(
            after.contains("truncated"),
            "must leave evidence that truncation happened: {after}"
        );
        // The newest content (the tail of the repeated line) must survive —
        // truncation drops the OLDEST bytes, not the newest.
        assert!(
            after.trim_end().ends_with(line.trim_end()),
            "must keep the newest lines: {after}"
        );
    }

    #[test]
    fn tick_bounds_global_schedule_log_for_timer_but_not_hook_invocation() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::env_lock();
        std::env::set_var("AIDA_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        let log = tmp.path().join(".aida").join("schedule-tick.log");
        let line = "error: unsupported\n";
        let big = line.repeat((GLOBAL_SCHEDULE_LOG_MAX_BYTES as usize / line.len()) + 100);
        let big_len = big.len() as u64;

        // A HOOK tick (the per-turn invoker) leaves the log alone — it's
        // meant to stay minimal, and the installed hook script redirects
        // its own output to /dev/null anyway.
        std::fs::write(&log, &big).unwrap();
        let _ = tick(tmp.path(), true, None).unwrap();
        let after_hook = std::fs::metadata(&log).unwrap().len();
        assert_eq!(
            after_hook, big_len,
            "a hook tick must not touch the global log"
        );

        // A timer/cron-shaped tick (hook = false — the shape the installed
        // crontab entry uses) bounds it.
        let _ = tick(tmp.path(), false, None).unwrap();
        let after_timer = std::fs::metadata(&log).unwrap().len();
        std::env::remove_var("AIDA_HOME");
        assert!(
            after_timer <= GLOBAL_SCHEDULE_LOG_MAX_BYTES,
            "a non-hook (timer/cron) tick must bound the log: {after_timer} bytes"
        );
    }

    #[test]
    fn failing_task_logs_and_later_task_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let seen_exec = Rc::clone(&seen);
        tick_with_executor(
            tmp.path(),
            config(vec![
                task("bad", "1h", "cache verify"),
                task("good", "1h", "queue gc"),
            ]),
            &mut state,
            at(12),
            false,
            move |_root, cmd| {
                seen_exec.borrow_mut().push(cmd.display.to_string());
                Ok(TaskOutcome {
                    status: if cmd.display == "cache verify" { 2 } else { 0 },
                    stdout: String::new(),
                    stderr: "boom".to_string(),
                })
            },
        )
        .unwrap();
        assert_eq!(&*seen.borrow(), &["cache verify", "queue gc"]);
        let log = std::fs::read_to_string(log_path(tmp.path())).unwrap();
        assert!(log.contains("task=bad"));
        assert!(log.contains("boom"));
        let ledger = schedule_ledger::load(&store_root(tmp.path()), "bad").unwrap();
        assert_eq!(ledger.result.as_deref(), Some("failed:2"));
    }

    fn performance_json(denominator: usize) -> String {
        serde_json::json!({
            "total": 1,
            "findings": [],
            "performance_audits": [{
                "command": "show",
                "budget_ms": 1000,
                "over_budget": if denominator == 0 { 0 } else { 19 },
                "denominator": denominator,
                "proportion_millipercent": if denominator == 0 { 0 } else { 19700 },
                "tolerated_millipercent": 10000,
                "window_hours": 24,
                "worst_ms": if denominator == 0 { serde_json::Value::Null } else { serde_json::json!(165672) },
                "excluded_samples": 3,
                "lineage_scoped": true
            }]
        })
        .to_string()
    }

    // trace:BUG-1573 | ai:codex
    #[test]
    fn performance_failure_is_auditable_and_routed_by_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        tick_with_executor(
            tmp.path(),
            config(vec![task(
                "performance-guard",
                "1h",
                "doctor check performance --fail-on-findings",
            )]),
            &mut state,
            at(12),
            false,
            |_root, _cmd| {
                Ok(TaskOutcome {
                    status: 1,
                    stdout: performance_json(96),
                    stderr: "format hint that must not replace evidence".into(),
                })
            },
        )
        .unwrap();

        let ledger = schedule_ledger::load(&store_root(tmp.path()), "performance-guard").unwrap();
        let trip = ledger.failure_trips.last().unwrap();
        let audit = &trip.performance[0];
        assert_eq!(
            (audit.proportion_millipercent, audit.denominator),
            (19_700, 96)
        );
        assert_eq!(
            (audit.budget_ms, audit.tolerated_millipercent),
            (1000, 10_000)
        );
        assert_eq!(audit.worst_ms, Some(165_672));

        let events = events::read_all(tmp.path());
        let routed = events
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::CronJobFailed {
                    trip_id,
                    performance,
                    ..
                } => Some((trip_id.as_deref(), performance)),
                _ => None,
            })
            .unwrap();
        assert_eq!(routed.0, Some(trip.trip_id.as_str()));
        assert_eq!(routed.1, &trip.performance);

        // Consumer-side proof: the routed event makes the advisor job due,
        // and carries enough evidence to perform its prompt without rerunning.
        let mut route_state = ScheduleState::default();
        route_state.tasks.insert(
            "performance-guard-route".into(),
            TaskState {
                last_seen_event_ts: Some(at(11)),
                ..Default::default()
            },
        );
        tick_core(
            tmp.path(),
            config(vec![seat_task(
                "performance-guard-route",
                &["advisor"],
                None,
                &["CronJobFailed"],
            )]),
            &mut route_state,
            at(13),
            false,
            |_root, _cmd| unreachable!(),
            |_| Snapshot::default(),
            &events,
        )
        .unwrap();
        let route =
            schedule_ledger::load(&store_root(tmp.path()), "performance-guard-route").unwrap();
        assert!(route.due_since.is_some());
        assert_eq!(
            route.due_failure.as_ref().map(|f| f.trip_id.as_str()),
            Some(trip.trip_id.as_str())
        );

        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            r#"
[[schedule.jobs]]
name = "performance-guard-route"
seats = ["advisor"]
on = ["CronJobFailed"]
prompt = "decide whether this is a regression"
enabled = true
"#,
        )
        .unwrap();
        let delivered = due_seat_jobs(tmp.path(), Some("advisor"));
        let artifact = render_due_jobs_block(&delivered, "advisor");
        assert!(artifact.contains(&trip.trip_id), "{artifact}");
        assert!(artifact.contains("19.700% over 1000 ms"), "{artifact}");
        assert!(artifact.contains("19 of 96 calls"), "{artifact}");
        assert!(artifact.contains("tolerance 10.000%"), "{artifact}");
        assert!(artifact.contains("worst 165672 ms"), "{artifact}");
        assert!(artifact.contains("lineage_scoped=true"), "{artifact}");

        let log = std::fs::read_to_string(log_path(tmp.path())).unwrap();
        assert!(log.contains("19.700%/96,budget=1000ms,tolerance=10.000%"));
        assert!(!log.contains(&performance_json(96)));
    }

    // trace:BUG-1573 | ai:codex
    #[test]
    fn performance_pass_has_no_failure_context_and_zero_denominator_is_explicit() {
        let tmp = tempfile::tempdir().unwrap();
        let perf = task(
            "performance-guard",
            "1h",
            "doctor check performance --fail-on-findings",
        );
        let mut state = ScheduleState::default();
        run_with_executor(
            tmp.path(),
            config(vec![perf.clone()]),
            &mut state,
            at(12),
            Some("performance-guard"),
            |_root, _cmd| {
                Ok(TaskOutcome {
                    status: 0,
                    stdout: "not parsed on success".into(),
                    stderr: String::new(),
                })
            },
        )
        .unwrap();
        assert!(
            schedule_ledger::load(&store_root(tmp.path()), "performance-guard")
                .unwrap()
                .failure_trips
                .is_empty()
        );

        run_with_executor(
            tmp.path(),
            config(vec![perf]),
            &mut state,
            at(13),
            Some("performance-guard"),
            |_root, _cmd| {
                Ok(TaskOutcome {
                    status: 1,
                    stdout: performance_json(0),
                    stderr: String::new(),
                })
            },
        )
        .unwrap();
        let ledger = schedule_ledger::load(&store_root(tmp.path()), "performance-guard").unwrap();
        let audit = &ledger.failure_trips[0].performance[0];
        assert_eq!(
            (
                audit.denominator,
                audit.proportion_millipercent,
                audit.worst_ms
            ),
            (0, 0, None)
        );
    }

    // STORY-1367: the three first substrate jobs the story registers
    // (hub-drift, stranded-branches, disk-headroom) all ride the same
    // GATING `doctor check <category> --fail-on-findings` shape as
    // performance-guard, with no job-specific evidence parsing — a plain
    // non-zero exit is enough for `failure_trip` to mint a trip_id and for
    // the routing seat job to pick it up. Drives the tick against a fixture
    // where the check trips (asserts each job reports, once, through the
    // existing CronJobFailed → due-seat-job surface) and a clean fixture
    // (asserts silence): acceptance criteria 2, 3 and 5.
    // trace:STORY-1367 | ai:claude
    #[test]
    fn each_new_guard_job_trips_and_routes_on_failure_and_is_silent_when_clean() {
        for command in [
            "doctor check remote-drift --fail-on-findings",
            "doctor check stale-remote-branches --fail-on-findings",
            "doctor check disk-headroom --fail-on-findings",
            "doctor check runaway-seats --fail-on-findings",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let job_name = "guard";
            let guard = task(job_name, "6h", command);

            // Clean fixture: the check finds nothing, exits 0. Silent — no
            // CronJobFailed event, no failure trip.
            let mut state = ScheduleState::default();
            run_with_executor(
                tmp.path(),
                config(vec![guard.clone()]),
                &mut state,
                at(12),
                Some(job_name),
                |_root, _cmd| {
                    Ok(TaskOutcome {
                        status: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    })
                },
            )
            .unwrap();
            assert!(
                schedule_ledger::load(&store_root(tmp.path()), job_name)
                    .unwrap()
                    .failure_trips
                    .is_empty(),
                "{command}: clean run must not trip"
            );
            assert!(
                events::read_all(tmp.path()).is_empty(),
                "{command}: clean run must emit nothing"
            );

            // Failing fixture: the check finds something, exits non-zero.
            // Reports exactly once, through CronJobFailed, and a routing seat
            // job on that event becomes due carrying the trip evidence.
            run_with_executor(
                tmp.path(),
                config(vec![guard]),
                &mut state,
                at(13),
                Some(job_name),
                |_root, _cmd| {
                    Ok(TaskOutcome {
                        status: 1,
                        stdout: String::new(),
                        stderr: "finding(s) detected".into(),
                    })
                },
            )
            .unwrap();
            let ledger = schedule_ledger::load(&store_root(tmp.path()), job_name).unwrap();
            let trip = ledger
                .failure_trips
                .last()
                .unwrap_or_else(|| panic!("{command}: failing run must trip"));

            let events = events::read_all(tmp.path());
            let routed = events
                .iter()
                .find_map(|event| match &event.kind {
                    EventKind::CronJobFailed { trip_id, .. } => trip_id.as_deref(),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{command}: failure must emit CronJobFailed"));
            assert_eq!(routed, trip.trip_id, "{command}");

            // A cold cursor (no prior tick) seeds itself to "now" and skips
            // event replay on its first tick (see the STORY-1226 comment in
            // `tick_core`), so seed a cursor before the failing run's
            // timestamp — same setup `performance_failure_is_auditable_and_routed_by_trip`
            // uses — or the routing job would never see the event it exists
            // to route.
            let mut route_state = ScheduleState::default();
            route_state.tasks.insert(
                "guard-route".into(),
                TaskState {
                    last_seen_event_ts: Some(at(12)),
                    ..Default::default()
                },
            );
            tick_core(
                tmp.path(),
                config(vec![seat_task(
                    "guard-route",
                    &["advisor"],
                    None,
                    &["CronJobFailed"],
                )]),
                &mut route_state,
                at(14),
                false,
                |_root, _cmd| unreachable!(),
                |_| Snapshot::default(),
                &events,
            )
            .unwrap();
            let route = schedule_ledger::load(&store_root(tmp.path()), "guard-route").unwrap();
            assert!(route.due_since.is_some(), "{command}: routing job not due");
            assert_eq!(
                route.due_failure.as_ref().map(|f| f.trip_id.as_str()),
                Some(trip.trip_id.as_str()),
                "{command}"
            );
        }
    }

    // trace:BUG-1573 | ai:codex
    #[test]
    fn malformed_performance_output_fails_visibly_without_persisting_payload() {
        let task = task(
            "performance-guard",
            "1h",
            "doctor check performance --fail-on-findings",
        );
        let outcome = TaskOutcome {
            status: 1,
            stdout: "SECRET noisy payload".into(),
            stderr: String::new(),
        };
        let trip = failure_trip(&task, at(12), &outcome).unwrap();
        assert!(trip
            .audit_error
            .as_deref()
            .unwrap()
            .contains("malformed typed doctor output"));
        assert!(!serde_yaml::to_string(&trip).unwrap().contains("SECRET"));
    }

    // trace:BUG-1573 | ai:codex
    #[test]
    fn failure_history_has_a_deterministic_oldest_first_bound() {
        let task = task("bad", "1h", "cache verify");
        let outcome = TaskOutcome {
            status: 2,
            stdout: String::new(),
            stderr: "boom".into(),
        };
        let mut ledger = JobLedger::new("bad");
        for hour in 0..=schedule_ledger::MAX_FAILURE_TRIPS as u32 {
            let now = at(hour);
            let trip = failure_trip(&task, now, &outcome).unwrap();
            apply_outcome_ledger(&mut ledger, now, &outcome, Some(&trip));
        }
        assert_eq!(
            ledger.failure_trips.len(),
            schedule_ledger::MAX_FAILURE_TRIPS
        );
        assert_eq!(ledger.failure_trips.first().unwrap().at, at(1));
        assert_eq!(ledger.failure_trips.last().unwrap().at, at(20));
    }

    #[test]
    fn emit_cron_uses_valid_crontab_prefixes() {
        assert_eq!(cron_interval(parse_duration("1h").unwrap()), "0 * * * *");
        assert_eq!(cron_interval(parse_duration("24h").unwrap()), "0 3 * * *");
        assert_eq!(cron_interval(parse_duration("7d").unwrap()), "0 3 * * 0");
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn legacy_task_config_still_parses() {
        // The STORY-1047 shape, verbatim: `interval`, `command`, `enabled`,
        // `quiet-hours`, under `[[schedule.tasks]]`.
        let cfg = parse_project(
            r#"
[schedule]
min_gap = "90s"

[[schedule.tasks]]
name = "cache-verify"
command = "cache verify"
interval = "24h"
quiet-hours = "22:00-06:00"
enabled = false
"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(cfg.min_gap, "90s");
        assert_eq!(cfg.tasks.len(), 1);
        let t = &cfg.tasks[0];
        assert_eq!(t.kind, JobKind::Substrate);
        assert_eq!(t.seats, vec!["*".to_string()]);
        assert_eq!(t.interval, Some(Duration::hours(24)));
        assert!(t.quiet_hours.is_some());
        assert!(!t.enabled);
        assert_eq!(t.schedule_summary(), "every 1d");
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn new_job_shape_parses_and_validates() {
        let cfg = parse_project(
            r#"
[schedule]

[[schedule.jobs]]
name = "mailbox-triage"
seats = ["advisor"]
every = "30m"
on = ["MailReceived"]
prompt = "triage the mailbox"
enabled = true

[[schedule.jobs]]
name = "mailbox-latency"
when = "mail.oldest_unread_age > 15m"
command = "notify check"
enabled = true
"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(cfg.tasks.len(), 2);
        let triage = &cfg.tasks[0];
        assert_eq!(triage.kind, JobKind::Seat);
        assert!(triage.applies_to_seat("advisor"));
        assert!(triage.applies_to_seat("dialog"), "legacy alias folds");
        assert!(!triage.applies_to_seat("product"));
        assert_eq!(triage.schedule_summary(), "every 30m + on MailReceived");
        let latency = &cfg.tasks[1];
        assert_eq!(latency.kind, JobKind::Substrate);
        assert!(latency.when.is_some());

        // Validation errors name the job and the fix.
        let err = parse_project(
            r#"
[schedule]
[[schedule.jobs]]
name = "x"
prompt = "p"
on = ["PrMergd"]
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown event 'PrMergd'"), "{err}");
        let err = parse_project(
            r#"
[schedule]
[[schedule.jobs]]
name = "x"
prompt = "p"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("needs a schedule"), "{err}");
        let err = parse_project(
            r#"
[schedule]
[[schedule.jobs]]
name = "x"
every = "1h"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("needs a `command`"), "{err}");
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn merge_registries_project_wins() {
        let project = config(vec![
            task("session-reap", "30m", "session reap"),
            seat_task("mailbox-triage", &["advisor"], Some("30m"), &[]),
        ]);
        let mut global_reap = task("session-reap", "6h", "session reap");
        global_reap.source = JobSource::Global;
        let mut global_doctor = task("doctor", "6h", "doctor");
        global_doctor.source = JobSource::Global;
        let global = LoadedScheduleConfig {
            min_gap: "5m".to_string(),
            tasks: vec![global_reap, global_doctor],
        };
        let merged = merge_registries(Some(project), Some(global)).unwrap();
        assert_eq!(merged.min_gap, DEFAULT_MIN_GAP, "project min_gap wins");
        let names: Vec<&str> = merged.tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["session-reap", "mailbox-triage", "doctor"]);
        let reap = merged
            .tasks
            .iter()
            .find(|t| t.name == "session-reap")
            .unwrap();
        assert_eq!(
            reap.interval,
            Some(Duration::minutes(30)),
            "project entry kept"
        );
        assert_eq!(reap.source, JobSource::Project);
        assert_eq!(
            merged
                .tasks
                .iter()
                .find(|t| t.name == "doctor")
                .unwrap()
                .source,
            JobSource::Global
        );
        // One-sided layers pass through; none → none.
        assert!(merge_registries(None, None).is_none());
        assert_eq!(
            merge_registries(None, Some(config(vec![task("a", "1h", "queue gc")])))
                .unwrap()
                .tasks
                .len(),
            1
        );
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn seat_job_never_executes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let out = tick_with_executor(
            tmp.path(),
            config(vec![seat_task(
                "mailbox-triage",
                &["advisor"],
                Some("30m"),
                &[],
            )]),
            &mut state,
            at(12),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();
        assert!(
            seen.borrow().is_empty(),
            "executor must never see a seat job"
        );
        assert_eq!(
            out,
            vec!["schedule tick: mailbox-triage due (every 30m) → seat advisor"]
        );
        let store = store_root(tmp.path());
        let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
        assert_eq!(ledger.due_since, Some(at(12)));
        assert_eq!(ledger.due_reason.as_deref(), Some("every 30m"));
        assert!(ledger.last_run.is_none(), "due ≠ ran");
        // The event stream carries the nudge.
        let events = events::read_all(tmp.path());
        assert!(events.iter().any(|e| matches!(
            &e.kind,
            EventKind::CronJobFired { job, seat } if job == "mailbox-triage" && seat == "advisor"
        )));
        // A second tick does not re-flag (still due, not re-nudged).
        let out = tick_with_executor(
            tmp.path(),
            config(vec![seat_task(
                "mailbox-triage",
                &["advisor"],
                Some("30m"),
                &[],
            )]),
            &mut state,
            at(13),
            false,
            ok_exec(Rc::clone(&seen)),
        )
        .unwrap();
        assert!(out.is_empty());
        // `done` clears it and ledgers the reporter.
        schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
            l.last_run = Some(at(13));
            l.due_since = None;
            l.due_reason = None;
            l.result = Some("done".into());
        })
        .unwrap();
        let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
        assert!(ledger.due_since.is_none());
        let t = seat_task("mailbox-triage", &["advisor"], Some("30m"), &[]);
        assert!(due_reason(&t, Some(&ledger), None, at(13)).is_none());
        assert_eq!(
            due_reason(&t, Some(&ledger), None, at(14)).as_deref(),
            Some("every 30m")
        );
    }

    // trace:STORY-1226 | ai:claude
    // Advisor review round 1 (#1946). trace:STORY-1226 | ai:claude
    #[test]
    fn on_job_first_sight_initialises_cursor_to_now_without_replaying_history() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mk = |ts: DateTime<Utc>, kind: EventKind| Event {
            ts,
            spec: None,
            run_uuid: String::new(),
            seat: None,
            kind,
        };
        // A week of historical merges before the job ever existed.
        let history = vec![
            mk(at(1), EventKind::PrMerged { pr: 1 }),
            mk(at(5), EventKind::PrMerged { pr: 2 }),
        ];
        let mut job = task("capture-sweep", "24h", "queue gc");
        job.interval = None;
        job.on = vec!["PrMerged".to_string()];
        let run = |state: &mut ScheduleState, now, events: &[Event]| {
            tick_core(
                tmp.path(),
                config(vec![job.clone()]),
                state,
                now,
                false,
                ok_exec(Rc::clone(&seen)),
                |_| Snapshot::default(),
                events,
            )
            .unwrap()
        };
        // First sight: nothing fires, cursor = now.
        let out = run(&mut state, at(10), &history);
        assert!(out.is_empty(), "{out:?}");
        assert!(seen.borrow().is_empty());
        assert_eq!(
            state.tasks["capture-sweep"].last_seen_event_ts,
            Some(at(10))
        );
        // An event after the cursor fires once.
        let mut later = history.clone();
        later.push(mk(at(11), EventKind::PrMerged { pr: 3 }));
        let out = run(&mut state, at(12), &later);
        assert_eq!(out, vec!["schedule tick: capture-sweep ok"]);
        assert_eq!(
            state.tasks["capture-sweep"].last_seen_event_ts,
            Some(at(11))
        );
    }

    // Advisor review round 1 (#1946). trace:STORY-1226 | ai:claude
    #[test]
    fn load_global_config_surfaces_a_type_error_inside_the_schedule_table() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".aida");
        std::fs::create_dir_all(&dir).unwrap();
        // `name` must be a string: a typed error INSIDE [schedule] must not be
        // swallowed by the bare-shape fallback.
        std::fs::write(
            dir.join(GLOBAL_SCHEDULE_FILE),
            "[schedule]\n[[schedule.jobs]]\nname = 5\nevery = \"30m\"\ncommand = \"doctor\"\n",
        )
        .unwrap();
        let err = load_global_config(home.path()).unwrap_err();
        assert!(err.to_string().contains("failed to parse"), "{err}");
        // The bare shape (no `schedule` table) still loads.
        std::fs::write(
            dir.join(GLOBAL_SCHEDULE_FILE),
            "[[jobs]]\nname = \"doctor\"\nevery = \"6h\"\ncommand = \"doctor\"\n",
        )
        .unwrap();
        let cfg = load_global_config(home.path()).unwrap().unwrap();
        assert_eq!(cfg.tasks.len(), 1);
    }

    #[test]
    fn on_job_fires_once_per_new_event() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mk = |ts: DateTime<Utc>, kind: EventKind| Event {
            ts,
            spec: None,
            run_uuid: String::new(),
            seat: None,
            kind,
        };
        let events_a = vec![
            mk(at(9), EventKind::PrMerged { pr: 1 }),
            mk(at(10), EventKind::RunStarted),
        ];
        let mut on_reap = task("queue-gc", "24h", "queue gc");
        on_reap.interval = None;
        on_reap.on = vec!["PrMerged".to_string()];
        let run = |state: &mut ScheduleState, now, events: &[Event]| {
            tick_core(
                tmp.path(),
                config(vec![on_reap.clone()]),
                state,
                now,
                false,
                ok_exec(Rc::clone(&seen)),
                |_| Snapshot::default(),
                events,
            )
            .unwrap()
        };
        // First tick: the job is seen for the first time — history is NOT
        // replayed (advisor round 1 on #1946); the cursor initialises to now.
        let out = run(&mut state, at(11), &events_a);
        assert!(out.is_empty(), "{out:?}");
        assert!(seen.borrow().is_empty());
        assert_eq!(
            state.tasks["queue-gc"].last_seen_event_ts,
            Some(at(11)),
            "cursor initialises to now on first sight"
        );
        // Same events again: nothing new → no run.
        let out = run(&mut state, at(12), &events_a);
        assert!(out.is_empty());
        assert!(seen.borrow().is_empty());
        // A new PrMerged after the cursor → runs once; an unlisted kind
        // (RunStarted) never triggers.
        let mut events_b = events_a.clone();
        events_b.push(mk(at(12), EventKind::RunStarted));
        events_b.push(mk(at(13), EventKind::PrMerged { pr: 2 }));
        let out = run(&mut state, at(14), &events_b);
        assert_eq!(out, vec!["schedule tick: queue-gc ok"]);
        assert_eq!(seen.borrow().len(), 1);
        assert_eq!(state.tasks["queue-gc"].last_seen_event_ts, Some(at(13)));
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn when_job_fires_once_until_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut latency = task("mailbox-latency", "1h", "notify check");
        latency.interval = None;
        latency.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
        latency.when_raw = Some("mail.oldest_unread_age > 15m".into());
        let run = |state: &mut ScheduleState, now, age_secs: i64| {
            tick_core(
                tmp.path(),
                config(vec![latency.clone()]),
                state,
                now,
                false,
                ok_exec(Rc::clone(&seen)),
                move |fields| {
                    assert!(fields.contains(&Field::MailOldestUnreadAge));
                    Snapshot {
                        mail_oldest_unread_age_secs: age_secs,
                        ..Default::default()
                    }
                },
                &[],
            )
            .unwrap()
        };
        // false → nothing.
        assert!(run(&mut state, at(9), 60).is_empty());
        // turns true → fires once.
        assert_eq!(
            run(&mut state, at(10), 20 * 60),
            vec!["schedule tick: mailbox-latency ok"]
        );
        // still true → no re-fire.
        assert!(run(&mut state, at(11), 30 * 60).is_empty());
        assert_eq!(seen.borrow().len(), 1);
        // clears.
        assert!(run(&mut state, at(12), 0).is_empty());
        let ledger = schedule_ledger::load(&store_root(tmp.path()), "mailbox-latency").unwrap();
        assert_eq!(ledger.episode.as_ref().unwrap().cleared_at, Some(at(12)));
        // true again → a new episode fires.
        assert_eq!(
            run(&mut state, at(13), 40 * 60),
            vec!["schedule tick: mailbox-latency ok"]
        );
        assert_eq!(seen.borrow().len(), 2);
    }

    // trace:TASK-1281 | ai:codex
    #[test]
    fn substrate_when_firing_writes_one_ledger_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_root(tmp.path());
        std::fs::create_dir_all(&store).unwrap();
        let git = |args: &[&str]| {
            let output = ProcessCommand::new("git")
                .arg("-C")
                .arg(&store)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "aida-store"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Schedule Test"]);

        let mut job = task("mailbox-latency", "1h", "notify check");
        job.interval = None;
        job.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
        job.when_raw = Some("mail.oldest_unread_age > 15m".into());
        let mut state = ScheduleState::default();
        tick_core(
            tmp.path(),
            config(vec![job]),
            &mut state,
            at(12),
            false,
            |_root, _command| {
                Ok(TaskOutcome {
                    status: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            },
            |_| Snapshot {
                mail_oldest_unread_age_secs: 20 * 60,
                ..Default::default()
            },
            &[],
        )
        .unwrap();

        assert_eq!(git(&["rev-list", "--count", "HEAD"]), "1");
        let ledger = schedule_ledger::load(&store, "mailbox-latency").unwrap();
        assert_eq!(ledger.result.as_deref(), Some("ok"));
        assert_eq!(ledger.episode.unwrap().fired_at, Some(at(12)));
    }

    // trace:TASK-1281 | ai:codex
    #[test]
    fn seat_when_stays_debounced_after_done_clears_due() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_root(tmp.path());
        let mut state = ScheduleState::default();
        let mut job = seat_task("mailbox-triage", &["advisor"], None, &[]);
        job.when = Some(schedule_predicate::parse("mail.oldest_unread_age > 15m").unwrap());
        job.when_raw = Some("mail.oldest_unread_age > 15m".into());
        let run = |state: &mut ScheduleState, now| {
            tick_core(
                tmp.path(),
                config(vec![job.clone()]),
                state,
                now,
                false,
                |_root, _command| panic!("seat jobs must never execute"),
                |_| Snapshot {
                    mail_oldest_unread_age_secs: 20 * 60,
                    ..Default::default()
                },
                &[],
            )
            .unwrap()
        };

        assert_eq!(
            run(&mut state, at(10)),
            vec!["schedule tick: mailbox-triage due (when) → seat advisor"]
        );
        let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
        assert!(ledger.episode.as_ref().is_some_and(|ep| ep.is_open()));

        // Simulate `aida schedule done`: it clears delivery state, but the
        // condition episode remains open until the predicate turns false.
        schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
            l.last_run = Some(at(11));
            l.result = Some("done".into());
            l.due_since = None;
            l.due_reason = None;
        })
        .unwrap();

        assert!(run(&mut state, at(12)).is_empty());
        let ledger = schedule_ledger::load(&store, "mailbox-triage").unwrap();
        assert!(ledger.due_since.is_none());
        assert!(ledger.episode.as_ref().is_some_and(|ep| ep.is_open()));
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn every_and_on_combined() {
        let tmp = tempfile::tempdir().unwrap();
        let mut state = ScheduleState::default();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let triage = seat_task(
            "mailbox-triage",
            &["advisor"],
            Some("30m"),
            &["MailReceived"],
        );
        let mail = Event {
            ts: at(12),
            spec: None,
            run_uuid: String::new(),
            seat: None,
            kind: EventKind::MailReceived {
                to: "advisor".into(),
            },
        };
        // Reported done at 11:50 → the interval is NOT due at 12:01, but the
        // 12:00 MailReceived is → due via the event fast path.
        let store = store_root(tmp.path());
        let done_at = Utc.with_ymd_and_hms(2026, 9, 11, 11, 50, 0).unwrap();
        schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
            l.last_run = Some(done_at);
            l.result = Some("done".into());
        })
        .unwrap();
        let now = Utc.with_ymd_and_hms(2026, 9, 11, 12, 1, 0).unwrap();
        let out = tick_core(
            tmp.path(),
            config(vec![triage.clone()]),
            &mut state,
            now,
            false,
            ok_exec(Rc::clone(&seen)),
            |_| Snapshot::default(),
            std::slice::from_ref(&mail),
        )
        .unwrap();
        assert_eq!(
            out,
            vec!["schedule tick: mailbox-triage due (on MailReceived) → seat advisor"]
        );
        assert!(seen.borrow().is_empty());
        // Clear it; with no new event, the interval fallback takes over at 12:30.
        schedule_ledger::write_cas(&store, "mailbox-triage", |l| {
            l.last_run = Some(now);
            l.due_since = None;
            l.due_reason = None;
        })
        .unwrap();
        let later = Utc.with_ymd_and_hms(2026, 9, 11, 12, 35, 0).unwrap();
        let out = tick_core(
            tmp.path(),
            config(vec![triage]),
            &mut state,
            later,
            false,
            ok_exec(Rc::clone(&seen)),
            |_| Snapshot::default(),
            std::slice::from_ref(&mail),
        )
        .unwrap();
        assert_eq!(
            out,
            vec!["schedule tick: mailbox-triage due (every 30m) → seat advisor"]
        );
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn due_jobs_block_renders_report_back_line() {
        let due = vec![DueJob {
            name: "mailbox-triage".into(),
            kind: JobKind::Seat,
            seats: vec!["advisor".into()],
            schedule: "every 30m".into(),
            reason: "every 30m".into(),
            prompt: Some("triage the mailbox".into()),
            command: None,
            last_run: Some(Utc::now() - Duration::minutes(47)),
            last_by: Some("advisor/claude".into()),
            due_since: None,
            failure: None,
        }];
        let block = render_due_jobs_block(&due, "advisor");
        assert!(block.starts_with("DUE JOBS (seat: advisor):\n"));
        assert!(block.contains("- mailbox-triage (every 30m, last 47m ago) → triage the mailbox"));
        assert!(block.contains("aida schedule done <job>"));
        assert_eq!(render_due_jobs_block(&[], "advisor"), "");
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn due_seat_jobs_reads_registry_and_ledger_file_only() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = crate::test_env::env_lock();
        std::env::set_var("AIDA_HOME", tmp.path());
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            r#"
[schedule]

[[schedule.jobs]]
name = "mailbox-triage"
seats = ["advisor"]
every = "30m"
prompt = "triage the mailbox"
enabled = true

[[schedule.jobs]]
name = "capture-sweep"
seats = ["product"]
on = ["QueueDrained"]
prompt = "capture sweep"
enabled = true

[[schedule.jobs]]
name = "session-reap"
command = "session reap"
every = "30m"
enabled = true
"#,
        )
        .unwrap();
        // Never run → the interval job is due; the on-job is not (no tick flagged it).
        let due = due_seat_jobs(tmp.path(), Some("advisor"));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "mailbox-triage");
        assert_eq!(due[0].reason, "every 30m");
        assert!(due_seat_jobs(tmp.path(), Some("product")).is_empty());
        // Substrate jobs never surface through the seat read.
        assert!(due_seat_jobs(tmp.path(), None)
            .iter()
            .all(|j| j.kind == JobKind::Seat));
        // A tick-flagged on-job surfaces for its seat with the event reason.
        schedule_ledger::write_cas(&store_root(tmp.path()), "capture-sweep", |l| {
            l.due_since = Some(Utc::now());
            l.due_reason = Some("on QueueDrained".into());
        })
        .unwrap();
        let due = due_seat_jobs(tmp.path(), Some("product"));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].reason, "on QueueDrained");
        // No seat filter → both.
        assert_eq!(due_seat_jobs(tmp.path(), None).len(), 2);
        std::env::remove_var("AIDA_HOME");
    }

    // trace:STORY-1226 | ai:claude
    #[test]
    fn global_layer_reads_schedule_toml_in_both_shapes() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".aida")).unwrap();
        std::fs::write(
            home.path().join(".aida/schedule.toml"),
            r#"
[[jobs]]
name = "doctor"
command = "doctor"
every = "6h"
enabled = true
"#,
        )
        .unwrap();
        let cfg = load_global_config(home.path()).unwrap().unwrap();
        assert_eq!(cfg.tasks.len(), 1);
        assert_eq!(cfg.tasks[0].source, JobSource::Global);
        std::fs::write(
            home.path().join(".aida/schedule.toml"),
            r#"
[schedule]
min_gap = "2m"
[[schedule.jobs]]
name = "doctor"
command = "doctor"
every = "6h"
enabled = true
"#,
        )
        .unwrap();
        let cfg = load_global_config(home.path()).unwrap().unwrap();
        assert_eq!(cfg.min_gap, "2m");
        assert_eq!(cfg.tasks.len(), 1);
        assert!(load_global_config(tempfile::tempdir().unwrap().path())
            .unwrap()
            .is_none());
    }

    /// STORY-1423 criterion 3 (THE TRIGGER): a dogfood regression test over
    /// this repo's OWN `.aida/config.toml`, not a fixture. `aida schedule
    /// list` reporting both jobs enabled was a point-in-time observation
    /// (2026-09-21, PR #2062); nothing stopped a later config edit from
    /// silently disabling or deleting either entry and letting the gate go
    /// dark again — exactly the "commented example is a trigger for nobody"
    /// defect this spec exists to close, recurring one layer out. Reads the
    /// real file via `CARGO_MANIFEST_DIR` so this fails the moment the
    /// dogfood surface regresses.
    // trace:STORY-1423 | ai:claude
    #[test]
    fn this_repo_keeps_the_performance_guard_pair_registered_and_enabled() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("aida-cli-lib has a workspace parent");
        let cfg = load_config(repo_root)
            .unwrap()
            .expect("this repo's .aida/config.toml must declare a [schedule] section");

        let guard = cfg
            .tasks
            .iter()
            .find(|t| t.name == "performance-guard")
            .expect("the performance-guard substrate job must stay registered in this repo");
        assert_eq!(guard.kind, JobKind::Substrate);
        assert!(
            guard.enabled,
            "performance-guard must stay enabled — a disabled entry is a trigger for nobody"
        );

        let route = cfg
            .tasks
            .iter()
            .find(|t| t.name == "performance-guard-route")
            .expect("the performance-guard-route seat job must stay registered in this repo");
        assert_eq!(route.kind, JobKind::Seat);
        assert!(
            route.enabled,
            "performance-guard-route must stay enabled so a trip still reaches a seat"
        );
    }
}
