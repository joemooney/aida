//! Drain-state file (STORY-301) — `.aida/drain-state.json`.
//!
//! # The problem
//!
//! An `aida queue work --auto-complete` orchestrator is invisible from inside
//! the Claude session it drives. A user mid-drain cannot tell (a) what command
//! launched it, (b) whether it is a single-spec or a multi-item batch drain,
//! (c) how far through the batch it is, or (d) what happens when they exit the
//! current session. Answering those questions used to take a `ps --forest`
//! plus a hand cross-reference of batch membership and PR states. The
//! orchestrator *knows* all of it — it just never wrote it down.
//!
//! # The drain-state file
//!
//! The orchestrator writes `.aida/drain-state.json` at drain start, updates it
//! per phase transition and per batch-member advance, and removes it on a
//! clean exit. So the file's *presence* means a drain that is either live or
//! crashed — never a finished one. `aida drain status` reads the file,
//! corroborates the recorded [`DrainState::orchestrator_pid`] against a
//! liveness probe, and prints the human summary. A file whose PID is dead is a
//! *stale* drain — the orchestrator crashed or was killed without cleaning up;
//! `aida drain clear` removes it.
//!
//! Every write goes through [`aida_core::write_atomic`] (TASK-331): a phase
//! transition and a concurrent `aida drain status` read must never see a torn
//! file.
//!
//! trace:STORY-301 | ai:claude

use std::path::{Path, PathBuf};
use std::time::Duration;

use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::{drain_lock, process_probe};

/// File name under `.aida/` holding the live drain's state. Gitignored by the
/// deny-by-default `.aida/*` rule — pure per-clone runtime state.
const DRAIN_STATE_FILE: &str = "drain-state.json";

/// Member state: the spec has not started its lifecycle yet.
pub(crate) const STATE_QUEUED: &str = "queued";
/// Member state: the spec finished its full lifecycle successfully.
pub(crate) const STATE_COMPLETED: &str = "completed";
/// Member state: the spec's lifecycle stopped on a phase failure.
pub(crate) const STATE_FAILED: &str = "failed";

/// Path of the drain-state file under `project_root`.
pub(crate) fn drain_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(DRAIN_STATE_FILE)
}

/// One spec in a drain, with its lifecycle state and (once opened) its PR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DrainMember {
    /// The spec id, e.g. `STORY-301`.
    pub(crate) spec: String,
    /// `queued` | `in-phase-N` | `completed` | `failed`.
    pub(crate) state: String,
    /// The PR number once a phase has discovered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr: Option<u32>,
    /// RFC-3339 timestamp when this spec's drain run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_at: Option<String>,
    /// RFC-3339 timestamp when this spec reached a terminal drain outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) finished_at: Option<String>,
}

impl DrainMember {
    /// A freshly-queued member — the shape every member has at drain start.
    fn queued(spec: impl Into<String>) -> Self {
        Self {
            spec: spec.into(),
            state: STATE_QUEUED.to_string(),
            pr: None,
            started_at: None,
            finished_at: None,
        }
    }

    /// True while this member is mid-pipeline (`in-phase-N`).
    pub(crate) fn is_running(&self) -> bool {
        aida_core::liveness::drain_member_is_running(&self.state)
    }
}

/// The full state of a live (or crashed) `--auto-complete` drain. Serialized
/// to `.aida/drain-state.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DrainState {
    /// The command that launched the drain, reconstructed from argv.
    pub(crate) command: String,
    /// `single` | `batch` | `next-n`.
    pub(crate) mode: String,
    /// The `batch:NAME` tag (without the `batch:` prefix) for a batch drain;
    /// `None` for a single-spec or `next-n` drain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) batch: Option<String>,
    /// Every spec the drain will run, in drain order.
    pub(crate) members: Vec<DrainMember>,
    /// Maximum number of members the orchestrator may keep active at once.
    /// The live default is `1`; values above `1` opt in to overlap.
    /// At depth `2`, a later implementer may overlap an earlier member's
    /// CI/review/merge wait while the merge side remains serial.
    // trace:STORY-1041 trace:STORY-1091 trace:ADR-27 trace:BUG-1585 | ai:codex
    #[serde(default = "default_pipeline_depth")]
    pub(crate) pipeline_depth: usize,
    /// The spec currently in its pipeline; `None` before the first member
    /// starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current: Option<String>,
    /// The phase the current member is in, e.g. `1 (implementer)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_phase: Option<String>,
    /// RFC-3339 timestamp when the current phase was entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase_started_at: Option<String>,
    // BUG-1290: 1-based attempt count for the CURRENT `current_phase` entry.
    // Set only by the code path that announces a phase entry (emits
    // `PhaseEntered`) — incremented when the same (spec, phase) is
    // re-announced (a retry re-running the phase), reset to 1 whenever
    // `current_phase` changes. A session-only attach (see
    // `attach_phase_session`) never touches this.
    // trace:BUG-1290 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase_attempt: Option<u32>,
    /// Headless vendor/session UUID for the current phase, when the phase
    /// writes a `.aida/headless-logs/*-<session>.jsonl` stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_session_id: Option<String>,
    /// Headless vendor for the current phase. Legacy drain-state files omit
    /// this and fall back to the resolved project vendor at read time.
    // trace:STORY-1054 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_vendor: Option<String>,
    /// PID of the orchestrator process — corroborated by `aida drain status`
    /// to tell a live drain from a stale crashed file.
    pub(crate) orchestrator_pid: u32,
    /// RFC-3339 timestamp the drain started.
    pub(crate) started_at: String,
    /// Plain-language prediction of the post-drain state — which queue items
    /// will and won't be auto-picked-up once the drain ends.
    pub(crate) on_drain_complete: String,
    /// TASK-336: the orchestrator's per-run UUID for the *current* spec's
    /// orchestration. A phase child carrying `AIDA_AUTO_COMPLETE_TOKEN=<uuid>`
    /// trusts orchestrator-mode only when this value matches the env-passed
    /// token and [`Self::orchestrator_pid`] is alive. Empty between batch
    /// members and before the first member starts.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) run_uuid: String,
    /// TASK-336 / BUG-237: whether the current spec's orchestration was
    /// started under `--zen`. A phase child trusts an inherited
    /// `AIDA_ZEN=1` only when [`Self::run_uuid`] corroborates *and* this is
    /// true.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) zen: bool,
    /// BUG-286: orchestrator-side gh/git retry attempts during phases 3-6,
    /// one entry per retried attempt. Lets post-hoc analysis correlate
    /// drain stalls with transient-API health. Omitted when empty so a
    /// blip-free drain leaves the file untouched.
    /// trace:BUG-286 | ai:claude
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) retries: Vec<DrainRetry>,
    /// TASK-1297: for a batch drain, how many OTHER approved specs are
    /// routed to this batch's role but excluded by the `--batch` filter — 0
    /// for a single-spec / `next-n` drain, and 0 rather than noise when the
    /// filter excluded nothing. Lets `aida drain status` show the same
    /// figure a live drain prints at start-up while it is still running.
    // trace:TASK-1297 | ai:claude
    #[serde(default)]
    pub(crate) excluded_from_batch: usize,
}

/// Corroborated live-drain facts shared by glance/status surfaces. This is the
/// one local-only probe result for `.aida/drain-state.json` + `.aida/drain.lock`;
/// renderers choose their own phrasing without reparsing the files.
// trace:TASK-1194 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveDrainProbe {
    pub(crate) position: usize,
    pub(crate) total: usize,
    pub(crate) spec: String,
    pub(crate) phase: Option<String>,
}

/// A spec whose lifecycle is currently owned by a corroborated live drain.
/// The drain lock PID, rather than the phase child's PID, is the liveness
/// authority after the implementer exits for CI/review/merge.
// trace:TASK-163 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveDrainSpec {
    pub(crate) pid: u32,
    pub(crate) phase: String,
    /// 1-based attempt count ("round") for the current phase — BUG-1290's
    /// `DrainState::phase_attempt`, carried through for machine consumers
    /// (`aida status <spec> --json`). `None` for a legacy/mid-transition
    /// state file that hasn't recorded an attempt count yet.
    // trace:BUG-1289 | ai:claude
    pub(crate) round: Option<u32>,
}

/// Return the live orchestrator activity for `spec`, when that member is in a
/// drain phase. Both state membership and the live lock must agree; stale state
/// or a dead/missing lock fails closed.
// trace:TASK-163 | ai:codex
pub(crate) fn live_drain_spec(project_root: &Path, spec: &str) -> Option<LiveDrainSpec> {
    let state = DrainState::read(project_root)?;
    let lock = drain_lock::read_pid_live_lock(project_root)?;
    let member = state
        .members
        .iter()
        .find(|m| m.spec.eq_ignore_ascii_case(spec) && m.is_running())?;
    let is_current = state
        .current
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(spec));
    let raw_phase = if is_current {
        state
            .current_phase
            .clone()
            .unwrap_or_else(|| member.state.clone())
    } else {
        member.state.clone()
    };
    // trace:BUG-1289 | ai:claude
    // `phase_attempt` is only meaningful for the CURRENT member — a
    // between-members entry has no attempt count of its own.
    let round = if is_current {
        state.phase_attempt
    } else {
        None
    };
    Some(LiveDrainSpec {
        pid: lock.pid,
        phase: drain_phase_display(&raw_phase),
        round,
    })
}

fn drain_phase_display(raw: &str) -> String {
    let number = raw
        .strip_prefix("in-phase-")
        .or_else(|| raw.split_whitespace().next())
        .unwrap_or(raw);
    let label = match number {
        "1" => "implementer",
        "2" => "CI wait",
        "3" => "reviewer",
        "4" => "merge",
        "5" => "pull",
        "6" => "complete",
        _ => return raw.to_string(),
    };
    format!("{number}/6 ({label})")
}

/// BUG-286: one orchestrator-side retry event recorded against the drain.
/// Mirrors the [`crate::network_retry::RetryEvent`] shape with the spec /
/// phase context the orchestrator carries.
/// trace:BUG-286 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DrainRetry {
    /// Subprocess label, e.g. `gh pr merge 157 --squash --delete-branch`.
    pub(crate) label: String,
    /// Spec the orchestrator was driving when the blip hit.
    pub(crate) spec: String,
    /// Phase identifier (e.g. `4 (merge)`), absent when the retry happened
    /// outside a phase context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    /// 1-indexed attempt number that *failed* and triggered this retry.
    pub(crate) attempt: u32,
    /// Configured max attempts (so a reader can tell "1/3" from "1/5").
    pub(crate) max: u32,
    /// STORY-975: typed cause for a whole-phase drain retry, e.g. `watchdog`.
    /// Network subprocess retries predate this field and omit it.
    // trace:STORY-975 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cause: Option<String>,
    /// Backoff before the next attempt, in milliseconds.
    pub(crate) backoff_ms: u64,
    /// First non-empty stderr line, trimmed to 180 chars.
    pub(crate) stderr_snippet: String,
    /// RFC-3339 timestamp the retry was recorded.
    pub(crate) at: String,
}

/// Default skip-helper for `#[serde(skip_serializing_if)]` on bool fields.
fn is_false(b: &bool) -> bool {
    !*b
}

impl DrainState {
    /// Initial state for a single-spec drain (`aida queue work <SPEC>
    /// --auto-complete`). The `run_uuid` + `zen` fields are baked in at
    /// creation time so phase children spawned by this run can corroborate
    /// `AIDA_AUTO_COMPLETE_TOKEN` against the drain-state file from the
    /// first phase onward. trace:TASK-336 | ai:claude
    pub(crate) fn new_single(spec: &str, run_uuid: &str, zen: bool) -> Self {
        Self {
            command: launch_command(),
            mode: "single".to_string(),
            batch: None,
            members: vec![DrainMember::queued(spec)],
            pipeline_depth: 1,
            current: Some(spec.to_string()),
            current_phase: None,
            phase_started_at: None,
            phase_attempt: None,
            current_session_id: None,
            current_vendor: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_single(spec),
            run_uuid: run_uuid.to_string(),
            zen,
            retries: Vec::new(),
            excluded_from_batch: 0,
        }
    }

    /// Initial state for a batch drain (`aida queue work --batch NAME
    /// --auto-complete`). `members` is the resolved pickup-order member list.
    /// `run_uuid` / `zen` are empty / `false` until [`set_run`] fills them in
    /// at each member's orchestration start.
    pub(crate) fn new_batch(batch_name: &str, members: &[String]) -> Self {
        Self {
            command: launch_command(),
            mode: "batch".to_string(),
            batch: Some(batch_name.to_string()),
            members: members.iter().map(DrainMember::queued).collect(),
            pipeline_depth: default_pipeline_depth(),
            current: None,
            current_phase: None,
            phase_started_at: None,
            phase_attempt: None,
            current_session_id: None,
            current_vendor: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_batch(batch_name),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
            excluded_from_batch: 0,
        }
    }

