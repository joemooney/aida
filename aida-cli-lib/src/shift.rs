//! The night shift (STORY-1218 slice 1): `aida shift tick`.
//!
//! A deterministic, LLM-free step that runs as the `night-shift` substrate job
//! of `aida schedule tick`, driven by whichever scheduler driver this repo
//! has: the crontab entry or the systemd user timer (`aida shift install
//! --systemd-user|--cron`, slice 2 — see `schedule_driver.rs`). Each tick:
//!
//! 1. does nothing unless THIS clone's local layer enables it
//!    (`~/.aida/shift-local.toml`, keyed by the canonical repo path — never
//!    the committed project config);
//! 2. takes `.aida/shift.lock` (a contended lock is a no-op);
//! 3. reaps finished sessions;
//! 4. settles the previous shift wave into the circuit breakers kept in
//!    `.aida/shift-state.json`;
//! 5. evaluates a fixed list of fail-closed guards; and only when every one
//!    passes
//! 6. records the launch intent, tags the next explicit-drain-mode queue slice
//!    `batch:shift-YYYYMMDD-HHMM`, spawns one bounded, detached
//!    `aida queue work --batch … --auto-complete --no-human=both …` wave
//!    (which takes the drain lock exactly like a manual drain), and records
//!    its pid;
//! 7. emits one `ShiftTick` event only when it acted or its refusing-guard
//!    set changed.
//!
//! A refusal, a no-op and a live lock all exit 0: a refused tick must never
//! become a `CronJobFailed` that wakes a seat every 15 minutes. Only an
//! internal error (unreadable state, failed spawn, failed store write) exits
//! non-zero.
//!
//! Slice 3 adds two steps (TASK-1492):
//!
//! - **re-drive** (opt-in, OFF by default per ADR-26 fork C — enabling the
//!   shift does not enable it): while no drain is live, transient parks of
//!   explicit drain-mode specs go back to Approved and to the HEAD of the
//!   queue, within the ADR-26 cap (3 attempts, 2m/8m/30m backoff, counted
//!   from `SpecReDriven` events in the live stream AND its rotated archive).
//!   A park at the cap is reclassified to needs-human. No re-drive at all
//!   when the attempt evidence cannot be read (`redrive-evidence`);
//! - **mail latency**: per KNOWN recipient (a seat or role in the agent
//!   registry, a team roster member or an active session identity), the age
//!   of the oldest unread message; above `[shift] mail_latency` the operator
//!   is notified through `aida notify` (never the mailbox or chat), once per
//!   episode per recipient, re-armed when the age drops back under the
//!   threshold. Mail to unknown addresses never pages; a notification names
//!   at most five recipients, then "+N more".
//!
//! Not yet here: headless cold-boot of a seat for an overdue seat job.
//!
//! Every side effect goes through [`ShiftExec`], so the tick core is tested
//! with a recording mock: no test launches a drain.
// trace:STORY-1218 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, Instant};

use aida_core::{DatabaseBackend, ExecutionMode, RequirementStatus};
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::ShiftCommand;
use crate::events::{self, EventKind, ShiftLaunch};

/// Local enable layer, under `<home>/.aida/`. Outside every repository, so
/// the switch cannot be committed by accident (A1).
pub(crate) const LOCAL_LAYER_FILE: &str = "shift-local.toml";
const STATE_FILE: &str = "shift-state.json";
const LOCK_FILE: &str = "shift.lock";
const STATE_VERSION: u32 = 1;

/// The whole tick must finish well inside the scheduler's 120s child kill.
pub(crate) const TICK_DEADLINE: StdDuration = StdDuration::from_secs(90);
/// A launch (record intent, tag, spawn, record pid) is not started with less
/// than this left on the deadline.
const LAUNCH_RESERVE: StdDuration = StdDuration::from_secs(30);
/// The longest a tick waits on the operator's notify command. With the time
/// left on [`TICK_DEADLINE`] as a further bound, a hung command ends the tick
/// well inside the scheduler's 120s kill.
// trace:TASK-1492 | ai:claude
pub(crate) const NOTIFY_TIMEOUT: StdDuration = StdDuration::from_secs(15);

/// The role whose queue view the wave drains. The wave passes `--role` with
/// it, and selection reads the same role-routed view.
const WAVE_ROLE: &str = "implementer";
/// Vendors whose spend the runaway-seat watchdog aggregates measure.
const COVERED_VENDORS: &[&str] = &["claude"];
/// The registry command of the watchdog job.
const WATCHDOG_COMMAND: &str = "doctor check runaway-seats --fail-on-findings";
/// The registry command of the tick job itself.
pub(crate) const TICK_COMMAND: &str = "shift tick";
/// Name and cadence `aida shift enable` registers the tick job under.
const TICK_JOB_NAME: &str = "night-shift";
const TICK_JOB_EVERY: &str = "10m";

/// A8(b): consecutive zero-progress waves before launches stop.
const ZERO_PROGRESS_BREAKER: u32 = 2;
/// A8(c): shift waves a spec may appear in within 24h without finishing.
const SPEC_WAVE_CAP: usize = 2;
/// A9: watchdog aggregates older than this are not evidence.
const BUDGET_EVIDENCE_MAX_AGE_MINS: i64 = 60;
/// A runaway-seat watchdog failure within this window blocks launches.
const WATCHDOG_QUIET_HOURS: i64 = 2;

/// A2: never inherited by the wave.
const SCRUBBED_ENV: &[&str] = &[
    "AIDA_DRAIN_FORCE",
    "AIDA_DRAIN_BORROW",
    "AIDA_DRAIN_LOCK_STALE_SECS",
    "AIDA_EVENTS_DISABLE",
    "AIDA_SCHEDULE_CHILD",
];

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Effective night-shift settings. Tunables come from the committed
/// `.aida/config.toml [shift]` table, overridden by the local layer; the
/// `enabled` switch and `allow_uncovered_vendors` come ONLY from the local
/// layer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ShiftConfig {
    pub enabled: bool,
    /// Where the switch was read from, for dry-run and status.
    pub enabled_source: String,
    /// The committed config says `enabled = true`; it is ignored (A1).
    pub committed_enable_ignored: bool,
    /// Specs per wave. Default 6 (SPIKE-82: bounded waves of 4-8 held).
    pub wave_size: usize,
    /// `--max-failures` for the wave. Default 2, clamped to 1..=wave_size.
    pub max_failures: usize,
    /// Drain this pre-tagged batch instead of auto-tagging.
    pub batch: Option<String>,
    /// Trailing-24h token budget; defaults to the watchdog's.
    pub daily_token_budget: u64,
    /// Refuse to launch at or above this share of the daily budget.
    pub budget_stop_pct: u64,
    /// `--max-tokens` ceiling for one wave.
    pub wave_token_budget: u64,
    /// `--max-runtime` for one wave.
    pub max_runtime: String,
    pub max_runtime_hours: u64,
    /// Launched waves per trailing 24h.
    pub max_waves_per_day: usize,
    /// 1-minute load average ceiling per logical CPU.
    pub load_per_cpu: f64,
    /// Vendors allowed to run although the watchdog cannot see their spend.
    pub allow_uncovered_vendors: Vec<String>,
    /// Automatic re-drive of transient parks. OFF unless THIS clone's local
    /// layer sets `redrive = true` (ADR-26 fork C; enabling the shift does
    /// not enable it).
    // trace:TASK-1492 | ai:claude
    pub redrive: bool,
    /// Where the re-drive switch was read from.
    pub redrive_source: String,
    /// The committed config says `redrive = true`; it is ignored.
    pub committed_redrive_ignored: bool,
    /// Re-drives per tick. Default 3.
    pub max_redrives_per_tick: usize,
    /// Oldest-unread age above which the operator is notified. Default 30m.
    pub mail_latency: String,
    pub mail_latency_secs: i64,
}

const DEFAULT_WAVE_SIZE: usize = 6;
const DEFAULT_MAX_FAILURES: usize = 2;
const DEFAULT_BUDGET_STOP_PCT: u64 = 80;
const DEFAULT_MAX_RUNTIME: &str = "3h";
const DEFAULT_MAX_WAVES_PER_DAY: usize = 8;
const DEFAULT_LOAD_PER_CPU: f64 = 1.5;
// trace:TASK-1492 | ai:claude
const DEFAULT_MAX_REDRIVES_PER_TICK: usize = 3;
const DEFAULT_MAIL_LATENCY: &str = "30m";

fn positive_int(v: Option<&toml::Value>) -> Option<u64> {
    v.and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
        .map(|n| n as u64)
}

