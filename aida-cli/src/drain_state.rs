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
//! `aida drain status --clear` removes it.
//!
//! Every write goes through [`aida_core::write_atomic`] (TASK-331): a phase
//! transition and a concurrent `aida drain status` read must never see a torn
//! file.
//!
//! trace:STORY-301 | ai:claude

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::process_probe;

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
}

impl DrainMember {
    /// A freshly-queued member — the shape every member has at drain start.
    fn queued(spec: impl Into<String>) -> Self {
        Self {
            spec: spec.into(),
            state: STATE_QUEUED.to_string(),
            pr: None,
        }
    }

    /// True while this member is mid-pipeline (`in-phase-N`).
    fn is_running(&self) -> bool {
        self.state.starts_with("in-phase-")
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
    /// The spec currently in its pipeline; `None` before the first member
    /// starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current: Option<String>,
    /// The phase the current member is in, e.g. `1 (implementer)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_phase: Option<String>,
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
            current: Some(spec.to_string()),
            current_phase: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_single(spec),
            run_uuid: run_uuid.to_string(),
            zen,
            retries: Vec::new(),
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
            current: None,
            current_phase: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_batch(batch_name),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
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
            current: None,
            current_phase: None,
            orchestrator_pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            on_drain_complete: predict_next_n(n),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
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
    /// success (`aida drain status --clear` on a project with no drain).
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
    let _ = state.write(project_root);
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
    let _ = state.write(project_root);
}

/// Record that the current member entered phase `phase_index` (`phase_slug` is
/// the phase's machine name, e.g. `implementer`). Updates the top-level
/// `current_phase` and flips the member's own state to `in-phase-N`.
/// Best-effort — a missing file is a no-op. trace:STORY-301 | ai:claude
pub(crate) fn set_phase(project_root: &Path, spec: &str, phase_index: i32, phase_slug: &str) {
    let Some(mut state) = DrainState::read(project_root) else {
        return;
    };
    state.current = Some(spec.to_string());
    state.current_phase = Some(format!("{phase_index} ({phase_slug})"));
    if let Some(member) = state.members.iter_mut().find(|m| m.spec == spec) {
        member.state = format!("in-phase-{phase_index}");
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
                backoff_ms: ev.backoff_ms,
                stderr_snippet: ev.stderr_snippet.clone(),
                at: chrono::Utc::now().to_rfc3339(),
            },
        );
    }
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
    }
    // The member is no longer the active pipeline — clear the phase so a stale
    // `current_phase` does not outlive the run that set it.
    state.current_phase = None;
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
    /// crashed or was killed without cleaning up. `aida drain status --clear`
    /// removes it.
    Stale(DrainState),
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

/// Render a registry glyph honoring the active profile. Default Unicode profile
/// reproduces the historical literals byte-for-byte. trace:TASK-840 | ai:claude
fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

/// The glyph + state description for one member row.
fn member_line(member: &DrainMember) -> String {
    let (glyph, desc) = match member.state.as_str() {
        STATE_COMPLETED => (glyph(crate::glyphs::Glyph::Check), "completed".to_string()),
        STATE_FAILED => (glyph(crate::glyphs::Glyph::Cross), "failed".to_string()),
        // `○` (U+25CB) is not a registry glyph — left as a literal marker.
        STATE_QUEUED => ("○", "queued".to_string()),
        // `in-phase-N` — the member currently running.
        other => (
            glyph(crate::glyphs::Glyph::FlowActive),
            other
                .strip_prefix("in-phase-")
                .map(|n| format!("phase {n}"))
                .unwrap_or_else(|| other.to_string()),
        ),
    };
    let pr = member.pr.map(|n| format!("   PR-{n}")).unwrap_or_default();
    format!("  {glyph} {:<13} {}{}", member.spec, desc, pr)
}

/// Render the human summary for a drain (`stale` adds the crashed-orchestrator
/// framing + the `--clear` hint). Pure — the command handler does the I/O.
pub(crate) fn render_human(state: &DrainState, stale: bool) -> String {
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
            "{} Stale drain-state file — orchestrator (pid {}) is no longer running.\n",
            glyph(crate::glyphs::Glyph::Warning),
            state.orchestrator_pid
        ));
        out.push_str("  The drain crashed or was killed without cleaning up.\n\n");
    }

    out.push_str(&format!(
        "{} drain: {}\n",
        if stale { "Last" } else { "Active" },
        scope
    ));
    out.push_str(&format!("  {}\n", state.command));
    out.push_str(&format!(
        "  orchestrator pid {} · started {}\n\n",
        state.orchestrator_pid,
        fmt_local(&state.started_at)
    ));

    for member in &state.members {
        let mut line = member_line(member);
        // Annotate the running member with the precise phase + a pointer at
        // what the orchestrator is waiting on.
        if member.is_running() {
            if let Some(phase) = &state.current_phase {
                line = format!(
                    "  {} {:<13} phase {}",
                    glyph(crate::glyphs::Glyph::FlowActive),
                    member.spec,
                    phase
                );
            }
        }
        out.push_str(&line);
        out.push('\n');
    }

    // A "spec N of M" line answers the user's "how far through am I?" — the
    // current member is the spec the orchestrator-child session is running.
    if let Some(cur) = &state.current {
        if let Some(pos) = state.position_of(cur) {
            if state.members.len() > 1 {
                out.push_str(&format!(
                    "\n  {cur} is spec {pos} of {} in this drain.\n",
                    state.members.len()
                ));
            }
        }
    }

    out.push('\n');
    if !stale {
        out.push_str(&format!("  On exit: {}\n", state.on_drain_complete));
    } else {
        out.push_str("  Run `aida drain status --clear` to remove this stale file.\n");
    }
    out
}

/// Render the `--json` payload for `aida drain status`.
pub(crate) fn render_json(status: &DrainStatus) -> String {
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
            }
            obj
        }
    };
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_state() -> DrainState {
        DrainState {
            command: "aida queue work STORY-301 --auto-complete --zen".to_string(),
            mode: "single".to_string(),
            batch: None,
            members: vec![DrainMember::queued("STORY-301")],
            current: Some("STORY-301".to_string()),
            current_phase: None,
            orchestrator_pid: std::process::id(),
            started_at: "2026-05-18T23:28:00+00:00".to_string(),
            on_drain_complete: predict_single("STORY-301"),
            run_uuid: String::new(),
            zen: false,
            retries: Vec::new(),
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
        let member = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert_eq!(member.state, "in-phase-3");
        assert!(member.is_running());
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
        let failed = read.members.iter().find(|m| m.spec == "STORY-285").unwrap();
        assert_eq!(failed.state, "failed");
        assert_eq!(read.current_phase, None);
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
        // The "how far through" line names the current member's position.
        assert!(out.contains("STORY-285 is spec 2 of 3 in this drain."));
    }

    // AC9 / AC5: drain status output for a stale (crashed) drain.
    #[test]
    fn render_human_stale_reports_crash_and_clear_hint() {
        let state = single_state();
        let out = render_human(&state, true);
        assert!(out.contains("Stale drain-state file"));
        assert!(out.contains("no longer running"));
        assert!(out.contains("aida drain status --clear"));
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