    /// Initial state for a `nextN` drain (`aida queue work nextN
    /// --auto-complete`). `members` is the resolved queue-head member list.
    /// `run_uuid` / `zen` are filled in per-member by [`set_run`].
    pub(crate) fn new_next_n(n: usize, members: &[String]) -> Self {
        Self {
            command: launch_command(),
            mode: "next-n".to_string(),
            batch: None,
            members: members.iter().map(DrainMember::queued).collect(),
            pipeline_depth: default_pipeline_depth(),
            current: None,
            current_phase: None,
            phase_started_at: None,
            phase_attempt: None,
            current_session_id: None,
            current_vendor: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_next_n(n),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
            excluded_from_batch: 0,
        }
    }

    /// Serialize to JSON.
    fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Write the state to `.aida/drain-state.json` atomically (TASK-331), so a
    /// concurrent `aida drain status` reader never sees a torn file. Creates
    /// `.aida/` if absent. Best-effort: a write failure is non-fatal — the
    /// drain still runs, it just is not observable.
    pub(crate) fn write(&self, project_root: &Path) -> std::io::Result<()> {
        let path = drain_state_path(project_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        aida_core::write_atomic(&path, self.to_json())
    }

    /// Read + parse the drain-state file under `project_root`, or `None` when
    /// it is absent or unparseable (a torn write fails safe to "no drain").
    pub(crate) fn read(project_root: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(drain_state_path(project_root)).ok()?;
        serde_json::from_str(&body).ok()
    }

    /// Remove the drain-state file. Idempotent — a missing file is a clean
    /// success (`aida drain clear` on a project with no drain).
    pub(crate) fn clear(project_root: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(drain_state_path(project_root)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// 1-based position of `spec` among the members, for the "spec N of M"
    /// banner. `None` when the spec is not a member.
    pub(crate) fn position_of(&self, spec: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|m| m.spec == spec)
            .map(|i| i + 1)
    }

    /// Clamp and record the configured in-flight window. Best-effort callers
    /// may set this after constructing the state, before writing it.
    // trace:STORY-1041 trace:ADR-27 | ai:codex
    pub(crate) fn with_pipeline_depth(mut self, depth: usize) -> Self {
        self.pipeline_depth = clamp_pipeline_depth(depth);
        self
    }

    /// Record the "M other approved specs routed to this role are not in
    /// this batch" count so `aida drain status` can echo it while the drain
    /// is live. Best-effort callers set this after construction, before
    /// writing the state.
    // trace:TASK-1297 | ai:claude
    pub(crate) fn with_excluded_from_batch(mut self, excluded: usize) -> Self {
        self.excluded_from_batch = excluded;
        self
    }
}

// Default 1 = serial (the proven path). Concurrency is opt-in per project via
// [drain] pipeline_depth until the pipelined scheduler has earned wild mileage.
// ADR-27 originally set 2 while depth was inert; STORY-1091 makes depth live, so
// the default drops to 1 (amended in ADR-27).
// trace:STORY-1041 trace:STORY-1091 trace:ADR-27 | ai:claude
pub(crate) fn default_pipeline_depth() -> usize {
    1
}

// trace:STORY-1041 trace:ADR-27 | ai:codex
pub(crate) fn clamp_pipeline_depth(depth: usize) -> usize {
    depth.clamp(1, 3)
}

/// Short live-drain segment for glance surfaces (`statusline`, `statusbar`).
///
/// This intentionally reads only `.aida/drain-state.json` and `.aida/drain.lock`.
/// The lock PID is the liveness authority; if the lock is absent or stale,
/// stale state renders as nothing so prompt/title surfaces stay noise-free.
// trace:STORY-824 TASK-1194 | ai:codex
pub(crate) fn status_segment(project_root: &Path) -> Option<String> {
    drain_liveness_probe(project_root).map(|probe| probe.status_segment())
}

/// One shared local-only live-drain probe for `statusline`, `statusbar`, and
/// `status`. Reads the drain-state payload and corroborates liveness through
/// the drain lock's pid; stale/missing/corrupt inputs all fail closed to `None`.
// trace:TASK-1194 | ai:codex
pub(crate) fn drain_liveness_probe(project_root: &Path) -> Option<LiveDrainProbe> {
    let state = DrainState::read(project_root)?;
    drain_lock::read_pid_live_lock(project_root)?;
    LiveDrainProbe::from_state(&state)
}

impl LiveDrainProbe {
    fn from_state(state: &DrainState) -> Option<Self> {
        let spec = state.current.as_deref()?;
        let position = state.position_of(spec)?;
        let total = state.members.len();
        if total == 0 {
            return None;
        }
        Some(Self {
            position,
            total,
            spec: spec.to_string(),
            phase: state
                .current_phase
                .as_deref()
                .and_then(phase_slug)
                .map(str::to_string),
        })
    }

    /// Compact segment for glance surfaces.
    // trace:STORY-824 TASK-1194 | ai:codex
    pub(crate) fn status_segment(&self) -> String {
        match self.phase.as_deref().and_then(abbrev_phase) {
            Some(phase) => format!(
                "drain:{}/{} {}({})",
                self.position, self.total, self.spec, phase
            ),
            None => format!("drain:{}/{} {}", self.position, self.total, self.spec),
        }
    }

    /// Human line for `aida status`.
    // trace:TASK-1194 | ai:codex
    pub(crate) fn status_line(&self) -> String {
        let phase = self
            .phase
            .as_deref()
            .map(|p| format!(" ({p})"))
            .unwrap_or_default();
        format!(
            "▶ drain in flight: spec {}/{} {}{} — watch: aida drain status",
            self.position, self.total, self.spec, phase
        )
    }
}

/// Accept both current drain-state spellings (`2 (ci)`) and bare slugs.
fn phase_slug(raw: &str) -> Option<&str> {
    let slug = raw
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')').map(|(slug, _)| slug))
        .unwrap_or(raw)
        .trim();
    (!slug.is_empty()).then_some(slug)
}

/// Accept both current drain-state spellings (`2 (ci)`) and bare slugs.
fn abbrev_phase(raw: &str) -> Option<&'static str> {
    let slug = phase_slug(raw)?;
    match slug {
        "implementer" | "impl" => Some("impl"),
        "ci" => Some("ci"),
        "reviewer" | "review" => Some("review"),
        "merge" => Some("merge"),
        "pull" => Some("pull"),
        "build" => Some("build"),
        _ => None,
    }
}

/// Reconstruct the command that launched this process — `aida` plus the
/// arguments, so the drain-state file records what the user actually typed.
fn launch_command() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        "aida".to_string()
    } else {
        format!("aida {}", args.join(" "))
    }
}

/// The `on_drain_complete` prediction for a single-spec drain.
fn predict_single(spec: &str) -> String {
    format!(
        "single-spec drain — the orchestrator exits once {spec} finishes its \
         pipeline; no other queued item is auto-picked-up, so the queue head \
         waits for a new command."
    )
}

/// The `on_drain_complete` prediction for a batch drain.
fn predict_batch(batch_name: &str) -> String {
    format!(
        "batch drain — the orchestrator runs every queued `batch:{batch_name}` \
         member in turn, then exits. Queue items outside the batch are NOT \
         auto-picked-up; they wait for a new command."
    )
}

/// The `on_drain_complete` prediction for a `nextN` drain.
fn predict_next_n(n: usize) -> String {
    format!(
        "next-{n} drain — the orchestrator runs the next {n} drivable queue \
         items in turn, then exits. Anything past the {n}th item is NOT \
         auto-picked-up; it waits for a new command."
    )
}

/// TASK-336: record the orchestrator run that is starting for `spec` — its
/// per-run UUID (the BUG-233 corroboration token) and its `--zen` flag (the
/// BUG-237 zen-provenance anchor). A phase child carrying
/// `AIDA_AUTO_COMPLETE_TOKEN=<uuid>` corroborates against [`DrainState::
/// run_uuid`] read back from the file; the [`DrainState::zen`] flag plays the
/// same role [`crate::orchestrator::RunMarker::zen`] used to play for
/// `AIDA_ZEN`. Best-effort — a missing file is a no-op (the drain still runs,
/// just unobservable). trace:TASK-336 | ai:claude
pub(crate) fn set_run(project_root: &Path, spec: &str, run_uuid: &str, zen: bool) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    state.current = Some(spec.to_string());
    state.run_uuid = run_uuid.to_string();
    state.zen = zen;
    if let Some(member) = state.members.iter_mut().find(|m| m.spec == spec) {
        member
            .started_at
            .get_or_insert_with(|| chrono::Utc::now().to_rfc3339());
        member.finished_at = None;
    }
    let _ = state.write(project_root);
    // TASK-993: bound the event stream — rotate at this run-started boundary if
    // it has outgrown the size cap, so it can't grow unbounded across many
    // drains. Rotating only here (a drain/member boundary, never mid-phase)
    // keeps the offset-tracking consumers safe: they reset to offset 0 on the
    // shrink and re-read the fresh stream. trace:TASK-993
    crate::events::rotate_if_oversized(project_root);
    // STORY-712: emit the matching event-stream line so a watcher can be woken
    // by a stream-tail instead of polling this snapshot. Best-effort. trace:TASK-988
    crate::events::emit(
        project_root,
        &crate::events::Event::new(
            Some(spec.to_string()),
            run_uuid,
            crate::events::EventKind::RunStarted,
        ),
    );
}

/// STORY-712: best-effort snapshot of the live drain's current spec + run uuid,
/// for event correlation at emit sites that don't already carry them (the CI
/// wait, the punt ledger, the drain summary). Returns `(None, "")` when no
/// drain-state file exists.
// trace:TASK-988 | ai:claude
pub(crate) fn current_context(project_root: &Path) -> (Option<String>, String) {
    match DrainState::read(project_root) {
        Some(state) => (state.current, state.run_uuid),
        None => (None, String::new()),
    }
}

/// TASK-336: clear the run-scoped fields a child uses to corroborate — the
/// per-run UUID and the `--zen` flag — when the current spec's orchestration
/// returns. Between batch members the drain-state file lives on, but a
/// would-be child carrying a now-stale token must no longer corroborate
/// against it (it was minted by a sibling member that has finished).
/// `current_phase` is also cleared so a stale phase string does not outlive
/// the run that set it. Best-effort. trace:TASK-336 | ai:claude
pub(crate) fn clear_run(project_root: &Path) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    state.run_uuid.clear();
    state.zen = false;
    state.current_phase = None;
    state.phase_started_at = None;
    state.phase_attempt = None; // trace:BUG-1290 | ai:claude
    state.current_session_id = None;
    state.current_vendor = None;
    let _ = state.write(project_root);
}

/// Record that the current member entered phase `phase_index` (`phase_slug` is
/// the phase's machine name, e.g. `implementer`). Updates the top-level
/// `current_phase` and flips the member's own state to `in-phase-N`.
/// Best-effort — a missing file is a no-op. trace:STORY-301 | ai:claude
#[allow(dead_code)] // compatibility wrapper; new drains use set_phase_with_tuning (STORY-1033)
pub(crate) fn set_phase(project_root: &Path, spec: &str, phase_index: i32, phase_slug: &str) {
    set_phase_inner(
        project_root,
        spec,
        phase_index,
        phase_slug,
        None,
        None,
        None,
    );
}

/// TASK-1292: `pr` binds the member's `pr` field the moment the phase
/// starts — not only once the member reaches a terminal outcome
/// (`set_member_outcome`). `aida pr ship`'s PR-keyed drive-ownership check
/// (`pr_ship::reviewer_liveness_for_pr`) reads this live binding, which is
/// what lets it catch a reviewer phase running under a DIFFERENT spec than
/// the one the caller thinks owns the PR (the 2026-09-18 sibling-spec
/// near-miss: a reviewer live on PR-N under spec A did not stop a ship of
/// PR-N driven under spec B). `None` leaves any previously-recorded `pr`
/// untouched.
// trace:STORY-1033 | ai:codex
// trace:TASK-1292 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn set_phase_with_tuning(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    vendor: Option<&str>,
    seat: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    pr: Option<u32>,
) {
    set_phase_inner_with_tuning(
        project_root,
        spec,
        phase_index,
        phase_slug,
        None,
        vendor,
        seat,
        model,
        effort,
        pr,
    );
}

/// BUG-872: record the concrete phase session id alongside the phase. Drain
/// status and `aida tail drain` use this id to resolve the active log; retries
/// for the same spec must never inherit an older sibling attempt's mtime.
///
/// BUG-1290: this compatibility wrapper predates the announce/attach split
/// below and always behaved as an *announcement* (it is the historical sole
/// entry point for a phase that has no separate `set_phase_with_tuning`
/// call), so it keeps that behavior — `announce: true`.
// trace:BUG-872 | ai:codex
#[allow(dead_code)] // compatibility wrapper; new call sites use set_phase_session_vendor
pub(crate) fn set_phase_session(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    session_id: &str,
) {
    let vendor = crate::session::resolve_headless_vendor(project_root);
    set_phase_session_vendor(
        project_root,
        spec,
        phase_index,
        phase_slug,
        session_id,
        vendor,
        true,
    );
}