/// Build the effective config from the committed project config and the
/// local layer. Pure over its inputs.
pub(crate) fn build_config(
    committed: Option<&toml::Value>,
    local: Option<&toml::Value>,
    repo_key: &str,
    local_path: &str,
) -> ShiftConfig {
    let committed_shift = committed.and_then(|c| c.get("shift"));
    let local_repo = local
        .and_then(|l| l.get("repo"))
        .and_then(|r| r.get(repo_key));
    // Local wins over committed for every tunable.
    let get = |key: &str| -> Option<&toml::Value> {
        local_repo
            .and_then(|t| t.get(key))
            .or_else(|| committed_shift.and_then(|t| t.get(key)))
    };
    let enabled = local_repo
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let committed_enable_ignored = committed_shift
        .and_then(|t| t.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let enabled_source = if local_repo.and_then(|t| t.get("enabled")).is_some() {
        format!("{local_path} [repo.\"{repo_key}\"]")
    } else {
        "default (off)".to_string()
    };
    let wave_size = positive_int(get("wave_size"))
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_WAVE_SIZE);
    // Q4: the unattended default is 2, clamped to 1..=wave_size.
    let max_failures = positive_int(get("max_failures"))
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_FAILURES)
        .clamp(1, wave_size.max(1));
    let watchdog = crate::runaway_seats::policy(committed);
    let daily_token_budget =
        positive_int(get("daily_token_budget")).unwrap_or(watchdog.daily_token_budget);
    let budget_stop_pct = positive_int(get("budget_stop_pct"))
        .filter(|p| *p <= 100)
        .unwrap_or(DEFAULT_BUDGET_STOP_PCT);
    let max_waves_per_day = positive_int(get("max_waves_per_day"))
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_WAVES_PER_DAY);
    let stop_threshold = daily_token_budget / 100 * budget_stop_pct;
    let wave_token_budget = positive_int(get("wave_token_budget"))
        .unwrap_or(stop_threshold / max_waves_per_day.max(1) as u64);
    let max_runtime = get("max_runtime")
        .and_then(|v| v.as_str())
        .filter(|s| crate::maintenance_schedule::parse_duration(s).is_ok())
        .unwrap_or(DEFAULT_MAX_RUNTIME)
        .to_string();
    let max_runtime_hours = crate::maintenance_schedule::parse_duration(&max_runtime)
        .map(|d| ((d.num_minutes() + 59) / 60).max(1) as u64)
        .unwrap_or(3);
    let load_per_cpu = get("load_per_cpu")
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .filter(|f| *f > 0.0)
        .unwrap_or(DEFAULT_LOAD_PER_CPU);
    let batch = get("batch")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().trim_start_matches("batch:").to_string())
        .filter(|s| !s.is_empty());
    let allow_uncovered_vendors = local_repo
        .and_then(|t| t.get("allow_uncovered_vendors"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_ascii_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    // trace:TASK-1492 | ai:claude — the re-drive switch lives only in the
    // local layer, next to `enabled` (A5).
    let redrive = local_repo
        .and_then(|t| t.get("redrive"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let redrive_source = if local_repo.and_then(|t| t.get("redrive")).is_some() {
        format!("{local_path} [repo.\"{repo_key}\"]")
    } else {
        "ADR-26 default (off)".to_string()
    };
    let committed_redrive_ignored = committed_shift
        .and_then(|t| t.get("redrive"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_redrives_per_tick = positive_int(get("max_redrives_per_tick"))
        .map(|n| n as usize)
        .unwrap_or(DEFAULT_MAX_REDRIVES_PER_TICK);
    let mail_latency = get("mail_latency")
        .and_then(|v| v.as_str())
        .filter(|s| {
            crate::maintenance_schedule::parse_duration(s).is_ok_and(|d| d > Duration::zero())
        })
        .unwrap_or(DEFAULT_MAIL_LATENCY)
        .to_string();
    let mail_latency_secs = crate::maintenance_schedule::parse_duration(&mail_latency)
        .map(|d| d.num_seconds())
        .unwrap_or(30 * 60);
    ShiftConfig {
        enabled,
        enabled_source,
        committed_enable_ignored,
        wave_size,
        max_failures,
        batch,
        daily_token_budget,
        budget_stop_pct,
        wave_token_budget,
        max_runtime,
        max_runtime_hours,
        max_waves_per_day,
        load_per_cpu,
        allow_uncovered_vendors,
        redrive,
        redrive_source,
        committed_redrive_ignored,
        max_redrives_per_tick,
        mail_latency,
        mail_latency_secs,
    }
}

/// `<AIDA_HOME|home>/.aida/shift-local.toml`.
pub(crate) fn local_layer_path() -> Option<PathBuf> {
    crate::maintenance_schedule::global_home().map(|h| h.join(".aida").join(LOCAL_LAYER_FILE))
}

/// The key a repo is filed under in the local layer.
pub(crate) fn repo_key(project_root: &Path) -> String {
    project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .display()
        .to_string()
}

fn read_toml(path: &Path) -> Option<toml::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|b| toml::from_str::<toml::Value>(&b).ok())
}

pub(crate) fn load_config(project_root: &Path) -> ShiftConfig {
    let committed = crate::read_project_config_value(project_root);
    let local_path = local_layer_path();
    let local = local_path.as_deref().and_then(read_toml);
    build_config(
        committed.as_ref(),
        local.as_ref(),
        &repo_key(project_root),
        &local_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.aida/shift-local.toml".to_string()),
    )
}

/// Write `enabled` for this repo into the local layer.
fn write_local_enabled(path: &Path, repo: &str, enabled: bool) -> Result<()> {
    let mut doc = crate::config_edit::load_doc(path)?;
    if !doc.contains_table("repo") {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert("repo", toml_edit::Item::Table(t));
    }
    let repos = doc["repo"]
        .as_table_mut()
        .context("[repo] in the local night-shift layer is not a table")?;
    if !repos.contains_key(repo) {
        repos.insert(repo, toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let table = repos[repo]
        .as_table_mut()
        .context("the repo entry in the local night-shift layer is not a table")?;
    table.insert("enabled", toml_edit::value(enabled));
    crate::config_edit::save_doc(path, &doc)
}

// ---------------------------------------------------------------------------
// State (A8 circuit breakers live here, not in events: events rotate)
// ---------------------------------------------------------------------------

/// How a launched wave ended, settled on the first tick after its pid died.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WaveOutcome {
    pub settled_at: DateTime<Utc>,
    pub shipped: usize,
    pub shelved: usize,
    /// A8(a): false when no `QueueDrained` followed the launch.
    pub progress: bool,
}

/// One shift wave. `pid == None` is a recorded launch intent whose spawn
/// never happened (the tick was killed between tagging and spawning); the
/// next tick reuses its batch (A4). An intent whose wave DID start (the
/// tick died before saving the pid) is adopted once its drain lock is seen
/// live (TASK-1497).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WaveRecord {
    pub batch: String,
    pub specs: Vec<String>,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pid_start: Option<String>,
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub outcome: Option<WaveOutcome>,
}

/// A8(b): launches stopped after consecutive zero-progress waves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Breaker {
    pub tripped_at: DateTime<Utc>,
    pub reason: String,
    /// The queue as it stood at the trip; a change to it resumes launches.
    pub queue_fingerprint: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ShiftState {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub last_tick_at: Option<DateTime<Utc>>,
    /// Guards that refused on the last tick (event only on change).
    #[serde(default)]
    pub last_refused: Vec<String>,
    /// Last stale drain-lock pid reported, so it is reported once.
    #[serde(default)]
    pub last_stale_pid: Option<u32>,
    /// Waves, oldest first; the last one is the most recent launch.
    #[serde(default)]
    pub waves: Vec<WaveRecord>,
    #[serde(default)]
    pub consecutive_zero_progress: u32,
    #[serde(default)]
    pub breaker: Option<Breaker>,
    /// A8(c): per spec, the launch times of unfinished shift waves.
    #[serde(default)]
    pub spec_waves: BTreeMap<String, Vec<DateTime<Utc>>>,
    /// A8(c): specs already escalated for hitting the wave cap.
    #[serde(default)]
    pub escalated: BTreeSet<String>,
    /// Mail latency: recipients whose open over-threshold episode was
    /// already escalated, with when. Removed (re-armed) once the recipient's
    /// oldest unread age drops back under the threshold.
    // trace:TASK-1492 | ai:claude
    #[serde(default)]
    pub mail_episodes: BTreeMap<String, DateTime<Utc>>,
}

impl ShiftState {
    /// A8(d): launched waves in the trailing 24h.
    pub(crate) fn waves_in_day(&self, now: DateTime<Utc>) -> usize {
        self.waves
            .iter()
            .filter(|w| w.pid.is_some() && w.at > now - Duration::hours(24))
            .count()
    }

    /// The most recent wave whose process was started.
    pub(crate) fn last_launched(&self) -> Option<&WaveRecord> {
        self.waves.iter().rev().find(|w| w.pid.is_some())
    }

    /// A recorded intent with no pid, still unsettled (A4).
    fn pending_intent(&self) -> Option<&WaveRecord> {
        self.waves
            .last()
            .filter(|w| w.pid.is_none() && w.outcome.is_none())
    }
}

pub(crate) fn state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(STATE_FILE)
}

/// Missing file = fresh state; unparseable = error (launches blocked, A8).
pub(crate) fn load_state(path: &Path) -> Result<ShiftState> {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str::<ShiftState>(&body)
            .with_context(|| format!("night-shift state {} is unreadable", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ShiftState::default()),
        Err(e) => {
            Err(e).with_context(|| format!("night-shift state {} is unreadable", path.display()))
        }
    }
}

fn save_state_to(path: &Path, state: &ShiftState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state)?;
    aida_core::write_atomic(path, body)
        .map_err(|e| anyhow::anyhow!("could not write {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Selection (A11 floors, A8(c) cap, Q5 auto-tag)
// ---------------------------------------------------------------------------

/// One queued spec as the selection sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub spec: String,
    pub status: RequirementStatus,
    pub execution_mode: Option<ExecutionMode>,
    pub req_type: String,
    pub tags: Vec<String>,
    pub merge_held: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ineligible {
    Status,
    Mode,
    Keystone,
    MergeHeld,
    WaveCap,
}

fn unfinished_waves_in_day(state: &ShiftState, spec: &str, now: DateTime<Utc>) -> usize {
    state
        .spec_waves
        .get(spec)
        .map(|v| v.iter().filter(|t| **t > now - Duration::hours(24)).count())
        .unwrap_or(0)
}

/// Why `c` may not ride a shift wave, if it may not.
pub(crate) fn ineligibility(
    c: &Candidate,
    state: &ShiftState,
    now: DateTime<Utc>,
) -> Option<(Ineligible, String)> {
    if !matches!(
        c.status,
        RequirementStatus::Approved | RequirementStatus::Planned
    ) {
        return Some((Ineligible::Status, format!("status {}", c.status)));
    }
    // A11: only an EXPLICIT drain mode counts. A missing mode never selects.
    match c.execution_mode {
        Some(ExecutionMode::Drain) => {}
        Some(other) => {
            return Some((
                Ineligible::Mode,
                format!("execution mode {}", format!("{other:?}").to_lowercase()),
            ))
        }
        None => return Some((Ineligible::Mode, "no explicit execution mode".to_string())),
    }
    if crate::presence::is_keystone_class(&c.req_type, c.tags.iter().map(|s| s.as_str())) {
        return Some((Ineligible::Keystone, "keystone-class".to_string()));
    }
    if c.merge_held {
        return Some((Ineligible::MergeHeld, "live merge hold".to_string()));
    }
    if unfinished_waves_in_day(state, &c.spec, now) >= SPEC_WAVE_CAP {
        return Some((
            Ineligible::WaveCap,
            format!("in {SPEC_WAVE_CAP} shift waves in 24h without finishing"),
        ));
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct Selection {
    pub batch: Option<String>,
    pub specs: Vec<String>,
    /// Auto mode tags the members; a configured batch is already tagged.
    pub auto_tag: bool,
    /// An un-launched shift batch reused from a killed tick (A4).
    pub reused: bool,
    /// Configured batch: the first member that fails the floors.
    pub offender: Option<String>,
    /// Queued specs left out, with the reason.
    pub skipped: Vec<(String, String)>,
    /// Specs newly over the A8(c) wave cap.
    pub capped: Vec<String>,
}

pub(crate) fn shift_batch_name(now: DateTime<Utc>) -> String {
    format!("shift-{}", now.format("%Y%m%d-%H%M"))
}

pub(crate) fn select_wave(
    cfg: &ShiftConfig,
    candidates: &[Candidate],
    state: &ShiftState,
    now: DateTime<Utc>,
) -> Selection {
    let mut sel = Selection::default();
    let mut eligible: Vec<&Candidate> = Vec::new();
    for c in candidates {
        match ineligibility(c, state, now) {
            None => eligible.push(c),
            Some((kind, why)) => {
                if kind == Ineligible::WaveCap && !state.escalated.contains(&c.spec) {
                    sel.capped.push(c.spec.clone());
                }
                sel.skipped.push((c.spec.clone(), why));
            }
        }
    }
    if let Some(name) = &cfg.batch {
        // Configured batch: every queued member must pass the floors, or the
        // tick fails closed naming the offender (`--batch` takes the head, so
        // one supervised member would get driven).
        let want = format!("batch:{name}");
        let members: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| c.tags.iter().any(|t| t.eq_ignore_ascii_case(&want)))
            .collect();
        sel.batch = Some(name.clone());
        for m in &members {
            if let Some((_, why)) = ineligibility(m, state, now) {
                sel.offender = Some(format!("{} ({why})", m.spec));
                return sel;
            }
        }
        sel.specs = members.iter().map(|c| c.spec.clone()).collect();
        return sel;
    }
    sel.auto_tag = true;
    if let Some(intent) = state.pending_intent() {
        let reused: Vec<String> = eligible
            .iter()
            .filter(|c| intent.specs.contains(&c.spec))
            .map(|c| c.spec.clone())
            .collect();
        if !reused.is_empty() {
            sel.batch = Some(intent.batch.clone());
            sel.specs = reused;
            sel.reused = true;
            return sel;
        }
    }
    sel.specs = eligible
        .iter()
        .take(cfg.wave_size)
        .map(|c| c.spec.clone())
        .collect();
    if !sel.specs.is_empty() {
        sel.batch = Some(shift_batch_name(now));
    }
    sel
}

/// Q5: a spec carries at most one `batch:shift-*` tag; re-tagging replaces
/// it. Other tags are untouched. Returns whether the set changed.
pub(crate) fn retag(tags: &mut HashSet<String>, batch: &str) -> bool {
    let want = format!("batch:{batch}");
    let before = tags.clone();
    tags.retain(|t| {
        !t.to_ascii_lowercase().starts_with("batch:shift-") || t.eq_ignore_ascii_case(&want)
    });
    if !tags.iter().any(|t| t.eq_ignore_ascii_case(&want)) {
        tags.insert(want);
    }
    *tags != before
}

/// Tag each spec for the wave through the store backend.
pub(crate) fn tag_specs(
    backend: &dyn DatabaseBackend,
    batch: &str,
    specs: &[String],
) -> Result<()> {
    for spec in specs {
        let mut req = backend
            .get_requirement_by_spec_id(spec)?
            .with_context(|| format!("{spec} is not in the store"))?;
        if retag(&mut req.tags, batch) {
            req.modified_at = Utc::now();
            backend
                .update_requirement(&req)
                .with_context(|| format!("tagging {spec} batch:{batch}"))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Wave command (A2, A9)
// ---------------------------------------------------------------------------

/// The exact argv handed to `aida`. Never carries `--force-claim`,
/// `--steal` or `--force`; the environment never carries `AIDA_DRAIN_FORCE`.
pub(crate) fn build_wave_argv(cfg: &ShiftConfig, batch: &str, max_tokens: u64) -> Vec<String> {
    let n = cfg.wave_size.to_string();
    [
        "queue",
        "work",
        "--batch",
        batch,
        "--auto-complete",
        "--no-human=both",
        "--escalate-blocks",
        "--role",
        WAVE_ROLE,
        "--max",
        &n,
        "--max-iterations",
        &n,
        "--max-failures",
        &cfg.max_failures.to_string(),
        "--max-tokens",
        &max_tokens.to_string(),
        "--max-runtime",
        &cfg.max_runtime,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// A9: min(wave budget, stop threshold − spent 24h).
pub(crate) fn wave_max_tokens(cfg: &ShiftConfig, spent_24h: u64) -> u64 {
    let stop = cfg.daily_token_budget / 100 * cfg.budget_stop_pct;
    cfg.wave_token_budget.min(stop.saturating_sub(spent_24h))
}

/// Build the detached wave process (A2): own session, stdin from
/// /dev/null, stdout and stderr to `log`, force/borrow/test env removed.
pub(crate) fn wave_command(
    program: &Path,
    args: &[String],
    cwd: &Path,
    log: &Path,
) -> Result<std::process::Command> {
    use std::process::Stdio;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("opening wave log {}", log.display()))?;
    let err = out.try_clone()?;
    let mut cmd = std::process::Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    for key in SCRUBBED_ENV {
        cmd.env_remove(key);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A new session: the scheduler kills its child's process GROUP on
        // timeout, and a session leader is outside that group.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    Ok(cmd)
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct GuardVerdict {
    pub name: &'static str,
    pub pass: bool,
    pub detail: String,
}

fn verdict(name: &'static str, pass: bool, detail: impl Into<String>) -> GuardVerdict {
    GuardVerdict {
        name,
        pass,
        detail: detail.into(),
    }
}

/// The local drain lock, as the tick sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockView {
    Free,
    Running(u32),
    Stale(u32),
}

/// Who holds a live drain lock: enough to recognise a shift wave whose pid
/// was never recorded (TASK-1497).
// trace:TASK-1497 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockHolder {
    pub pid: u32,
    pub pid_start: Option<String>,
    /// The command the drain recorded in its lock, e.g.
    /// `queue work --auto-complete --batch shift-20260924-2210`.
    pub command: String,
}

/// Everything the tick reads from the machine and the store. Gathered by
/// [`gather_probes`] in production and built by hand in tests.
#[derive(Debug, Clone)]
pub(crate) struct Probes {
    pub lock: LockView,
    /// The holder of a LIVE drain lock (`lock == Running`), else `None`.
    // trace:TASK-1497 | ai:claude
    pub lock_holder: Option<LockHolder>,
    /// A10: a live drain claim held by ANOTHER clone.
    pub foreign_claim: Option<String>,
    /// A3: where the persisted no-human acknowledgement was found.
    pub no_human_ack: Option<String>,
    pub budget: Option<crate::runaway_seats::BudgetEvidence>,
    /// Resolved headless vendor of the wave's phases — resolved the way
    /// `queue work` resolves it at launch (`[agents] enabled` included).
    pub vendor: String,
    /// The launch-path vendor resolution failed; the budget guard refuses.
    pub vendor_error: Option<String>,
    /// Runaway-seat watchdog failures within the quiet window.
    pub watchdog_failures: usize,
    /// (1-minute load, logical CPUs); `None` = unreadable.
    pub load: Option<(f64, usize)>,
    /// (available, required) memory bytes; `None` = unreadable.
    pub memory: Option<(u64, u64)>,
    /// Disk headroom verdict and detail; `None` = unreadable.
    pub disk: Option<(bool, String)>,
    pub queue_user: String,
    pub candidates: Vec<Candidate>,
    /// Every queued spec id, sorted — the A8(b) resume fingerprint.
    pub queue_fingerprint: Vec<String>,
    /// Is the last launched wave's process still alive?
    pub last_wave_alive: bool,
    /// `QueueDrained` events as (ts, shipped, shelved).
    pub queue_drained: Vec<(DateTime<Utc>, usize, usize)>,
    /// Specs of the last launched wave that have since finished.
    pub finished_specs: BTreeSet<String>,
    /// NeedsAttention specs (read only when re-drive is on).
    // trace:TASK-1492 | ai:claude
    pub parked: Vec<aida_core::Requirement>,
    /// Upper-cased spec ids with a live merge hold.
    pub held: BTreeSet<String>,
    /// ADR-26 attempt evidence from the live stream and its archive; `Err`
    /// = unreadable or not recorded, so no re-drive this tick.
    pub redrive_history: std::result::Result<events::RedriveHistory, String>,
    /// Unread mail per recipient; `Err` = the mailbox could not be read.
    pub mail: std::result::Result<BTreeMap<String, crate::mailbox_store::RecipientUnread>, String>,
    /// Lower-cased recipients the mail step may page for: roles and seats in
    /// the agent registry, team roster members and active session
    /// identities. Mail to any other address never escalates.
    // trace:TASK-1492 | ai:claude
    pub mail_known: BTreeSet<String>,
}

/// Per-tick context the shell supplies.
#[derive(Debug, Clone)]
pub(crate) struct TickCtx {
    pub now: DateTime<Utc>,
    pub dry_run: bool,
    /// Optional steps (reap) may run.
    pub optional_allowed: bool,
    /// A launch may still start before the deadline.
    pub launch_allowed: bool,
    /// The state file could not be read (launches blocked).
    pub state_error: Option<String>,
    /// Where the wave log goes.
    pub log_path: PathBuf,
    /// The live deadline clock (start, budget). `None` in tests that drive
    /// the booleans directly. Re-read after the reap and again right before
    /// the spawn, so a slow reap or store write cannot push a spawn past the
    /// scheduler's kill (A4).
    pub clock: Option<(Instant, StdDuration)>,
}

impl TickCtx {
    /// May an optional step (mail latency) still run? The static flag AND
    /// the live clock.
    // trace:TASK-1492 | ai:claude
    pub(crate) fn optional_ok(&self) -> bool {
        self.optional_allowed
            && self
                .clock
                .is_none_or(|(started, deadline)| started.elapsed() < deadline)
    }

    /// May a launch still start? The static flag AND the live clock.
    pub(crate) fn launch_ok(&self) -> bool {
        self.launch_allowed
            && self
                .clock
                .is_none_or(|(started, deadline)| started.elapsed() + LAUNCH_RESERVE <= deadline)
    }

    /// How long a notify command may run this tick: [`NOTIFY_TIMEOUT`], cut
    /// to the time left on the deadline (at least one second, so a message
    /// that is due still gets a chance). The deadline plus the cap stays well
    /// inside the scheduler's kill.
    // trace:TASK-1492 | ai:claude
    pub(crate) fn notify_timeout(&self) -> StdDuration {
        match self.clock {
            None => NOTIFY_TIMEOUT,
            Some((started, deadline)) => deadline
                .saturating_sub(started.elapsed())
                .clamp(StdDuration::from_secs(1), NOTIFY_TIMEOUT),
        }
    }

    pub(crate) fn from_clock(
        now: DateTime<Utc>,
        dry_run: bool,
        started: Instant,
        deadline: StdDuration,
        log_path: PathBuf,
    ) -> Self {
        let elapsed = started.elapsed();
        Self {
            now,
            dry_run,
            optional_allowed: elapsed < deadline,
            launch_allowed: elapsed + LAUNCH_RESERVE <= deadline,
            state_error: None,
            log_path,
            clock: Some((started, deadline)),
        }
    }
}

pub(crate) fn evaluate_guards(
    cfg: &ShiftConfig,
    p: &Probes,
    state: &ShiftState,
    sel: &Selection,
    ctx: &TickCtx,
) -> Vec<GuardVerdict> {
    let now = ctx.now;
    let mut v = Vec::new();
    v.push(verdict(
        "enabled",
        cfg.enabled,
        if cfg.enabled {
            format!("on ({})", cfg.enabled_source)
        } else if cfg.committed_enable_ignored {
            "off: the committed config's `enabled = true` is ignored; enable per clone with `aida shift enable`".to_string()
        } else {
            "off for this clone (`aida shift enable`)".to_string()
        },
    ));
    v.push(verdict(
        "state-readable",
        ctx.state_error.is_none(),
        ctx.state_error.clone().unwrap_or_else(|| "ok".to_string()),
    ));
    let in_flight = state
        .last_launched()
        .filter(|w| w.outcome.is_none() && p.last_wave_alive);
    v.push(verdict(
        "wave-in-flight",
        in_flight.is_none(),
        match in_flight {
            Some(w) => format!(
                "wave {} (pid {}) is still running",
                w.batch,
                w.pid.unwrap_or_default()
            ),
            None => "no shift wave running".to_string(),
        },
    ));
    v.push(match p.lock {
        LockView::Free => verdict("lock-free", true, "no drain lock"),
        LockView::Running(pid) => verdict(
            "lock-free",
            false,
            format!("a drain holds the lock (pid {pid})"),
        ),
        LockView::Stale(pid) => verdict(
            "lock-free",
            true,
            format!("stale lock from dead pid {pid}; the wave reclaims it"),
        ),
    });
    v.push(verdict(
        "cross-clone-lock",
        p.foreign_claim.is_none(),
        p.foreign_claim
            .clone()
            .unwrap_or_else(|| "no other clone is draining".to_string()),
    ));
    v.push(verdict(
        "no-human-ack",
        p.no_human_ack.is_some(),
        match &p.no_human_ack {
            Some(src) => format!("acknowledged ({src})"),
            None => "not acknowledged: run `aida no-human acknowledge`".to_string(),
        },
    ));
    v.push(match (&sel.offender, sel.specs.is_empty()) {
        (Some(off), _) => verdict(
            "drain-mode-only",
            false,
            format!("configured batch has a member that may not drain: {off}"),
        ),
        (None, true) => verdict(
            "drain-mode-only",
            false,
            "no eligible drain-mode spec queued",
        ),
        (None, false) => verdict(
            "drain-mode-only",
            true,
            format!("{} eligible spec(s)", sel.specs.len()),
        ),
    });
    let stop = cfg.daily_token_budget / 100 * cfg.budget_stop_pct;
    v.push(match &p.budget {
        Some(b) => verdict(
            "token-budget",
            b.spent_24h < stop,
            format!(
                "{} of {} spent in 24h (stop at {}%)",
                b.spent_24h, cfg.daily_token_budget, cfg.budget_stop_pct
            ),
        ),
        None => verdict("token-budget", false, "no spend evidence"),
    });
    v.push(budget_evidence_verdict(cfg, p, now));
    v.push(verdict(
        "watchdog-quiet",
        p.watchdog_failures == 0,
        if p.watchdog_failures == 0 {
            format!("no runaway-seat failure in {WATCHDOG_QUIET_HOURS}h")
        } else {
            format!(
                "{} runaway-seat watchdog failure(s) in {WATCHDOG_QUIET_HOURS}h",
                p.watchdog_failures
            )
        },
    ));
    v.push(match p.load {
        Some((load, cpus)) if cpus > 0 => {
            let ceiling = cfg.load_per_cpu * cpus as f64;
            verdict(
                "load-ceiling",
                load < ceiling,
                format!("load {load:.2}, ceiling {ceiling:.2}"),
            )
        }
        _ => verdict("load-ceiling", false, "load unreadable"),
    });
    v.push(match p.memory {
        Some((avail, need)) if avail > 0 => verdict(
            "mem-ceiling",
            avail >= need,
            format!("{avail} bytes available, {need} needed"),
        ),
        _ => verdict("mem-ceiling", false, "memory unreadable"),
    });
    v.push(match &p.disk {
        Some((ok, detail)) => verdict("disk-headroom", *ok, detail.clone()),
        None => verdict("disk-headroom", false, "disk unreadable"),
    });
    let waves = state.waves_in_day(now);
    v.push(verdict(
        "wave-cap",
        waves < cfg.max_waves_per_day,
        format!("{waves} of {} waves in 24h", cfg.max_waves_per_day),
    ));
    v.push(verdict(
        "no-progress",
        state.breaker.is_none(),
        match &state.breaker {
            Some(b) => format!("{} — `aida shift resume` to continue", b.reason),
            None => "breaker closed".to_string(),
        },
    ));
    let launch_ok = ctx.launch_ok();
    v.push(verdict(
        "deadline",
        launch_ok,
        if launch_ok {
            "time to launch"
        } else {
            "too close to the tick deadline to launch"
        },
    ));
    v
}

fn budget_evidence_verdict(cfg: &ShiftConfig, p: &Probes, now: DateTime<Utc>) -> GuardVerdict {
    if let Some(err) = &p.vendor_error {
        return verdict(
            "budget-evidence",
            false,
            format!("cannot resolve the vendor the wave would launch: {err}"),
        );
    }
    let Some(b) = &p.budget else {
        return verdict(
            "budget-evidence",
            false,
            "no watchdog aggregates (is the `watchdog` job enabled?)",
        );
    };
    let Some(at) = b.last_run_at else {
        return verdict(
            "budget-evidence",
            false,
            "the watchdog has not recorded a run time yet",
        );
    };
    let age = now.signed_duration_since(at).num_minutes();
    if age > BUDGET_EVIDENCE_MAX_AGE_MINS {
        return verdict(
            "budget-evidence",
            false,
            format!("watchdog aggregates are {age}m old"),
        );
    }
    match b.last_verdict.as_deref() {
        Some("degraded") | Some("unknown") | None => {
            return verdict(
                "budget-evidence",
                false,
                format!(
                    "the watchdog's last run was {}",
                    b.last_verdict.as_deref().unwrap_or("unrecorded")
                ),
            )
        }
        _ => {}
    }
    let vendor = p.vendor.to_ascii_lowercase();
    if !COVERED_VENDORS.contains(&vendor.as_str()) {
        if cfg.allow_uncovered_vendors.contains(&vendor) {
            return verdict(
                "budget-evidence",
                true,
                format!(
                    "{age}m old; vendor {vendor} is NOT measured by the watchdog, allowed by the local layer"
                ),
            );
        }
        return verdict(
            "budget-evidence",
            false,
            format!("the watchdog cannot see {vendor} spend"),
        );
    }
    verdict(
        "budget-evidence",
        true,
        format!("{age}m old, vendor {vendor} measured"),
    )
}

// ---------------------------------------------------------------------------
// Side effects
// ---------------------------------------------------------------------------

/// Every side effect of a tick. Production is [`RealExec`]; tests record.
pub(crate) trait ShiftExec {
    /// Reap finished sessions; returns the count.
    fn reap(&mut self) -> usize;
    fn tag_batch(&mut self, batch: &str, specs: &[String]) -> Result<()>;
    /// Spawn the detached wave; returns (pid, pid start identity).
    fn spawn_wave(&mut self, argv: &[String], log: &Path) -> Result<(u32, Option<String>)>;
    fn save_state(&mut self, state: &ShiftState) -> Result<()>;
    fn emit(&mut self, kind: EventKind);
    /// Re-queue the `would-re-drive` decisions (`SpecReDriven` recorded
    /// first, then status back to Approved and a queue entry at the head).
    /// Returns the specs applied, and why the pass stopped when an attempt
    /// could not be recorded.
    // trace:TASK-1492 | ai:claude
    fn requeue(
        &mut self,
        decisions: &[crate::supervisor::SuperviseDecision],
        queue: &crate::supervisor::QueueTarget,
    ) -> Result<crate::supervisor::RequeueOutcome>;
    /// The ADR-26 cap branch for one decision; false when the spec moved.
    fn reclassify(&mut self, decision: &crate::supervisor::SuperviseDecision) -> Result<bool>;
    /// Notify the operator through `aida notify` (its own min_interval and
    /// quiet hours apply), waiting at most `timeout` for the command.
    /// Returns what really happened: sent, deferred to quiet hours,
    /// suppressed by the rule's min_interval, or no command configured.
    // trace:TASK-1492 | ai:claude
    fn notify(
        &mut self,
        rule: &str,
        title: &str,
        message: &str,
        timeout: StdDuration,
    ) -> Result<crate::notify::DirectDelivery>;
}

pub(crate) struct RealExec<'a> {
    pub project_root: PathBuf,
    pub backend: &'a aida_core::CachedGitBackend,
}

impl ShiftExec for RealExec<'_> {
    fn reap(&mut self) -> usize {
        crate::session_reap::reap_quiet(&self.project_root)
    }
    fn tag_batch(&mut self, batch: &str, specs: &[String]) -> Result<()> {
        tag_specs(self.backend, batch, specs)
    }
    fn spawn_wave(&mut self, argv: &[String], log: &Path) -> Result<(u32, Option<String>)> {
        let mut cmd = wave_command(&crate::aida_exe_path(), argv, &self.project_root, log)?;
        let child = cmd.spawn().context("spawning the night-shift wave")?;
        let pid = child.id();
        // The child is deliberately not waited on: it is its own session and
        // outlives this tick. Dropping the handle does not kill it.
        drop(child);
        Ok((pid, crate::process_probe::process_start_identity(pid)))
    }
    fn save_state(&mut self, state: &ShiftState) -> Result<()> {
        save_state_to(&state_path(&self.project_root), state)
    }
    fn emit(&mut self, kind: EventKind) {
        let mut ev = events::Event::new(None, "", kind);
        ev.seat = Some("night-shift".to_string());
        events::emit(&self.project_root, &ev);
    }
    // trace:TASK-1492 | ai:claude
    fn requeue(
        &mut self,
        decisions: &[crate::supervisor::SuperviseDecision],
        queue: &crate::supervisor::QueueTarget,
    ) -> Result<crate::supervisor::RequeueOutcome> {
        crate::supervisor::apply_requeue(
            self.backend,
            &self.project_root,
            decisions,
            crate::supervisor::DEFAULT_MAX_ATTEMPTS,
            Some(queue),
        )
    }
    fn reclassify(&mut self, decision: &crate::supervisor::SuperviseDecision) -> Result<bool> {
        crate::supervisor::apply_cap(
            self.backend,
            &self.project_root,
            decision,
            crate::supervisor::DEFAULT_MAX_ATTEMPTS,
        )
    }
    // trace:TASK-1492 | ai:claude
    fn notify(
        &mut self,
        rule: &str,
        title: &str,
        message: &str,
        timeout: StdDuration,
    ) -> Result<crate::notify::DirectDelivery> {
        crate::notify::send_direct_bounded(&self.project_root, rule, title, message, Some(timeout))
            .map(|o| o.delivery())
    }
}

// ---------------------------------------------------------------------------
// The tick
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct TickReport {
    pub dry_run: bool,
    pub enabled: bool,
    pub enabled_source: String,
    pub queue_user: String,
    pub queue_role: String,
    pub vendor: String,
    pub guards: Vec<GuardVerdict>,
    pub batch: Option<String>,
    pub specs: Vec<String>,
    pub reused_batch: bool,
    pub skipped: Vec<(String, String)>,
    pub argv: Vec<String>,
    pub launched: Option<ShiftLaunch>,
    pub reaped: usize,
    pub recovered_stale_pid: Option<u32>,
    pub breaker: Option<String>,
    pub escalated: Vec<String>,
    pub event_emitted: bool,
    pub redrive: String,
    pub mail_latency: String,
    /// Re-drive step verdicts (separate from the launch guards: a held
    /// re-drive never blocks a launch).
    // trace:TASK-1492 | ai:claude
    pub redrive_guards: Vec<GuardVerdict>,
    pub redrive_plan: Vec<crate::supervisor::SuperviseDecision>,
    pub redriven: Vec<String>,
    pub reclassified: Vec<String>,
    /// Set when a re-drive attempt could not be recorded: the step stopped
    /// and re-queued nothing further this tick (ADR-26 fail closed).
    pub redrive_held: Option<String>,
    pub mail: Vec<MailVerdict>,
    pub mail_escalated: Vec<String>,
    /// Notifications that failed to send (never fatal to the tick).
    pub notify_errors: Vec<String>,
}

/// One recipient's mail-latency reading.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MailVerdict {
    pub recipient: String,
    pub unread: i64,
    pub oldest_age_secs: i64,
    pub over: bool,
    /// Already escalated in the current episode.
    pub escalated_earlier: bool,
}

/// What the mail step does this tick.
// trace:TASK-1492 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MailPlan {
    pub verdicts: Vec<MailVerdict>,
    /// Over the threshold with no open episode: notify now.
    pub escalate: Vec<String>,
    /// Open episode, now back under the threshold (or read): re-arm.
    pub rearm: Vec<String>,
    /// Recipients with unread mail that match no known seat, role, roster
    /// member or session. Never escalated.
    pub unknown: Vec<String>,
}

/// At most this many recipient names in one mail-latency notification or
/// report line; the rest are summarized as "+N more".
// trace:TASK-1492 | ai:claude
pub(crate) const MAIL_NOTIFY_MAX_NAMES: usize = 5;

/// Join at most `max` items, then "+N more" for the rest.
// trace:TASK-1492 | ai:claude
pub(crate) fn cap_list(items: &[String], max: usize, sep: &str) -> String {
    let mut out = items
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(sep);
    if items.len() > max {
        out.push_str(&format!("{sep}+{} more", items.len() - max));
    }
    out
}

/// Is `recipient` a known seat, role, roster member or session identity?
// trace:TASK-1492 | ai:claude
fn is_known_recipient(known: &BTreeSet<String>, recipient: &str) -> bool {
    known.contains(&recipient.trim().to_lowercase())
}

/// PURE: once per episode per recipient. Only KNOWN recipients (`known`,
/// lower-cased) are considered; mail to any other address (a typo, a stray
/// number, a file name) is listed in `unknown` and never escalates. A known
/// recipient over the threshold with no open episode escalates; one with an
/// open episode stays quiet; an open episode whose recipient is back under
/// the threshold, or no longer known, re-arms.
// trace:TASK-1492 | ai:claude
pub(crate) fn mail_escalations(
    threshold_secs: i64,
    unread: &BTreeMap<String, crate::mailbox_store::RecipientUnread>,
    known: &BTreeSet<String>,
    episodes: &BTreeMap<String, DateTime<Utc>>,
    now: DateTime<Utc>,
) -> MailPlan {
    let mut plan = MailPlan::default();
    for (recipient, u) in unread {
        if !is_known_recipient(known, recipient) {
            plan.unknown.push(recipient.clone());
            continue;
        }
        let age = ((now.timestamp_millis() - u.oldest_ts) / 1000).max(0);
        let over = age > threshold_secs;
        let open = episodes.contains_key(recipient);
        if over && !open {
            plan.escalate.push(recipient.clone());
        }
        plan.verdicts.push(MailVerdict {
            recipient: recipient.clone(),
            unread: u.count,
            oldest_age_secs: age,
            over,
            escalated_earlier: over && open,
        });
    }
    for recipient in episodes.keys() {
        let still_over = plan
            .verdicts
            .iter()
            .any(|v| &v.recipient == recipient && v.over);
        if !still_over {
            plan.rearm.push(recipient.clone());
        }
    }
    plan
}

/// The `redrive-lock-free` detail for a launched wave that exited but is not
/// settled yet. It describes the state only: enabling the shift grants
/// unattended launches, so it is never suggested just to clear this hold.
// trace:BUG-1623 | ai:claude
pub(crate) fn unsettled_wave_detail(shift_enabled: bool) -> String {
    if shift_enabled {
        "wave unsettled: the last shift wave has exited and waits for the next shift tick \
         to settle it"
            .to_string()
    } else {
        "wave unsettled: the last shift wave has exited, but a disabled shift does not tick, \
         so re-drive waits until the shift runs again"
            .to_string()
    }
}

/// The re-drive step's own guards. Evaluated only when re-drive is on.
// trace:TASK-1492 | ai:claude
fn redrive_guards(
    p: &Probes,
    state: &ShiftState,
    ctx: &TickCtx,
    shift_enabled: bool,
) -> Vec<GuardVerdict> {
    let wave_open = state.last_launched().is_some_and(|w| w.outcome.is_none());
    let lock_live = match p.lock {
        LockView::Running(pid) => Some(format!("a drain holds the lock (pid {pid})")),
        _ if p.foreign_claim.is_some() => p.foreign_claim.clone(),
        _ if wave_open && p.last_wave_alive => Some("a shift wave is still running".to_string()),
        // A launched wave that has exited but was never settled: its
        // outcome (and so the zero-progress breaker) is still unknown. The
        // tick settles before this step, so only an out-of-tick caller
        // (`supervise watch --execute`) can see one; it holds until the next
        // tick settles the wave (BUG-1621 B1). The message states the
        // state only; it never advises enabling the shift (BUG-1623 n1).
        // trace:BUG-1621 trace:BUG-1623 | ai:claude
        _ if wave_open => Some(unsettled_wave_detail(shift_enabled)),
        _ => None,
    };
    vec![
        verdict(
            "redrive-evidence",
            p.redrive_history.is_ok(),
            match &p.redrive_history {
                Ok(_) => "attempts counted from the event stream and its archive".to_string(),
                Err(e) => format!("cannot count attempts: {e}"),
            },
        ),
        verdict(
            "redrive-lock-free",
            lock_live.is_none(),
            lock_live.unwrap_or_else(|| "no drain is running".to_string()),
        ),
        verdict(
            "redrive-breaker",
            state.breaker.is_none(),
            match &state.breaker {
                Some(b) => format!("launches are stopped ({})", b.reason),
                None => "breaker closed".to_string(),
            },
        ),
        verdict(
            "redrive-state",
            ctx.state_error.is_none(),
            ctx.state_error.clone().unwrap_or_else(|| "ok".to_string()),
        ),
        verdict(
            "redrive-deadline",
            ctx.launch_ok(),
            if ctx.launch_ok() {
                "time to re-drive"
            } else {
                "too close to the tick deadline"
            },
        ),
    ]
}

/// The plan action of a re-drive or cap decision on a held pass.
// trace:BUG-1621 | ai:claude
pub(crate) const REDRIVE_HELD: &str = "held";

/// Step 3b: plan (and, live, apply) the opt-in re-drive. Returns the
/// candidates the re-queued specs become, head first, so they join this
/// tick's wave selection.
// trace:TASK-1492 | ai:claude
fn redrive_step(
    cfg: &ShiftConfig,
    p: &Probes,
    state: &ShiftState,
    ctx: &TickCtx,
    live: bool,
    report: &mut TickReport,
    exec: &mut dyn ShiftExec,
) -> Result<Vec<Candidate>> {
    use crate::supervisor::{plan_redrives, QueueTarget, RedriveFloors, SuperviseOpts};
    if !cfg.redrive {
        report.redrive = if cfg.committed_redrive_ignored {
            "off (ADR-26 default; the committed `redrive = true` is ignored — the switch is per clone)".to_string()
        } else {
            "off (ADR-26 default)".to_string()
        };
        return Ok(Vec::new());
    }
    report.redrive_guards = redrive_guards(p, state, ctx, cfg.enabled);
    let Ok(history) = &p.redrive_history else {
        report.redrive = "held: redrive-evidence".to_string();
        return Ok(Vec::new());
    };
    let opts = SuperviseOpts {
        max: Some(cfg.max_redrives_per_tick),
        floors: Some(RedriveFloors {
            drain_mode_only: true,
            exclude_keystone: true,
            held: p.held.clone(),
        }),
        ..Default::default()
    };
    let mut plan = plan_redrives(&p.parked, history, &opts, ctx.now);
    let held: Vec<&str> = report
        .redrive_guards
        .iter()
        .filter(|g| !g.pass)
        .map(|g| g.name)
        .collect();
    let would: Vec<String> = plan
        .iter()
        .filter(|d| d.action == "would-re-drive")
        .map(|d| d.spec.clone())
        .collect();
    let capped: Vec<String> = plan
        .iter()
        .filter(|d| d.action == "reclassify-needs-human")
        .map(|d| d.spec.clone())
        .collect();
    if !held.is_empty() {
        report.redrive = format!("held: {}", held.join(", "));
        // Nothing is applied on a held pass, so the plan says so instead of
        // listing actions that will not happen.
        // trace:BUG-1621 | ai:claude
        for d in plan.iter_mut() {
            if d.action == "would-re-drive" {
                d.attempts = d.attempts.saturating_sub(1);
                d.action = REDRIVE_HELD.to_string();
            } else if d.action == "reclassify-needs-human" {
                d.action = REDRIVE_HELD.to_string();
            }
        }
        report.redrive_plan = plan;
        return Ok(Vec::new());
    }
    let requeued: Vec<String> = if live {
        for d in plan
            .iter_mut()
            .filter(|d| d.action == "reclassify-needs-human")
        {
            if exec.reclassify(d)? {
                report.reclassified.push(d.spec.clone());
            } else {
                d.action = "skip-status-moved".to_string();
            }
        }
        let queue = QueueTarget {
            user: p.queue_user.clone(),
            role: WAVE_ROLE.to_string(),
        };
        let outcome = exec.requeue(&plan, &queue)?;
        let applied = outcome.applied;
        // ADR-26 fail closed: after an attempt that could not be recorded,
        // every spec not yet re-queued stays parked for this tick.
        for d in plan.iter_mut().filter(|d| d.action == "would-re-drive") {
            d.action = if applied.contains(&d.spec) {
                "re-drive".to_string()
            } else if outcome.held.contains(&d.spec) {
                crate::supervisor::HELD_UNRECORDED.to_string()
            } else {
                "skip-status-moved".to_string()
            };
            if d.action != "re-drive" {
                d.attempts = d.attempts.saturating_sub(1);
            }
        }
        report.redrive_held = outcome.unrecorded;
        report.redriven = applied.clone();
        applied
    } else {
        would.clone()
    };
    let verb = if live { "re-queued" } else { "would re-queue" };
    let mut parts = Vec::new();
    if !requeued.is_empty() {
        parts.push(format!("{verb} {}", requeued.join(", ")));
    }
    let left: &[String] = if live { &report.reclassified } else { &capped };
    if !left.is_empty() {
        parts.push(format!(
            "{} {} for a human (re-drive cap reached)",
            if live { "left" } else { "would leave" },
            left.join(", ")
        ));
    }
    report.redrive = if let Some(reason) = &report.redrive_held {
        let mut line =
            format!("held: attempt-record — {reason}; nothing more re-queued this check");
        if !parts.is_empty() {
            line.push_str(&format!(" ({})", parts.join("; ")));
        }
        line
    } else if parts.is_empty() {
        "on — nothing to re-drive".to_string()
    } else {
        format!("on — {}", parts.join("; "))
    };
    report.redrive_plan = plan;
    Ok(requeued
        .iter()
        .filter_map(|spec| p.parked.iter().find(|r| &r.display_id() == spec))
        .map(|r| Candidate {
            spec: r.display_id(),
            status: RequirementStatus::Approved,
            execution_mode: r.execution_mode,
            req_type: r.req_type.to_string(),
            tags: r.tags.iter().cloned().collect(),
            merge_held: false,
        })
        .collect())
}

/// Step 6b: mail latency. Reads are reported on dry runs; notifications and
/// episode changes happen only on a live tick with time left.
// trace:TASK-1492 | ai:claude
fn mail_step(
    cfg: &ShiftConfig,
    p: &Probes,
    state: &mut ShiftState,
    ctx: &TickCtx,
    live: bool,
    report: &mut TickReport,
    exec: &mut dyn ShiftExec,
) {
    let unread = match &p.mail {
        Ok(u) => u,
        Err(e) => {
            report.mail_latency = format!("mailbox unreadable, not checked: {e}");
            return;
        }
    };
    let plan = mail_escalations(
        cfg.mail_latency_secs,
        unread,
        &p.mail_known,
        &state.mail_episodes,
        ctx.now,
    );
    report.mail = plan.verdicts.clone();
    let over: Vec<String> = plan
        .verdicts
        .iter()
        .filter(|v| v.over)
        .map(|v| {
            format!(
                "{} {}",
                v.recipient,
                human_age(Duration::seconds(v.oldest_age_secs))
            )
        })
        .collect();
    let escalate_names = cap_list(&plan.escalate, MAIL_NOTIFY_MAX_NAMES, ", ");
    report.mail_latency = if over.is_empty() {
        format!("ok (nothing unread past {})", cfg.mail_latency)
    } else {
        format!(
            "{} over {}{}",
            cap_list(&over, MAIL_NOTIFY_MAX_NAMES, ", "),
            cfg.mail_latency,
            if plan.escalate.is_empty() {
                " — already notified this episode".to_string()
            } else if live {
                String::new()
            } else {
                format!(" — would notify for {escalate_names}")
            }
        )
    };
    if !plan.unknown.is_empty() {
        report.mail_latency.push_str(&format!(
            " ({} unknown recipient(s) ignored)",
            plan.unknown.len()
        ));
    }
    if !live || !ctx.optional_ok() {
        return;
    }
    for r in &plan.rearm {
        state.mail_episodes.remove(r);
    }
    if plan.escalate.is_empty() {
        return;
    }
    let lines: Vec<String> = plan
        .verdicts
        .iter()
        .filter(|v| plan.escalate.contains(&v.recipient))
        .map(|v| {
            format!(
                "{}: {} unread, oldest {} old",
                v.recipient,
                v.unread,
                human_age(Duration::seconds(v.oldest_age_secs))
            )
        })
        .collect();
    let message = format!(
        "Mail is waiting longer than {}:\n{}\n",
        cfg.mail_latency,
        cap_list(&lines, MAIL_NOTIFY_MAX_NAMES, "\n")
    );
    // An episode opens only when the operator will actually get the
    // message: sent now, or queued for the end of quiet hours. A message the
    // rule's min_interval dropped opens nothing, so a later check retries.
    // trace:TASK-1492 | ai:claude
    use crate::notify::DirectDelivery;
    match exec.notify(
        "mail-latency",
        "AIDA: mail waiting",
        &message,
        ctx.notify_timeout(),
    ) {
        Ok(delivery @ (DirectDelivery::Sent | DirectDelivery::Deferred)) => {
            for r in &plan.escalate {
                state.mail_episodes.insert(r.clone(), ctx.now);
            }
            report.mail_escalated = plan.escalate.clone();
            report.mail_latency.push_str(&if delivery == DirectDelivery::Sent {
                format!(" — notified for {escalate_names}")
            } else {
                format!(" — notification for {escalate_names} queued until quiet hours end")
            });
        }
        Ok(DirectDelivery::Suppressed) => report.mail_latency.push_str(&format!(
            " — notification for {escalate_names} held by `[notify] min_interval`; retried on a later check"
        )),
        Ok(DirectDelivery::NotConfigured) => report
            .mail_latency
            .push_str(" — no notify command configured (`[notify] command`)"),
        Err(e) => report.notify_errors.push(format!("mail-latency: {e:#}")),
    }
}

impl TickReport {
    pub(crate) fn refused(&self) -> Vec<String> {
        self.guards
            .iter()
            .filter(|g| !g.pass)
            .map(|g| g.name.to_string())
            .collect()
    }
}

/// A8(a)/(b)/(c): fold the outcome of a finished wave into the breakers.
/// Returns the breaker reason when it trips on this call.
fn settle_last_wave(state: &mut ShiftState, p: &Probes, now: DateTime<Utc>) -> Option<String> {
    let idx = state
        .waves
        .iter()
        .rposition(|w| w.pid.is_some() && w.outcome.is_none())?;
    if p.last_wave_alive {
        return None;
    }
    let at = state.waves[idx].at;
    let (shipped, shelved) = p
        .queue_drained
        .iter()
        .filter(|(ts, _, _)| *ts >= at)
        .fold((0, 0), |(a, b), (_, s, h)| (a + s, b + h));
    let seen_drained = p.queue_drained.iter().any(|(ts, _, _)| *ts >= at);
    let progress = seen_drained && shipped + shelved > 0;
    state.waves[idx].outcome = Some(WaveOutcome {
        settled_at: now,
        shipped,
        shelved,
        progress,
    });
    for spec in &state.waves[idx].specs.clone() {
        if p.finished_specs.contains(spec) {
            state.spec_waves.remove(spec);
            state.escalated.remove(spec);
        }
    }
    if progress {
        state.consecutive_zero_progress = 0;
        return None;
    }
    state.consecutive_zero_progress += 1;
    if state.consecutive_zero_progress >= ZERO_PROGRESS_BREAKER && state.breaker.is_none() {
        let reason = format!(
            "{} consecutive shift waves made no progress",
            state.consecutive_zero_progress
        );
        state.breaker = Some(Breaker {
            tripped_at: now,
            reason: reason.clone(),
            queue_fingerprint: p.queue_fingerprint.clone(),
        });
        return Some(reason);
    }
    None
}

fn prune_state(state: &mut ShiftState, now: DateTime<Utc>) {
    let day = now - Duration::hours(24);
    for times in state.spec_waves.values_mut() {
        times.retain(|t| *t > day);
    }
    state.spec_waves.retain(|_, v| !v.is_empty());
    let spec_waves = state.spec_waves.clone();
    state
        .escalated
        .retain(|s| spec_waves.get(s).is_some_and(|v| v.len() >= SPEC_WAVE_CAP));
    let keep_from = now - Duration::hours(48);
    let last = state.waves.len().saturating_sub(1);
    let mut i = 0;
    state.waves.retain(|w| {
        let keep = i == last || w.at > keep_from;
        i += 1;
        keep
    });
}

/// Does a drain-lock command name `batch` as its `--batch`?
// trace:TASK-1497 | ai:claude
fn lock_command_names_batch(command: &str, batch: &str) -> bool {
    let words: Vec<&str> = command.split_whitespace().collect();
    words.windows(2).any(|w| w[0] == "--batch" && w[1] == batch)
}

/// A4 kill window: a tick killed after the spawn but before the pid was
/// saved leaves a pid-less intent while its wave runs. When the live drain
/// lock is held by a drain of that intent's batch, the wave DID launch:
/// adopt the holder's pid so the wave counts toward waves-per-day, the
/// per-spec cap and progress settlement. Returns true when it adopted.
// trace:TASK-1497 | ai:claude
fn adopt_launched_intent(state: &mut ShiftState, p: &Probes) -> bool {
    let (LockView::Running(_), Some(holder)) = (p.lock, &p.lock_holder) else {
        return false;
    };
    let Some(intent) = state.pending_intent() else {
        return false;
    };
    if !lock_command_names_batch(&holder.command, &intent.batch) {
        return false;
    }
    let (at, specs) = (intent.at, intent.specs.clone());
    if let Some(last) = state.waves.last_mut() {
        last.pid = Some(holder.pid);
        last.pid_start = holder.pid_start.clone();
    }
    for spec in specs {
        state.spec_waves.entry(spec).or_default().push(at);
    }
    true
}

/// One tick over already-gathered inputs. Mutates `state` (the caller
/// passes a clone on a dry run) and performs side effects only through
/// `exec`, and never on a dry run.
pub(crate) fn tick_core(
    cfg: &ShiftConfig,
    probes: &Probes,
    state: &mut ShiftState,
    ctx: &TickCtx,
    exec: &mut dyn ShiftExec,
) -> Result<TickReport> {
    let now = ctx.now;
    let mut report = TickReport {
        dry_run: ctx.dry_run,
        enabled: cfg.enabled,
        enabled_source: cfg.enabled_source.clone(),
        queue_user: probes.queue_user.clone(),
        queue_role: WAVE_ROLE.to_string(),
        vendor: probes.vendor.clone(),
        redrive: "off (ADR-26 default)".to_string(),
        mail_latency: "not checked".to_string(),
        ..Default::default()
    };
    let live = cfg.enabled && !ctx.dry_run;

    // 1. Reap first, so dead-pid leases are freed before a wave starts.
    if live && ctx.optional_allowed && ctx.state_error.is_none() {
        report.reaped = exec.reap();
    }

    // 2. Breakers: prune, adopt a launched pid-less intent, settle the last
    // wave, resume on a queue change.
    prune_state(state, now);
    // trace:TASK-1497 | ai:claude
    let adopted_probes;
    let probes = if adopt_launched_intent(state, probes) {
        // The adopted wave holds a live lock, so it is the live last wave.
        adopted_probes = Probes {
            last_wave_alive: true,
            ..probes.clone()
        };
        &adopted_probes
    } else {
        probes
    };
    let tripped = settle_last_wave(state, probes, now);
    if tripped.is_none() {
        if let Some(b) = &state.breaker {
            if b.queue_fingerprint != probes.queue_fingerprint {
                state.breaker = None;
                state.consecutive_zero_progress = 0;
            }
        }
    }
    report.breaker = tripped.clone();

    // 3. Stale lock: report once; the wave's own acquire reclaims it.
    let stale_pid = match probes.lock {
        LockView::Stale(pid) => Some(pid),
        _ => None,
    };
    if stale_pid.is_some() && stale_pid != state.last_stale_pid {
        report.recovered_stale_pid = stale_pid;
    }
    state.last_stale_pid = stale_pid;

    // 3b. Opt-in re-drive (ADR-26 default off). Re-queued specs join this
    // tick's selection at the head, oldest-parked first (A7).
    // trace:TASK-1492 | ai:claude
    let redriven = redrive_step(cfg, probes, state, ctx, live, &mut report, exec)?;
    let candidates: Vec<Candidate> = redriven
        .iter()
        .cloned()
        .chain(
            probes
                .candidates
                .iter()
                .filter(|c| !redriven.iter().any(|r| r.spec == c.spec))
                .cloned(),
        )
        .collect();

    // 4. Select and guard.
    let sel = select_wave(cfg, &candidates, state, now);
    let mut escalated_now = Vec::new();
    for spec in &sel.capped {
        if state.escalated.insert(spec.clone()) {
            escalated_now.push(spec.clone());
        }
    }
    report.escalated = escalated_now.clone();
    report.batch = sel.batch.clone();
    report.specs = sel.specs.clone();
    report.reused_batch = sel.reused;
    report.skipped = sel.skipped.clone();
    let spent = probes.budget.as_ref().map(|b| b.spent_24h).unwrap_or(0);
    if let Some(batch) = &sel.batch {
        report.argv = build_wave_argv(cfg, batch, wave_max_tokens(cfg, spent));
    }
    report.guards = evaluate_guards(cfg, probes, state, &sel, ctx);
    let all_pass = report.guards.iter().all(|g| g.pass);

    // 5. Launch: record intent -> tag -> spawn -> record pid (A4).
    if live && all_pass {
        let batch = sel.batch.clone().expect("drain-mode-only passed");
        let intent = WaveRecord {
            batch: batch.clone(),
            specs: sel.specs.clone(),
            at: now,
            argv: report.argv.clone(),
            pid: None,
            pid_start: None,
            log: Some(ctx.log_path.display().to_string()),
            outcome: None,
        };
        if sel.reused {
            if let Some(last) = state.waves.last_mut() {
                *last = intent;
            }
        } else {
            state.waves.push(intent);
        }
        exec.save_state(state)?;
        if sel.auto_tag {
            exec.tag_batch(&batch, &sel.specs)?;
        }
        // A4: re-read the clock after the reap, intent write and tagging.
        // Too late to spawn safely: leave the recorded intent (no pid) for
        // the next tick, which reuses this batch instead of tagging anew.
        if !ctx.launch_ok() {
            if let Some(g) = report.guards.iter_mut().find(|g| g.name == "deadline") {
                g.pass = false;
                g.detail =
                    "deadline reached before the spawn; the next check reuses this batch".into();
            }
        } else {
            let (pid, pid_start) = exec.spawn_wave(&report.argv, &ctx.log_path)?;
            if let Some(last) = state.waves.last_mut() {
                last.pid = Some(pid);
                last.pid_start = pid_start;
            }
            for spec in &sel.specs {
                state.spec_waves.entry(spec.clone()).or_default().push(now);
            }
            // TASK-1497: persist the pid at once, before the event, so a kill
            // from here on cannot lose the launch record.
            // trace:TASK-1497 | ai:claude
            exec.save_state(state)?;
            report.launched = Some(ShiftLaunch {
                batch,
                specs: sel.specs.clone(),
                pid,
                argv: report.argv.clone(),
            });
        }
    }

    // 5b. A breaker trip reaches the operator through notify (A8(b)), and
    // the mail-latency step runs.
    // trace:TASK-1492 | ai:claude
    if live {
        if let Some(reason) = &report.breaker {
            let message = format!(
                "The night shift stopped launching drain waves: {reason}.\n\
                 It launches again after `aida shift resume` or a change to the queue.\n"
            );
            match exec.notify(
                "shift-breaker",
                "AIDA: night shift stopped",
                &message,
                ctx.notify_timeout(),
            ) {
                Ok(_) => {}
                Err(e) => report.notify_errors.push(format!("shift-breaker: {e:#}")),
            }
        }
    }
    if cfg.enabled || ctx.dry_run {
        mail_step(cfg, probes, state, ctx, live, &mut report, exec);
    }

    // 6. One event, only when the tick acted or its verdicts changed.
    let refused = report.refused();
    let acted = report.launched.is_some()
        || report.reaped > 0
        || report.recovered_stale_pid.is_some()
        || report.breaker.is_some()
        || !report.escalated.is_empty()
        || !report.redriven.is_empty()
        || !report.reclassified.is_empty()
        || !report.mail_escalated.is_empty()
        || report.redrive_held.is_some();
    if live && (acted || refused != state.last_refused) {
        exec.emit(EventKind::ShiftTick {
            launched: report.launched.clone(),
            reaped: report.reaped,
            recovered_stale_pid: report.recovered_stale_pid,
            refused: refused.clone(),
            breaker: report.breaker.clone(),
            escalated: report.escalated.clone(),
            redriven: report.redriven.clone(),
            reclassified: report.reclassified.clone(),
            mail_escalated: report.mail_escalated.clone(),
            redrive_held: report.redrive_held.clone(),
        });
        report.event_emitted = true;
    }
    if live {
        state.last_refused = refused;
        state.last_tick_at = Some(now);
        state.version = STATE_VERSION;
        if ctx.state_error.is_none() {
            exec.save_state(state)?;
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Gathering (production inputs)
// ---------------------------------------------------------------------------

/// The vendor the wave will actually spend, resolved through the SAME
/// launch preflight `queue work` uses (`resolve_enabled_headless_vendor`, so
/// an `[agents] enabled` profile list is honoured). An error is returned as
/// the second element and makes the budget-evidence guard refuse.
pub(crate) fn resolve_wave_vendor(project_root: &Path) -> (String, Option<String>) {
    match crate::session::resolve_enabled_headless_vendor(project_root) {
        Ok(v) => (v.as_str().to_string(), None),
        Err(e) => (
            crate::session::resolve_headless_vendor(project_root)
                .as_str()
                .to_string(),
            Some(format!("{e:#}")),
        ),
    }
}

fn no_human_ack_source(project_root: &Path) -> Option<String> {
    if crate::home_dir()
        .map(|h| h.join(".aida").join("no-human-acknowledged").exists())
        .unwrap_or(false)
    {
        return Some("~/.aida/no-human-acknowledged".to_string());
    }
    if project_root
        .join(".aida")
        .join("no-human-acknowledged")
        .exists()
    {
        return Some(".aida/no-human-acknowledged".to_string());
    }
    None
}

fn held_specs(project_root: &Path) -> BTreeSet<String> {
    crate::merge_hold::list_holds(project_root)
        .into_iter()
        .filter_map(|(pr, _)| crate::merge_hold::read_hold_record(project_root, pr))
        .filter_map(|r| r.spec)
        .map(|s| s.to_ascii_uppercase())
        .collect()
}

fn gather_candidates(
    backend: &aida_core::CachedGitBackend,
    user: &str,
    held: &BTreeSet<String>,
) -> Vec<Candidate> {
    let Ok(entries) = crate::queue_role_fallback::queue_list_with_role_fallback(
        backend,
        user,
        Some(WAVE_ROLE),
        false,
    ) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|e| crate::entry_matches_role_filter(e.for_role.as_deref(), Some(WAVE_ROLE), false))
        .filter_map(|e| backend.get_requirement(&e.requirement_id).ok().flatten())
        .map(|req| {
            let spec = req.display_id();
            Candidate {
                merge_held: held.contains(&spec.to_ascii_uppercase()),
                spec,
                status: req.status.clone(),
                execution_mode: req.execution_mode,
                req_type: req.req_type.to_string(),
                tags: req.tags.iter().cloned().collect(),
            }
        })
        .collect()
}

/// This clone's drain lock (and its live holder) plus a live drain claim held
/// by another clone. Shared by the tick and the out-of-tick re-drive pass.
// trace:TASK-1497 trace:BUG-1621 | ai:claude
fn probe_drain_locks(
    project_root: &Path,
    now: DateTime<Utc>,
) -> (LockView, Option<LockHolder>, Option<String>) {
    let (lock, lock_holder) = match crate::drain_lock::probe_lock(project_root) {
        crate::drain_lock::LockStatus::None => (LockView::Free, None),
        crate::drain_lock::LockStatus::Running(l) => (
            LockView::Running(l.pid),
            Some(LockHolder {
                pid: l.pid,
                pid_start: l.pid_start_time,
                command: l.command,
            }),
        ),
        crate::drain_lock::LockStatus::Stale(l) => (LockView::Stale(l.pid), None),
    };
    let foreign_claim = crate::coordination::live_foreign_lock_claim(
        &project_root.join(".aida-store"),
        crate::coordination::LockKind::Drain,
        project_root,
        now,
        crate::process_probe::process_identity_is_alive,
    )
    .map(|c| {
        format!(
            "another clone is draining ({} on {}, pid {})",
            c.clone_path, c.host, c.pid
        )
    });
    (lock, lock_holder, foreign_claim)
}

/// Is the last launched shift wave's process still alive?
// trace:BUG-1621 | ai:claude
fn last_wave_is_alive(state: &ShiftState) -> bool {
    state
        .waves
        .iter()
        .rev()
        .find(|w| w.pid.is_some())
        .is_some_and(|w| {
            w.outcome.is_none()
                && crate::process_probe::process_identity_is_alive(
                    w.pid.unwrap_or_default(),
                    w.pid_start.as_deref(),
                )
        })
}

fn gather_probes(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    cfg: &ShiftConfig,
    state: &ShiftState,
    now: DateTime<Utc>,
) -> Probes {
    let (lock, lock_holder, foreign_claim) = probe_drain_locks(project_root, now);
    let events = events::read_all(project_root);
    let watchdog_jobs: Vec<String> =
        crate::maintenance_schedule::jobs_running_command(project_root, WATCHDOG_COMMAND)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
    let quiet_from = now - Duration::hours(WATCHDOG_QUIET_HOURS);
    let watchdog_failures = events
        .iter()
        .filter(|e| e.ts >= quiet_from)
        .filter(|e| matches!(&e.kind, EventKind::CronJobFailed { job, .. } if watchdog_jobs.contains(job)))
        .count();
    let queue_drained = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::QueueDrained {
                shipped, shelved, ..
            } => Some((e.ts, *shipped, *shelved)),
            _ => None,
        })
        .collect();
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    let load = Some((sysinfo::System::load_average().one, cpus));
    let (_, mem_need) =
        crate::machine_readiness::thresholds(cfg.max_runtime_hours, 1, cfg.wave_size);
    let memory = Some((system.available_memory(), mem_need));
    let readiness =
        crate::machine_readiness::probe(project_root, cfg.max_runtime_hours, 1, cfg.wave_size);
    let disk_checks: Vec<_> = readiness
        .checks
        .iter()
        .filter(|c| c.name.starts_with("disk:"))
        .collect();
    let disk = (!disk_checks.is_empty()).then(|| {
        let failing: Vec<_> = disk_checks
            .iter()
            .filter(|c| c.level == crate::machine_readiness::Level::Fail)
            .collect();
        match failing.first() {
            Some(c) => (false, c.detail.clone()),
            None => (
                true,
                format!("{} volume(s) have headroom", disk_checks.len()),
            ),
        }
    });
    let queue_user = crate::current_user_id(None);
    let held = held_specs(project_root);
    let candidates = gather_candidates(backend, &queue_user, &held);
    let mut queue_fingerprint: Vec<String> = candidates.iter().map(|c| c.spec.clone()).collect();
    queue_fingerprint.sort();
    let last = state.waves.iter().rev().find(|w| w.pid.is_some());
    let last_wave_alive = last_wave_is_alive(state);
    let mut finished_specs = BTreeSet::new();
    if let Some(w) = last {
        for spec in &w.specs {
            let status_done = backend
                .get_requirement_by_spec_id(spec)
                .ok()
                .flatten()
                .is_some_and(|r| {
                    !matches!(
                        r.status,
                        RequirementStatus::Approved
                            | RequirementStatus::Planned
                            | RequirementStatus::InProgress
                            | RequirementStatus::Draft
                    )
                });
            let event_done = events.iter().any(|e| {
                e.ts >= w.at && e.spec.as_deref() == Some(spec.as_str()) && e.kind.is_terminal()
            });
            if status_done || event_done {
                finished_specs.insert(spec.clone());
            }
        }
    }
    let (wave_vendor, wave_vendor_error) = resolve_wave_vendor(project_root);
    let (parked, redrive_history) = redrive_probes(project_root, backend, cfg);
    let mail = crate::mailbox_store::oldest_unread_by_recipient(
        project_root,
        &project_root.join(".aida-store"),
    )
    .map_err(|e| format!("{e:#}"));
    let mail_known = if mail.as_ref().is_ok_and(|m| !m.is_empty()) {
        known_mail_recipients(project_root)
    } else {
        BTreeSet::new()
    };
    Probes {
        lock,
        lock_holder,
        foreign_claim,
        no_human_ack: no_human_ack_source(project_root),
        budget: crate::runaway_seats::budget_evidence(
            &project_root
                .join(".aida")
                .join("watchdog")
                .join("state.json"),
            now,
        ),
        vendor: wave_vendor.clone(),
        vendor_error: wave_vendor_error.clone(),
        watchdog_failures,
        load,
        memory,
        disk,
        queue_user,
        candidates,
        queue_fingerprint,
        last_wave_alive,
        queue_drained,
        finished_specs,
        parked,
        held,
        redrive_history,
        mail,
        mail_known,
    }
}

/// The re-drive inputs, read only when re-drive is on: the parked specs and
/// the strict (fail-closed) attempt history from the live stream and its
/// archive.
// trace:TASK-1492 | ai:claude
pub(crate) type RedriveProbes = (
    Vec<aida_core::Requirement>,
    std::result::Result<events::RedriveHistory, String>,
);

// trace:TASK-1492 | ai:claude
pub(crate) fn redrive_probes<B: aida_core::DatabaseBackend>(
    project_root: &Path,
    backend: &B,
    cfg: &ShiftConfig,
) -> RedriveProbes {
    if !cfg.redrive {
        return (Vec::new(), Err("re-drive is off".to_string()));
    }
    match backend.load() {
        Ok(store) => (
            store
                .requirements
                .into_iter()
                .filter(|r| r.status == RequirementStatus::NeedsAttention)
                .collect(),
            events::read_redrive_history_strict(project_root),
        ),
        Err(e) => (Vec::new(), Err(format!("cannot read the store: {e:#}"))),
    }
}

// ---------------------------------------------------------------------------
// BUG-1621: the re-drive step for a caller outside the tick
// ---------------------------------------------------------------------------

/// The re-drive step for a caller outside the tick (`aida supervise watch
/// --execute`). It runs the tick's own [`redrive_step`], so that caller gets
/// exactly the same pipeline and floors: the per-clone opt-in (ADR-26 fork C,
/// default off), the redrive-evidence / lock / breaker / state guards, the
/// drain-mode, keystone, merge-hold and needs-human floors, the attempt
/// recorded before each requeue (and the rest of the pass held when it
/// cannot be), and the cap branch. Re-driven specs go back to the head of the
/// queue for the next drain wave; nothing is launched here, so there is no
/// foreground `queue work` and no forced claim. Side effects go only through
/// `exec`, and none on a dry run.
// trace:BUG-1621 | ai:claude
pub(crate) fn redrive_pass(
    cfg: &ShiftConfig,
    p: &Probes,
    state: &ShiftState,
    ctx: &TickCtx,
    exec: &mut dyn ShiftExec,
) -> Result<TickReport> {
    let mut report = TickReport {
        dry_run: ctx.dry_run,
        enabled: cfg.enabled,
        enabled_source: cfg.enabled_source.clone(),
        queue_user: p.queue_user.clone(),
        queue_role: WAVE_ROLE.to_string(),
        redrive: "off (ADR-26 default)".to_string(),
        ..Default::default()
    };
    redrive_step(cfg, p, state, ctx, !ctx.dry_run, &mut report, exec)?;
    Ok(report)
}

impl Probes {
    /// The probes [`redrive_pass`] reads, with every launch-only field left
    /// neutral (the out-of-tick pass never launches a wave).
    // trace:BUG-1621 | ai:claude
    pub(crate) fn for_redrive(
        lock: LockView,
        foreign_claim: Option<String>,
        last_wave_alive: bool,
        queue_user: String,
        (parked, redrive_history): RedriveProbes,
        held: BTreeSet<String>,
    ) -> Self {
        Probes {
            lock,
            lock_holder: None,
            foreign_claim,
            no_human_ack: None,
            budget: None,
            vendor: String::new(),
            vendor_error: None,
            watchdog_failures: 0,
            load: None,
            memory: None,
            disk: None,
            queue_user,
            candidates: Vec::new(),
            queue_fingerprint: Vec::new(),
            last_wave_alive,
            queue_drained: Vec::new(),
            finished_specs: BTreeSet::new(),
            parked,
            held,
            redrive_history,
            mail: Ok(BTreeMap::new()),
            mail_known: BTreeSet::new(),
        }
    }
}

/// Production shell of [`redrive_pass`]: this clone's shift config (the
/// local layer holds the opt-in), then [`run_redrive_pass_with`].
// trace:BUG-1621 | ai:claude
pub(crate) fn run_redrive_pass(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    dry_run: bool,
) -> Result<TickReport> {
    run_redrive_pass_with(project_root, backend, &load_config(project_root), dry_run)
}

/// [`run_redrive_pass`] with the config injected (tests build it without
/// reading `~/.aida`). A live pass takes `.aida/shift.lock` like a tick, so
/// it never interleaves with one: when a tick holds the lock, the pass
/// re-drives nothing and leaves the parks to that tick.
// trace:BUG-1621 | ai:claude
pub(crate) fn run_redrive_pass_with(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    cfg: &ShiftConfig,
    dry_run: bool,
) -> Result<TickReport> {
    let now = Utc::now();
    let mut exec = RealExec {
        project_root: project_root.to_path_buf(),
        backend,
    };
    let quiet_ctx = |state_error| TickCtx {
        now,
        dry_run,
        optional_allowed: true,
        launch_allowed: true,
        state_error,
        log_path: PathBuf::new(),
        clock: None,
    };
    if !cfg.redrive {
        // Off: the step reports why and returns before reading or writing
        // anything, so nothing is probed.
        let p = Probes::for_redrive(
            LockView::Free,
            None,
            false,
            String::new(),
            (Vec::new(), Err("re-drive is off".to_string())),
            BTreeSet::new(),
        );
        return redrive_pass(cfg, &p, &ShiftState::default(), &quiet_ctx(None), &mut exec);
    }
    let _lock = if dry_run {
        None
    } else {
        match try_shift_lock(project_root)? {
            Some(f) => Some(f),
            None => {
                return Ok(TickReport {
                    dry_run,
                    enabled: cfg.enabled,
                    enabled_source: cfg.enabled_source.clone(),
                    redrive:
                        "held: a night-shift check is running; its re-drive step handles the parks"
                            .to_string(),
                    ..Default::default()
                });
            }
        }
    };
    let (state, state_error) = match load_state(&state_path(project_root)) {
        Ok(s) => (s, None),
        Err(e) => (ShiftState::default(), Some(format!("{e:#}"))),
    };
    let ctx = quiet_ctx(state_error);
    let (lock, _, foreign_claim) = probe_drain_locks(project_root, now);
    let p = Probes::for_redrive(
        lock,
        foreign_claim,
        last_wave_is_alive(&state),
        crate::current_user_id(None),
        redrive_probes(project_root, backend, cfg),
        held_specs(project_root),
    );
    redrive_pass(cfg, &p, &state, &ctx, &mut exec)
}

/// The recipients the mail-latency step may page for, lower-cased. Reuses
/// the existing identity lookups: the mailbox's known identities (built-in
/// roles, role files and the agent registry's seats), the team roster
/// (`registry/team.toml` members and their roles) and the active work
/// session leases (owner and role).
// trace:TASK-1492 | ai:claude
fn known_mail_recipients(project_root: &Path) -> BTreeSet<String> {
    let mut known: BTreeSet<String> = crate::known_mailbox_identities(project_root)
        .into_iter()
        .collect();
    let mut add = |s: &str| {
        let t = s.trim().to_lowercase();
        if !t.is_empty() {
            known.insert(t);
        }
    };
    let roster = crate::team::TeamRoster::load(&project_root.join(".aida-store"));
    for (user, role) in &roster.members {
        add(user);
        add(&crate::canonical_role_name(role));
    }
    for lease in crate::list_leases(project_root) {
        add(&lease.owner);
        if let Some(role) = &lease.role {
            add(&crate::canonical_role_name(role));
        }
    }
    known
}

fn try_shift_lock(project_root: &Path) -> Result<Option<std::fs::File>> {
    use aida_core::file_lock::is_lock_contended;
    use fs2::FileExt;
    let dir = project_root.join(".aida");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join(LOCK_FILE))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(err) if is_lock_contended(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// The production tick. `Ok(None)` = disabled or lock contended (exit 0).
pub(crate) fn run_tick(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    dry_run: bool,
) -> Result<Option<TickReport>> {
    let started = Instant::now();
    let cfg = load_config(project_root);
    if !cfg.enabled && !dry_run {
        return Ok(None);
    }
    let _lock = if dry_run {
        None
    } else {
        match try_shift_lock(project_root)? {
            Some(f) => Some(f),
            None => return Ok(None),
        }
    };
    let now = Utc::now();
    let log_path = project_root
        .join(".aida")
        .join(format!("shift-wave-{}.log", now.format("%Y%m%d-%H%M%S")));
    let mut ctx = TickCtx::from_clock(now, dry_run, started, TICK_DEADLINE, log_path);
    let (mut state, state_err) = match load_state(&state_path(project_root)) {
        Ok(s) => (s, None),
        Err(e) => (ShiftState::default(), Some(format!("{e:#}"))),
    };
    ctx.state_error = state_err.clone();
    let probes = gather_probes(project_root, backend, &cfg, &state, now);
    // Re-evaluate the clock after gathering: the probes can be slow.
    let fresh = TickCtx::from_clock(now, dry_run, started, TICK_DEADLINE, ctx.log_path.clone());
    ctx.optional_allowed = fresh.optional_allowed;
    ctx.launch_allowed = fresh.launch_allowed;
    let mut exec = RealExec {
        project_root: project_root.to_path_buf(),
        backend,
    };
    let report = if dry_run {
        let mut scratch = state.clone();
        tick_core(&cfg, &probes, &mut scratch, &ctx, &mut exec)?
    } else {
        tick_core(&cfg, &probes, &mut state, &ctx, &mut exec)?
    };
    if let Some(err) = state_err {
        if !dry_run {
            anyhow::bail!("{err}; launches are blocked until the file is repaired or removed");
        }
    }
    Ok(Some(report))
}

// ---------------------------------------------------------------------------
// Rendering, status, CLI
// ---------------------------------------------------------------------------

pub(crate) fn render_report(r: &TickReport) -> String {
    let mut out = String::new();
    let head = if r.dry_run {
        "night shift (dry run — nothing written)"
    } else {
        "night shift"
    };
    out.push_str(&format!(
        "{head}\n  enabled: {} — {}\n",
        if r.enabled { "yes" } else { "no" },
        r.enabled_source
    ));
    out.push_str(&format!(
        "  queue: user {} · role {} · vendor {}\n",
        r.queue_user, r.queue_role, r.vendor
    ));
    out.push_str("  guards:\n");
    for g in &r.guards {
        out.push_str(&format!(
            "    {} {:<16} {}\n",
            if g.pass { "pass" } else { "FAIL" },
            g.name,
            g.detail
        ));
    }
    match &r.batch {
        Some(b) => out.push_str(&format!(
            "  wave: batch:{b}{} — {}\n",
            if r.reused_batch {
                " (reused from an interrupted launch)"
            } else {
                ""
            },
            r.specs.join(", ")
        )),
        None => out.push_str("  wave: nothing eligible\n"),
    }
    // Specs merely not ready (finished, in progress) are counted, not listed:
    // a long-lived queue holds many, and the reasons worth reading are the
    // floors (mode, keystone, hold, wave cap).
    let (by_status, by_floor): (Vec<_>, Vec<_>) = r
        .skipped
        .iter()
        .partition(|(_, why)| why.starts_with("status "));
    for (spec, why) in &by_floor {
        out.push_str(&format!("    skip {spec}: {why}\n"));
    }
    if !by_status.is_empty() {
        out.push_str(&format!(
            "    skip {} queued spec(s) that are not Approved/Planned\n",
            by_status.len()
        ));
    }
    if !r.argv.is_empty() {
        out.push_str(&format!("  command: aida {}\n", r.argv.join(" ")));
    }
    // trace:TASK-1492 | ai:claude
    out.push_str(&format!("  re-drive: {}\n", r.redrive));
    for g in r.redrive_guards.iter().filter(|g| !g.pass) {
        out.push_str(&format!("    FAIL {:<16} {}\n", g.name, g.detail));
    }
    for d in r
        .redrive_plan
        .iter()
        .filter(|d| d.class == crate::supervisor::ParkClass::Transient)
    {
        out.push_str(&format!(
            "    {} ({}, {} of {} attempts): {}\n",
            d.spec,
            d.reason,
            d.attempts,
            crate::supervisor::DEFAULT_MAX_ATTEMPTS,
            d.action
        ));
    }
    out.push_str(&format!("  mail latency: {}\n", r.mail_latency));
    for e in &r.notify_errors {
        out.push_str(&format!("  notify failed: {e}\n"));
    }
    if let Some(l) = &r.launched {
        out.push_str(&format!(
            "  launched batch:{} ({} specs, pid {})\n",
            l.batch,
            l.specs.len(),
            l.pid
        ));
    }
    if r.reaped > 0 {
        out.push_str(&format!("  reaped {} finished session(s)\n", r.reaped));
    }
    if let Some(pid) = r.recovered_stale_pid {
        out.push_str(&format!(
            "  found a stale drain lock (pid {pid}); the next drain reclaims it\n"
        ));
    }
    if let Some(b) = &r.breaker {
        out.push_str(&format!("  STOPPED: {b}\n"));
    }
    if !r.escalated.is_empty() {
        out.push_str(&format!(
            "  escalated (excluded from further waves): {}\n",
            r.escalated.join(", ")
        ));
    }
    out
}

fn human_age(d: Duration) -> String {
    let m = d.num_minutes().max(0);
    if m < 60 {
        format!("{m}m")
    } else if m < 48 * 60 {
        format!("{}h", m / 60)
    } else {
        format!("{}d", m / (24 * 60))
    }
}

/// The one-line `aida status` summary. `None` when the night shift is off,
/// so existing status output is unchanged.
pub(crate) fn status_line(
    cfg: &ShiftConfig,
    state: &ShiftState,
    wave_alive: bool,
    now: DateTime<Utc>,
) -> Option<String> {
    if !cfg.enabled {
        return None;
    }
    let check = match state.last_tick_at {
        Some(t) => format!("last check {} ago", human_age(now - t)),
        None => "no check yet".to_string(),
    };
    let what = if let Some(b) = &state.breaker {
        format!("paused: {} (`aida shift resume`)", b.reason)
    } else if let Some(w) = state
        .last_launched()
        .filter(|w| w.outcome.is_none() && wave_alive)
    {
        format!("working {} items", w.specs.len())
    } else {
        let holding: Vec<&String> = state
            .last_refused
            .iter()
            .filter(|g| g.as_str() != "drain-mode-only")
            .collect();
        if holding.is_empty() {
            "idle".to_string()
        } else {
            format!(
                "holding: {}",
                holding
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    Some(format!("night shift: on · {check} · {what}"))
}

/// Print the status line for `aida status`; silent when off.
pub(crate) fn print_status_line(project_root: &Path) {
    let cfg = load_config(project_root);
    if !cfg.enabled {
        return;
    }
    let state = load_state(&state_path(project_root)).unwrap_or_default();
    let alive = state.last_launched().is_some_and(|w| {
        crate::process_probe::process_identity_is_alive(
            w.pid.unwrap_or_default(),
            w.pid_start.as_deref(),
        )
    });
    if let Some(line) = status_line(&cfg, &state, alive, Utc::now()) {
        use colored::Colorize;
        println!("{}", line.cyan());
        println!();
    }
}

fn status_command(
    project_root: &Path,
    backend: &aida_core::CachedGitBackend,
    json: bool,
) -> Result<()> {
    let cfg = load_config(project_root);
    let state_result = load_state(&state_path(project_root));
    let state = state_result.as_ref().cloned().unwrap_or_default();
    let report = run_tick(project_root, backend, true)?.unwrap_or_default();
    let now = Utc::now();
    // trace:TASK-1491 | ai:claude
    let driver = crate::schedule_driver::driver_status(project_root).label();
    let jobs = crate::maintenance_schedule::jobs_running_command(project_root, TICK_COMMAND);
    let job = if jobs.iter().any(|(_, enabled)| *enabled) {
        "registered"
    } else if jobs.is_empty() {
        "not registered (`aida shift enable` registers it)"
    } else {
        "registered but disabled"
    };
    let last = state.last_launched().cloned();
    if json {
        let body = serde_json::json!({
            "enabled": cfg.enabled,
            "enabled_source": cfg.enabled_source,
            "committed_enable_ignored": cfg.committed_enable_ignored,
            "driver": driver,
            "job": job,
            "state_readable": state_result.is_ok(),
            "last_tick_at": state.last_tick_at,
            "last_wave": last,
            "waves_in_24h": state.waves_in_day(now),
            "max_waves_per_day": cfg.max_waves_per_day,
            "breaker": state.breaker,
            "escalated": state.escalated,
            "guards": report.guards,
            // trace:TASK-1492 | ai:claude
            "redrive": report.redrive,
            "redrive_guards": report.redrive_guards,
            "mail_latency": report.mail_latency,
            "mail_episodes": state.mail_episodes,
            "config": cfg,
        });
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }
    println!(
        "night shift: {} — {}",
        if cfg.enabled { "on" } else { "off" },
        cfg.enabled_source
    );
    if cfg.committed_enable_ignored {
        println!(
            "  note: the committed config sets `enabled = true`; that is ignored — the switch is per clone"
        );
    }
    println!("  driver: {driver} · job: {job}");
    match state.last_tick_at {
        Some(t) => println!("  last check: {} ago", human_age(now - t)),
        None => println!("  last check: never"),
    }
    match &last {
        Some(w) => println!(
            "  last wave: batch:{} ({} specs) {} ago — {}",
            w.batch,
            w.specs.len(),
            human_age(now - w.at),
            match &w.outcome {
                Some(o) => format!("{} shipped, {} shelved", o.shipped, o.shelved),
                None => "not settled yet".to_string(),
            }
        ),
        None => println!("  last wave: none"),
    }
    println!(
        "  waves in 24h: {} of {}",
        state.waves_in_day(now),
        cfg.max_waves_per_day
    );
    if let Some(b) = &state.breaker {
        println!("  STOPPED: {} (`aida shift resume`)", b.reason);
    }
    if !state.escalated.is_empty() {
        println!(
            "  escalated: {}",
            state
                .escalated
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("  guards:");
    for g in &report.guards {
        println!(
            "    {} {:<16} {}",
            if g.pass { "pass" } else { "FAIL" },
            g.name,
            g.detail
        );
    }
    // trace:TASK-1492 | ai:claude
    println!("  re-drive: {}", report.redrive);
    println!("  mail latency: {}", report.mail_latency);
    Ok(())
}

/// Who is asking, and how to ask them. `aida shift enable` and
/// `aida shift resume` arm (or re-arm) unattended drain launches, so they
/// carry the same human-at-a-terminal floor as `aida merge-hold clear`: an
/// interactive stdin, not agent output mode, and an explicit y/N. The seams
/// are injectable so tests never read a real terminal or `~/.aida`.
pub(crate) struct Operator<'a> {
    pub stdin_tty: bool,
    pub agent_mode: bool,
    pub confirm: &'a mut dyn FnMut(&str) -> Result<bool>,
}

/// PURE: why `verb` must refuse for this caller, if it must.
pub(crate) fn operator_gate_refusal(
    verb: &str,
    stdin_tty: bool,
    agent_mode: bool,
) -> Option<String> {
    let why = if agent_mode {
        "agent output mode is on"
    } else if !stdin_tty {
        "stdin is not an interactive terminal"
    } else {
        return None;
    };
    Some(format!(
        "`aida shift {verb}` lets the scheduler launch unattended drains, so it needs a human \
         at an interactive terminal ({why}). Run it yourself in a terminal; an agent cannot \
         arm the night shift."
    ))
}

/// Gate + confirmation. `Ok(false)` = the human declined.
fn operator_approves(verb: &str, question: &str, op: &mut Operator<'_>) -> Result<bool> {
    if let Some(msg) = operator_gate_refusal(verb, op.stdin_tty, op.agent_mode) {
        anyhow::bail!("{msg}");
    }
    (op.confirm)(question)
}

/// `aida shift enable`. Gated: it arms unattended launches.
fn enable_command(project_root: &Path, layer: &Path, op: &mut Operator<'_>) -> Result<()> {
    let key = repo_key(project_root);
    let question = format!(
        "Let the scheduler launch unattended drain waves for {key} while nobody is at the keyboard? [y/N] "
    );
    if !operator_approves("enable", &question, op)? {
        println!("night shift: not enabled (declined).");
        return Ok(());
    }
    apply_enable(project_root, layer, true)
}

/// `aida shift install --systemd-user|--cron`: enable + install that driver,
/// behind ONE gate and ONE yes. Installing a scheduled unattended driver is
/// the same human floor as enabling. The driver switch installs and
/// verifies the new driver before removing this repo's other one (A13a).
// trace:TASK-1491 | ai:claude
pub(crate) fn install_command(
    project_root: &Path,
    layer: &Path,
    driver: crate::schedule_driver::Driver,
    op: &mut Operator<'_>,
    host: &mut dyn crate::schedule_driver::DriverHost,
) -> Result<Option<crate::schedule_driver::SwitchReport>> {
    let key = repo_key(project_root);
    let question = format!(
        "Let the scheduler launch unattended drain waves for {key} while nobody is at the keyboard, \
         and install a {} to run the check? [y/N] ",
        driver.label()
    );
    if !operator_approves("install", &question, op)? {
        println!("night shift: not enabled and no driver installed (declined).");
        return Ok(None);
    }
    apply_enable(project_root, layer, false)?;
    // trace:TASK-1491 | ai:claude
    let switched = crate::schedule_driver::real_tick_invocation(project_root).and_then(|inv| {
        crate::schedule_driver::switch_driver(host, &inv, driver).map(|report| (inv, report))
    });
    // trace:BUG-1619 | ai:claude
    let (inv, report) = switched.map_err(|e| {
        let state = driver_state_after_failure(&e);
        e.context(format!(
            "night shift is now enabled for {key}, but installing the {} failed. {state}; \
             `aida shift disable` turns the shift back off",
            driver.label()
        ))
    })?;
    crate::schedule_driver::print_switch_report(&report, Path::new(&inv.exe));
    Ok(Some(report))
}

/// PURE: what a failed driver switch left of this repo's scheduler driver,
/// read from the [`SystemdInstallFailure`] context when the systemd install
/// itself failed. "Unchanged" is only claimed when it is true.
///
/// [`SystemdInstallFailure`]: crate::schedule_driver::SystemdInstallFailure
// trace:BUG-1619 | ai:claude
pub(crate) fn driver_state_after_failure(err: &anyhow::Error) -> String {
    use crate::schedule_driver::{SystemdInstallFailure, UnitFilesAfterFailure};
    match err
        .downcast_ref::<SystemdInstallFailure>()
        .map(|f| &f.files)
    {
        Some(UnitFilesAfterFailure::Unchanged) => {
            "This repo's systemd unit files are unchanged".to_string()
        }
        Some(UnitFilesAfterFailure::CleanedUp {
            timer_enabled: true,
            left,
            disable_error: None,
            ..
        }) if left.is_empty() => {
            "The partly installed systemd timer was disabled and removed, so this repo's \
             scheduler driver is unchanged"
                .to_string()
        }
        // A write failed before `enable`: nothing was enabled or disabled.
        // trace:BUG-1619 | ai:claude
        Some(UnitFilesAfterFailure::CleanedUp {
            timer_enabled: false,
            left,
            ..
        }) if left.is_empty() => {
            "No timer was enabled, and the unit file(s) this install wrote were removed, so \
             this repo's scheduler driver is unchanged"
                .to_string()
        }
        Some(UnitFilesAfterFailure::CleanedUp { .. }) => {
            "The partly installed systemd timer could not be fully cleaned up (see below)"
                .to_string()
        }
        Some(UnitFilesAfterFailure::Rewritten(_)) => {
            "This repo's existing systemd unit files were rewritten before the failure and \
             left in place (see below)"
                .to_string()
        }
        None => "Unless the error below says a driver was installed, this repo's scheduler \
                 driver is unchanged"
            .to_string(),
    }
}

/// PURE: the `aida shift enable` driver hint. Unknown is reported as
/// unknown, and a disabled or stopped timer as installed but disabled / not
/// running, exactly as `DriverStatus::label` does, never as "needed".
// trace:BUG-1619 | ai:claude
pub(crate) fn driver_hint(status: &crate::schedule_driver::DriverStatus) -> Option<String> {
    if status.any_installed() {
        None
    } else if !status.unknown_reasons().is_empty() {
        Some(format!(
            "  scheduler driver: {} — `aida doctor` shows more",
            status.label()
        ))
    } else if matches!(
        status.systemd,
        crate::schedule_driver::SystemdDriverStatus::Disabled
            | crate::schedule_driver::SystemdDriverStatus::Stopped
    ) {
        // Installed but disabled / not running: say so, as label() does.
        // trace:BUG-1619 | ai:claude
        Some(format!("  scheduler driver: {}", status.label()))
    } else {
        Some(
            "  needed: a scheduler driver — `aida shift install --systemd-user` (Linux) or `--cron`"
                .to_string(),
        )
    }
}

/// The writes behind an approved `enable` / `install`.
fn apply_enable(project_root: &Path, layer: &Path, hint_driver: bool) -> Result<()> {
    let key = repo_key(project_root);
    write_local_enabled(layer, &key, true)?;
    println!("night shift: on for {key}");
    println!(
        "  switch: {} (local to this machine, never committed)",
        layer.display()
    );
    let config = project_root.join(".aida").join("config.toml");
    let added = crate::config_edit::ensure_array_table_entry(
        &config,
        "schedule",
        "jobs",
        "command",
        TICK_COMMAND,
        &[
            ("name", TICK_JOB_NAME.into()),
            ("command", TICK_COMMAND.into()),
            ("every", TICK_JOB_EVERY.into()),
            ("enabled", true.into()),
        ],
    )?;
    if added {
        println!(
            "  registered the `{TICK_JOB_NAME}` job in {} (inert in any clone that has not run `aida shift enable`)",
            config.display()
        );
    }
    if no_human_ack_source(project_root).is_none() {
        println!("  needed: `aida no-human acknowledge` — waves run fully headless and the tick refuses until it is acknowledged");
    }
    if !crate::maintenance_schedule::jobs_running_command(project_root, WATCHDOG_COMMAND)
        .iter()
        .any(|(_, enabled)| *enabled)
    {
        // toml-ok: operator hint quoting a fixed constant, not a TOML writer.
        println!("  needed: an enabled `watchdog` job (`command = \"{WATCHDOG_COMMAND}\"`) — without its spend evidence the tick refuses");
    }
    // trace:TASK-1491 | ai:claude
    // trace:BUG-1619 | ai:claude
    if hint_driver {
        if let Some(hint) = driver_hint(&crate::schedule_driver::driver_status(project_root)) {
            println!("{hint}");
        }
    }
    println!("  preflight: `aida shift tick --dry-run`");
    Ok(())
}

/// `aida shift disable`. Never gated: turning launches off is always safe.
fn disable_command(project_root: &Path, layer: &Path) -> Result<()> {
    let key = repo_key(project_root);
    write_local_enabled(layer, &key, false)?;
    println!(
        "night shift: off for {key} ({}). A wave already running is not stopped.",
        layer.display()
    );
    Ok(())
}

/// `aida shift resume`. Gated: it clears the no-progress breaker.
fn resume_command(project_root: &Path, op: &mut Operator<'_>) -> Result<()> {
    let path = state_path(project_root);
    let mut state = load_state(&path)?;
    let Some(breaker) = state.breaker.clone() else {
        println!("night shift: not stopped — nothing to resume.");
        return Ok(());
    };
    let question = format!(
        "The night shift stopped itself: {}. Allow it to launch again? [y/N] ",
        breaker.reason
    );
    if !operator_approves("resume", &question, op)? {
        println!("night shift: still stopped (declined).");
        return Ok(());
    }
    state.breaker = None;
    state.consecutive_zero_progress = 0;
    save_state_to(&path, &state)?;
    println!("night shift: resumed — the next check may launch again.");
    Ok(())
}

fn real_operator_gate(f: impl FnOnce(&mut Operator<'_>) -> Result<()>) -> Result<()> {
    use std::io::IsTerminal;
    let mut confirm = |q: &str| crate::prompt_yes_no(q, false);
    let mut op = Operator {
        stdin_tty: std::io::stdin().is_terminal(),
        agent_mode: crate::agent_output_mode(),
        confirm: &mut confirm,
    };
    f(&mut op)
}

pub(crate) fn handle_shift_command(
    cmd: &ShiftCommand,
    backend: &aida_core::CachedGitBackend,
    store_path: &Path,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
    match cmd {
        ShiftCommand::Tick { dry_run, json } => match run_tick(project_root, backend, *dry_run)? {
            None => {
                if *json {
                    println!("{}", serde_json::json!({ "enabled": false, "ran": false }));
                } else {
                    println!("night shift: off for this clone, or another check is running.");
                }
            }
            Some(report) => {
                if *json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else if *dry_run || report.launched.is_some() || report.event_emitted {
                    print!("{}", render_report(&report));
                }
            }
        },
        ShiftCommand::Status { json } => status_command(project_root, backend, *json)?,
        ShiftCommand::Enable => {
            let layer = local_layer_path().context("could not resolve the home directory")?;
            real_operator_gate(|op| enable_command(project_root, &layer, op))?
        }
        ShiftCommand::Disable => {
            let layer = local_layer_path().context("could not resolve the home directory")?;
            disable_command(project_root, &layer)?
        }
        ShiftCommand::Resume => real_operator_gate(|op| resume_command(project_root, op))?,
        // trace:TASK-1491 | ai:claude
        ShiftCommand::Install { systemd_user, cron } => {
            let driver = if *systemd_user && !*cron {
                crate::schedule_driver::Driver::Systemd
            } else {
                crate::schedule_driver::Driver::Cron
            };
            let layer = local_layer_path().context("could not resolve the home directory")?;
            real_operator_gate(|op| {
                install_command(
                    project_root,
                    &layer,
                    driver,
                    op,
                    &mut crate::schedule_driver::RealDriverHost,
                )
                .map(|_| ())
            })?
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1218_shift_tick_tests.rs"]
mod story_1218_shift_tick_tests;