/// BUG-1290: record which concrete session/vendor is serving the CURRENT
/// phase entry. `announce` distinguishes the two shapes this call has:
///
/// - `announce: true` — this call IS the phase's sole entry point (only
///   `run_implementer` uses this shape today: phase 1 has no separate
///   `mark_drain_phase` call). Behaves exactly like [`set_phase_with_tuning`]
///   — updates `current_phase`/`phase_started_at`, computes the attempt
///   number, and emits `PhaseEntered`.
/// - `announce: false` — the phase was already announced earlier in the same
///   function (`mark_drain_phase` ran first) and this call is only attaching
///   a session id to that already-announced entry — a second reviewer-gate
///   session, or the review-verdict handshake's own session. This is the
///   BUG-1290 fix: the old code re-ran the full announce path here too,
///   which re-emitted `PhaseEntered` a few seconds after the first one, and
///   — because this path never threads a `seat` — the duplicate was always
///   the one missing `seat`, exactly the measured fingerprint. Routes
///   through [`attach_phase_session`] instead, which updates session/vendor
///   only and never emits.
// trace:STORY-1054 trace:BUG-1290 | ai:claude
pub(crate) fn set_phase_session_vendor(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    session_id: &str,
    vendor: crate::session::HeadlessVendor,
    announce: bool,
) {
    if announce {
        // TASK-1292: this announce path never carries a `pr` of its own —
        // it's the session/vendor tracking call, not a PR-binding call. Any
        // PR binding for this phase happens separately via
        // `set_phase_with_tuning`'s `pr` param, and `None` here leaves that
        // binding untouched.
        set_phase_inner(
            project_root,
            spec,
            phase_index,
            phase_slug,
            Some(session_id),
            Some(vendor.as_str()),
            None,
        );
    } else {
        attach_phase_session(
            project_root,
            spec,
            phase_index,
            phase_slug,
            session_id,
            Some(vendor.as_str()),
        );
    }
}

/// BUG-1290: attach a session id/vendor to the phase entry ALREADY announced
/// (via `mark_drain_phase` / `set_phase_with_tuning`) earlier in the same
/// phase function — never re-announces and never emits `PhaseEntered`. A
/// stale attach (the phase moved on, or was never announced, since this
/// session was minted) is a best-effort no-op rather than resurrecting a
/// dead entry's session fields.
// trace:BUG-1290 | ai:claude
fn attach_phase_session(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    session_id: &str,
    vendor: Option<&str>,
) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    let phase_label = format!("{phase_index} ({phase_slug})");
    if state.current.as_deref() != Some(spec)
        || state.current_phase.as_deref() != Some(phase_label.as_str())
    {
        return;
    }
    state.current_session_id = Some(session_id.to_string());
    state.current_vendor = vendor.map(str::to_string);
    let _ = state.write(project_root);
}

fn set_phase_inner(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    session_id: Option<&str>,
    vendor: Option<&str>,
    pr: Option<u32>,
) {
    set_phase_inner_with_tuning(
        project_root,
        spec,
        phase_index,
        phase_slug,
        session_id,
        vendor,
        None,
        None,
        None,
        pr,
    );
}

/// BUG-1290: the ONE site that announces a phase entry and emits
/// `PhaseEntered`. Every announcing caller (`set_phase`, `set_phase_with_tuning`,
/// and `set_phase_session_vendor`'s `announce: true` shape) routes through
/// here. A session-only update that must NOT re-announce goes through
/// [`attach_phase_session`] instead — see its doc comment and
/// [`set_phase_session_vendor`] for the call-site split that fixed the
/// duplicate emission.
#[allow(clippy::too_many_arguments)]
fn set_phase_inner_with_tuning(
    project_root: &Path,
    spec: &str,
    phase_index: i32,
    phase_slug: &str,
    session_id: Option<&str>,
    vendor: Option<&str>,
    seat: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    pr: Option<u32>,
) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    let phase_label = format!("{phase_index} ({phase_slug})");
    // BUG-1290: a retry re-running this SAME phase for this spec (current
    // spec/phase unchanged since the last announce) increments the attempt
    // number; any other case — the very first entry, or advancing to a
    // different phase — starts a fresh count. This is the by-value signal
    // the spec asks for: a legitimate re-entry no longer depends on a
    // `SpecRetried` event happening to sit next to it in the feed.
    let same_entry = state.current.as_deref() == Some(spec)
        && state.current_phase.as_deref() == Some(phase_label.as_str());
    let attempt = if same_entry {
        state.phase_attempt.unwrap_or(1).saturating_add(1)
    } else {
        1
    };
    state.phase_attempt = Some(attempt);
    state.current = Some(spec.to_string());
    state.current_phase = Some(phase_label);
    state.phase_started_at = Some(chrono::Utc::now().to_rfc3339());
    state.current_session_id = session_id.map(str::to_string);
    state.current_vendor = vendor.map(str::to_string);
    // A batch drain re-resolves its queue head between members. Work tagged
    // into the batch after launch was therefore absent from the initial
    // snapshot, even though it could become `current`. Phase entry is the
    // authoritative point at which a refreshed member joins the live drain.
    // trace:BUG-1441 | ai:codex
    if !state.members.iter().any(|m| m.spec == spec) {
        state.members.push(DrainMember::queued(spec));
    }
    let member = state
        .members
        .iter_mut()
        .find(|m| m.spec == spec)
        .expect("phase member was inserted above");
    member
        .started_at
        .get_or_insert_with(|| chrono::Utc::now().to_rfc3339());
    member.state = format!("in-phase-{phase_index}");
    // TASK-1292: bind the PR live, the moment it's known, rather than
    // only at the member's terminal outcome (`set_member_outcome`) — a
    // reviewer phase's PR ownership must be visible to a concurrent
    // `aida pr ship` call WHILE the review is in flight.
    // trace:TASK-1292 | ai:claude
    if pr.is_some() {
        member.pr = pr;
    }
    let _ = state.write(project_root);
    // STORY-712: phase churn is the benign majority — emitted (so a `--all`
    // feed can show it) but classified silent so it never wakes the LLM.
    // Best-effort. trace:TASK-988
    crate::events::emit(
        project_root,
        &crate::events::Event::new(
            Some(spec.to_string()),
            state.run_uuid.clone(),
            crate::events::EventKind::PhaseEntered {
                idx: phase_index,
                slug: phase_slug.to_string(),
                vendor: vendor.map(str::to_string),
                seat: seat.map(str::to_string),
                model: model.map(str::to_string),
                effort: effort.map(str::to_string),
                attempt,
            },
        ),
    );
}

/// BUG-286: append a retry event to the live drain-state file. Best-effort —
/// a missing drain-state file silently no-ops so non-orchestrator paths
/// (`aida pull`, `aida push`, manual `gh pr view`) that piggy-back on
/// `network_retry` outside a drain do not need to know whether one exists.
/// trace:BUG-286 | ai:claude
pub(crate) fn append_retry(project_root: &Path, retry: DrainRetry) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    state.retries.push(retry);
    let _ = state.write(project_root);
}

/// BUG-286: [`crate::network_retry::RetrySink`] that records each retry into
/// the live drain-state file. The orchestrator pairs this with
/// [`crate::network_retry::StderrSink`] via [`crate::network_retry::DualSink`]
/// so retries surface both to the user inspecting the live drain *and* to
/// post-hoc analysis reading `.aida/drain-state.json`.
/// trace:BUG-286 | ai:claude
pub(crate) struct DrainStateSink<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) spec: String,
    pub(crate) phase: Option<String>,
}

impl crate::network_retry::RetrySink for DrainStateSink<'_> {
    fn on_retry(&mut self, ev: &crate::network_retry::RetryEvent) {
        append_retry(
            self.project_root,
            DrainRetry {
                label: ev.label.clone(),
                spec: self.spec.clone(),
                phase: self.phase.clone(),
                attempt: ev.attempt,
                max: ev.max,
                cause: None,
                backoff_ms: ev.backoff_ms,
                stderr_snippet: ev.stderr_snippet.clone(),
                at: chrono::Utc::now().to_rfc3339(),
            },
        );
    }
}

/// STORY-975: append a whole-phase retry to drain state. Kept separate from
/// network retry sinks so transient spec retries can carry the typed cause and
/// current attempt without pretending they are subprocess backoffs.
// trace:STORY-975 | ai:codex
pub(crate) fn append_phase_retry(
    project_root: &Path,
    spec: &str,
    phase: &str,
    cause: &str,
    attempt: u32,
    max: u32,
) {
    append_retry(
        project_root,
        DrainRetry {
            label: "phase retry".to_string(),
            spec: spec.to_string(),
            phase: Some(phase.to_string()),
            attempt,
            max,
            cause: Some(cause.to_string()),
            backoff_ms: 0,
            stderr_snippet: String::new(),
            at: chrono::Utc::now().to_rfc3339(),
        },
    );
}

/// Record a member's terminal outcome — `completed` (its full lifecycle
/// shipped) or `failed` (a phase failed) — and its PR number if one was
/// discovered. Best-effort. trace:STORY-301 | ai:claude
pub(crate) fn set_member_outcome(
    project_root: &Path,
    spec: &str,
    completed: bool,
    pr: Option<u32>,
) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    if let Some(member) = state.members.iter_mut().find(|m| m.spec == spec) {
        member.state = if completed {
            STATE_COMPLETED
        } else {
            STATE_FAILED
        }
        .to_string();
        if pr.is_some() {
            member.pr = pr;
        }
        member.finished_at = Some(chrono::Utc::now().to_rfc3339());
    }
    // The member is no longer the active pipeline — clear the phase so a stale
    // `current_phase` does not outlive the run that set it.
    state.current_phase = None;
    state.phase_started_at = None;
    state.phase_attempt = None; // trace:BUG-1290 | ai:claude
    let _ = state.write(project_root);
    // STORY-712: a member that shipped with a PR is an actionable wake (merge /
    // advance). The *shelved* (completed=false) case is emitted from
    // `punt::append_failure_to_ledger` instead — the one disjoint SpecShelved
    // site, where the phase + kind are known — so it is NOT emitted here, to
    // avoid a double-emit. Best-effort. trace:TASK-988
    if completed {
        if let Some(pr) = pr {
            crate::events::emit(
                project_root,
                &crate::events::Event::new(
                    Some(spec.to_string()),
                    state.run_uuid.clone(),
                    crate::events::EventKind::PhaseDonePr { pr },
                ),
            );
        }
    }
}

/// The corroborated verdict for `aida drain status`: whether the drain-state
/// file describes a live drain, a crashed stale one, or none at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DrainStatus {
    /// No drain-state file — no drain is (or recently was) in progress.
    None,
    /// The file exists and its `orchestrator_pid` is alive — a live drain.
    Active(DrainState),
    /// The file exists but its `orchestrator_pid` is dead — the orchestrator
    /// crashed or was killed without cleaning up. `aida drain clear`
    /// removes it.
    Stale(DrainState),
}

/// Context-aware next command for `aida drain status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrainNext {
    pub(crate) cmd: String,
    pub(crate) why: String,
}

impl DrainNext {
    fn new(cmd: impl Into<String>, why: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            why: why.into(),
        }
    }
}

/// Read the drain-state file and corroborate it against a liveness probe of
/// the recorded orchestrator PID.
pub(crate) fn probe(project_root: &Path) -> DrainStatus {
    match DrainState::read(project_root) {
        None => DrainStatus::None,
        Some(state) => {
            if process_probe::pid_is_alive(state.orchestrator_pid) {
                DrainStatus::Active(state)
            } else {
                DrainStatus::Stale(state)
            }
        }
    }
}

/// Render an RFC-3339 timestamp in the user's local timezone for display;
/// fall back to the raw string if it does not parse.
fn fmt_local(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => rfc3339.to_string(),
    }
}

fn fmt_local_time(rfc3339: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

fn parse_rfc3339_utc(rfc3339: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn format_duration_short(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 60 * 60 {
        format!("{}m", secs / 60)
    } else if secs < 60 * 60 * 24 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {mins}m")
        }
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn duration_between(start: &str, end: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    let start = parse_rfc3339_utc(start)?;
    end.signed_duration_since(start).to_std().ok()
}

fn duration_between_strings(start: &str, end: &str) -> Option<Duration> {
    let end = parse_rfc3339_utc(end)?;
    duration_between(start, end)
}

fn system_time_rfc3339(t: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
}

fn drain_quiet_warn_minutes(project_root: Option<&Path>) -> u64 {
    let Some(project_root) = project_root else {
        return 5;
    };
    crate::config_lookup(
        crate::read_project_config_value(project_root).as_ref(),
        "drain",
        "quiet_warn_minutes",
    )
    .and_then(|v| v.as_integer())
    .and_then(|n| u64::try_from(n).ok())
    .unwrap_or(5)
}

fn last_activity_time(
    project_root: Option<&Path>,
    state: &DrainState,
    spec: &str,
) -> Option<(String, &'static str)> {
    if let (Some(root), Some(session_id)) = (project_root, state.current_session_id.as_deref()) {
        let vendor = state
            .current_vendor
            .as_deref()
            .and_then(crate::session::HeadlessVendor::parse)
            .unwrap_or_else(|| crate::session::resolve_headless_vendor(root));
        let ctx = crate::vendor_activity::VendorActivityContext::new(root, session_id);
        let snap = crate::vendor_activity::snapshot(vendor, &ctx);
        if let Some(mtime) = snap.last_activity {
            return Some((
                system_time_rfc3339(mtime),
                snap.source.unwrap_or("vendor_activity"),
            ));
        }
    }
    state
        .phase_started_at
        .clone()
        .or_else(|| member_started_at(state, spec).map(str::to_string))
        .or_else(|| Some(state.started_at.clone()))
        .map(|ts| (ts, "phase_or_spec_start"))
}

fn member_started_at<'a>(state: &'a DrainState, spec: &str) -> Option<&'a str> {
    state
        .members
        .iter()
        .find(|m| m.spec == spec)
        .and_then(|m| m.started_at.as_deref())
}

// BUG-866: current-row pacing is anchored to drain-state's per-spec and
// per-phase timestamps. The drain start is only a legacy-file fallback when
// those newer anchors are absent.
// trace:BUG-866 | ai:codex
fn current_spec_started_at<'a>(state: &'a DrainState, spec: &str) -> Option<&'a str> {
    member_started_at(state, spec).or(Some(state.started_at.as_str()))
}

fn current_phase_started_at<'a>(state: &'a DrainState, spec: &str) -> Option<&'a str> {
    state
        .phase_started_at
        .as_deref()
        .or_else(|| member_started_at(state, spec))
        .or(Some(state.started_at.as_str()))
}

/// Render a registry glyph honoring the active profile. Default Unicode profile
/// reproduces the historical literals byte-for-byte. trace:TASK-840 | ai:claude
fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

/// The glyph + state description for one member row.
fn member_line(member: &DrainMember) -> String {
    let (glyph, desc) = match member.state.as_str() {
        STATE_COMPLETED => (
            glyph(crate::glyphs::Glyph::Check),
            "completed".green().to_string(),
        ),
        STATE_FAILED => (
            glyph(crate::glyphs::Glyph::Cross),
            "failed".red().to_string(),
        ),
        // `○` (U+25CB) is not a registry glyph — left as a literal marker.
        STATE_QUEUED => ("○", "queued".dimmed().to_string()),
        // `in-phase-N` — the member currently running.
        other => (
            glyph(crate::glyphs::Glyph::FlowActive),
            other
                .strip_prefix("in-phase-")
                .map(|n| format!("phase {n}"))
                .unwrap_or_else(|| other.to_string())
                .cyan()
                .to_string(),
        ),
    };
    let pr = member.pr.map(|n| format!("   PR-{n}")).unwrap_or_default();
    format!("  {glyph} {:<13} {}{}", member.spec, desc, pr)
}

fn member_line_with_pacing(
    member: &DrainMember,
    state: &DrainState,
    project_root: Option<&Path>,
    quiet_warn_minutes: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let mut line = member_line(member);
    if member.is_running() {
        if let Some(phase) = &state.current_phase {
            line = format!(
                "  {} {:<13} {}",
                glyph(crate::glyphs::Glyph::FlowActive),
                member.spec,
                format!("phase {phase}").cyan()
            );
        }
        let mut bits = Vec::new();
        if let Some(started_at) = current_spec_started_at(state, &member.spec) {
            if let Some(elapsed) = duration_between(started_at, now) {
                bits.push(format!("spec {}", format_duration_short(elapsed)));
            }
        }
        if let Some(phase_started_at) = current_phase_started_at(state, &member.spec) {
            if let Some(elapsed) = duration_between(phase_started_at, now) {
                bits.push(format!("phase {}", format_duration_short(elapsed)));
            }
        }
        if let Some((last_at, _source)) = last_activity_time(project_root, state, &member.spec) {
            if let Some(age) = duration_between(&last_at, now) {
                let warn = Duration::from_secs(quiet_warn_minutes.saturating_mul(60));
                let last_output = format!("last output {} ago", format_duration_short(age));
                if quiet_warn_minutes > 0 && age >= warn {
                    // BUG-1210: make the pacing signal progressively urgent:
                    // yellow at the configured threshold, red at twice it.
                    // trace:BUG-1210 | ai:codex
                    let severe = age >= warn.saturating_mul(2);
                    bits.push(if severe {
                        last_output.red().to_string()
                    } else {
                        last_output.yellow().to_string()
                    });
                    let quiet = format!(
                        "{} quiet {}",
                        glyph(crate::glyphs::Glyph::Warning),
                        format_duration_short(age)
                    );
                    bits.push(if severe {
                        quiet.red().to_string()
                    } else {
                        quiet.yellow().to_string()
                    });
                } else {
                    bits.push(last_output);
                }
            }
        }
        if let Some(retry) = latest_phase_retry_for_member(state, &member.spec) {
            bits.push(format!("attempt {}/{}", retry.attempt, retry.max));
        }
        if !bits.is_empty() {
            line.push_str(&format!(" · {}", bits.join(" · ")));
        }
    } else if matches!(member.state.as_str(), STATE_COMPLETED | STATE_FAILED) {
        let mut bits = Vec::new();
        if let Some(finished_at) = member.finished_at.as_deref() {
            if let Some(time) = fmt_local_time(finished_at) {
                bits.push(time);
            }
            if let Some(started_at) = member.started_at.as_deref() {
                if let Some(duration) = duration_between_strings(started_at, finished_at) {
                    bits.push(format!("({})", format_duration_short(duration)));
                }
            }
        }
        if !bits.is_empty() {
            line.push_str(&format!(" · {}", bits.join(" ")));
        }
        if member.state == STATE_FAILED {
            if let Some(summary) = failed_member_reason(project_root, &member.spec) {
                line.push_str(&format!(" · {summary}"));
            }
        }
    }
    line
}

// TASK-1208: choose the drain-status next command from the same member state
// and quietness facts the table renders. Most actionable wins.
// trace:TASK-1208 | ai:codex
pub(crate) fn next_hint(
    state: &DrainState,
    project_root: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> DrainNext {
    if let Some(member) = state.members.iter().find(|m| m.state == STATE_FAILED) {
        return DrainNext::new(
            format!("aida why {} · aida findings list", member.spec),
            "triage",
        );
    }

    if let Some(spec) = state.current.as_deref() {
        if let Some(member) = state
            .members
            .iter()
            .find(|m| m.spec == spec && m.is_running())
        {
            let quiet_warn_minutes = drain_quiet_warn_minutes(project_root);
            let _quiet = last_activity_time(project_root, state, &member.spec)
                .and_then(|(last_at, _)| duration_between(&last_at, now))
                .map(|age| {
                    quiet_warn_minutes > 0
                        && age >= Duration::from_secs(quiet_warn_minutes.saturating_mul(60))
                })
                .unwrap_or(false);
            return DrainNext::new("aida drain tail", "watch live");
        }
    }

    DrainNext::new("aida awaiting", "")
}

fn render_next_human(next: &DrainNext) -> String {
    if next.why.is_empty() {
        format!("  {} {}\n", glyph(crate::glyphs::Glyph::Arrow), next.cmd)
    } else {
        format!(
            "  {} {} {}\n",
            next.why,
            glyph(crate::glyphs::Glyph::Arrow),
            next.cmd
        )
    }
}

// trace:STORY-975 | ai:codex
fn latest_phase_retry_for_member<'a>(state: &'a DrainState, spec: &str) -> Option<&'a DrainRetry> {
    let phase = state.current_phase.as_deref()?;
    state.retries.iter().rev().find(|retry| {
        retry.spec == spec
            && retry.cause.is_some()
            && retry.phase.as_deref() == Some(phase)
            && retry.max > 1
    })
}

// trace:STORY-974 | ai:codex
fn failed_member_reason(project_root: Option<&Path>, spec: &str) -> Option<String> {
    let root = project_root?;
    let store = crate::load_store_for_lookup(root)?;
    let req = store.requirements.iter().find(|r| {
        [r.agreed_id.as_deref(), r.spec_id.as_deref()]
            .into_iter()
            .flatten()
            .any(|id| id.eq_ignore_ascii_case(spec))
    })?;
    let fr = req.failure_reason.as_ref()?;
    let cause = crate::auto_complete_telemetry::failure_cause_label(Some(&fr.kind));
    let detail = crate::auto_complete_telemetry::failure_detail_first_line(Some(&fr.detail));
    Some(format!("{cause} — {detail}"))
}

/// Render the human summary for a drain (`stale` adds the crashed-orchestrator
/// framing + the `--clear` hint). Pure — the command handler does the I/O.
#[cfg(test)]
pub(crate) fn render_human(state: &DrainState, stale: bool) -> String {
    render_human_inner(state, stale, None, chrono::Utc::now())
}

/// The members a READER should see, which is not always the members the state
/// STORES.
///
/// A legacy or crash-window state file can name a `current` spec that has no
/// matching entry in `members`. Rendering `members` verbatim then claims
/// nothing is running while the drain is actively working that spec — the
/// defect this function exists to close.
///
/// THIS IS A VIEW, AND THE DISTINCTION IS LOAD-BEARING. The synthetic member
/// is produced for DISPLAY and lives only in the returned Vec; it is never
/// inserted into `DrainState`. That is deliberate and not stylistic: six
/// non-test sites do `let Some(mut state) = DrainState::read(..)`, mutate, and
/// persist with `state.write(..)`, and five of them write back through the
/// same value. Normalising into the state itself — the obvious "fix it once"
/// move — would let a fabricated member reach disk through any of those five
/// and be read back later as real drain state. A row rendered wrong is a
/// papercut; a drain acting on a member that never existed is not.
///
/// SO: every renderer calls this, and nothing writes its result back.
// trace:BUG-1441 | ai:claude
fn display_members(state: &DrainState) -> Vec<DrainMember> {
    let mut members = state.members.clone();
    if let Some(current) = state.current.as_deref() {
        if !members.iter().any(|m| m.spec == current) {
            let phase_index = state
                .current_phase
                .as_deref()
                .and_then(|phase| phase.split_whitespace().next())
                .unwrap_or("?");
            let mut member = DrainMember::queued(current);
            member.state = format!("in-phase-{phase_index}");
            member.started_at = state.phase_started_at.clone();
            members.push(member);
        }
    }
    members
}

/// Render the human summary with project-local pacing context.
// trace:STORY-948 | ai:codex
pub(crate) fn render_human_with_context(
    state: &DrainState,
    stale: bool,
    project_root: &Path,
) -> String {
    render_human_inner(state, stale, Some(project_root), chrono::Utc::now())
}

fn render_human_inner(
    state: &DrainState,
    stale: bool,
    project_root: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let mut out = String::new();
    let scope = match &state.batch {
        Some(name) => format!("batch:{name}"),
        None => match state.mode.as_str() {
            "next-n" => "next-N queue drain".to_string(),
            _ => state
                .members
                .first()
                .map(|m| m.spec.clone())
                .unwrap_or_else(|| "single spec".to_string()),
        },
    };

    if stale {
        out.push_str(&format!(
            "{}\n",
            format!(
                "{} Stale drain-state file — orchestrator (pid {}) is no longer running.",
                glyph(crate::glyphs::Glyph::Warning),
                state.orchestrator_pid
            )
            .red()
            .bold()
        ));
        out.push_str("  The drain crashed or was killed without cleaning up.\n\n");
    }

    out.push_str(&format!(
        "{} drain: {}\n",
        if stale { "Last" } else { "Active" },
        scope
    ));
    out.push_str(&format!("  {}\n", state.command.dimmed()));
    out.push_str(&format!(
        "  {}\n",
        format!(
            "orchestrator pid {} · started {}",
            state.orchestrator_pid,
            fmt_local(&state.started_at)
        )
        .dimmed()
    ));
    // TASK-1297: echo the "M other approved specs routed to this role are
    // not in this batch" figure a live batch drain prints at start-up, so
    // `aida drain status` shows the same number while it is still running.
    // Silent when there is nothing to declare (M == 0). trace:TASK-1297
    if state.batch.is_some() {
        if let Some(clause) = crate::batch_exclusion_clause(state.excluded_from_batch, None) {
            out.push_str(&format!("  {}\n", clause.yellow()));
        }
    }
    out.push('\n');

    let quiet_warn_minutes = drain_quiet_warn_minutes(project_root);
    // The view, not `state.members` — see `display_members`. The missing-active
    // row used to be special-cased HERE, which is why TOON and JSON never got
    // it. trace:BUG-1441 | ai:claude
    let members = display_members(state);
    for member in &members {
        let line = member_line_with_pacing(member, state, project_root, quiet_warn_minutes, now);
        out.push_str(&line);
        out.push('\n');
    }

    // STORY-1041: a pipelined drain can have more than one active member, so
    // the progress line reports merged + active counts instead of pretending
    // there is only one "spec N of M". trace:STORY-1041 trace:ADR-27 | ai:codex
    let merged = members
        .iter()
        .filter(|m| m.state == STATE_COMPLETED)
        .count();
    let in_flight: Vec<&DrainMember> = members.iter().filter(|m| m.is_running()).collect();
    if state.pipeline_depth > 1 && members.len() > 1 {
        out.push_str(&format!(
            "\n  {merged} merged, {} in flight (pipeline depth {}).\n",
            in_flight.len(),
            state.pipeline_depth
        ));
    } else if let Some(cur) = &state.current {
        if let Some(pos) = state.position_of(cur) {
            if members.len() > 1 {
                out.push_str(&format!(
                    "\n  {cur} is spec {pos} of {} in this drain.\n",
                    members.len()
                ));
            }
        }
    }

    out.push('\n');
    if !stale {
        let mut footer = Vec::new();
        if let Some(elapsed) = duration_between(&state.started_at, now) {
            footer.push(format!("elapsed {}", format_duration_short(elapsed)));
        }
        if let Some((spec, at)) = state
            .members
            .iter()
            .filter(|m| m.state == STATE_COMPLETED)
            .filter_map(|m| m.finished_at.as_deref().map(|at| (m.spec.as_str(), at)))
            .max_by_key(|(_, at)| {
                parse_rfc3339_utc(at)
                    .map(|dt| dt.timestamp())
                    .unwrap_or(i64::MIN)
            })
        {
            if let Some(time) = fmt_local_time(at) {
                footer.push(format!("last ship: {spec} at {time}"));
            }
        }
        if !footer.is_empty() {
            out.push_str(&format!(
                "  Drain started {} · {}\n",
                fmt_local(&state.started_at),
                footer.join(" · ")
            ));
        }
        out.push_str(&format!("  On exit: {}\n", state.on_drain_complete));
    } else {
        out.push_str("  Run `aida drain clear` to remove this stale file.\n");
    }
    let next = next_hint(state, project_root, now);
    out.push_str(&render_next_human(&next));
    out
}

// BUG-759: a `burndown run` launcher holds `.aida/drain.lock` for its entire
// wall-clock (pid, started, command, blessed spec set) but writes NO per-phase
// drain-state file — its fan-out lives inside the headless `claude -p` child,
// not an orchestrator. `aida drain status` used to read only the drain-state
// file, so mid-burndown it printed "No drain in progress" while implementers
// were demonstrably working. These renderers give the command a truthful
// lock-backed report for that case. Pure — the command handler does the I/O.
// trace:BUG-759 | ai:claude
pub(crate) fn render_lock_human(lock: &crate::drain_lock::DrainLock, stale_state: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("Active drain: {}\n", lock.command));
    let host = if lock.host.is_empty() {
        String::new()
    } else {
        format!(" · host {}", lock.host)
    };
    out.push_str(&format!(
        "  launcher pid {} · started {}{}\n",
        lock.pid,
        fmt_local(&lock.started_at_utc),
        host
    ));
    if !lock.specs.is_empty() {
        out.push_str(&format!("  specs: {}\n", lock.specs.join(", ")));
    }
    out.push('\n');
    out.push_str(
        "  This drain is launcher-held (e.g. `aida burndown run` / `aida queue integrate`):\n  \
         its work fans out inside the launched session, so there is no per-phase progress\n  \
         file to report. A second drain launched now would be refused until it exits.\n",
    );
    if stale_state {
        out.push_str(&format!(
            "\n  {} a stale drain-state file from an earlier drain also exists — \
             `aida drain clear` removes it.\n",
            glyph(crate::glyphs::Glyph::Warning)
        ));
    }
    out
}

/// The `--json` payload when the live drain is known only from the launcher's
/// drain lock (no drain-state file). `source` disambiguates the shape from the
/// richer drain-state payload.
// trace:BUG-759 | ai:claude
pub(crate) fn render_lock_json(lock: &crate::drain_lock::DrainLock, stale_state: bool) -> String {
    let value = serde_json::json!({
        "status": "active",
        "source": "drain-lock",
        "pid": lock.pid,
        "started_at": lock.started_at_utc,
        "command": lock.command,
        "host": lock.host,
        "specs": lock.specs,
        "stale_drain_state": stale_state,
        "next": {
            "cmd": "aida drain tail",
            "why": "watch live",
        },
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

/// Render the agent-mode TOON payload for a launcher-held drain.
// trace:TASK-1208 | ai:codex
pub(crate) fn render_lock_toon(lock: &crate::drain_lock::DrainLock, stale_state: bool) -> String {
    let mut out = crate::toon::scalar("status", "active");
    out.push('\n');
    out.push_str(&crate::toon::scalar("source", "drain-lock"));
    out.push('\n');
    out.push_str(&crate::toon::scalar("command", &lock.command));
    out.push('\n');
    out.push_str(&crate::toon::scalar("pid", &lock.pid.to_string()));
    out.push('\n');
    out.push_str(&crate::toon::scalar(
        "stale_drain_state",
        if stale_state { "true" } else { "false" },
    ));
    out.push('\n');
    out.push_str(&crate::toon::table_raw(
        "specs",
        &["spec"],
        &lock
            .specs
            .iter()
            .map(|spec| vec![spec.clone()])
            .collect::<Vec<_>>(),
    ));
    out.push('\n');
    out.push_str(&crate::toon::table_raw(
        "next",
        &["cmd", "to"],
        &[vec![
            "aida drain tail".to_string(),
            "watch live".to_string(),
        ]],
    ));
    out
}

/// Render the agent-mode TOON payload for `aida drain status`.
// trace:TASK-1208 | ai:codex
pub(crate) fn render_toon_with_context(status: &DrainStatus, project_root: &Path) -> String {
    render_toon_inner(status, Some(project_root), chrono::Utc::now())
}

#[cfg(test)]
fn render_toon(status: &DrainStatus) -> String {
    render_toon_inner(status, None, chrono::Utc::now())
}

fn render_toon_inner(
    status: &DrainStatus,
    project_root: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    match status {
        DrainStatus::None => crate::toon::scalar("status", "none"),
        DrainStatus::Active(state) | DrainStatus::Stale(state) => {
            let status_word = if matches!(status, DrainStatus::Active(_)) {
                "active"
            } else {
                "stale"
            };
            let mut out = crate::toon::scalar("status", status_word);
            out.push('\n');
            out.push_str(&crate::toon::scalar("command", &state.command));
            out.push('\n');
            out.push_str(&crate::toon::scalar(
                "current",
                state.current.as_deref().unwrap_or(""),
            ));
            out.push('\n');
            out.push_str(&crate::toon::table_raw(
                "members",
                &["spec", "state", "pr"],
                &display_members(state)
                    .iter()
                    .map(|m| {
                        vec![
                            m.spec.clone(),
                            m.state.clone(),
                            m.pr.map(|n| format!("PR-{n}")).unwrap_or_default(),
                        ]
                    })
                    .collect::<Vec<_>>(),
            ));
            let next = next_hint(state, project_root, now);
            out.push('\n');
            out.push_str(&crate::toon::table_raw(
                "next",
                &["cmd", "to"],
                &[vec![next.cmd, next.why]],
            ));
            out
        }
    }
}

/// Render the `--json` payload for `aida drain status`.
#[cfg(test)]
pub(crate) fn render_json(status: &DrainStatus) -> String {
    render_json_inner(status, None, chrono::Utc::now())
}

/// Render the `--json` payload with project-local pacing context.
// trace:STORY-948 | ai:codex
pub(crate) fn render_json_with_context(status: &DrainStatus, project_root: &Path) -> String {
    render_json_inner(status, Some(project_root), chrono::Utc::now())
}

fn render_json_inner(
    status: &DrainStatus,
    project_root: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let value = match status {
        DrainStatus::None => serde_json::json!({ "status": "none" }),
        DrainStatus::Active(state) | DrainStatus::Stale(state) => {
            let mut obj = serde_json::to_value(state).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(map) = obj.as_object_mut() {
                let word = if matches!(status, DrainStatus::Active(_)) {
                    "active"
                } else {
                    "stale"
                };
                map.insert("status".to_string(), serde_json::json!(word));
                // Serde emitted `state.members`; the reader must see the VIEW.
                // trace:BUG-1441 | ai:claude
                map.insert(
                    "members".to_string(),
                    serde_json::json!(display_members(state)),
                );
                map.insert("pacing".to_string(), pacing_json(state, project_root, now));
                let next = next_hint(state, project_root, now);
                map.insert(
                    "next".to_string(),
                    serde_json::json!({ "cmd": next.cmd, "why": next.why }),
                );
            }
            obj
        }
    };
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

fn pacing_json(
    state: &DrainState,
    project_root: Option<&Path>,
    now: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    let quiet_warn_minutes = drain_quiet_warn_minutes(project_root);
    let current = state.current.as_deref().map(|spec| {
        let spec_started_at = current_spec_started_at(state, spec).unwrap_or(&state.started_at);
        let phase_started_at = current_phase_started_at(state, spec).unwrap_or(&state.started_at);
        let (last_output_at, last_output_source) = last_activity_time(project_root, state, spec)
            .unwrap_or_else(|| (phase_started_at.to_string(), "phase_or_spec_start"));
        let last_output_age_secs = duration_between(&last_output_at, now).map(|d| d.as_secs());
        serde_json::json!({
            "spec": spec,
            "spec_started_at": spec_started_at,
            "spec_elapsed_secs": duration_between(spec_started_at, now).map(|d| d.as_secs()),
            "phase": state.current_phase,
            "phase_started_at": phase_started_at,
            "phase_elapsed_secs": duration_between(phase_started_at, now).map(|d| d.as_secs()),
            "last_output_at": last_output_at,
            "last_output_source": last_output_source,
            "last_output_age_secs": last_output_age_secs,
            "quiet_warn_minutes": quiet_warn_minutes,
            "quiet": quiet_warn_minutes > 0
                && last_output_age_secs
                    .map(|age| age >= quiet_warn_minutes.saturating_mul(60))
                    .unwrap_or(false),
        })
    });
    let members: Vec<serde_json::Value> = state
        .members
        .iter()
        .map(|member| {
            serde_json::json!({
                "spec": member.spec,
                "state": member.state,
                "started_at": member.started_at,
                "finished_at": member.finished_at,
                "duration_secs": member.started_at.as_deref().zip(member.finished_at.as_deref())
                    .and_then(|(start, finish)| duration_between_strings(start, finish))
                    .map(|d| d.as_secs()),
            })
        })
        .collect();
    let last_ship = state
        .members
        .iter()
        .filter(|m| m.state == STATE_COMPLETED)
        .filter_map(|m| m.finished_at.as_deref().map(|at| (m.spec.as_str(), at)))
        .max_by_key(|(_, at)| {
            parse_rfc3339_utc(at)
                .map(|dt| dt.timestamp())
                .unwrap_or(i64::MIN)
        })
        .map(|(spec, at)| serde_json::json!({ "spec": spec, "at": at }));
    serde_json::json!({
        "started_at": state.started_at,
        "elapsed_secs": duration_between(&state.started_at, now).map(|d| d.as_secs()),
        "quiet_warn_minutes": quiet_warn_minutes,
        "current": current,
        "members": members,
        "last_ship": last_ship,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // trace:BUG-1585 | ai:codex
    fn autonomous_drain_guide_matches_pipeline_depth_default() {
        let guide = include_str!("../../docs/autonomous-drain.md");
        let source = include_str!("drain_state.rs");
        let default = default_pipeline_depth();
        let field_docs = source
            .split_once("pub(crate) pipeline_depth: usize")
            .expect("DrainState.pipeline_depth field exists")
            .0
            .rsplit_once("pub(crate) members: Vec<DrainMember>")
            .expect("pipeline_depth follows DrainState.members")
            .1;
        let lines: Vec<_> = guide.lines().collect();
        let configured = lines.windows(2).find_map(|pair| {
            (pair[0] == "[drain]")
                .then(|| pair[1].strip_prefix("pipeline_depth = "))
                .flatten()
                .and_then(|value| value.parse::<usize>().ok())
        });

        assert_eq!(configured, Some(default));
        assert!(guide.contains(&format!("The default is depth `{default}`")));
        assert!(guide.contains("ADR-27"));
        assert!(guide.contains("STORY-1091"));
        assert!(field_docs.contains(&format!(
            "/// The live default is `{default}`; values above `{default}` opt in to overlap."
        )));
        assert!(!field_docs.contains("`2` is the pipelined\n    /// default"));
    }

    fn write_lock(dir: &std::path::Path, pid: u32) {
        let path = drain_lock::drain_lock_path(dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Keep this fixture in the pre-TASK-1284 shape: legacy locks without a
        // start time must remain readable and use PID-only liveness.
        // trace:TASK-1284 | ai:codex
        let lock = serde_json::json!({
            "pid": pid,
            "started_at_utc": "2026-09-05T12:00:00+00:00",
            "command": "aida queue work --auto-complete",
            "host": "test-host",
            "specs": [],
        });
        std::fs::write(path, serde_json::to_string(&lock).unwrap()).unwrap();
    }

    fn single_state() -> DrainState {
        DrainState {
            command: "aida queue work STORY-301 --auto-complete --zen".to_string(),
            mode: "single".to_string(),
            batch: None,
            members: vec![DrainMember::queued("STORY-301")],
            pipeline_depth: 1,
            current: Some("STORY-301".to_string()),
            current_phase: None,
            phase_started_at: None,
            phase_attempt: None,
            current_session_id: None,
            current_vendor: None,
            orchestrator_pid: std::process::id(),
            started_at: "2026-05-18T23:28:00+00:00".to_string(),
            on_drain_complete: predict_single("STORY-301"),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
            excluded_from_batch: 0,
        }
    }

    fn batch_state() -> DrainState {
        let mut s = DrainState::new_batch(
            "autonomy-modes",
            &[
                "STORY-301".to_string(),
                "STORY-285".to_string(),
                "STORY-276".to_string(),
            ],
        );
        s.orchestrator_pid = std::process::id();
        s.started_at = "2026-05-18T23:28:00+00:00".to_string();
        s
    }

    #[test]
    fn status_segment_renders_live_drain_progress() {
        let dir = tempfile::tempdir().unwrap();
        write_lock(dir.path(), std::process::id());
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("2 (ci)".to_string());
        state.members[1].state = "in-phase-2".to_string();
        state.write(dir.path()).unwrap();

        assert_eq!(
            status_segment(dir.path()).as_deref(),
            Some("drain:2/3 STORY-285(ci)")
        );
        assert_eq!(
            drain_liveness_probe(dir.path()).map(|probe| probe.status_line()),
            Some(
                "▶ drain in flight: spec 2/3 STORY-285 (ci) — watch: aida drain status".to_string()
            )
        );
    }

    // BUG-1441: even an old/in-flight state file with a top-level current
    // spec missing from the launch snapshot must show the active row.
    // trace:BUG-1441 | ai:codex
    #[test]
    fn render_human_synthesizes_missing_current_member() {
        let mut state = batch_state();
        state.current = Some("BUG-REFRESHED".to_string());
        state.current_phase = Some("2 (ci)".to_string());
        state.phase_started_at = Some("2026-05-18T23:30:00+00:00".to_string());

        let rendered = render_human(&state, false);
        assert!(rendered.contains("BUG-REFRESHED"));
        assert!(rendered.contains("ci"));
    }
    /// The human renderer was the ONLY one that synthesised the missing row,
    /// so a headless caller (TOON by default) and the documented JSON monitor
    /// surface both still reported an empty members view for a drain that was
    /// actively working a spec. This asserts all THREE agree, because fixing
    /// the two cited representations and leaving the shape is how the next
    /// renderer inherits the bug.
    // trace:BUG-1441 | ai:claude
    #[test]
    fn every_representation_shows_the_missing_current_member() {
        let mut state = batch_state();
        state.current = Some("BUG-REFRESHED".to_string());
        state.current_phase = Some("2 (ci)".to_string());
        state.phase_started_at = Some("2026-05-18T23:30:00+00:00".to_string());
        let status = DrainStatus::Active(state.clone());

        assert!(
            render_human(&state, false).contains("BUG-REFRESHED"),
            "human"
        );

        // Parse the members TABLE rather than searching the whole document:
        // TOON also emits `current` as a scalar, so a bare contains() passes
        // whether or not the members table was fixed. The first revision of
        // this test did exactly that and survived reverting the TOON change.
        let toon = render_toon(&status);
        let members_at = toon.find("members[").expect("a members table renders");
        let table =
            crate::toon::parse_table(&toon[members_at..]).expect("the members table parses");
        let toon_specs: Vec<&str> = table
            .rows
            .iter()
            .filter_map(|row| row.first().map(String::as_str))
            .collect();
        assert!(
            toon_specs.contains(&"BUG-REFRESHED"),
            "TOON is what non-TTY and headless callers get; members were {toon_specs:?}"
        );

        let json: serde_json::Value =
            serde_json::from_str(&render_json(&status)).expect("json renders");
        let specs: Vec<&str> = json["members"]
            .as_array()
            .expect("members is an array")
            .iter()
            .filter_map(|m| m["spec"].as_str())
            .collect();
        assert!(
            specs.contains(&"BUG-REFRESHED"),
            "JSON is the documented monitor surface; members were {specs:?}"
        );
    }

    /// THE SAFETY PROPERTY, and the reason the synthetic lives in a view
    /// rather than in the state.
    ///
    /// Five non-test sites read the state, mutate it and write it back. If
    /// normalisation happened at the loader, any of them would persist a
    /// member that never existed and a later read would treat it as real.
    /// Rendering must therefore leave the persisted file untouched - this test
    /// fails the moment someone "simplifies" the view into the state.
    // trace:BUG-1441 | ai:claude
    #[test]
    fn rendering_never_persists_the_synthetic_member() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = batch_state();
        state.current = Some("BUG-REFRESHED".to_string());
        state.current_phase = Some("2 (ci)".to_string());
        state.write(dir.path()).unwrap();

        let status = DrainStatus::Active(state.clone());
        let _ = render_human(&state, false);
        let _ = render_toon(&status);
        let _ = render_json(&status);

        assert!(
            !state.members.iter().any(|m| m.spec == "BUG-REFRESHED"),
            "rendering must not mutate the state it was given"
        );
        let reloaded = DrainState::read(dir.path()).expect("state reloads");
        assert!(
            !reloaded.members.iter().any(|m| m.spec == "BUG-REFRESHED"),
            "a synthetic display member must never reach the persisted file"
        );
        assert_eq!(
            reloaded.members.len(),
            3,
            "the batch fixture has three real members and gains none"
        );
    }

    #[test]
    fn live_drain_spec_uses_orchestrator_for_ci_wait() {
        let dir = tempfile::tempdir().unwrap();
        write_lock(dir.path(), std::process::id());
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("2 (ci)".to_string());
        state.members[1].state = "in-phase-2".to_string();
        state.write(dir.path()).unwrap();

        assert_eq!(
            live_drain_spec(dir.path(), "story-285"),
            Some(LiveDrainSpec {
                pid: std::process::id(),
                phase: "2/6 (CI wait)".to_string(),
                round: None,
            })
        );
        assert_eq!(live_drain_spec(dir.path(), "STORY-301"), None);
    }

    #[test]
    fn status_segment_hides_stale_or_missing_drain() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = single_state();
        state.current_phase = Some("1 (implementer)".to_string());
        state.write(dir.path()).unwrap();
        assert_eq!(status_segment(dir.path()), None);
        assert_eq!(drain_liveness_probe(dir.path()), None);

        write_lock(dir.path(), u32::MAX - 1);
        assert_eq!(status_segment(dir.path()), None);
        assert_eq!(drain_liveness_probe(dir.path()), None);
    }

    #[test]
    fn status_segment_abbreviates_phase_names() {
        assert_eq!(abbrev_phase("1 (implementer)"), Some("impl"));
        assert_eq!(abbrev_phase("3 (reviewer)"), Some("review"));
        assert_eq!(abbrev_phase("merge"), Some("merge"));
        assert_eq!(abbrev_phase("7 (unknown)"), None);
    }

    // AC9: single-spec drain state file round-trips through write + read.
    #[test]
    fn single_spec_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let state = single_state();
        state.write(dir.path()).unwrap();
        assert_eq!(DrainState::read(dir.path()), Some(state));
    }

    // AC9: batch drain state file round-trips, members preserved in order.
    #[test]
    fn batch_state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let state = batch_state();
        state.write(dir.path()).unwrap();
        let read = DrainState::read(dir.path()).unwrap();
        assert_eq!(read, state);
        assert_eq!(read.members.len(), 3);
        assert_eq!(read.members[0].spec, "STORY-301");
        assert_eq!(read.batch.as_deref(), Some("autonomy-modes"));
    }

    // The file is written atomically and leaves no temp litter behind.
    #[test]
    fn write_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        single_state().write(dir.path()).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".aida"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "atomic write left a temp file behind");
    }

    // AC4: no drain-state file → DrainStatus::None.
    #[test]
    fn probe_none_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe(dir.path()), DrainStatus::None);
    }

    // A live orchestrator PID → DrainStatus::Active.
    #[test]
    fn probe_active_for_live_pid() {
        let dir = tempfile::tempdir().unwrap();
        single_state().write(dir.path()).unwrap();
        assert!(matches!(probe(dir.path()), DrainStatus::Active(_)));
    }

    // AC5: a dead orchestrator PID → DrainStatus::Stale.
    #[test]
    fn probe_stale_for_dead_pid() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = single_state();
        state.orchestrator_pid = u32::MAX - 1; // no real process owns this
        state.write(dir.path()).unwrap();
        assert!(matches!(probe(dir.path()), DrainStatus::Stale(_)));
    }

    // clear() removes the file and is idempotent.
    #[test]
    fn clear_removes_file_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        single_state().write(dir.path()).unwrap();
        assert!(drain_state_path(dir.path()).exists());
        DrainState::clear(dir.path()).unwrap();
        assert!(!drain_state_path(dir.path()).exists());
        // A second clear on the now-absent file is still a clean success.
        DrainState::clear(dir.path()).unwrap();
    }

    // set_phase updates the top-level phase AND the member's own state.
    #[test]
    fn set_phase_updates_current_and_member() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase(dir.path(), "STORY-285", 3, "reviewer");
        let read = DrainState::read(dir.path()).unwrap();
        assert_eq!(read.current.as_deref(), Some("STORY-285"));
        assert_eq!(read.current_phase.as_deref(), Some("3 (reviewer)"));
        assert!(read.phase_started_at.is_some());
        let member = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert_eq!(member.state, "in-phase-3");
        assert!(member.started_at.is_some());
        assert!(member.is_running());
    }

    // BUG-1290: a single phase entry via `set_phase_with_tuning` (the
    // announcer `mark_drain_phase` uses for CI/Reviewer/Merge/Pull/Build)
    // emits exactly one PhaseEntered, stamped attempt 1.
    #[test]
    fn set_phase_with_tuning_emits_exactly_one_phase_entered() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase_with_tuning(
            dir.path(),
            "STORY-285",
            3,
            "reviewer",
            Some("claude"),
            Some("reviewer"),
            None,
            None,
            None,
        );
        let entered: Vec<_> = crate::events::read_all(dir.path())
            .into_iter()
            .filter(|e| matches!(e.kind, crate::events::EventKind::PhaseEntered { .. }))
            .collect();
        assert_eq!(entered.len(), 1, "exactly one PhaseEntered per phase entry");
        match &entered[0].kind {
            crate::events::EventKind::PhaseEntered { attempt, seat, .. } => {
                assert_eq!(*attempt, 1);
                assert_eq!(seat.as_deref(), Some("reviewer"));
            }
            other => panic!("expected PhaseEntered, got {other:?}"),
        }
    }

    // BUG-1290: the exact reviewer-phase shape that produced the measured
    // duplicate — `mark_drain_phase` (here, `set_phase_with_tuning`) at the
    // top of the phase, followed later by a session mint (here,
    // `set_phase_session_vendor(.., announce=false)`) for the review-verdict
    // handshake or an agent gate. The fix must not re-announce: only the
    // FIRST call may emit, and the second must still attach the session id
    // onto the live state so status/tail readers keep working.
    #[test]
    fn session_attach_after_announce_does_not_reannounce() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase_with_tuning(
            dir.path(),
            "STORY-285",
            3,
            "reviewer",
            Some("claude"),
            Some("reviewer"),
            None,
            None,
            None,
        );
        set_phase_session_vendor(
            dir.path(),
            "STORY-285",
            3,
            "reviewer",
            "session-abc",
            crate::session::HeadlessVendor::Claude,
            false,
        );
        let entered: Vec<_> = crate::events::read_all(dir.path())
            .into_iter()
            .filter(|e| matches!(e.kind, crate::events::EventKind::PhaseEntered { .. }))
            .collect();
        assert_eq!(
            entered.len(),
            1,
            "a session attach on an already-announced entry must not emit a second PhaseEntered"
        );
        let read = DrainState::read(dir.path()).unwrap();
        assert_eq!(read.current_session_id.as_deref(), Some("session-abc"));
    }

    // BUG-1290 acceptance #2/#4: a retry re-entering the SAME phase (the
    // orchestrator's transient-retry loop re-runs the phase function, which
    // re-announces via `mark_drain_phase`) still emits a fresh PhaseEntered —
    // it is NOT swallowed — and its attempt number is one greater than the
    // attempt that preceded it, so the re-entry is distinguishable by value.
    #[test]
    fn retry_reentry_emits_second_phase_entered_with_incremented_attempt() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        for _ in 0..2 {
            set_phase_with_tuning(
                dir.path(),
                "STORY-285",
                3,
                "reviewer",
                Some("claude"),
                Some("reviewer"),
                None,
                None,
                None,
            );
        }
        let attempts: Vec<u32> = crate::events::read_all(dir.path())
            .into_iter()
            .filter_map(|e| match e.kind {
                crate::events::EventKind::PhaseEntered { attempt, .. } => Some(attempt),
                _ => None,
            })
            .collect();
        assert_eq!(
            attempts,
            vec![1, 2],
            "a retry re-entering the same phase must emit its own PhaseEntered, \
             carrying an incremented attempt — never swallowed by a blanket dedup"
        );
    }

    // BUG-1290: a genuinely NEW phase entry (a different phase, or the same
    // phase for a spec that has since moved on) resets the attempt count —
    // the increment only fires on an exact (spec, phase) repeat.
    #[test]
    fn new_phase_resets_attempt_to_one() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase_with_tuning(
            dir.path(),
            "STORY-285",
            2,
            "ci",
            Some("claude"),
            Some("implementer"),
            None,
            None,
            None,
        );
        set_phase_with_tuning(
            dir.path(),
            "STORY-285",
            3,
            "reviewer",
            Some("claude"),
            Some("reviewer"),
            None,
            None,
            None,
        );
        let attempts: Vec<u32> = crate::events::read_all(dir.path())
            .into_iter()
            .filter_map(|e| match e.kind {
                crate::events::EventKind::PhaseEntered { attempt, .. } => Some(attempt),
                _ => None,
            })
            .collect();
        assert_eq!(
            attempts,
            vec![1, 1],
            "advancing to a new phase is not a retry"
        );
    }

    // BUG-1290 fingerprint guard: phase 1 (implementer) has no separate
    // `mark_drain_phase` call — `set_phase_session_vendor(.., announce=true)`
    // is its sole entry point, and seat is legitimately unknown there. This
    // seat-less entry must survive as the phase's one-and-only emission, not
    // be mistaken for (or dropped as) the spurious no-seat duplicate.
    #[test]
    fn seatless_phase_one_entry_survives_as_the_only_emission() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase_session_vendor(
            dir.path(),
            "STORY-301",
            1,
            "implementer",
            "session-xyz",
            crate::session::HeadlessVendor::Claude,
            true,
        );
        let entered: Vec<_> = crate::events::read_all(dir.path())
            .into_iter()
            .filter(|e| matches!(e.kind, crate::events::EventKind::PhaseEntered { .. }))
            .collect();
        assert_eq!(entered.len(), 1);
        match &entered[0].kind {
            crate::events::EventKind::PhaseEntered { seat, attempt, .. } => {
                assert_eq!(*seat, None, "phase 1's seat is legitimately unknown");
                assert_eq!(*attempt, 1);
            }
            other => panic!("expected PhaseEntered, got {other:?}"),
        }
    }

    // TASK-1292: set_phase_with_tuning binds the member's `pr` LIVE, at
    // phase-entry, not only once the member reaches a terminal outcome —
    // this is what `pr_ship::reviewer_liveness_for_pr` reads to gate
    // `aida pr ship` while a reviewer is mid-review.
    // trace:TASK-1292 | ai:claude
    #[test]
    fn set_phase_with_tuning_binds_pr_live_before_the_member_finishes() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_phase_with_tuning(
            dir.path(),
            "STORY-285",
            3,
            "reviewer",
            Some("claude"),
            Some("reviewer"),
            None,
            None,
            Some(1948),
        );
        let read = DrainState::read(dir.path()).unwrap();
        let member = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert_eq!(member.state, "in-phase-3");
        assert_eq!(
            member.pr,
            Some(1948),
            "the PR must be bound while the member is still running, not only at set_member_outcome"
        );
    }

    // BUG-1441 acceptance fixture: the batch snapshot is formed first, then a
    // newly tagged/queued spec is selected by the live refresh path. Entering
    // its phase must append the same member row launch-time work receives.
    // trace:BUG-1441 | ai:codex
    #[test]
    fn phase_entry_records_member_added_after_batch_launch() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();

        set_phase(dir.path(), "BUG-REFRESHED", 1, "implementer");

        let read = DrainState::read(dir.path()).unwrap();
        assert_eq!(read.current.as_deref(), Some("BUG-REFRESHED"));
        assert_eq!(read.current_phase.as_deref(), Some("1 (implementer)"));
        let refreshed = read
            .members
            .iter()
            .find(|member| member.spec == "BUG-REFRESHED")
            .expect("refresh-added work must join the members table");
        assert_eq!(refreshed.state, "in-phase-1");
        assert!(refreshed.started_at.is_some());
    }

    // TASK-1292 regression fixture (full drain_state round trip): a reviewer
    // phase live on PR-N under spec A must be visible to a PR-keyed liveness
    // check even though the drive's "current" bookkeeping points at a
    // different, terminal sibling spec B — the 2026-09-18 near-miss shape.
    // trace:TASK-1292 | ai:claude
    #[test]
    fn sibling_spec_reviewer_binding_is_visible_pr_keyed_not_spec_keyed() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        // spec B (STORY-285) shelves out — terminal, no PR of its own.
        set_member_outcome(dir.path(), "STORY-285", false, None);
        // spec A (STORY-276) is live-reviewing PR #1948.
        set_phase_with_tuning(
            dir.path(),
            "STORY-276",
            3,
            "reviewer",
            Some("claude"),
            Some("reviewer"),
            None,
            None,
            Some(1948),
        );
        let read = DrainState::read(dir.path()).unwrap();
        let liveness = crate::pr_ship::reviewer_liveness_for_pr(
            read.members.iter().map(|m| (m.is_running(), m.pr)),
            1948,
        );
        assert_eq!(
            liveness,
            crate::pr_ship::ReviewerLiveness::OnThisPr,
            "PR-1948's live sibling-spec reviewer must be found regardless of which spec's run it belongs to"
        );
    }

    // set_member_outcome marks completed/failed and records the PR.
    #[test]
    fn set_member_outcome_marks_terminal_state() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_member_outcome(dir.path(), "STORY-301", true, Some(82));
        set_member_outcome(dir.path(), "STORY-285", false, None);
        let read = DrainState::read(dir.path()).unwrap();
        let done = read.members.iter().find(|m| m.spec == "STORY-301").unwrap();
        assert_eq!(done.state, "completed");
        assert_eq!(done.pr, Some(82));
        assert!(done.finished_at.is_some());
        let failed = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert_eq!(failed.state, "failed");
        assert!(failed.finished_at.is_some());
        assert_eq!(read.current_phase, None);
        assert_eq!(read.phase_started_at, None);
    }

    // set_phase / set_member_outcome / set_run / clear_run on a project with
    // no file are silent no-ops.
    #[test]
    fn updates_are_noops_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        set_phase(dir.path(), "STORY-1", 1, "implementer");
        set_member_outcome(dir.path(), "STORY-1", true, None);
        // TASK-336: same no-op semantics for set_run / clear_run.
        set_run(dir.path(), "STORY-1", "tok", true);
        clear_run(dir.path());
        assert_eq!(probe(dir.path()), DrainStatus::None);
    }

    // TASK-336: AC1+AC2 — new_single bakes run_uuid + zen into the initial file.
    #[test]
    fn new_single_includes_run_uuid_and_zen() {
        let s = DrainState::new_single("STORY-301", "abc-uuid", true);
        assert_eq!(s.run_uuid, "abc-uuid");
        assert!(s.zen);
        // new_batch / new_next_n start empty — members not yet running.
        let b = DrainState::new_batch("autonomy-modes", &["STORY-301".to_string()]);
        assert!(b.run_uuid.is_empty());
        assert!(!b.zen);
    }

    // TASK-336: AC3 — set_run records the current spec's run-UUID + zen flag.
    #[test]
    fn set_run_records_current_run_uuid_and_zen() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_run(dir.path(), "STORY-285", "live-token", true);
        let read = DrainState::read(dir.path()).unwrap();
        assert_eq!(read.current.as_deref(), Some("STORY-285"));
        assert_eq!(read.run_uuid, "live-token");
        assert!(read.zen);
        let member = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert!(member.started_at.is_some());
    }

    // trace:STORY-975 | ai:codex
    #[test]
    fn render_human_shows_current_phase_retry_attempt() {
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("3 (reviewer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:40:00+00:00".to_string());
        if let Some(member) = state.members.iter_mut().find(|m| m.spec == "STORY-285") {
            member.state = "in-phase-3".to_string();
            member.started_at = Some("2026-05-18T23:35:00+00:00".to_string());
        }
        state.retries.push(DrainRetry {
            label: "phase retry".to_string(),
            spec: "STORY-285".to_string(),
            phase: Some("3 (reviewer)".to_string()),
            attempt: 2,
            max: 2,
            cause: Some("watchdog".to_string()),
            backoff_ms: 0,
            stderr_snippet: String::new(),
            at: "2026-05-18T23:41:00+00:00".to_string(),
        });

        let rendered = render_human(&state, false);
        assert!(rendered.contains("STORY-285"));
        assert!(rendered.contains("attempt 2/2"));
    }

    // TASK-336: clear_run wipes the run-scoped fields so a stale child token
    // can no longer corroborate against the file (between batch members).
    #[test]
    fn clear_run_wipes_run_uuid_and_zen() {
        let dir = tempfile::tempdir().unwrap();
        batch_state().write(dir.path()).unwrap();
        set_run(dir.path(), "STORY-285", "live-token", true);
        set_phase(dir.path(), "STORY-285", 3, "reviewer");
        clear_run(dir.path());
        let read = DrainState::read(dir.path()).unwrap();
        assert!(read.run_uuid.is_empty());
        assert!(!read.zen);
        assert_eq!(read.current_phase, None);
        assert_eq!(read.phase_started_at, None);
    }

    // TASK-336: a pre-TASK-336 drain-state file (no run_uuid / zen fields)
    // parses cleanly with the fields defaulted to empty / false — keeps
    // recovery working across a binary upgrade in the middle of a drain.
    #[test]
    fn pre_task_336_file_parses_with_default_run_fields() {
        let body = r#"{
          "command": "aida queue work STORY-301 --auto-complete",
          "mode": "single",
          "members": [{"spec": "STORY-301", "state": "queued"}],
          "current": "STORY-301",
          "orchestrator_pid": 1,
          "started_at": "2026-05-18T23:28:00+00:00",
          "on_drain_complete": "single-spec drain"
        }"#;
        let state: DrainState = serde_json::from_str(body).unwrap();
        assert!(state.run_uuid.is_empty());
        assert!(!state.zen);
        assert_eq!(state.phase_started_at, None);
        assert_eq!(state.members[0].started_at, None);
        assert_eq!(state.members[0].finished_at, None);
    }

    // TASK-336: AC4 — a stale drain-state file (dead PID) classifies as
    // Stale, so any corroboration keyed off `probe` falls to "not live".
    #[test]
    fn stale_pid_makes_run_uuid_not_corroborate() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = single_state();
        state.run_uuid = "any-token".to_string();
        state.orchestrator_pid = u32::MAX - 1; // not a real pid
        state.write(dir.path()).unwrap();
        // probe() returns Stale; the orchestrator-side corroboration uses the
        // same pid_is_alive check, so this drives `run_is_live` → false.
        assert!(matches!(probe(dir.path()), DrainStatus::Stale(_)));
    }

    #[test]
    fn position_of_is_one_based() {
        let state = batch_state();
        assert_eq!(state.position_of("STORY-301"), Some(1));
        assert_eq!(state.position_of("STORY-276"), Some(3));
        assert_eq!(state.position_of("BUG-999"), None);
    }

    // AC9: drain status output for a single-spec drain mid-pipeline.
    #[test]
    fn render_human_single_shows_command_and_phase() {
        let mut state = single_state();
        state.current_phase = Some("1 (implementer)".to_string());
        state.members[0].state = "in-phase-1".to_string();
        let out = render_human(&state, false);
        assert!(out.contains("Active drain: STORY-301"));
        assert!(out.contains("aida queue work STORY-301 --auto-complete --zen"));
        assert!(out.contains("phase 1 (implementer)"));
        assert!(out.contains("On exit:"));
        assert!(out.contains("auto-picked-up"));
    }

    // AC9: drain status output for a batch drain — progress across members.
    #[test]
    fn render_human_batch_shows_member_progress() {
        let mut state = batch_state();
        // Exercise the depth>1 rendering explicitly; the default is now serial (1).
        state.pipeline_depth = 2;
        state.members[0].state = "completed".to_string();
        state.members[0].pr = Some(80);
        state.members[1].state = "in-phase-3".to_string();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("3 (reviewer)".to_string());
        let out = render_human(&state, false);
        assert!(out.contains("Active drain: batch:autonomy-modes"));
        let check = crate::glyphs::Glyph::Check.render(crate::glyphs::active_profile(None));
        let active = crate::glyphs::Glyph::FlowActive.render(crate::glyphs::active_profile(None));
        assert!(out.contains(&format!("{check} STORY-301")));
        assert!(out.contains("PR-80"));
        assert!(out.contains(&format!("{active} STORY-285")));
        assert!(out.contains("phase 3 (reviewer)"));
        assert!(out.contains("○ STORY-276"));
        // STORY-1041: pipelined drains report merged + active counts, not a
        // single current-position line.
        assert!(out.contains("1 merged, 1 in flight (pipeline depth 2)."));
    }

    // TASK-1297: `aida drain status` echoes the "M other approved specs
    // routed to this role are not in this batch" figure while a batch drain
    // is live — the same line the drain printed at start-up.
    // trace:TASK-1297 | ai:claude
    #[test]
    fn render_human_batch_shows_excluded_from_batch_when_nonzero() {
        let mut state = batch_state();
        state.excluded_from_batch = 16;
        let out = render_human(&state, false);
        assert!(
            out.contains("16 other approved specs routed to this role are not in this batch"),
            "{out}"
        );
    }

    /// TASK-1297: M == 0 renders no extra noise, per acceptance.
    #[test]
    fn render_human_batch_omits_exclusion_line_when_zero() {
        let state = batch_state();
        assert_eq!(state.excluded_from_batch, 0);
        let out = render_human(&state, false);
        assert!(!out.contains("are not in this batch"), "{out}");
    }

    /// TASK-1297: a single-spec drain has no `--batch` filter, so the
    /// exclusion line never renders even if the field were somehow nonzero.
    #[test]
    fn render_human_single_never_shows_exclusion_line() {
        let mut state = single_state();
        state.excluded_from_batch = 5;
        let out = render_human(&state, false);
        assert!(!out.contains("are not in this batch"), "{out}");
    }

    // STORY-948: terminal rows show finish clock + per-spec duration; the
    // footer repeats total elapsed and the latest completed ship.
    #[test]
    fn render_human_shows_finished_time_duration_and_last_ship() {
        let mut state = batch_state();
        state.members[0].state = "completed".to_string();
        state.members[0].started_at = Some("2026-05-18T23:28:00+00:00".to_string());
        state.members[0].finished_at = Some("2026-05-18T23:55:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-19T00:10:00+00:00").unwrap();

        let out = render_human_inner(&state, false, None, now);

        // Keep this assertion stable when CLICOLOR_FORCE=1 inserts a reset
        // between the coloured state word and the pacing separator.
        assert!(out.contains("completed"));
        assert!(out.contains(" ·"));
        assert!(out.contains("(27m)"));
        assert!(out.contains("elapsed 42m"));
        assert!(out.contains("last ship: STORY-301"));
    }

    // STORY-948: without a log, the current row falls back to phase-entry time
    // for last-output age and applies the configurable quiet threshold.
    #[test]
    fn render_human_current_row_shows_elapsed_last_output_and_quiet_warning() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "[drain]\nquiet_warn_minutes = 1\n",
        )
        .unwrap();
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("1 (implementer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:40:00+00:00".to_string());
        state.members[1].state = "in-phase-1".to_string();
        state.members[1].started_at = Some("2026-05-18T23:30:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let out = render_human_inner(&state, false, Some(dir.path()), now);

        assert!(out.contains("spec 12m"));
        assert!(out.contains("phase 2m"));
        assert!(out.contains("last output 2m ago"));
        assert!(out.contains("quiet 2m"));
    }

    // TASK-1208: a quiet active phase points the operator at the live drain log.
    #[test]
    fn next_hint_quiet_active_phase_points_at_tail_drain() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(
            dir.path().join(".aida").join("config.toml"),
            "[drain]\nquiet_warn_minutes = 1\n",
        )
        .unwrap();
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("3 (reviewer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:40:00+00:00".to_string());
        state.members[1].state = "in-phase-3".to_string();
        state.members[1].started_at = Some("2026-05-18T23:30:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let next = next_hint(&state, Some(dir.path()), now);

        assert_eq!(next.cmd, "aida drain tail");
        assert_eq!(next.why, "watch live");
        let out = render_human_inner(&state, false, Some(dir.path()), now);
        assert!(out.contains("watch live"));
        assert!(out.contains("aida drain tail"));
    }

    // TASK-1208: a shelved/failed member is more actionable than watching the
    // active row, so it wins the next-command hint.
    #[test]
    fn next_hint_failed_member_points_at_why_and_findings() {
        let mut state = batch_state();
        state.members[0].state = STATE_FAILED.to_string();
        state.members[1].state = "in-phase-3".to_string();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("3 (reviewer)".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let next = next_hint(&state, None, now);

        assert_eq!(next.cmd, "aida why STORY-301 · aida findings list");
        assert_eq!(next.why, "triage");
    }

    // TASK-1208: when the drain has no running or shelved member left to drive,
    // route to awaiting rather than inventing a member-specific command.
    #[test]
    fn next_hint_exiting_points_at_awaiting() {
        let mut state = batch_state();
        state.current = None;
        state.current_phase = None;
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let next = next_hint(&state, None, now);

        assert_eq!(next.cmd, "aida awaiting");
        assert_eq!(next.why, "");
    }

    #[test]
    fn current_row_ignores_old_attempt_log_for_different_session() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join(".aida").join("headless-logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("story-285-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl"),
            "{}\n",
        )
        .unwrap();
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("1 (implementer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:40:00+00:00".to_string());
        state.current_session_id = Some("019e9999-0000-7000-8000-000000000000".to_string());
        state.members[1].state = "in-phase-1".to_string();
        state.members[1].started_at = Some("2026-05-18T23:30:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let out = render_human_inner(&state, false, Some(dir.path()), now);

        assert!(out.contains("last output 2m ago"), "got: {out}");
    }

    // BUG-866: spec/phase elapsed are anchored to the current member and phase
    // timestamps, not to the older drain start timestamp.
    #[test]
    fn render_human_current_row_uses_member_and_phase_elapsed_anchors() {
        let mut state = batch_state();
        state.started_at = "2026-05-18T22:35:00+00:00".to_string();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("1 (implementer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:35:00+00:00".to_string());
        state.members[1].state = "in-phase-1".to_string();
        state.members[1].started_at = Some("2026-05-18T23:35:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:40:00+00:00").unwrap();

        let out = render_human_inner(&state, false, None, now);

        assert!(out.contains("spec 5m"), "got: {out}");
        assert!(out.contains("phase 5m"), "got: {out}");
        assert!(!out.contains("spec 1h"), "got: {out}");
        assert!(!out.contains("phase 1h"), "got: {out}");
    }

    // AC9 / AC5: drain status output for a stale (crashed) drain.
    #[test]
    fn render_human_stale_reports_crash_and_clear_hint() {
        let state = single_state();
        let out = render_human(&state, true);
        assert!(out.contains("Stale drain-state file"));
        assert!(out.contains("no longer running"));
        assert!(out.contains("aida drain clear"));
    }

    // The --json payload carries the corroborated status word.
    #[test]
    fn render_json_carries_status_word() {
        let none = render_json(&DrainStatus::None);
        assert!(none.contains("\"status\": \"none\""));
        let active = render_json(&DrainStatus::Active(single_state()));
        assert!(active.contains("\"status\": \"active\""));
        assert!(active.contains("STORY-301"));
        let stale = render_json(&DrainStatus::Stale(single_state()));
        assert!(stale.contains("\"status\": \"stale\""));
    }

    // TASK-1297: `aida drain status --json` carries the exclusion count for
    // machine consumers — a monitor should not have to scrape the human line.
    // trace:TASK-1297 | ai:claude
    #[test]
    fn render_json_carries_excluded_from_batch() {
        let mut state = batch_state();
        state.excluded_from_batch = 16;
        let active = render_json(&DrainStatus::Active(state));
        assert!(active.contains("\"excluded_from_batch\": 16"), "{active}");
    }

    // STORY-948: the JSON projection carries the same raw timestamps and
    // derived durations used by the human renderer.
    #[test]
    fn render_json_pacing_carries_raw_timestamps() {
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.current_phase = Some("1 (implementer)".to_string());
        state.phase_started_at = Some("2026-05-18T23:40:00+00:00".to_string());
        state.members[0].state = "completed".to_string();
        state.members[0].started_at = Some("2026-05-18T23:28:00+00:00".to_string());
        state.members[0].finished_at = Some("2026-05-18T23:55:00+00:00".to_string());
        state.members[1].state = "in-phase-1".to_string();
        state.members[1].started_at = Some("2026-05-18T23:30:00+00:00".to_string());
        let now = parse_rfc3339_utc("2026-05-18T23:42:00+00:00").unwrap();

        let out = render_json_inner(&DrainStatus::Active(state), None, now);
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(
            value["pacing"]["current"]["spec_started_at"],
            "2026-05-18T23:30:00+00:00"
        );
        assert_eq!(
            value["pacing"]["current"]["phase_started_at"],
            "2026-05-18T23:40:00+00:00"
        );
        assert_eq!(
            value["pacing"]["current"]["last_output_source"],
            "phase_or_spec_start"
        );
        assert_eq!(value["pacing"]["current"]["spec_elapsed_secs"], 720);
        assert_eq!(value["pacing"]["members"][0]["duration_secs"], 1620);
        assert_eq!(value["pacing"]["last_ship"]["spec"], "STORY-301");
    }

    // TASK-1208: JSON supervisors get the same next command as the human line.
    #[test]
    fn render_json_carries_next_hint() {
        let mut state = batch_state();
        state.members[0].state = STATE_FAILED.to_string();

        let out = render_json_inner(&DrainStatus::Active(state), None, chrono::Utc::now());
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();

        assert_eq!(
            value["next"]["cmd"],
            "aida why STORY-301 · aida findings list"
        );
        assert_eq!(value["next"]["why"], "triage");
    }

    // TASK-1208: TOON mirrors the established next[] table shape, while the no
    // active drain path stays a single compact status line.
    #[test]
    fn render_toon_carries_next_table_but_none_is_single_line() {
        let mut state = batch_state();
        state.current = Some("STORY-285".to_string());
        state.members[1].state = "in-phase-1".to_string();

        let out = render_toon(&DrainStatus::Active(state));

        assert!(out.contains("next[1]{cmd,to}:"));
        assert!(out.contains("aida drain tail,watch live"));
        assert_eq!(render_toon(&DrainStatus::None), "status: none");
    }

    // ── BUG-759: lock-backed report when the launcher holds the drain ──

    fn burndown_lock() -> crate::drain_lock::DrainLock {
        crate::drain_lock::DrainLock {
            pid: 3_822_683,
            pid_start_time: Some("2026-07-19T11:29:59+00:00".to_string()),
            started_at_utc: "2026-07-19T11:30:00+00:00".to_string(),
            command: "burndown run (status=approved)".to_string(),
            host: "devbox".to_string(),
            wave_id: "wave-test".to_string(),
            binary_sha: "abc1234".to_string(),
            binary_mtime_secs: Some(1_700_000_000),
            binary_path: "/repo/target/release/aida".to_string(),
            launched_stale: false,
            specs: vec!["BUG-101".to_string(), "TASK-202".to_string()],
        }
    }

    // BUG-759: with no drain-state file but a live launcher-held lock, the
    // human report names the drain — pid, started, command, spec set — instead
    // of "No drain in progress".
    #[test]
    fn render_lock_human_names_pid_started_command_and_specs() {
        let out = render_lock_human(&burndown_lock(), false);
        assert!(out.contains("Active drain: burndown run (status=approved)"));
        assert!(out.contains("launcher pid 3822683"));
        assert!(out.contains("host devbox"));
        assert!(out.contains("specs: BUG-101, TASK-202"));
        assert!(out.contains("refused"));
        assert!(!out.contains("stale drain-state"));
    }

    // BUG-759: a leftover stale drain-state tombstone alongside the live lock
    // is called out with the --clear hint, without hiding the live drain.
    #[test]
    fn render_lock_human_notes_a_stale_drain_state_tombstone() {
        let out = render_lock_human(&burndown_lock(), true);
        assert!(out.contains("Active drain: burndown run (status=approved)"));
        assert!(out.contains("stale drain-state file"));
        assert!(out.contains("aida drain clear"));
    }

    // BUG-759: an empty spec set (a lock written by an older binary, or a
    // launcher that recorded none) omits the specs line rather than printing
    // an empty one.
    #[test]
    fn render_lock_human_omits_empty_spec_set() {
        let mut lock = burndown_lock();
        lock.specs.clear();
        let out = render_lock_human(&lock, false);
        assert!(!out.contains("specs:"));
    }

    // BUG-759: the JSON payload is active + source-tagged so machine consumers
    // can tell a lock-backed report from the richer drain-state payload.
    #[test]
    fn render_lock_json_is_active_and_source_tagged() {
        let out = render_lock_json(&burndown_lock(), false);
        assert!(out.contains("\"status\": \"active\""));
        assert!(out.contains("\"source\": \"drain-lock\""));
        assert!(out.contains("\"pid\": 3822683"));
        assert!(out.contains("BUG-101"));
        assert!(out.contains("\"stale_drain_state\": false"));
        assert!(out.contains("\"next\""));
        assert!(out.contains("aida drain tail"));
    }

    // AC6: on_drain_complete predicts which queue items will / won't be
    // auto-picked-up, distinctly per mode.
    #[test]
    fn on_drain_complete_predicts_per_mode() {
        assert!(predict_single("STORY-301").contains("no other queued item"));
        assert!(predict_batch("autonomy-modes").contains("batch:autonomy-modes"));
        assert!(predict_batch("autonomy-modes").contains("NOT"));
        assert!(predict_next_n(3).contains("next 3"));
    }
}
