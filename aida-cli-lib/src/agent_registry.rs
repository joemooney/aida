// trace:TASK-3429 | ai:codex
//! Agent process registry.
//!
//! STORY-431 (Phase 1) deliberately keeps process liveness separate from
//! session leases: leases describe scope ownership, while `.aida/agents/*.toml`
//! describes an observable process. Phase 1 only registers MCP-serving
//! processes because AIDA does not yet launch every agent; Phase 2/3 launchers
//! will widen coverage to interactive sessions that never call MCP tools.
//!
//! STORY-435 (Phase 4) adds heartbeat-driven busy/idle: every MCP tool call
//! bumps `last_active_at`, and `classify_status` flips Busy → Idle once that
//! timestamp is older than the configurable `[agent_registry]
//! busy_threshold_secs` (default 30s). An active Live session lease covering
//! the agent's worktree uses `work_grace_secs` (default 10s) to distinguish
//! recent descendant work from a finished seat.

// trace:BUG-2425 | ai:claude — the one namespace judge lives in aida-core.
use crate::leases::SessionLease;
use crate::process_retry::RetryEtxtbsy;
#[cfg(test)]
use aida_core::pid_observer::PidNamespaceProbe;
pub(crate) use aida_core::pid_observer::PidObserver;
use aida_core::proc_identity::{self, ProcIdentity, Verdict};
use std::path::{Path, PathBuf};

use aida_core::liveness::ProcTree;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// STORY-528 (SPIKE-13 MVP): an agent's *availability* is a coarse, explicit
// operator-set state that is orthogonal to the heartbeat-derived busy/idle
// signal. A budget-exhausted or rate-limited agent is still a live process
// (so STORY-435 keeps reading it Busy/Idle), but it cannot usefully take new
// work until its quota resets. `Paused` records that fact so `aida status`
// can warn an operator (and brief-time dispatch can flag) before work is
// routed to an agent that will just sit on it. trace:STORY-528 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Availability {
    #[default]
    Available,
    Paused,
}

impl Availability {
    pub(crate) fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }
}

/// Why an agent is paused. `Unknown` is the safe fallback for a record that
/// somehow carries `availability = paused` without a recorded reason.
// trace:STORY-528 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PauseReason {
    Budget,
    RateLimit,
    Manual,
    #[default]
    Unknown,
}

impl PauseReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::RateLimit => "rate-limit",
            Self::Manual => "manual",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "budget" => Some(Self::Budget),
            "rate-limit" | "ratelimit" => Some(Self::RateLimit),
            "manual" => Some(Self::Manual),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRegistryEntry {
    #[serde(deserialize_with = "proc_identity::deserialize_registry_id")]
    pub(crate) id: String,
    pub(crate) agent_type: String,
    pub(crate) pid: u32,
    /// Kernel start time of `pid`, so a recycled pid does not read as the
    /// live agent. Missing on entries written by older binaries; liveness
    /// then remains unknown until the session registers again.
    // trace:TASK-1750 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid_start: Option<String>,
    // Namespace in which the stored numeric PID was observed. Mail routing
    // treats a different or unobservable namespace as unknown, never dead.
    // trace:STORY-1843 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid_namespace: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal: Option<TerminalIdentity>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_active_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) role: Option<String>,
    /// The named seat this session holds (`aida agent new --seat`, exported
    /// as `AIDA_ROTATION_SEAT`), e.g. `integration` for a session whose role
    /// is `implementer`. Mail addresses it as `seat:<name>`. `None` on rows
    /// from older binaries and on sessions launched without a seat, which
    /// then route exactly as before.
    // trace:STORY-2745 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) current_spec: Option<String>,
    pub(crate) worktree_path: PathBuf,
    pub(crate) source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) binary_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) build_sha: Option<String>,
    // STORY-528: paused-availability state. `serde(default)` keeps legacy
    // records (written before this field existed) readable — they deserialize
    // as `Available` with no pause metadata. trace:STORY-528 | ai:claude
    #[serde(default)]
    pub(crate) availability: Availability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) paused_since: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) paused_reason: Option<PauseReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expected_back: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claude_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ended_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resumed_from: Option<String>,
    /// The spec's status AT THE MOMENT this session ended — recorded so a
    /// later `aida agent resume` can render the drift brief's required
    /// "status then vs now" comparison instead of only the current status.
    /// `None` on legacy records and sessions with no spec.
    // trace:STORY-790 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spec_status_at_end: Option<String>,
    /// Whether the project's mail-wake watcher wakes this session's native
    /// thread for its own mail. `None` on rows nothing has armed.
    // trace:STORY-HOME-096 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mail_wake: Option<MailWake>,
    /// The `key:` binding token of the seat grant this session holds the
    /// session key for, recorded by its own hooks only after proving
    /// possession. A key-only (sandboxed) submitter's heartbeat vouches for
    /// a grant only when this token is one of that grant's bindings, since a
    /// directly issued grant's subject is the OS user, not this row's name.
    // trace:STORY-2338 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) grant_key: Option<String>,
    /// Explicit launcher narrowing, restored on resume and revalidated at issuance.
    /// This is a requested selection, never authority; empty/legacy rows inherit
    /// the current validated parent just like a fresh launch without flags.
    // trace:BUG-2467 | ai:codex
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) delegate_seats: Vec<String>,
}

/// The `key:` token of the seat grant this process exports AND holds the
/// session key for, or `None`. Possession is the proof: the token is only
/// returned when the keyring secret hashes to one of the grant's bindings.
// trace:STORY-2338 | ai:claude
pub(crate) fn held_grant_key() -> Option<String> {
    let id = crate::gate::seat_authority::current_grant_id()?;
    let grant = crate::gate::seat_authority::read_grant(&id).ok()?;
    crate::gate::seat_authority::possession_token(&grant)
}

/// A registry row's mail-wake state. Only `armed` rows are watched; a
/// retired row keeps its ledger on disk.
// trace:STORY-HOME-096 | ai:claude
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MailWake {
    /// Requested at launch; the session has no thread to bind yet.
    Pending,
    /// The watcher wakes this session's thread for its own mail.
    Armed,
    /// Arming was attempted and failed.
    Failed,
    /// The operator opted out.
    Off,
    /// The session ended; the watcher stopped waking it.
    Retired,
    /// The vendor (or this Codex version) has no live wake primitive; mail
    /// arrives on its next turn.
    Unsupported,
}

impl MailWake {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Armed => "armed",
            Self::Failed => "failed",
            Self::Off => "off",
            Self::Retired => "retired",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Set the mail-wake state of the live session `target` (name or
/// `<type>#<pid>`). Returns the updated row.
// trace:STORY-HOME-096 | ai:claude
pub(crate) fn set_mail_wake(
    project_root: &Path,
    target: &str,
    state: MailWake,
) -> Result<AgentRegistryEntry> {
    let (_, mut entry) = resolve_target_entry(project_root, target)?;
    entry.mail_wake = Some(state);
    write_entry(project_root, &entry)?;
    Ok(entry)
}

/// Set `entry`'s mail-wake state on its row as it is on disk now; returns
/// that row. The read and the write share one namespace-lock hold, so a
/// hook write that landed since `entry` was read is kept and none can land
/// in between. Refuses a row that another process (a recycled pid) has
/// taken over. The launcher calls this for a child already running, so it
/// waits out launch-time contention.
// trace:STORY-HOME-096 | ai:claude
pub(crate) fn set_mail_wake_on_row(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    state: MailWake,
) -> Result<AgentRegistryEntry> {
    update_mail_wake_on_row(
        project_root,
        entry,
        crate::agent_handle::LAUNCHER_LOCK_BUDGET,
        // The child's own hook may already have armed this launch from the
        // launcher's decision; `pending` would only re-arm it.
        // trace:STORY-2256 | ai:claude
        |now| (!(state == MailWake::Pending && now == Some(MailWake::Armed))).then_some(state),
    )
}

/// Record `state` on `entry`'s row only when nothing has decided its mail
/// wake yet; the SessionStart hook's default for a hand-started session
/// never replaces a launcher's stamp. Returns the row as it is now.
// trace:STORY-2256 | ai:claude
pub(crate) fn adopt_mail_wake_on_row(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    state: MailWake,
) -> Result<AgentRegistryEntry> {
    update_mail_wake_on_row(project_root, entry, ARM_LOCK_BUDGET, |now| {
        now.is_none().then_some(state)
    })
}

/// How long arming waits for the namespace lock: it runs in the launcher
/// and in the SessionStart hook, which must not stall behind a wedged lock.
const ARM_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// Move `entry`'s row from any of `from` to `to`, under the same lock hold
/// as the read. A row in another state (retired, armed by the other side of
/// the launch, opted out) is left as it is.
// trace:STORY-HOME-096 | ai:claude
pub(crate) fn transition_mail_wake_on_row(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    from: &[MailWake],
    to: MailWake,
) -> Result<AgentRegistryEntry> {
    update_mail_wake_on_row(project_root, entry, ARM_LOCK_BUDGET, |now| {
        now.is_some_and(|now| from.contains(&now)).then_some(to)
    })
}

/// Read-decide-write of one row's mail wake under the namespace lock;
/// `decide` gets the row's current state and returns `None` to leave it.
fn update_mail_wake_on_row(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    budget: std::time::Duration,
    decide: impl FnOnce(Option<MailWake>) -> Option<MailWake>,
) -> Result<AgentRegistryEntry> {
    let _lock = crate::agent_handle::lock_namespace_within(project_root, budget)?;
    let path = registry_path(project_root, &entry.id);
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut row: AgentRegistryEntry =
        toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    if !same_registry_process(&row, entry) {
        anyhow::bail!("the registry row now belongs to another process");
    }
    if let Some(state) = decide(row.mail_wake) {
        row.mail_wake = Some(state);
        write_entry_locked(project_root, &row)?;
    }
    Ok(row)
}

/// The SessionStart hook's row (`entry`) with a launcher write that landed
/// between the hook's read (`read`) and its write folded in; `disk` is the
/// row as it is now, read under the lock the hook's write holds. The
/// launcher owns the mail wake and the launch identity; the hook keeps the
/// session id, handle reservation and terminal it observed.
// trace:STORY-HOME-096 | ai:claude
pub(crate) fn carry_concurrent_launcher_write(
    entry: &mut AgentRegistryEntry,
    read: Option<&AgentRegistryEntry>,
    disk: Option<&AgentRegistryEntry>,
) {
    let Some(disk) = disk.filter(|d| {
        same_registry_process(d, entry)
            || entry.process_identity().is_some_and(|id| {
                registration_matches(d, &id) && proc_identity::observe(&id, true) == Verdict::Alive
            })
    }) else {
        return;
    };
    // Only the launcher stamps the wake and only arming moves it on; the
    // hook's row never decides it.
    entry.mail_wake = disk.mail_wake;
    entry.delegate_seats = disk.delegate_seats.clone();
    if read == Some(disk) || disk.source != "agent-launcher" {
        return;
    }
    entry.source = disk.source.clone();
    if disk.name.is_some() {
        entry.name = disk.name.clone();
    }
    entry.role = disk.role.clone().or(entry.role.take());
    // trace:STORY-2745 | ai:claude
    entry.seat = disk.seat.clone().or(entry.seat.take());
    entry.current_spec = disk.current_spec.clone().or(entry.current_spec.take());
    entry.native_session_id = disk
        .native_session_id
        .clone()
        .or(entry.native_session_id.take());
    entry.description = disk.description.clone().or(entry.description.take());
    entry.resumed_from = disk.resumed_from.clone().or(entry.resumed_from.take());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AgentStatus {
    Busy,
    Idle,
    Stale,
}

impl AgentStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Idle => "idle",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentRegistryView {
    pub(crate) id: String,
    pub(crate) agent_type: String,
    pub(crate) pid: u32,
    /// Kernel start time of `pid`, so a recycled pid does not read as the
    /// live agent or receive a signal (BUG-2148).
    // trace:BUG-2148 | ai:antigravity
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid_start: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) tty: Option<String>,
    pub(crate) terminal: Option<TerminalIdentity>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_active_at: DateTime<Utc>,
    pub(crate) role: Option<String>,
    pub(crate) current_spec: Option<String>,
    pub(crate) worktree_path: PathBuf,
    pub(crate) source: String,
    pub(crate) binary_version: Option<String>,
    pub(crate) build_sha: Option<String>,
    pub(crate) status: AgentStatus,
    // trace:STORY-528 | ai:claude
    pub(crate) availability: Availability,
    pub(crate) paused_since: Option<DateTime<Utc>>,
    pub(crate) paused_reason: Option<PauseReason>,
    pub(crate) expected_back: Option<DateTime<Utc>>,
    // trace:STORY-790 | ai:codex
    pub(crate) native_session_id: Option<String>,
    pub(crate) ended_at: Option<DateTime<Utc>>,
    pub(crate) resumed_from: Option<String>,
    // trace:BUG-2212 | ai:claude
    #[serde(default, skip_serializing_if = "LivenessBasis::is_process")]
    pub(crate) liveness: LivenessBasis,
    // trace:STORY-HOME-096 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mail_wake: Option<MailWake>,
}

// trace:STORY-994 | ai:codex
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TerminalIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminator_uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tmux_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tmux_socket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wezterm_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) windowid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) kitty_window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) emulator: Option<String>,
}

impl TerminalIdentity {
    pub(crate) fn short_cell(&self) -> String {
        let tty = self
            .tty
            .as_deref()
            .map(short_tty)
            .unwrap_or_else(|| "-".to_string());
        match self.emulator.as_deref() {
            Some(emulator) if !emulator.is_empty() => format!("{tty} · {emulator}"),
            _ => tty,
        }
    }
}

// trace:STORY-994 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalTitleRestore {
    pub(crate) terminal: Option<TerminalIdentity>,
    pub(crate) previous_title: Option<String>,
}

// trace:TASK-1184 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentGcRow {
    pub(crate) id: String,
    pub(crate) agent_type: String,
    pub(crate) pid: u32,
    pub(crate) source: String,
    pub(crate) path: PathBuf,
    pub(crate) resumable_session_id: Option<String>,
    pub(crate) last_active_at: DateTime<Utc>,
    pub(crate) outcome: &'static str,
}

// trace:TASK-1184 | ai:codex
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct AgentGcReport {
    pub(crate) removed: Vec<AgentGcRow>,
    pub(crate) retained: Vec<AgentGcRow>,
    // Retained rows whose dead process gc newly stamped ended.
    // trace:STORY-1843 | ai:claude
    pub(crate) marked_ended: usize,
}

impl AgentGcReport {
    pub(crate) fn removed_count(&self) -> usize {
        self.removed.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentBinaryIdentity {
    pub(crate) version: String,
    pub(crate) sha: String,
}

impl AgentBinaryIdentity {
    pub fn new(version: String, sha: String) -> Self {
        Self { version, sha }
    }
}

/// Inputs the status classifier needs that aren't on the registry entry
/// itself. Built once per `list_agent_views` call so every entry is
/// classified against the same wall-clock and the same lease snapshot.
/// trace:STORY-435 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct AgentClassifyContext {
    pub(crate) now: DateTime<Utc>,
    pub(crate) threshold_secs: u64,
    pub(crate) live_lease_worktrees: Vec<PathBuf>,
    pub(crate) proc_tree: Option<ProcTree>,
    pub(crate) work_grace_secs: u64,
    // trace:BUG-2212 | ai:claude
    pub(crate) observer: PidObserver,
    /// How long a heartbeat-only row (process unobservable) counts as live.
    /// `None` — the guard default — means no bound: a row this observer
    /// cannot verify stays live however old its heartbeat, so guards fail
    /// CLOSED (session-start-hook rows heartbeat only once, at SessionStart).
    /// Display surfaces opt into a finite listing window with
    /// [`Self::for_display`].
    // trace:TASK-2272 | ai:claude
    pub(crate) heartbeat_ttl_secs: Option<i64>,
}

impl AgentClassifyContext {
    pub(crate) fn new(
        now: DateTime<Utc>,
        threshold_secs: u64,
        live_lease_worktrees: Vec<PathBuf>,
    ) -> Self {
        Self {
            now,
            threshold_secs,
            live_lease_worktrees,
            proc_tree: None,
            work_grace_secs: 10,
            observer: PidObserver::current(),
            heartbeat_ttl_secs: None,
        }
    }

    /// A listing a human reads: a heartbeat-only row ages out of the listing
    /// after `UNOBSERVED_HEARTBEAT_TTL_SECS`. Guards (launch dedupe, nudge
    /// routing, reaping) keep the unbounded default: a row they cannot verify
    /// counts as live, so a duplicate launch is refused rather than allowed
    /// over a possibly-live host session.
    // trace:TASK-2272 | ai:claude
    pub(crate) fn for_display(mut self) -> Self {
        self.heartbeat_ttl_secs = Some(UNOBSERVED_HEARTBEAT_TTL_SECS);
        self
    }

    // trace:BUG-2212 | ai:claude
    pub(crate) fn with_observer(mut self, observer: PidObserver) -> Self {
        self.observer = observer;
        self
    }

    // trace:BUG-1704 | ai:codex
    pub(crate) fn with_work_probe(mut self, tree: ProcTree, grace_secs: u64) -> Self {
        self.proc_tree = Some(tree);
        self.work_grace_secs = grace_secs;
        self
    }
}

/// `[agent_registry]` section in `.aida/config.toml`. Sensible default
/// (30s) means a project that never writes the section gets reasonable
/// busy/idle behaviour for free; missing file / section / keys all fall
/// through to defaults — a config error never blocks `aida status`.
/// trace:STORY-435 | ai:claude
///
/// TRUST DOCTRINE for `.aida/config.toml` (BUG-1723 decision, 2026-10-02):
/// the file is worktree-resident and committable, so any branch can ship its
/// own copy. A value read from the branch-local copy may therefore only tune
/// POLICY — timeouts, thresholds, role lists, display preferences. It may
/// NOT select an executable, a shell command, or a credential: those must
/// come from the running binary, from outside the worktree, or from the
/// trusted default-branch copy via `crate::trusted_config` (TASK-969).
/// This is the same doctrine BUG-1624 established for `.aida/session-env.sh`.
/// Every section/key read from the branch-local copy is enumerated and
/// classified in `scripts/config-trust.toml`, enforced by the guard in
/// `tests/bug_1723_config_trust_tests.rs` — a new key fails that guard until
/// it is classified there.
// trace:BUG-1723 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) busy_threshold_secs: u64,
    pub(crate) work_grace_secs: u64,
    pub(crate) singleton_roles: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            busy_threshold_secs: 30,
            work_grace_secs: 10,
            singleton_roles: vec![
                "advisor".to_string(),
                "product".to_string(),
                "integrator".to_string(),
                "implementer".to_string(),
                "reviewer".to_string(),
            ],
        }
    }
}

impl Config {
    pub(crate) fn load(project_root: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        else {
            return Self::default();
        };
        Self::from_toml_str(&content)
    }

    pub(crate) fn from_toml_str(content: &str) -> Self {
        let mut cfg = Self::default();
        for (key, val) in scan_agent_registry_section(content) {
            if key == "busy_threshold_secs" {
                if let Ok(n) = val.parse::<u64>() {
                    cfg.busy_threshold_secs = n;
                }
            } else if key == "singleton_roles" {
                cfg.singleton_roles = parse_csv_words(&val);
            } else if key == "work_grace_secs" {
                if let Ok(n) = val.parse::<u64>() {
                    cfg.work_grace_secs = n;
                }
            }
        }
        cfg
    }

    pub(crate) fn role_is_singleton(&self, role: &str) -> bool {
        self.singleton_roles
            .iter()
            .any(|r| r.eq_ignore_ascii_case(role))
    }
}

fn parse_csv_words(raw: &str) -> Vec<String> {
    raw.trim_matches(['[', ']'])
        .split(',')
        .map(|part| {
            part.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_ascii_lowercase()
        })
        .filter(|part| !part.is_empty())
        .collect()
}

/// Hand-rolled `[agent_registry]` scanner — mirrors `OrchestratorConfig`
/// so we don't pull a serde-toml dependency for one scalar.
fn scan_agent_registry_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "agent_registry";
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                pairs.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    pairs
}

fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

pub(crate) fn agents_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("agents")
}

pub(crate) fn registry_path(project_root: &Path, id: &str) -> PathBuf {
    agents_dir(project_root).join(format!("{id}.toml"))
}

/// BUG-416: count the LIVE agents registered as operating within `cwd` — i.e.
/// whose `worktree_path` equals or is an ancestor of `cwd`. Used to detect a
/// SHARED worktree: when two `aida agent new` sessions land in the same
/// worktree, the `aida add` "this session owns scope X" hint cannot be
/// confidently attributed to the current process, because the lease
/// `active_lease_for_cwd` resolves may belong to a peer agent. A human session
/// (not in the registry) yields 0; a lone agent yields 1; co-located agents
/// yield ≥2 — the caller suppresses the hint only at ≥2. Dead PIDs are skipped
/// so an exited agent's stale record doesn't keep a worktree "shared".
/// trace:BUG-416 | ai:claude
pub(crate) fn live_agents_covering_cwd(project_root: &Path, cwd: &Path) -> usize {
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut count = 0usize;
    if let Ok(entries) = std::fs::read_dir(agents_dir(project_root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(record) = toml::from_str::<AgentRegistryEntry>(&body) else {
                continue;
            };
            if !registry_entry_is_live(project_root, &record) {
                continue;
            }
            let wt = record
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| record.worktree_path.clone());
            if !wt.as_os_str().is_empty() && (canon_cwd == wt || canon_cwd.starts_with(&wt)) {
                count += 1;
            }
        }
    }
    count
}

fn agent_id(agent_type: &str, pid: u32) -> String {
    format!("{agent_type}-{pid}")
}

// trace:STORY-791 | ai:codex
fn find_launcher_ancestor_entry(
    project_root: &Path,
    child_pid: u32,
) -> Option<(PathBuf, AgentRegistryEntry)> {
    load_entries(project_root).into_iter().find(|(_, entry)| {
        entry.source == "agent-launcher"
            && entry_process_verdict(entry) == Verdict::Alive
            && registry_entry_is_live(project_root, entry)
            && pid_has_ancestor(child_pid, entry.pid)
    })
}

pub fn touch_mcp_agent(
    project_root: &Path,
    binary: &AgentBinaryIdentity,
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let pid = std::process::id();
    let agent_type = detect_agent_type();
    let id = agent_id(&agent_type, pid);
    let existing_launcher = find_launcher_ancestor_entry(project_root, pid);
    let path = existing_launcher
        .as_ref()
        .map(|(path, _)| path.clone())
        .unwrap_or_else(|| registry_path(project_root, &id));

    // A row a recycled pid left belongs to a dead process: retire it, never
    // inherit its identity. trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    let current_identity = ProcIdentity::capture(pid)
        .map_err(anyhow::Error::msg)
        .context("cannot verify local process coordinates; retry registration on the host")?;
    let own_row = match existing_launcher {
        Some((_, entry)) => Some(entry),
        None => match std::fs::read_to_string(&path)
            .ok()
            .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
        {
            Some(old) if !registration_matches(&old, &current_identity) => {
                ensure_local_replacement(&old, Some(&current_identity))?;
                retire_displaced_entry(project_root, &old)?;
                None
            }
            other => other,
        },
    };
    let mut entry = own_row.unwrap_or_else(|| AgentRegistryEntry {
        id: id.clone(),
        agent_type: agent_type.clone(),
        pid,
        pid_start: Some(current_identity.legacy().start),
        pid_namespace: Some(current_identity.pidns_inode),
        name: None,
        description: None,
        tty: current_tty(),
        terminal: current_terminal_identity(),
        started_at: now,
        last_active_at: now,
        role: None,
        seat: None,
        current_spec: None,
        worktree_path: project_root.to_path_buf(),
        source: "mcp".to_string(),
        binary_version: None,
        build_sha: None,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        grant_key: None,
        delegate_seats: Vec::new(),
        resumed_from: None,
    });

    if entry.source != "agent-launcher" {
        entry.id = id;
        entry.agent_type = agent_type;
        if entry.pid != pid || entry.process_identity().is_none() {
            entry.pid_start = Some(current_identity.legacy().start);
            entry.pid_namespace = Some(current_identity.pidns_inode);
        }
        entry.pid = pid;
    }
    if entry.pid == pid && registration_matches(&entry, &current_identity) {
        entry.pid_start = Some(current_identity.legacy().start);
        entry.pid_namespace = Some(current_identity.pidns_inode);
    }
    entry.last_active_at = now;
    entry.role = env_nonempty("AIDA_SESSION_ROLE").or(entry.role);
    entry.current_spec = env_nonempty("AIDA_SESSION_SCOPE")
        .filter(|s| looks_like_spec_id(s))
        .or(entry.current_spec);
    if entry.source != "agent-launcher" {
        entry.worktree_path = project_root.to_path_buf();
        entry.source = "mcp".to_string();
    }
    entry.binary_version = Some(binary.version.clone());
    entry.build_sha = Some(binary.sha.clone());
    if entry.tty.is_none() {
        entry.tty = current_tty();
    }
    if entry.terminal.is_none() {
        entry.terminal = current_terminal_identity();
    }

    write_entry(project_root, &entry)?;
    Ok(entry)
}

/// The lease an `aida agent new --bg` launch linked to this Claude session
/// (its manifest records the session id, possibly as a short prefix), with
/// the name the launch gave it (TASK-2102).
// trace:TASK-1955 | ai:claude (BUG-2167, TASK-2102)
fn lease_linked_to_session(project_root: &Path, session_id: &str) -> Option<LinkedLaunch> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }
    let linked: Vec<(String, Option<String>, Option<String>)> =
        crate::session_manifest::list_all(project_root)
            .into_iter()
            .filter(|m| {
                m.claude_session_id
                    .as_deref()
                    .is_some_and(|sid| !sid.is_empty() && session_id.starts_with(sid))
            })
            .map(|m| (m.session_id, m.agent_name, m.seat))
            .collect();
    crate::leases::list_leases(project_root)
        .into_iter()
        .find_map(|l| {
            let (_, name, seat) = linked.iter().find(|(id, _, _)| *id == l.id)?.clone();
            Some(LinkedLaunch {
                lease: l,
                name,
                seat,
            })
        })
}

/// The lease a launcher linked to a session, with the launch's name and
/// named seat as its manifest recorded them.
// trace:STORY-2745 | ai:claude
struct LinkedLaunch {
    lease: SessionLease,
    name: Option<String>,
    seat: Option<String>,
}

/// Record the session a SessionStart hook fires in. `worktree_root` is the
/// hook's own worktree (not the main checkout `project_root`).
///
/// BUG-2167: a `claude --bg` session runs in a spare process the Claude
/// supervisor pre-forked, so its environment can be an EARLIER launch's
/// (`AIDA_AGENT_NAME`, `AIDA_SESSION_SCOPE`), and its pid a recycled one.
/// The lease the launcher linked to this session id is the substrate truth
/// for spec, role and worktree; when it contradicts the env, the env's name
/// is not trusted either. A pid entry left by an earlier session never
/// carries that session's identity over.
// trace:STORY-994 | ai:codex
// trace:TASK-1955 | ai:claude (BUG-2167)
pub(crate) fn touch_session_start_agent(
    project_root: &Path,
    worktree_root: &Path,
    session_id: Option<&str>,
    binary: &AgentBinaryIdentity,
) -> Result<AgentRegistryEntry> {
    let hook_pid = std::process::id();
    let pid = session_start_agent_pid(hook_pid, &detect_agent_type()).unwrap_or(hook_pid);
    touch_session_start_agent_for(project_root, worktree_root, session_id, binary, pid)
}

/// [`touch_session_start_agent`] for the agent process `pid`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
fn touch_session_start_agent_for(
    project_root: &Path,
    worktree_root: &Path,
    session_id: Option<&str>,
    binary: &AgentBinaryIdentity,
    pid: u32,
) -> Result<AgentRegistryEntry> {
    touch_session_start_agent_before_write(
        project_root,
        worktree_root,
        session_id,
        binary,
        pid,
        || {},
    )
}

// Keep the read/write boundary explicit so regressions can interleave a real
// background adoption without timing or process-global test hooks.
// trace:BUG-2590 | ai:codex
fn touch_session_start_agent_before_write(
    project_root: &Path,
    worktree_root: &Path,
    session_id: Option<&str>,
    binary: &AgentBinaryIdentity,
    pid: u32,
    before_write: impl FnOnce(),
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let agent_type = detect_agent_type();
    let id = agent_id(&agent_type, pid);
    let path = registry_path(project_root, &id);
    let session_id = session_id.filter(|s| !s.trim().is_empty());
    let current_identity = ProcIdentity::capture(pid)
        .map_err(anyhow::Error::msg)
        .context("cannot verify local process coordinates; retry registration on the host")?;
    let linked = session_id.and_then(|sid| lease_linked_to_session(project_root, sid));
    let (linked, launch_name, launch_seat) = match linked {
        Some(LinkedLaunch { lease, name, seat }) => (Some(lease), name, seat),
        None => (None, None, None),
    };
    let env_scope = env_nonempty("AIDA_SESSION_SCOPE").filter(|s| looks_like_spec_id(s));
    let env_trusted = !matches!(
        &linked,
        Some(lease) if env_scope.as_deref() != Some(lease.scope.as_str())
    );
    let env_name = if env_trusted {
        // A headless child's own handle, else its launch name.
        // trace:STORY-1843 | ai:claude
        env_nonempty(crate::agent_handle::HANDLE_ENV).or_else(|| env_nonempty("AIDA_AGENT_NAME"))
    } else {
        None
    };
    // The named seat. A linked session's seat is its launch's own record,
    // and a launch that named none holds none: the spare it runs in may
    // carry an earlier launch's env, and a hook that fired before the link
    // may have written that env's seat to the row. An unlinked session
    // reads its own env. trace:STORY-2745 | ai:claude
    let session_seat = if linked.is_some() {
        launch_seat
    } else {
        env_nonempty(crate::agent_rotation::SEAT_ENV)
    }
    .and_then(|s| crate::seat_rotation::sanitize_seat(&s).ok());
    let read_row = std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok());
    // A recycled pid's entry belongs to another process: keep it only
    // while its recorded start identity is this process's. A live
    // process that starts a new session (`/clear`, a new conversation)
    // keeps its launcher entry. Matching legacy ticks upgrade in place only
    // with this positive local capture; other incomplete rows cannot contribute
    // session/thread fields to a fresh registration. trace:TASK-3366 | ai:codex
    let same_process = |old: &AgentRegistryEntry| registration_matches(&old, &current_identity);
    let displaced = read_row.as_ref().filter(|old| !same_process(old)).cloned();
    if let Some(old) = &displaced {
        ensure_local_replacement(old, Some(&current_identity))?;
    }
    let mut entry = read_row
        .clone()
        .filter(same_process)
        .unwrap_or_else(|| AgentRegistryEntry {
            id: id.clone(),
            agent_type: agent_type.clone(),
            pid,
            pid_start: Some(current_identity.legacy().start),
            pid_namespace: Some(current_identity.pidns_inode),
            name: env_name.clone(),
            description: None,
            tty: process_tty(pid).or_else(current_tty),
            terminal: None,
            started_at: now,
            last_active_at: now,
            role: None,
            seat: None,
            current_spec: None,
            worktree_path: worktree_root.to_path_buf(),
            source: "session-start-hook".to_string(),
            binary_version: None,
            build_sha: None,
            availability: Availability::Available,
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            claude_session_id: None,
            ended_at: None,
            spec_status_at_end: None,
            mail_wake: None,
            grant_key: None,
            delegate_seats: Vec::new(),
            resumed_from: None,
        });

    entry.id = id;
    entry.agent_type = agent_type;
    if entry.pid != pid || entry.process_identity().is_none() {
        entry.pid_start = Some(current_identity.legacy().start);
        entry.pid_namespace = Some(current_identity.pidns_inode);
    }
    entry.pid = pid;
    entry.last_active_at = now;
    // A hook runs inside this session, so it is live whatever an earlier
    // gc concluded. trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    entry.ended_at = None;
    entry.spec_status_at_end = None;
    if let Some(lease) = &linked {
        // The launch's own name wins over the spare's env and over a name a
        // same-start entry already carried; a hook firing after the link has
        // no later adopt step to correct it. trace:TASK-1955 | ai:claude (TASK-2102)
        if let Some(name) = launch_name.clone().or_else(|| env_name.clone()) {
            entry.name = Some(name);
        }
        entry.role = lease.role.clone().or(entry.role);
        entry.current_spec = Some(lease.scope.clone());
        entry.worktree_path = lease.worktree_path.clone();
    } else {
        entry.role = env_nonempty("AIDA_SESSION_ROLE").or(entry.role);
        entry.current_spec = env_scope.or(entry.current_spec);
        entry.worktree_path = worktree_root.to_path_buf();
    }
    // The linked launch decides the seat outright, as it does the spec; an
    // unlinked session keeps a seat its launcher already wrote to the row.
    // trace:STORY-2745 | ai:claude
    entry.seat = if linked.is_some() {
        session_seat
    } else {
        session_seat.or(entry.seat)
    };
    if entry.source != "agent-launcher" {
        entry.source = "session-start-hook".to_string();
    }
    entry.binary_version = Some(binary.version.clone());
    entry.build_sha = Some(binary.sha.clone());
    if entry.tty.is_none() {
        entry.tty = process_tty(pid).or_else(current_tty);
    }
    entry.terminal = terminal_identity_for_tty(entry.tty.clone()).or(entry.terminal);
    if let Some(session_id) = session_id.filter(|s| !s.trim().is_empty()) {
        entry.claude_session_id = Some(session_id.to_string());
    }
    // Every session gets one stable handle at start; the launch's name when
    // it gave one. trace:STORY-1843 | ai:claude
    crate::agent_handle::assign_session_handle(project_root, &mut entry, session_id);

    // The launcher may have registered or stamped this row since it was
    // read: fold that in under the same lock hold as the write.
    // trace:STORY-HOME-096 | ai:claude
    before_write();
    let _lock = crate::agent_handle::lock_namespace(project_root)?;
    let disk = std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok());
    carry_concurrent_launcher_write(&mut entry, read_row.as_ref(), disk.as_ref());
    // A background link/adoption may have landed after the initial lookup,
    // even on a hook-owned row. Re-read at the write boundary and apply after
    // the launcher merge: a seatless launch's None must clear any stale seat.
    // Later adoptions acquire this same lock when publishing their row.
    // trace:BUG-2590 | ai:codex
    if let Some(launch) = session_id.and_then(|sid| lease_linked_to_session(project_root, sid)) {
        entry.seat = launch
            .seat
            .and_then(|seat| crate::seat_rotation::sanitize_seat(&seat).ok());
    }
    if let Some(old) = &displaced {
        ensure_local_replacement(old, Some(&current_identity))?;
        retire_displaced_entry(project_root, old)?;
    }
    write_entry_locked(project_root, &entry)?;
    Ok(entry)
}

/// What [`heartbeat_session_agent`] did.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Heartbeat {
    /// The session's own row was there; its `last_active_at` is now.
    Refreshed,
    /// The session had no row of its own (a missed SessionStart write, a
    /// row removed or replaced since): it is registered again, as this row.
    Reregistered(Box<AgentRegistryEntry>),
    /// No agent client is an ancestor of this hook, so there is no seat
    /// process to record.
    NoAgentProcess,
}

/// The per-prompt registry heartbeat: refresh this session's row, or
/// register the session again when its row is missing, ended, or belongs to
/// another process or session. SessionStart is the only other writer, and
/// it fails open on a contended lock, so without this a seat that missed
/// that one write stays unregistered (reported "no advisor") for its whole
/// life, and every row's heartbeat stays at its start.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn heartbeat_session_agent(
    project_root: &Path,
    worktree_root: &Path,
    session_id: Option<&str>,
    binary: &AgentBinaryIdentity,
) -> Result<Heartbeat> {
    let Some(pid) = session_start_agent_pid(std::process::id(), &detect_agent_type()) else {
        return Ok(Heartbeat::NoAgentProcess);
    };
    let beat = heartbeat_session_agent_for(project_root, worktree_root, session_id, binary, pid)?;
    let id = agent_id(&detect_agent_type(), pid);
    record_grant_key(project_root, &id, held_grant_key().as_deref());
    Ok(beat)
}

/// Record on live registry row `id` the `key:` token its session proved it
/// holds (see [`AgentRegistryEntry::grant_key`]), replacing a stale one.
/// `None` clears it, so a session that dropped its grant stops vouching.
/// Fail-open: a hook never blocks on it.
// trace:STORY-2338 | ai:claude
pub(crate) fn record_grant_key(project_root: &Path, id: &str, key: Option<&str>) {
    let path = registry_path(project_root, id);
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
    };
    let key = key.filter(|k| k.starts_with("key:")).map(str::to_string);
    if read(&path).is_none_or(|row| row.grant_key == key) {
        return;
    }
    let Ok(_lock) = crate::agent_handle::lock_namespace(project_root) else {
        return;
    };
    if let Some(mut row) = read(&path).filter(|row| row.ended_at.is_none()) {
        row.grant_key = key;
        let _ = write_entry_locked(project_root, &row);
    }
}

/// [`heartbeat_session_agent`] for the agent process `pid`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn heartbeat_session_agent_for(
    project_root: &Path,
    worktree_root: &Path,
    session_id: Option<&str>,
    binary: &AgentBinaryIdentity,
    pid: u32,
) -> Result<Heartbeat> {
    let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
    let path = registry_path(project_root, &agent_id(&detect_agent_type(), pid));
    let current_identity = ProcIdentity::capture(pid)
        .map_err(anyhow::Error::msg)
        .context("cannot verify local process coordinates; retry registration on the host")?;
    let own = |row: &AgentRegistryEntry| {
        row.ended_at.is_none()
            && row.pid == pid
            && registration_matches(row, &current_identity)
            && match (session_id, row.claude_session_id.as_deref()) {
                (Some(sid), Some(recorded)) => sid == recorded,
                (Some(_), None) => false,
                (None, _) => true,
            }
    };
    let read = |path: &Path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
    };
    if read(&path).is_some_and(|row| own(&row)) {
        let lock = crate::agent_handle::lock_namespace(project_root)?;
        // Re-read under the lock so a concurrent launcher or hook write is
        // refreshed, not clobbered.
        if let Some(mut row) = read(&path).filter(|row| own(row)) {
            row.last_active_at = Utc::now();
            row.pid_start = Some(current_identity.legacy().start);
            row.pid_namespace = Some(current_identity.pidns_inode);
            write_entry_locked(project_root, &row)?;
            return Ok(Heartbeat::Refreshed);
        }
        drop(lock);
    }
    let entry =
        touch_session_start_agent_for(project_root, worktree_root, session_id, binary, pid)?;
    Ok(Heartbeat::Reregistered(Box::new(entry)))
}

/// Keep the row an earlier, now-dead process left at a recycled pid's path
/// before a new process's row replaces it: it moves to `retired/` under a
/// name and id unique to that process's start, ended at its last heartbeat.
/// A row name is per pid, so without this a reused pid erases the earlier
/// session's history.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
fn retire_displaced_entry(project_root: &Path, old: &AgentRegistryEntry) -> Result<()> {
    let mut row = old.clone();
    row.id = format!("{}-{}", old.id, old.started_at.format("%Y%m%dT%H%M%S%.3fZ"));
    if row.ended_at.is_none() {
        row.ended_at = Some(old.last_active_at);
        crate::mail_wake::retire(&mut row);
    }
    let dir = agents_dir(project_root).join("retired");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating agent registry dir {}", dir.display()))?;
    let path = dir.join(format!("{}.toml", row.id));
    let body = registry_body(&row)?;
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing agent registry entry {}", path.display()))
}

/// BUG-2167: after `aida agent new --bg` links its lease to the Claude
/// session id `sid` (a full id or its short prefix), give any registry entry
/// that session's SessionStart hook already wrote this launch's identity —
/// the spare process it runs in may carry an earlier launch's environment.
/// Returns how many entries were updated.
// trace:TASK-1955 | ai:claude (BUG-2167)
pub(crate) fn adopt_bg_session_identity(
    project_root: &Path,
    sid: &str,
    name: &str,
    spec: Option<&str>,
    role: Option<&str>,
    seat: Option<&str>,
    worktree: &Path,
) -> usize {
    let sid = sid.trim();
    if sid.is_empty() {
        return 0;
    }
    let mut updated = 0;
    for (_, mut entry) in load_entries(project_root) {
        if !entry
            .claude_session_id
            .as_deref()
            .is_some_and(|own| own.starts_with(sid))
        {
            continue;
        }
        // The launch reserved `name`; bind it to this session so the
        // generated handle its SessionStart took is released. This runs in
        // the launcher, which holds the reservation's token.
        // trace:STORY-1843 | ai:claude
        let token = crate::agent_handle::launch_token_for(project_root, name);
        let reserved = crate::agent_handle::reserve_handle(
            project_root,
            &crate::agent_handle::HandleRequest {
                agent_type: &entry.agent_type,
                role,
                explicit: Some(name),
                native_session_id: entry.claude_session_id.as_deref(),
                registry_id: Some(&entry.id),
                launch_token: token.as_deref(),
                launcher: false,
                ended_relay_cleared: None,
            },
        );
        // Never write a name the namespace refused: the session keeps the
        // handle its SessionStart took, so two sessions never share one.
        match reserved {
            Ok(_) => entry.name = Some(name.to_string()),
            Err(err) => eprintln!(
                "  warning: the launched session keeps its handle `{}`: {err:#}",
                entry.name.as_deref().unwrap_or("-")
            ),
        }
        entry.current_spec = spec.map(str::to_string);
        entry.role = role.map(str::to_string).or(entry.role);
        // The launch's seat replaces whatever the pre-link hook wrote from
        // the spare's env; a launch with no seat leaves none.
        // trace:STORY-2745 | ai:claude
        entry.seat = seat.and_then(|s| crate::seat_rotation::sanitize_seat(s).ok());
        entry.worktree_path = worktree.to_path_buf();
        if write_entry(project_root, &entry).is_ok() {
            updated += 1;
        }
    }
    updated
}

/// Register an agent process spawned by `aida agent new ...`.
///
/// Multiple concurrent sessions for the same project are intentionally
/// supported: registry IDs are `<agent-type>-<pid>`, so each process gets a
/// distinct entry even when cwd/spec match.
// trace:STORY-432 | ai:codex
// why: command-dispatch fn whose params mirror distinct CLI flags; bundling into a struct adds indirection without clarifying the call sites.
#[allow(clippy::too_many_arguments)]
pub(crate) fn register_spawned_agent(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    role: Option<String>,
    current_spec: Option<String>,
    worktree_path: PathBuf,
    binary: Option<&AgentBinaryIdentity>,
    name: Option<String>,
    native_session_id: Option<String>,
    resumed_from: Option<String>,
    description: Option<String>,
) -> Result<AgentRegistryEntry> {
    register_spawned_agent_with_row(
        project_root,
        agent_type,
        pid,
        SpawnedRow {
            role,
            current_spec,
            worktree_path,
            binary,
            name,
            native_session_id,
            resumed_from,
            description,
            delegate_seats: Vec::new(),
            seat: None,
        },
    )
}

// trace:BUG-2467 | ai:codex
pub(crate) fn register_spawned_agent_with_row(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    row: SpawnedRow<'_>,
) -> Result<AgentRegistryEntry> {
    let captured = ProcIdentity::capture(pid).ok();
    register_spawned_agent_as(
        project_root,
        agent_type,
        pid,
        captured.as_ref().map(|id| id.legacy().start),
        captured.as_ref().map(|id| id.pidns_inode),
        row,
    )
}

/// The descriptive fields of a launcher-registered row.
// trace:STORY-2254 | ai:claude
pub(crate) struct SpawnedRow<'a> {
    pub role: Option<String>,
    pub current_spec: Option<String>,
    pub worktree_path: PathBuf,
    pub binary: Option<&'a AgentBinaryIdentity>,
    pub name: Option<String>,
    pub native_session_id: Option<String>,
    pub resumed_from: Option<String>,
    pub description: Option<String>,
    pub delegate_seats: Vec<String>,
    /// The launch's named seat (`--seat`, or the rotation seat).
    // trace:STORY-2745 | ai:claude
    pub seat: Option<String>,
}

/// STORY-2254: register a detached worker strictly. Its start identity is
/// REQUIRED and must equal `expected_start` (read again now, so a reused PID
/// is refused), and the row records the worker's own PID namespace rather
/// than the registering supervisor's. A SessionStart hook row merges only
/// for that same pid/start; any other row at the id is displaced as today.
/// The persisted row is re-read and verified.
// trace:STORY-2254 | ai:claude
pub(crate) fn register_detached_worker(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    expected_start: &str,
    row: SpawnedRow<'_>,
) -> Result<AgentRegistryEntry> {
    let captured = ProcIdentity::capture(pid).map_err(anyhow::Error::msg)?;
    // The launch receipt still transports raw ticks. Compare only after a
    // verified capture, and persist the boot-qualified registry format.
    let start = captured.legacy_ticks_start();
    anyhow::ensure!(
        !expected_start.is_empty() && start == expected_start,
        "detached worker {pid} is no longer the spawned process (start identity changed)"
    );
    let namespace = captured.pidns_inode;
    let entry = register_spawned_agent_as(
        project_root,
        agent_type,
        pid,
        Some(captured.legacy().start),
        Some(namespace),
        row,
    )?;
    let path = registry_path(project_root, &entry.id);
    let persisted: AgentRegistryEntry = toml::from_str(
        &std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("parsing {}", path.display()))?;
    anyhow::ensure!(
        persisted.process_identity().as_ref() == Some(&captured) && persisted.ended_at.is_none(),
        "detached worker registry row did not persist as written"
    );
    Ok(persisted)
}

/// The PID-namespace inode of process `pid`, when readable.
// trace:STORY-2254 | ai:claude
pub(crate) fn process_pid_namespace(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(format!("/proc/{pid}/ns/pid"))
            .ok()
            .map(|m| m.ino())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

/// STORY-2254: end the row of exactly `pid` with start `start`. A row that
/// now belongs to another process (a successor at a reused pid) is left
/// alone and `Ok(None)` is returned.
// trace:STORY-2254 | ai:claude
pub(crate) fn mark_detached_worker_ended(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    start: &str,
) -> Result<Option<AgentRegistryEntry>> {
    let agent_type = normalize_agent_type(agent_type.to_string());
    let path = registry_path(project_root, &agent_id(&agent_type, pid));
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let entry: AgentRegistryEntry =
        toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    // The legacy child receipt is local to this supervisor's boot and PID
    // coordinates. Validate that context without recapturing the exited PID.
    // trace:TASK-3366 | ai:codex
    let caller = ProcIdentity::capture(std::process::id()).map_err(anyhow::Error::msg)?;
    if entry.process_identity().is_none_or(|id| {
        id.boot_id != caller.boot_id
            || id.pidns_inode != caller.pidns_inode
            || id.legacy_ticks_start() != start
    }) || entry.ended_at.is_some()
    {
        return Ok(None);
    }
    end_entry(project_root, entry).map(Some)
}

fn register_spawned_agent_as(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    pid_start: Option<String>,
    pid_namespace: Option<u64>,
    row: SpawnedRow<'_>,
) -> Result<AgentRegistryEntry> {
    let SpawnedRow {
        role,
        current_spec,
        worktree_path,
        binary,
        name,
        native_session_id,
        resumed_from,
        description,
        delegate_seats,
        seat,
    } = row;
    let now = Utc::now();
    let agent_type = normalize_agent_type(agent_type.to_string());
    // The agent is already running; wait out launch-time contention rather
    // than fail on the SessionStart budget.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-030 | ai:claude
    let _lock = crate::agent_handle::lock_namespace_within(
        project_root,
        crate::agent_handle::LAUNCHER_LOCK_BUDGET,
    )?;
    // The child's SessionStart hook can run before this write: keep the
    // session id (the Codex thread) and the handle it recorded for this same
    // process. Read under the lock this write holds, so no hook write lands
    // in between. trace:STORY-HOME-096 | ai:claude
    let disk_row =
        std::fs::read_to_string(registry_path(project_root, &agent_id(&agent_type, pid)))
            .ok()
            .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok());
    let (hook_row, displaced) = match disk_row {
        Some(old)
            if same_identity(
                old.process_identity(),
                ProcIdentity::from_registry(pid, pid_start.as_deref(), pid_namespace),
            ) =>
        {
            (Some(old), None)
        }
        other => (None, other),
    };
    // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    if let Some(old) = &displaced {
        ensure_local_replacement(
            old,
            ProcIdentity::from_registry(pid, pid_start.as_deref(), pid_namespace).as_ref(),
        )?;
        retire_displaced_entry(project_root, old)?;
    }
    let hook_session_id = hook_row
        .as_ref()
        .and_then(|old| old.claude_session_id.clone());
    // A seat the same process's hook already recorded survives a launcher
    // write that names none. trace:STORY-2745 | ai:claude
    let seat = seat.or_else(|| hook_row.as_ref().and_then(|old| old.seat.clone()));
    let name = name.or_else(|| hook_row.and_then(|old| old.name));
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start,
        pid_namespace,
        name,
        description,
        tty: current_tty(),
        terminal: current_terminal_identity(),
        started_at: now,
        last_active_at: now,
        role,
        seat,
        current_spec,
        worktree_path,
        source: "agent-launcher".to_string(),
        binary_version: binary.map(|b| b.version.clone()),
        build_sha: binary.map(|b| b.sha.clone()),
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id,
        claude_session_id: hook_session_id,
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        grant_key: None,
        delegate_seats,
        resumed_from,
    };
    write_entry_locked(project_root, &entry)?;
    Ok(entry)
}

/// Register the interactive shell seat that just evaluated `aida role enter`.
///
/// `role enter` itself is a short-lived helper whose stdout is shell code; the
/// durable launch identity is the parent shell that keeps the exported role.
// trace:STORY-994 | ai:codex
pub(crate) fn register_role_enter_agent(
    project_root: &Path,
    role: &str,
    worktree_path: PathBuf,
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let pid = role_enter_shell_pid();
    let agent_type = "shell".to_string();
    let tty = process_tty(pid).or_else(current_tty);
    let terminal = terminal_identity_for_tty(tty.clone());
    let captured = ProcIdentity::capture(pid).ok();
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start: captured.as_ref().map(|id| id.legacy().start),
        pid_namespace: captured.as_ref().map(|id| id.pidns_inode),
        name: Some(format!("role:{role}")),
        description: Some("aida role enter shell".to_string()),
        tty,
        terminal,
        started_at: now,
        last_active_at: now,
        role: Some(role.to_string()),
        seat: None,
        current_spec: env_nonempty("AIDA_SESSION_SCOPE").filter(|s| looks_like_spec_id(s)),
        worktree_path,
        source: "role-enter".to_string(),
        binary_version: None,
        build_sha: None,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        grant_key: None,
        delegate_seats: Vec::new(),
        resumed_from: None,
    };
    write_entry(project_root, &entry)?;
    Ok(entry)
}

/// Validate a custom agent name or generate a default `<vendor>-<role>-NN`
/// handle. Validates uniqueness across all active (non-stale) agents and
/// launch reservations. With `reserve`, the name is reserved under the
/// handle namespace lock so a concurrent launch cannot take it before the
/// launched session binds it; a preview passes `false` and reserves nothing.
// trace:TASK-542 | ai:antigravity
// trace:STORY-1843 | ai:claude
pub(crate) fn generate_or_validate_name(
    project_root: &Path,
    agent_type: &str,
    role: Option<&str>,
    custom_name: Option<&str>,
    reserve: bool,
) -> Result<String> {
    let agent_type_clean = normalize_agent_type(agent_type.to_string());

    if let Some(name) = custom_name {
        let name_trimmed = name.trim();
        if name_trimmed.is_empty() {
            anyhow::bail!("--name cannot be empty");
        }
        let reserved = crate::agent_handle::reserved_addresses(project_root);
        crate::agent_handle::validate_explicit_handle(name_trimmed, &reserved)?;
    }

    let request = crate::agent_handle::HandleRequest {
        agent_type: &agent_type_clean,
        role,
        explicit: custom_name.map(str::trim),
        native_session_id: None,
        registry_id: None,
        launch_token: None,
        launcher: true,
        ended_relay_cleared: None,
    };
    if reserve {
        return Ok(crate::agent_handle::reserve_handle(project_root, &request)?.handle);
    }
    crate::agent_handle::preview_handle(project_root, &request)
}

/// Resolve an agent brief target to its allowed brief directories (exact name, type-role, and type)
/// or fall back to literal type-level routing with an ambiguity warning.
// trace:TASK-542 | ai:antigravity
pub(crate) fn resolve_brief_directories(
    project_root: &Path,
    target: &str,
) -> (Vec<String>, Option<String>) {
    let mut alive_agents = Vec::new();
    if let Ok(entries) = std::fs::read_dir(agents_dir(project_root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(body) = std::fs::read_to_string(&path) {
                if let Ok(record) = toml::from_str::<AgentRegistryEntry>(&body) {
                    if registry_entry_is_live(project_root, &record) {
                        alive_agents.push(record);
                    }
                }
            }
        }
    }

    // Find if target is exactly the name of an active agent (case-insensitive)
    let found_agent = alive_agents.iter().find(|agent| {
        if let Some(ref name) = agent.name {
            name.eq_ignore_ascii_case(target)
        } else {
            false
        }
    });

    if let Some(agent) = found_agent {
        let name = agent.name.as_ref().unwrap();
        let mut dirs = vec![name.clone()];
        if let Some(ref role) = agent.role {
            dirs.push(format!("{}-{}", agent.agent_type, role.to_lowercase()));
        }
        dirs.push(agent.agent_type.clone());
        dirs.sort();
        dirs.dedup();
        return (dirs, None);
    }

    // Fallback: target is not an active agent's name. Check for ambiguity.
    let mut matching_agents = Vec::new();
    for agent in &alive_agents {
        let type_match = agent.agent_type.eq_ignore_ascii_case(target);
        let type_role_match = if let Some(ref role) = agent.role {
            format!("{}-{}", agent.agent_type, role.to_lowercase()).eq_ignore_ascii_case(target)
        } else {
            false
        };
        if type_match || type_role_match {
            if let Some(ref name) = agent.name {
                matching_agents.push(name.clone());
            } else {
                matching_agents.push(format!("{}#{}", agent.agent_type, agent.pid));
            }
        }
    }

    let warning = if matching_agents.len() > 1 {
        Some(format!(
            "warning: agent target '{}' is ambiguous — matches multiple active agents: {}",
            target,
            matching_agents.join(", ")
        ))
    } else {
        None
    };

    (vec![target.to_string()], warning)
}

/// Register a raw-launched process that was not spawned by `aida agent new`.
///
/// The entry intentionally uses the same `<agent-type>-<pid>` id/key shape as
/// supervised launches so status rendering and stale-PID handling stay shared.
// trace:TASK-543 | ai:codex
pub(crate) fn register_existing_agent(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    role: String,
    current_spec: Option<String>,
    worktree_path: PathBuf,
    name: Option<String>,
    description: Option<String>,
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let agent_type = normalize_agent_type(agent_type.to_string());
    let tty = process_tty(pid).or_else(current_tty);
    let terminal = terminal_identity_for_tty(tty.clone());
    let captured = ProcIdentity::capture(pid).ok();
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start: captured.as_ref().map(|id| id.legacy().start),
        pid_namespace: captured.as_ref().map(|id| id.pidns_inode),
        name,
        description,
        tty,
        terminal,
        started_at: now,
        last_active_at: now,
        role: Some(role),
        seat: None,
        current_spec,
        worktree_path,
        source: "manual-register".to_string(),
        binary_version: None,
        build_sha: None,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        grant_key: None,
        delegate_seats: Vec::new(),
        resumed_from: None,
    };
    write_entry(project_root, &entry)?;
    Ok(entry)
}

// trace:STORY-791 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentScopeConflict {
    pub(crate) name_or_id: String,
    pub(crate) pid: u32,
    pub(crate) session_id: String,
    pub(crate) spec: Option<String>,
}

pub(crate) fn agent_scope(
    role: Option<&str>,
    current_spec: Option<&str>,
    worktree_path: &Path,
) -> String {
    if let Some(spec) = current_spec.filter(|s| !s.trim().is_empty()) {
        return format!("spec:{}", spec.to_ascii_uppercase());
    }
    if role.is_some_and(is_repo_scoped_role) {
        return "repo".to_string();
    }
    format!("worktree:{}", worktree_path.display())
}

/// Roles whose singleton scope is the whole repo rather than a spec/worktree.
pub(crate) fn is_repo_scoped_role(role: &str) -> bool {
    matches!(
        role.to_ascii_lowercase().as_str(),
        "advisor" | "product" | "integrator"
    )
}

/// Roles whose generic (no `--spec`) members form a pool: any number may run
/// per repo, each under its own handle, and exclusivity applies only at the
/// spec claim/lease. Advisor, product, integrator and reviewer stay singular.
// trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
pub(crate) const POOL_ROLES: &[&str] = &["implementer"];

// trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
pub(crate) fn is_pool_role(role: &str) -> bool {
    POOL_ROLES.iter().any(|r| r.eq_ignore_ascii_case(role))
}

/// A pool-role member launched without a spec: it holds no exclusive scope.
// trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
pub(crate) fn is_pool_launch(role: Option<&str>, current_spec: Option<&str>) -> bool {
    role.is_some_and(is_pool_role) && current_spec.is_none_or(|s| s.trim().is_empty())
}

pub(crate) fn same_scope_conflict(
    project_root: &Path,
    cfg: &Config,
    role: Option<&str>,
    current_spec: Option<&str>,
    worktree_path: &Path,
) -> Option<AgentScopeConflict> {
    let role = role?;
    if !cfg.role_is_singleton(role) {
        return None;
    }
    // A generic pool member never conflicts; two members still cannot share
    // a spec, because the spec scope below (and the lease) is exclusive.
    // trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
    if is_pool_launch(Some(role), current_spec) {
        return None;
    }
    let scope = agent_scope(Some(role), current_spec, worktree_path);
    load_entries(project_root)
        .into_iter()
        .map(|(_, entry)| entry)
        .filter(|entry| registry_entry_is_live(project_root, entry))
        .find(|entry| {
            entry
                .role
                .as_deref()
                .map(|r| r.eq_ignore_ascii_case(role))
                .unwrap_or(false)
                && agent_scope(
                    entry.role.as_deref(),
                    entry.current_spec.as_deref(),
                    &entry.worktree_path,
                ) == scope
        })
        .map(|entry| AgentScopeConflict {
            name_or_id: entry
                .name
                .clone()
                .unwrap_or_else(|| format!("{}#{}", entry.agent_type, entry.pid)),
            pid: entry.pid,
            session_id: entry.id,
            spec: entry.current_spec,
        })
}

// trace:STORY-791 | ai:codex
pub(crate) fn describe_agent(
    project_root: &Path,
    target: &str,
    description: String,
) -> Result<AgentRegistryEntry> {
    let (_, mut entry) = resolve_target_entry(project_root, target)?;
    entry.description = normalize_description(Some(description));
    write_entry(project_root, &entry)?;
    Ok(entry)
}

// trace:TASK-1184 | ai:codex
fn resumable_session_id(entry: &AgentRegistryEntry) -> Option<String> {
    entry
        .native_session_id
        .as_ref()
        .or(entry.claude_session_id.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// trace:TASK-1184 | ai:codex
fn gc_row(
    path: PathBuf,
    entry: &AgentRegistryEntry,
    resumable_session_id: Option<String>,
    outcome: &'static str,
) -> AgentGcRow {
    AgentGcRow {
        id: entry.id.clone(),
        agent_type: entry.agent_type.clone(),
        pid: entry.pid,
        source: entry.source.clone(),
        path,
        resumable_session_id,
        last_active_at: entry.last_active_at,
        outcome,
    }
}

// trace:TASK-1184 | ai:codex
pub(crate) fn gc_dead_agents(
    project_root: &Path,
    dry_run: bool,
    older_than_days: Option<u64>,
) -> Result<AgentGcReport> {
    let now = Utc::now();
    let mut report = AgentGcReport::default();
    // trace:BUG-2212 | ai:claude
    let observer = PidObserver::current();
    let entries = if older_than_days.is_some() {
        load_entries(project_root)
    } else {
        load_active_entries(project_root)
    };
    for (path, mut entry) in entries {
        // A row this process cannot observe is neither live nor dead to it:
        // a sandboxed gc must not prune or end a host session's row.
        // trace:STORY-1843 | ai:claude
        if observed_liveness_from(project_root, &entry, observer) != Some(false) {
            continue;
        }
        let resumable = resumable_session_id(&entry);
        let ended_launcher = entry.source == "agent-launcher" && entry.ended_at.is_some();
        let protected_ended_or_resumable = (resumable.is_some() || ended_launcher)
            && older_than_days
                .map(|days| {
                    elapsed_secs_clamped(now, entry.last_active_at)
                        < (days.saturating_mul(86_400)) as i64
                })
                .unwrap_or(true);
        if protected_ended_or_resumable {
            // Keep the resumable history but stop it counting as a mail
            // reader: an observably dead process has ended. Its end is the
            // last heartbeat, never now, so an old crash cannot outrank a
            // recent end in resume ordering. trace:STORY-1843 | ai:claude
            if entry.ended_at.is_none() {
                if !dry_run {
                    entry.ended_at = Some(entry.last_active_at);
                    // trace:STORY-HOME-096 | ai:claude
                    crate::mail_wake::retire(&mut entry);
                    write_entry(project_root, &entry)?;
                }
                report.marked_ended += 1;
            }
            report
                .retained
                .push(gc_row(path, &entry, resumable, "retained"));
            continue;
        }
        let row = gc_row(path.clone(), &entry, resumable, "removed");
        if !dry_run {
            std::fs::remove_file(&path).with_context(|| {
                format!("removing dead agent registry entry {}", path.display())
            })?;
        }
        report.removed.push(row);
    }
    report.removed.sort_by(|a, b| a.id.cmp(&b.id));
    report.retained.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(report)
}

// trace:STORY-432 | ai:codex
pub(crate) fn remove_agent(project_root: &Path, agent_type: &str, pid: u32) -> Result<bool> {
    let agent_type = normalize_agent_type(agent_type.to_string());
    let path = registry_path(project_root, &agent_id(&agent_type, pid));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => {
            Err(err).with_context(|| format!("removing agent registry entry {}", path.display()))
        }
    }
}

// trace:STORY-790 | ai:codex
pub(crate) fn mark_agent_ended(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
) -> Result<Option<AgentRegistryEntry>> {
    let agent_type = normalize_agent_type(agent_type.to_string());
    let path = registry_path(project_root, &agent_id(&agent_type, pid));
    let body = match std::fs::read_to_string(&path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    };
    let entry: AgentRegistryEntry =
        toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
    end_entry(project_root, entry).map(Some)
}

/// End the live row of the native session `session_id` at now, if one is
/// registered. A headless phase's launcher calls this when the child exits:
/// the child's only heartbeats are its prompts, and a one-prompt `-p` lane
/// would otherwise be ended by gc at its start. Matched by session id, not
/// pid, because a wrapped child's pid is the wrapper's.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn mark_session_ended(
    project_root: &Path,
    session_id: &str,
) -> Result<Option<AgentRegistryEntry>> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Ok(None);
    }
    let Some((_, entry)) = load_active_entries(project_root)
        .into_iter()
        .filter(|(_, row)| row.ended_at.is_none())
        .find(|(_, row)| {
            row.native_session_id.as_deref() == Some(session_id)
                || row.claude_session_id.as_deref() == Some(session_id)
        })
    else {
        return Ok(None);
    };
    end_entry(project_root, entry).map(Some)
}

fn end_entry(project_root: &Path, entry: AgentRegistryEntry) -> Result<AgentRegistryEntry> {
    // An exit receipt must not retire a replacement's mail wake or thread.
    // Legacy explicit endings require an unchanged row, never PID recapture.
    // trace:TASK-3366 | ai:codex
    let _lock = crate::agent_handle::lock_namespace_within(
        project_root,
        crate::agent_handle::LAUNCHER_LOCK_BUDGET,
    )?;
    let path = registry_path(project_root, &entry.id);
    let current: AgentRegistryEntry = toml::from_str(
        &aida_core::mailbox_identity::read_bounded_regular(&path)
            .context("registry row is not a bounded readable regular file")?,
    )?;
    anyhow::ensure!(
        (same_registry_process(&current, &entry) || current == entry)
            && current.claude_session_id == entry.claude_session_id
            && current.native_session_id == entry.native_session_id,
        "registry row changed before session exit; retaining its mail wake"
    );
    let mut entry = current;
    entry.ended_at = Some(Utc::now());
    entry.last_active_at = entry.ended_at.unwrap();
    // trace:STORY-790 | ai:claude — freeze the spec's status for the resume
    // drift brief's then-vs-now line. Best-effort: a store hiccup leaves None.
    entry.spec_status_at_end = entry
        .current_spec
        .as_deref()
        .and_then(|spec| crate::current_spec_status_line(project_root, spec));
    entry.availability = Availability::Available;
    entry.paused_since = None;
    entry.paused_reason = None;
    entry.expected_back = None;
    // The session ended: the watcher stops waking it; its ledger stays.
    // trace:STORY-HOME-096 | ai:claude
    crate::mail_wake::retire(&mut entry);
    // The launcher marks its child ended under the same launch-time
    // contention it registered through; the SessionStart budget would leave
    // a dead pid listed as running.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-030 | ai:claude
    write_entry_locked(project_root, &entry)?;
    Ok(entry)
}

/// Load every registry entry (alive or stale) as `(path, entry)` pairs.
/// Unlike `list_agent_views` this hands back the raw record so callers can
/// mutate + rewrite it (pause/resume). trace:STORY-528 | ai:claude
pub(crate) fn load_entries(project_root: &Path) -> Vec<(PathBuf, AgentRegistryEntry)> {
    let mut out = load_active_entries(project_root);
    let mut ids: std::collections::HashSet<_> = out.iter().map(|(_, e)| e.id.clone()).collect();
    out.extend(
        load_entries_in(&agents_dir(project_root).join("retired"))
            .into_iter()
            .filter(|(_, e)| ids.insert(e.id.clone())),
    );
    out
}

// Archived rows are provably ended and need no live-reader/allocation probe.
// trace:STORY-1843 | ai:codex
pub(crate) fn load_active_entries(project_root: &Path) -> Vec<(PathBuf, AgentRegistryEntry)> {
    load_entries_in(&agents_dir(project_root))
}

fn load_entries_in(dir: &Path) -> Vec<(PathBuf, AgentRegistryEntry)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(record) = toml::from_str::<AgentRegistryEntry>(&body) else {
                continue;
            };
            out.push((path, record));
        }
    }
    out
}

/// Explicit maintenance retains ended history outside the allocation scan.
/// The original rows remain readable for native-session resume and --ended.
// trace:STORY-1843 | ai:codex
pub(crate) fn archive_ended_entries(root: &Path, dry_run: bool) -> Result<usize> {
    if !agents_dir(root).is_dir() {
        return Ok(0);
    }
    let _lock = crate::agent_handle::lock_namespace(root)?;
    let mut count = 0;
    for (path, entry) in load_active_entries(root) {
        if entry.ended_at.is_none() {
            continue;
        }
        count += 1;
        if dry_run {
            continue;
        }
        let archived = agents_dir(root).join("retired");
        std::fs::create_dir_all(&archived)?;
        // Registry writers share the namespace lock, so no heartbeat or
        // revived row can replace this ended entry between copy and removal.
        let body = std::fs::read(&path)?;
        aida_core::write_atomic(&archived.join(path.file_name().unwrap()), &body)?;
        std::fs::remove_file(&path)?;
    }
    Ok(count)
}

/// Load all currently live agent registry entries for the project.
// trace:BUG-2010 | ai:antigravity
pub(crate) fn load_live_entries(project_root: &Path) -> Vec<AgentRegistryEntry> {
    load_live_identity_entries(project_root)
        .into_iter()
        .map(|mut entry| {
            // Preserve the existing lease/display API transport. Identity consumers
            // use load_live_identity_entries and retain the boot-qualified tuple.
            // trace:TASK-3366 | ai:codex
            if let Some(identity) = entry.process_identity() {
                entry.pid_start = Some(identity.legacy_ticks_start());
            }
            entry
        })
        .collect()
}

// trace:TASK-3366 | ai:codex
pub(crate) fn load_live_identity_entries(project_root: &Path) -> Vec<AgentRegistryEntry> {
    load_active_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| registry_entry_is_live(project_root, e))
        .collect()
}

/// BUG-2010: resolve the active agent-launcher entry when running inside an
/// interactive or headless agent session (AIDA_AGENT_NAME set).
// trace:BUG-2010 | ai:antigravity
pub(crate) fn resolve_session_agent_for_lease(project_root: &Path) -> Option<AgentRegistryEntry> {
    let agent_name = std::env::var("AIDA_AGENT_NAME").ok()?;
    let trimmed = agent_name.trim();
    if trimmed.is_empty() {
        return None;
    }

    let live_entries = load_live_identity_entries(project_root);
    let current_pid = std::process::id();

    live_entries.into_iter().find(|entry| {
        // Must match name or id
        let name_matches = entry.name.as_deref() == Some(trimmed) || entry.id == trimmed;
        if !name_matches {
            return false;
        }
        // Must be an `aida agent new` factory lane, not a Claude session-start hook or role-enter
        if entry.source != "agent-launcher" {
            return false;
        }
        // Must be a factory lane type (e.g. codex, antigravity), never a Claude operator session
        if entry.agent_type == "claude"
            || (!["codex", "antigravity"].contains(&entry.agent_type.as_str())
                && std::env::var("AIDA_AGENT_TYPE")
                    .map(|t| t != entry.agent_type)
                    .unwrap_or(true))
        {
            return false;
        }
        // Unknown liveness can retain a row, but cannot bind a caller to it.
        // trace:TASK-3366 | ai:codex
        if entry_process_verdict(entry) != Verdict::Alive {
            return false;
        }
        // Anti-spoofing: caller must be the agent process or a child/descendant of it
        if current_pid != entry.pid && !pid_has_ancestor(current_pid, entry.pid) {
            return false;
        }
        true
    })
}

/// BUG-2010: update an agent's current_spec and worktree_path in the registry.
// trace:BUG-2010 | ai:antigravity
pub(crate) fn update_agent_current_spec(
    project_root: &Path,
    agent_id: &str,
    spec_id: &str,
    worktree_path: &Path,
) -> Result<()> {
    let path = registry_path(project_root, agent_id);
    if !path.exists() {
        return Ok(());
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading agent registry entry {}", path.display()))?;
    let mut entry: AgentRegistryEntry = toml::from_str(&body)
        .with_context(|| format!("parsing agent registry entry {}", path.display()))?;
    entry.current_spec = Some(spec_id.to_string());
    entry.worktree_path = worktree_path.to_path_buf();
    entry.last_active_at = chrono::Utc::now();
    write_entry(project_root, &entry)
}

/// Find a live registered agent that matches a lease by exact owner identity.
/// Requires an exact owner match (lease.owner == agent.name or agent.id).
/// No worktree-prefix matching in either direction.
/// Protects against recycled lane names by requiring active_pid == agent.pid
/// with the lease's own start-time identity.
// trace:BUG-2010 | ai:antigravity
pub(crate) fn live_agent_for_lease_or_spec<'a>(
    live_agents: &'a [AgentRegistryEntry],
    lease: Option<&SessionLease>,
    spec_id: Option<&str>,
) -> Option<&'a AgentRegistryEntry> {
    let lease = lease?;
    live_agents.iter().find(|agent| {
        // Exact owner-identity match: lease owner must match agent name or id
        let owner_matches = agent
            .name
            .as_deref()
            .map(|n| n == lease.owner)
            .unwrap_or(false)
            || agent.id == lease.owner;

        if !owner_matches {
            return false;
        }

        // Recycled lane guard (BUG-2010):
        // A recycled lane name must not revive an old dead lease.
        // The lease must explicitly record its own active_pid and active_pid_start_time.
        // If either is missing, the lease is treated as unattributed.
        let (Some(active_pid), Some(ref start_time)) =
            (lease.active_pid, lease.active_pid_start_time.as_ref())
        else {
            return false;
        };
        if active_pid != agent.pid {
            return false;
        }
        if !agent.process_identity().is_some_and(|id| {
            id.legacy_ticks_start() == start_time.as_str()
                && lease.pid_namespace == Some(id.pidns_inode)
                && proc_identity::observe(&id, true) == Verdict::Alive
        }) {
            return false;
        }

        // If spec_id or lease.scope is specified and agent records current_spec, ensure it agrees
        let target_spec = spec_id.or_else(|| {
            if !lease.scope.is_empty() {
                Some(&lease.scope)
            } else {
                None
            }
        });
        if let Some(spec) = target_spec {
            if let Some(agent_spec) = agent.current_spec.as_deref() {
                if !agent_spec.eq_ignore_ascii_case(spec) {
                    return false;
                }
            }
        }

        true
    })
}

/// Resolve a `<target>` (name, `<type>#<pid>`, or `<type>-<pid>` id) to the
/// single matching live registry entry. Prefers an exact name match, falling
/// back to the synthetic id forms. Errors on no-match or ambiguity so the
/// caller can surface a clear message rather than silently picking one.
/// trace:STORY-528 | ai:claude
fn resolve_target_entry(
    project_root: &Path,
    target: &str,
) -> Result<(PathBuf, AgentRegistryEntry)> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("agent target cannot be empty");
    }
    let mut matches: Vec<(PathBuf, AgentRegistryEntry)> = load_entries(project_root)
        .into_iter()
        .filter(|(_, e)| registry_entry_is_live(project_root, e))
        .filter(|(_, e)| {
            let name_match = e
                .name
                .as_deref()
                .map(|n| n.eq_ignore_ascii_case(target))
                .unwrap_or(false);
            let hash_id = format!("{}#{}", e.agent_type, e.pid);
            let dash_id = format!("{}-{}", e.agent_type, e.pid);
            name_match
                || e.id.eq_ignore_ascii_case(target)
                || hash_id.eq_ignore_ascii_case(target)
                || dash_id.eq_ignore_ascii_case(target)
        })
        .collect();

    match matches.len() {
        0 => anyhow::bail!("no active agent found matching '{}'", target),
        1 => Ok(matches.remove(0)),
        _ => {
            let ids: Vec<String> = matches
                .iter()
                .map(|(_, e)| {
                    e.name
                        .clone()
                        .unwrap_or_else(|| format!("{}#{}", e.agent_type, e.pid))
                })
                .collect();
            anyhow::bail!(
                "agent target '{}' is ambiguous — matches: {}",
                target,
                ids.join(", ")
            )
        }
    }
}

/// The single live registry entry `target` names.
// trace:STORY-HOME-096 | ai:claude
pub(crate) fn resolve_live_target(project_root: &Path, target: &str) -> Result<AgentRegistryEntry> {
    resolve_target_entry(project_root, target).map(|(_, entry)| entry)
}

/// Mark a live agent `Paused`. Returns the updated entry so the caller can
/// render a confirmation. trace:STORY-528 | ai:claude
pub(crate) fn pause_agent(
    project_root: &Path,
    target: &str,
    reason: PauseReason,
    expected_back: Option<DateTime<Utc>>,
) -> Result<AgentRegistryEntry> {
    let (_, mut entry) = resolve_target_entry(project_root, target)?;
    entry.availability = Availability::Paused;
    entry.paused_since = Some(Utc::now());
    entry.paused_reason = Some(reason);
    entry.expected_back = expected_back;
    write_entry(project_root, &entry)?;
    Ok(entry)
}

/// Clear a paused agent back to `Available`, dropping all pause metadata.
/// trace:STORY-528 | ai:claude
pub(crate) fn resume_agent(project_root: &Path, target: &str) -> Result<AgentRegistryEntry> {
    let (_, mut entry) = resolve_target_entry(project_root, target)?;
    entry.availability = Availability::Available;
    entry.paused_since = None;
    entry.paused_reason = None;
    entry.expected_back = None;
    write_entry(project_root, &entry)?;
    Ok(entry)
}

// trace:STORY-790 | ai:codex
pub(crate) fn resolve_resumable_ended_agent(
    project_root: &Path,
    target: &str,
) -> Result<AgentRegistryEntry> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("agent target cannot be empty");
    }
    let mut matches: Vec<AgentRegistryEntry> = load_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.ended_at.is_some() && resumable_session_id(e).is_some())
        .filter(|e| {
            let name_match = e
                .name
                .as_deref()
                .map(|n| n.eq_ignore_ascii_case(target))
                .unwrap_or(false);
            let spec_match = e
                .current_spec
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(target))
                .unwrap_or(false);
            let hash_id = format!("{}#{}", e.agent_type, e.pid);
            let dash_id = format!("{}-{}", e.agent_type, e.pid);
            name_match
                || spec_match
                || e.id.eq_ignore_ascii_case(target)
                || hash_id.eq_ignore_ascii_case(target)
                || dash_id.eq_ignore_ascii_case(target)
        })
        .collect();
    matches.sort_by_key(|e| e.ended_at);
    matches.reverse();

    match matches.len() {
        0 => anyhow::bail!("no ended resumable agent found matching '{}'", target),
        1 => Ok(matches.remove(0)),
        _ => {
            let ids: Vec<String> = matches
                .iter()
                .map(|e| {
                    e.name
                        .clone()
                        .unwrap_or_else(|| format!("{}#{}", e.agent_type, e.pid))
                })
                .collect();
            anyhow::bail!(
                "ended agent target '{}' is ambiguous — matches: {}",
                target,
                ids.join(", ")
            )
        }
    }
}

// trace:STORY-790 | ai:codex
pub(crate) fn ended_resumable_agent_views(
    project_root: &Path,
    ctx: &AgentClassifyContext,
) -> Vec<AgentRegistryView> {
    let mut out: Vec<AgentRegistryView> = load_entries(project_root)
        .into_iter()
        .map(|(_, e)| e)
        .filter(|e| e.ended_at.is_some() && resumable_session_id(e).is_some())
        .map(|e| view_for(project_root, e, ctx))
        .collect();
    out.sort_by(|a, b| {
        b.ended_at
            .cmp(&a.ended_at)
            .then_with(|| a.agent_type.cmp(&b.agent_type))
            .then_with(|| a.pid.cmp(&b.pid))
    });
    out
}

/// Brief-time GUARD: if a brief target resolves to a *paused* live agent,
/// return a one-line warning string (does NOT refuse). Returns `None` when
/// the target isn't a paused live agent (unknown target, available agent,
/// stale entry — all of those are "no warning"). trace:STORY-528 | ai:claude
pub(crate) fn paused_warning_for_target(project_root: &Path, target: &str) -> Option<String> {
    let (_, entry) = resolve_target_entry(project_root, target).ok()?;
    if !entry.availability.is_paused() {
        return None;
    }
    let view_like = pause_detail(entry.paused_reason.unwrap_or_default(), entry.expected_back);
    let name = entry
        .name
        .clone()
        .unwrap_or_else(|| format!("{}#{}", entry.agent_type, entry.pid));
    // trace:TASK-840 | ai:claude — route the warning marker through the registry.
    let warn = crate::glyphs::get(
        crate::glyphs::Glyph::Warning,
        crate::find_project_root().ok().as_deref(),
    );
    Some(format!(
        "{warn} {name} is {view_like} — it may not pick this up until it resumes"
    ))
}

/// Render the `paused (budget, ~back HH:MM)` detail fragment shared by the
/// status line and the brief-time warning. trace:STORY-528 | ai:claude
pub(crate) fn pause_detail(reason: PauseReason, expected_back: Option<DateTime<Utc>>) -> String {
    match expected_back {
        Some(when) => format!(
            "paused ({}, ~back {})",
            reason.as_str(),
            when.with_timezone(&chrono::Local).format("%H:%M")
        ),
        None => format!("paused ({})", reason.as_str()),
    }
}

/// The `⏸ paused (...)` glyph fragment for an agent view, or `None` when the
/// agent is Available. trace:STORY-528 | ai:claude
pub(crate) fn paused_glyph(view: &AgentRegistryView) -> Option<String> {
    if !view.availability.is_paused() {
        return None;
    }
    Some(format!(
        "⏸ {}",
        pause_detail(view.paused_reason.unwrap_or_default(), view.expected_back)
    ))
}

pub(crate) fn list_agent_views(
    project_root: &Path,
    ctx: &AgentClassifyContext,
) -> Vec<AgentRegistryView> {
    let mut out: Vec<_> = load_entries(project_root)
        .into_iter()
        .map(|(_, record)| view_for(project_root, record, ctx))
        .collect();
    out.sort_by(|a, b| {
        a.agent_type
            .cmp(&b.agent_type)
            .then_with(|| a.pid.cmp(&b.pid))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

pub(crate) fn format_agent_status_lines(agents: &[AgentRegistryView]) -> Vec<String> {
    let now = Utc::now();
    agents
        .iter()
        .map(|agent| {
            let elapsed = humanize_elapsed(elapsed_secs_clamped(now, agent.last_active_at));
            let identity = if let Some(ref name) = agent.name {
                name.clone()
            } else if agent.source == "lease" {
                let short = agent.id.trim_start_matches("lease-");
                let short = &short[..short.len().min(8)];
                format!("{}#{}", agent.agent_type, short)
            } else {
                format!("{}#{}", agent.agent_type, agent.pid)
            };
            let source_note = if agent.source == "lease" {
                if agent.agent_type == "unknown" {
                    "  (via lease; agent type: unknown)"
                } else {
                    "  (via lease)"
                }
            } else {
                ""
            };
            let scope = agent_scope(
                agent.role.as_deref(),
                agent.current_spec.as_deref(),
                &agent.worktree_path,
            );
            let desc = agent.description.as_deref().unwrap_or("(none)");
            let terminal = terminal_cell(agent.terminal.as_ref(), agent.tty.as_deref());
            // STORY-528: a paused agent gets its `⏸ paused (...)` state
            // appended so the operator sees budget/rate-limit holds inline.
            let paused_note = match paused_glyph(agent) {
                Some(g) => format!("  {g}"),
                None => String::new(),
            };
            format!(
                "  {:<15} {:<8} {:<11} {:<12} {:<18} {:<5} {:<8} {:<20} {:<24} {}{}{}",
                identity,
                view_kind(agent),
                agent.role.as_deref().unwrap_or("(none)"),
                agent.current_spec.as_deref().unwrap_or("(none)"),
                scope,
                agent.status.as_str(),
                format!("({elapsed})"),
                terminal,
                desc,
                agent.worktree_path.display(),
                source_note,
                paused_note
            )
        })
        .collect()
}

pub(crate) fn mcp_authority_status_lines(agents: &[AgentRegistryView]) -> Vec<String> {
    // trace:BUG-1043 | ai:codex
    agents
        .iter()
        .filter(|agent| agent.source == "mcp" && agent.status != AgentStatus::Stale)
        .map(|agent| {
            let server_role = agent.role.as_deref().unwrap_or("(none)");
            let advisor = crate::gate::role_authority::advisor_authority_from(server_role, false, false);
            format!(
                "  MCP authority: {} server#{} seat={} (advisor authority: {}); recovery: enter advisor at a TTY, then restart MCP",
                agent.agent_type,
                agent.pid,
                server_role,
                if advisor { "yes" } else { "no" }
            )
        })
        .collect()
}

pub(crate) fn mcp_authority_status_lines_for_project(project_root: &Path) -> Vec<String> {
    let cfg = Config::load(project_root);
    let ctx =
        AgentClassifyContext::new(Utc::now(), cfg.busy_threshold_secs, Vec::new()).for_display();
    let agents = list_agent_views(project_root, &ctx);
    mcp_authority_status_lines(&agents)
}

pub(crate) fn write_entry(project_root: &Path, entry: &AgentRegistryEntry) -> Result<()> {
    // Serialize with ended-row archival and handle allocation, using their
    // bounded common-root lock. Readers still see atomic record replacement.
    // trace:STORY-1843 | ai:codex
    let _lock = crate::agent_handle::lock_namespace(project_root)?;
    write_entry_locked(project_root, entry)
}

/// [`write_entry`] with an explicit namespace-lock wait budget.
// trace:BUG-b6b1278890774ef993d2ebc327862470-030 | ai:claude
pub(crate) fn write_entry_within(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    budget: std::time::Duration,
) -> Result<()> {
    let _lock = crate::agent_handle::lock_namespace_within(project_root, budget)?;
    write_entry_locked(project_root, entry)
}

// Version only filesystem records: ordinary JSON/display serialization keeps
// the public agent ID stable. trace:TASK-3366 | ai:codex
fn registry_body(entry: &AgentRegistryEntry) -> Result<String> {
    let mut value = toml::Value::try_from(entry).context("serialising agent registry entry")?;
    value["id"] = toml::Value::String(proc_identity::registry_wire_id(&entry.id));
    toml::to_string_pretty(&value).context("serialising versioned agent registry entry")
}

fn write_entry_locked(project_root: &Path, entry: &AgentRegistryEntry) -> Result<()> {
    let path = registry_path(project_root, &entry.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating agent registry dir {}", parent.display()))?;
    }
    let body = registry_body(entry)?;
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing agent registry entry {}", path.display()))
}

fn view_for(
    project_root: &Path,
    entry: AgentRegistryEntry,
    ctx: &AgentClassifyContext,
) -> AgentRegistryView {
    // trace:BUG-2212 | ai:claude
    let (pid_alive, liveness) = observer_liveness(
        project_root,
        &entry,
        ctx.observer,
        ctx.now,
        ctx.heartbeat_ttl_secs,
    );
    let status = classify_status(&entry, pid_alive, ctx);
    let native_session_id = resumable_session_id(&entry);
    let compatible_start = entry
        .process_identity()
        .map(|id| id.legacy_ticks_start())
        .or_else(|| entry.pid_start.clone());
    AgentRegistryView {
        id: entry.id,
        agent_type: entry.agent_type,
        pid: entry.pid,
        // Legacy display/signal adapters are inventoried for the remaining
        // registry/GC slice; runtime row readers consume the full tuple.
        // trace:TASK-3366 | ai:codex
        pid_start: compatible_start,
        name: entry.name,
        description: entry.description,
        tty: entry.tty,
        terminal: entry.terminal,
        started_at: entry.started_at,
        last_active_at: entry.last_active_at,
        role: entry.role,
        current_spec: entry.current_spec,
        worktree_path: entry.worktree_path,
        source: entry.source,
        binary_version: entry.binary_version,
        build_sha: entry.build_sha,
        status,
        availability: entry.availability,
        paused_since: entry.paused_since,
        paused_reason: entry.paused_reason,
        expected_back: entry.expected_back,
        native_session_id,
        ended_at: entry.ended_at,
        resumed_from: entry.resumed_from,
        liveness,
        mail_wake: entry.mail_wake,
    }
}

// trace:BUG-1156 | ai:codex
pub(crate) fn agent_is_live(project_root: &Path, agent_type: &str, pid: u32) -> bool {
    let id = agent_id(&normalize_agent_type(agent_type.to_string()), pid);
    let path = registry_path(project_root, &id);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
        .is_some_and(|entry| registry_entry_is_live(project_root, &entry))
}

/// STORY-1420: is a seat NAMED `name` still live? Local only — the registry
/// files plus a pid probe. `Some(true)` when any entry with that name (case-
/// insensitive) is live, `Some(false)` when entries exist but all have
/// exited, `None` when the registry has never heard of the name.
// trace:STORY-1420 | ai:claude
pub(crate) fn named_agent_liveness(project_root: &Path, name: &str) -> Option<bool> {
    let name = name.trim();
    let mut seen = false;
    for (_, entry) in load_entries(project_root) {
        if !entry
            .name
            .as_deref()
            .is_some_and(|n| n.trim().eq_ignore_ascii_case(name))
        {
            continue;
        }
        if registry_entry_is_live(project_root, &entry) {
            return Some(true);
        }
        seen = true;
    }
    seen.then_some(false)
}

// Namespace of the observer, not /proc/<target>/ns/pid: a host observer
// records the host-visible PID even when the target itself is namespaced.
// trace:STORY-1843 | ai:codex
pub(crate) fn current_pid_namespace() -> Option<u64> {
    aida_core::pid_observer::current_pid_namespace()
}

/// Liveness as this observer can actually see it. `None` when the row's PID
/// lives in a namespace the observer cannot probe (a sandboxed command, or a
/// legacy unstamped row seen from outside the initial namespace). Ended rows
/// are dead to every observer.
// trace:STORY-1843 | ai:claude
pub(crate) fn observed_liveness(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    observer: Option<u64>,
) -> Option<bool> {
    let observer = PidObserver {
        namespace: observer,
        isolated: false,
    };
    observed_liveness_from(project_root, entry, observer)
}

/// [`observed_liveness`] from a full observer, so an isolated sandbox in the
/// initial namespace's clothing still cannot vouch for unstamped rows.
// trace:BUG-2212 | ai:claude
pub(crate) fn observed_liveness_from(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    observer: PidObserver,
) -> Option<bool> {
    if entry.ended_at.is_some() {
        return Some(false);
    }
    if !observer.can_observe(entry.pid_namespace) {
        return None;
    }
    // Legacy matches retain advisory listing/capacity behavior without lending
    // a full identity to signalling, replacement or watched-mail admission.
    let verdict = if entry.process_identity().is_none() && !entry.id.starts_with("proc-identity-v")
    {
        proc_identity::observe_legacy_registry(
            entry.pid,
            entry.pid_start.as_deref(),
            entry.pid_namespace,
        )
    } else {
        entry_process_verdict(entry)
    };
    match verdict {
        Verdict::Alive => Some(registry_entry_is_live(project_root, entry)),
        Verdict::Dead(_) => Some(false),
        Verdict::ForeignUnknown(_) => None,
    }
}

/// Inode of Linux's initial PID namespace.
// trace:STORY-1843 | ai:claude
#[cfg(test)]
pub(crate) const INITIAL_PID_NAMESPACE: u64 = aida_core::process_probe::INITIAL_PID_NAMESPACE_INODE;

/// How long a row whose process this observer cannot see stays in a human
/// LISTING, from its last heartbeat. Display only: guards never apply a
/// heartbeat bound, because session-start-hook rows heartbeat exactly once
/// (at SessionStart), so any finite bound makes every live host session look
/// stale from a sandbox eventually — the guard must fail closed instead.
// trace:BUG-2212 | ai:claude
// trace:TASK-2272 | ai:claude
pub(crate) const UNOBSERVED_HEARTBEAT_TTL_SECS: i64 = 24 * 60 * 60;

/// What a displayed row's liveness rests on.
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LivenessBasis {
    #[default]
    Process,
    /// The process is invisible from this PID namespace; the row's recent
    /// heartbeat stands in for it.
    Heartbeat,
}

impl LivenessBasis {
    pub(crate) fn is_process(&self) -> bool {
        *self == Self::Process
    }
}

/// Liveness as `observer` sees it. Process-observed when the observer can see
/// the row's pid. Otherwise the row is only PROVABLY dead via `ended_at`, so
/// it counts as live (unverified) — `heartbeat_ttl_secs: None`, the guard
/// default — unless a display caller passes a finite listing window.
// trace:BUG-2212 | ai:claude
// trace:TASK-2272 | ai:claude
pub(crate) fn observer_liveness(
    project_root: &Path,
    entry: &AgentRegistryEntry,
    observer: PidObserver,
    now: DateTime<Utc>,
    heartbeat_ttl_secs: Option<i64>,
) -> (bool, LivenessBasis) {
    if entry.ended_at.is_some() {
        return (false, LivenessBasis::Process);
    }
    if let Some(live) = observed_liveness_from(project_root, entry, observer) {
        return (live, LivenessBasis::Process);
    }
    let age = now
        .signed_duration_since(entry.last_active_at)
        .num_seconds();
    (
        heartbeat_ttl_secs.is_none_or(|ttl| age <= ttl),
        LivenessBasis::Heartbeat,
    )
}

/// The entry's process is still running: pid plus recorded start time, so a
/// recycled pid reads dead.
// trace:TASK-1750 | ai:claude
fn entry_process_alive(entry: &AgentRegistryEntry) -> bool {
    !matches!(entry_process_verdict(entry), Verdict::Dead(_))
}

// Registry files are local runtime records, never replicated node evidence.
// Incomplete legacy rows supply negative evidence only; never recapture an old PID.
// trace:TASK-3366 | ai:codex
fn entry_process_verdict(entry: &AgentRegistryEntry) -> Verdict {
    if entry.id.starts_with("proc-identity-v") {
        return Verdict::ForeignUnknown(proc_identity::UnknownReason::MissingIdentity);
    }
    entry.process_identity().map_or_else(
        || match proc_identity::observe_legacy_registry(
            entry.pid,
            entry.pid_start.as_deref(),
            entry.pid_namespace,
        ) {
            Verdict::Alive => {
                Verdict::ForeignUnknown(proc_identity::UnknownReason::MissingIdentity)
            }
            verdict => verdict,
        },
        |id| proc_identity::observe(&id, true),
    )
}

impl AgentRegistryEntry {
    pub(crate) fn process_identity(&self) -> Option<ProcIdentity> {
        if self.id.starts_with("proc-identity-v") {
            return None;
        }
        ProcIdentity::from_registry(self.pid, self.pid_start.as_deref(), self.pid_namespace)
    }
}
fn ensure_local_replacement(old: &AgentRegistryEntry, new: Option<&ProcIdentity>) -> Result<()> {
    if old.ended_at.is_none()
        && old
            .pid_namespace
            .zip(new)
            .is_some_and(|(ns, new)| ns != new.pidns_inode)
        && !matches!(entry_process_verdict(old), Verdict::Dead(_))
    {
        anyhow::bail!("registry PID belongs to an unobservable namespace; retry on the host");
    }
    Ok(())
}
// Upgrade only during positive local registration, never during passive inspection.
// trace:TASK-3366 | ai:codex
fn registration_matches(row: &AgentRegistryEntry, captured: &ProcIdentity) -> bool {
    row.process_identity().as_ref() == Some(captured)
        || (row.pid == captured.pid
            && row.pid_start.as_deref() == Some(captured.legacy_ticks_start().as_str())
            && row
                .pid_namespace
                .is_none_or(|ns| ns == captured.pidns_inode))
}

fn same_identity(a: Option<ProcIdentity>, b: Option<ProcIdentity>) -> bool {
    a.is_some() && a == b
}
pub(crate) fn same_registry_process(a: &AgentRegistryEntry, b: &AgentRegistryEntry) -> bool {
    same_identity(a.process_identity(), b.process_identity())
}

// trace:BUG-1156 | ai:codex
// trace:BUG-2010 | ai:antigravity
pub(crate) fn registry_entry_is_live(project_root: &Path, entry: &AgentRegistryEntry) -> bool {
    if entry.ended_at.is_some() || !entry_process_alive(entry) {
        return false;
    }
    if entry.source == "role-enter" {
        return role_enter_has_live_session(project_root, entry);
    }
    true
}

// trace:BUG-1156 | ai:codex
fn role_enter_has_live_session(project_root: &Path, role_entry: &AgentRegistryEntry) -> bool {
    load_entries(project_root)
        .into_iter()
        .map(|(_, entry)| entry)
        .any(|entry| {
            entry.id != role_entry.id
                && entry.source != "role-enter"
                && entry.ended_at.is_none()
                && entry_process_verdict(&entry) == Verdict::Alive
                && role_entry
                    .role
                    .as_deref()
                    .zip(entry.role.as_deref())
                    .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                && covers(
                    std::slice::from_ref(&role_entry.worktree_path),
                    &entry.worktree_path,
                )
                && pid_has_ancestor(entry.pid, role_entry.pid)
        })
}

// trace:STORY-791 | ai:codex
pub(crate) fn normalize_description(description: Option<String>) -> Option<String> {
    description
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
}

// trace:STORY-791 | ai:codex
pub(crate) fn view_kind(agent: &AgentRegistryView) -> &'static str {
    if agent.source == "lease" {
        "lease"
    } else {
        "process"
    }
}

// STORY-435 heartbeat freshness plus BUG-1704 live-lease work evidence.
// A covered process stays Busy when its subtree shows recent sustained work;
// missing evidence preserves the Busy result. trace:STORY-435 | ai:claude
fn classify_status(
    entry: &AgentRegistryEntry,
    pid_alive: bool,
    ctx: &AgentClassifyContext,
) -> AgentStatus {
    if entry.ended_at.is_some() {
        return AgentStatus::Stale;
    }
    if !pid_alive {
        return AgentStatus::Stale;
    }
    let elapsed = elapsed_secs_clamped(ctx.now, entry.last_active_at);
    if elapsed < ctx.threshold_secs as i64 {
        return AgentStatus::Busy;
    }
    if covers(&ctx.live_lease_worktrees, &entry.worktree_path) {
        match ctx.proc_tree.as_ref() {
            None => return AgentStatus::Busy,
            Some(tree) if seat_has_own_work(tree, entry.pid, elapsed, ctx.work_grace_secs) => {
                return AgentStatus::Busy
            }
            Some(_) => {}
        }
    }
    AgentStatus::Idle
}

// trace:BUG-1704 | ai:codex
fn seat_has_own_work(
    tree: &ProcTree,
    seat_pid: u32,
    secs_since_heartbeat: i64,
    grace_secs: u64,
) -> bool {
    // If a heartbeat lands during a long build, the heartbeat can move past build start and
    // clause 2 can exclude that build. Tool calls are sequential, so this is accepted in practice.
    let Some(summary) = tree.descendant_age_summary(seat_pid) else {
        return true;
    };
    // BUG-1727 AC1: an unsampleable descendant is the same class of evidence gap as a
    // missing root — the probe could not see — so it resolves the same way, toward Busy.
    // A false Idle gets a working seat stopped; a false Busy only delays a reap.
    // trace:BUG-1727 | ai:claude
    if summary.unsampled > 0 {
        return true;
    }
    if secs_since_heartbeat <= grace_secs.min(i64::MAX as u64) as i64 {
        return true;
    }
    ages_show_work(
        &summary.ages,
        secs_since_heartbeat,
        grace_secs.min(i64::MAX as u64) as i64,
    )
}

// trace:BUG-1704 | ai:codex
fn ages_show_work(descendant_ages: &[i64], secs_since_heartbeat: i64, grace_secs: i64) -> bool {
    if secs_since_heartbeat <= grace_secs {
        return false;
    }
    descendant_ages
        .iter()
        // The grace bound excludes momentary statusline/hook children.
        // Without it, a finished seat can flap Busy on those short-lived processes.
        // Its cost: a child younger than the grace window is not counted, so a seat can
        // read Idle for up to `work_grace_secs` after a delayed spawn. That is BUG-1727.
        // The upper bound is not a threshold on the child's age.
        // `age < secs_since_heartbeat` means `(now - child_start) < (now - heartbeat)`.
        // Cancel `now` on both sides and it becomes `child_start > heartbeat`.
        // That asks whether the child started after this seat's last heartbeat.
        // Time passing cannot change that ordering, so a long-running build stays Busy.
        // This holds while it runs provided no heartbeat landed after it started.
        // A later heartbeat can exclude it; that is BUG-1726. Its consequence, stated
        // rather than left to the bug id: a seat that launched a build DETACHED and then
        // kept making tool calls moves its heartbeat past the build's start, so the build
        // stops counting and the seat reads Idle while it still runs. The build is
        // STRANDED, not killed — `agent_stop` hands `terminate_pids_with_grace` the matched
        // registry rows' own pids and nothing else; that function signals each pid in the slice
        // (no negative pgid, no descendant walk), so the child is
        // reparented to init and completes ownerless, holding a cargo slot with no seat
        // left to read its result. trace:BUG-1726 | ai:claude
        .any(|age| *age >= grace_secs && *age < secs_since_heartbeat)
}

/// Accumulated CPU seconds for `pid`, via `ps -o time=`.
///
/// BUG-1701 AC5: a dispatched seat that has FINISHED its turn looks identical to one still
/// working — same pid, same registry row, and `classify_status` calls it `busy` whenever a live
/// lease covers its worktree, which for the main worktree is always. On 2026-09-27 three seats
/// reported `busy` after 48-88 minutes idle. The tell was CPU consumed: 3-46 SECONDS across
/// hours. Reporting that number lets a caller judge without attaching to the PTY.
///
/// Deliberately NOT used to reclassify: cumulative CPU cannot distinguish "finished" from
/// "blocked on a long build", and flipping the latter to `idle` would invite an orchestrator to
/// stop a working seat. A classification fix needs a CPU DELTA between two samples. BUG-1704 instead uses an age-window descendant rule.
// trace:BUG-1701 | ai:claude
pub(crate) fn process_cpu_secs(pid: u32) -> Option<i64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    parse_ps_cpu_time(String::from_utf8_lossy(&out.stdout).trim())
}

/// Parse `ps -o time=`: `MM:SS`, `HH:MM:SS`, or `DD-HH:MM:SS`. Pure, so the shapes are tested
/// without spawning `ps`.
// trace:BUG-1701 | ai:claude
fn parse_ps_cpu_time(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (days, clock) = match raw.split_once('-') {
        Some((d, rest)) => (d.trim().parse::<i64>().ok()?, rest),
        None => (0, raw),
    };
    let mut parts = clock.split(':').rev();
    let secs: i64 = parts.next()?.trim().parse().ok()?;
    let mins: i64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    let hours: i64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(days * 86_400 + hours * 3_600 + mins * 60 + secs)
}

pub(crate) fn elapsed_secs_clamped(now: DateTime<Utc>, at: DateTime<Utc>) -> i64 {
    now.signed_duration_since(at).num_seconds().max(0)
}

/// `agent` is covered by `worktrees` iff some entry is exactly `agent` or
/// `agent` lives under that entry. Mirrors `lease_covers_cwd` in
/// `aida-cli/src/main.rs` — empty paths are intentionally treated as
/// non-covering, since `Path::starts_with("")` is true for every path and
/// would otherwise let an advisory MCP lease silently match every agent.
/// trace:STORY-435 trace:TASK-474 | ai:claude
fn covers(worktrees: &[PathBuf], agent: &Path) -> bool {
    worktrees.iter().any(|w| {
        if w.as_os_str().is_empty() {
            return false;
        }
        agent == w.as_path() || agent.starts_with(w)
    })
}

pub(crate) fn humanize_elapsed(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Detect which agent tool this process runs under, from the process env.
///
/// See [`detect_agent_type_from_env`] for the precedence rule.
pub(crate) fn detect_agent_type() -> String {
    detect_agent_type_from_env(std::env::vars())
}

/// Pure agent-type detection over an explicit env snapshot.
///
/// Precedence (strongest signal wins):
/// 1. explicit override: a non-empty `AIDA_AGENT_TYPE`;
/// 2. live session/tool markers set by the running tool itself:
///    `CODEX_*`, then `ANTIGRAVITY_*`, then `CLAUDECODE` / `CLAUDE_CODE_*`;
/// 3. weak vendor prefixes that may be stale shell config: `GEMINI_*`, then
///    `CLAUDE*`;
/// 4. otherwise `"other"`.
///
/// Credential variable names (`*_KEY`, `*_TOKEN`, e.g. `GEMINI_API_KEY`) are
/// excluded from the scan: an exported API key says nothing about which tool
/// is live, so it must never decide attribution.
// trace:BUG-1920 | ai:claude
pub(crate) fn detect_agent_type_from_env<I, K, V>(vars: I) -> String
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut override_type: Option<String> = None;
    let names: Vec<String> = vars
        .into_iter()
        .filter_map(|(k, v)| {
            let key = k.as_ref();
            if key == "AIDA_AGENT_TYPE" {
                let value = v.as_ref().trim();
                if !value.is_empty() {
                    override_type = Some(value.to_string());
                }
                return None;
            }
            if is_credential_var(key) {
                return None;
            }
            Some(key.to_string())
        })
        .collect();
    if let Some(v) = override_type {
        return normalize_agent_type(v);
    }
    // Tier 2: live session markers.
    if any_name(&names, |k| k.starts_with("CODEX_")) {
        return "codex".to_string();
    }
    if any_name(&names, |k| k.starts_with("ANTIGRAVITY_")) {
        return "antigravity".to_string();
    }
    if any_name(&names, |k| {
        k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_")
    }) {
        return "claude".to_string();
    }
    // Tier 3: weak vendor prefixes.
    if any_name(&names, |k| k.starts_with("GEMINI_")) {
        return "antigravity".to_string();
    }
    if any_name(&names, |k| k.starts_with("CLAUDE")) {
        return "claude".to_string();
    }
    "other".to_string()
}

fn any_name(names: &[String], pred: impl Fn(&str) -> bool) -> bool {
    names.iter().any(|k| pred(k.as_str()))
}

/// Credential-shaped env var names (`GEMINI_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`)
/// are configuration, not evidence of a live tool session.
// trace:BUG-1920 | ai:claude
fn is_credential_var(key: &str) -> bool {
    key.ends_with("_KEY") || key.ends_with("_TOKEN")
}

pub(crate) fn normalize_agent_type(raw: String) -> String {
    aida_core::mailbox_identity::normalize_agent_type(&raw)
}

/// Canonical spec ids only: a lower-case `story-431` is a branch or session
/// name here, not a scope.
// trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude
fn looks_like_spec_id(s: &str) -> bool {
    crate::looks_like_canonical_spec_id(s)
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

// trace:STORY-994 | ai:codex
pub(crate) fn current_terminal_identity() -> Option<TerminalIdentity> {
    terminal_identity_for_tty(current_tty())
}

// trace:STORY-994 | ai:codex
pub(crate) fn terminal_identity_for_tty(tty: Option<String>) -> Option<TerminalIdentity> {
    terminal_identity_from_env(tty, std::env::vars())
}

// trace:STORY-994 | ai:codex
pub(crate) fn terminal_identity_from_env<I, K, V>(
    tty: Option<String>,
    vars: I,
) -> Option<TerminalIdentity>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let env: std::collections::BTreeMap<String, String> = vars
        .into_iter()
        .filter_map(|(k, v)| {
            let value = v.as_ref().trim();
            if value.is_empty() {
                None
            } else {
                Some((k.as_ref().to_string(), value.to_string()))
            }
        })
        .collect();
    let tmux_socket = env
        .get("TMUX")
        .and_then(|s| s.split(',').next())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let emulator = if env.contains_key("TERMINATOR_UUID") {
        Some("terminator".to_string())
    } else if env.contains_key("TMUX_PANE") || tmux_socket.is_some() {
        Some("tmux".to_string())
    } else if env.contains_key("WEZTERM_PANE") {
        Some("wezterm".to_string())
    } else if env.contains_key("KITTY_WINDOW_ID") {
        Some("kitty".to_string())
    } else if env.contains_key("WINDOWID") {
        Some("gnome-terminal".to_string())
    } else {
        None
    };
    let ident = TerminalIdentity {
        tty,
        terminator_uuid: env.get("TERMINATOR_UUID").cloned(),
        tmux_pane: env.get("TMUX_PANE").cloned(),
        tmux_socket,
        wezterm_pane: env.get("WEZTERM_PANE").cloned(),
        windowid: env.get("WINDOWID").cloned(),
        kitty_window_id: env.get("KITTY_WINDOW_ID").cloned(),
        emulator,
    };
    if ident.tty.is_none()
        && ident.terminator_uuid.is_none()
        && ident.tmux_pane.is_none()
        && ident.tmux_socket.is_none()
        && ident.wezterm_pane.is_none()
        && ident.windowid.is_none()
        && ident.kitty_window_id.is_none()
        && ident.emulator.is_none()
    {
        None
    } else {
        Some(ident)
    }
}

// trace:STORY-994 | ai:codex
pub(crate) fn short_tty(tty: &str) -> String {
    tty.strip_prefix("/dev/").unwrap_or(tty).to_string()
}

// trace:STORY-994 | ai:codex
pub(crate) fn terminal_cell(terminal: Option<&TerminalIdentity>, tty: Option<&str>) -> String {
    terminal
        .map(TerminalIdentity::short_cell)
        .or_else(|| tty.map(short_tty))
        .unwrap_or_else(|| "-".to_string())
}

// trace:STORY-994 | ai:codex
pub(crate) fn launch_title(role: &str, spec: Option<&str>, session_id: &str) -> String {
    let short_id: String = session_id.chars().take(8).collect();
    match spec.filter(|s| !s.trim().is_empty()) {
        Some(spec) => format!("aida · {role} · {spec} · {short_id}"),
        None => format!("aida · {role} · {short_id}"),
    }
}

// trace:STORY-994 | ai:codex
pub(crate) fn terminal_title_enabled(project_root: &Path) -> bool {
    let path = project_root.join(".aida").join("config.toml");
    let Ok(body) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(value) = body.parse::<toml::Value>() else {
        return true;
    };
    value
        .get("terminal")
        .and_then(|v| v.get("title"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

// trace:STORY-994 | ai:codex
pub(crate) fn apply_terminal_title(
    title: &str,
    terminal: Option<&TerminalIdentity>,
) -> TerminalTitleRestore {
    use std::io::Write;
    let Some(terminal) = terminal else {
        print!("{}", crate::statusline_cmd::osc_terminal_title(title));
        let _ = std::io::stdout().flush();
        return TerminalTitleRestore {
            terminal: None,
            previous_title: None,
        };
    };
    let previous_title = current_native_title(terminal);
    match terminal.emulator.as_deref() {
        Some("tmux") => {
            // BUG-1154: cosmetic best-effort title set — never let the child's
            // stdout/stderr leak into the terminal (e.g. a usage/error banner
            // from a tool-version mismatch). Same on wezterm/terminator below
            // and on the restore path. trace:BUG-1154 | ai:claude
            let _ = std::process::Command::new("tmux")
                .args(["rename-window", title])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Some("wezterm") => {
            let mut cmd = std::process::Command::new("wezterm");
            cmd.args(["cli", "set-tab-title", title]);
            if let Some(pane) = terminal.wezterm_pane.as_deref() {
                cmd.args(["--pane-id", pane]);
            }
            let _ = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        Some("terminator") => {
            if let Some(uuid) = terminal.terminator_uuid.as_deref() {
                // A remotinator that lacks the `set_tab_title` verb prints its
                // usage banner + "invalid choice" — swallow it. trace:BUG-1154
                let _ = std::process::Command::new("remotinator")
                    .args(["set_tab_title", uuid, title])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        _ => {
            print!("{}", crate::statusline_cmd::osc_terminal_title(title));
            let _ = std::io::stdout().flush();
        }
    }
    TerminalTitleRestore {
        terminal: Some(terminal.clone()),
        previous_title,
    }
}

// trace:STORY-994 | ai:codex
pub(crate) fn restore_terminal_title(restore: TerminalTitleRestore) {
    let Some(terminal) = restore.terminal.as_ref() else {
        use std::io::Write;
        print!("{}", crate::statusbar_cmd::RESTORE_TITLE);
        let _ = std::io::stdout().flush();
        return;
    };
    if let Some((program, args)) = native_restore_title_command(terminal, &restore.previous_title) {
        // BUG-1154: best-effort title restore — suppress child output too.
        // trace:BUG-1154 | ai:claude
        let _ = std::process::Command::new(program)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status_retrying_etxtbsy();
        return;
    }
    match terminal.emulator.as_deref() {
        Some("tmux") | Some("wezterm") | Some("terminator") => {
            return;
        }
        _ => {}
    }
    use std::io::Write;
    print!("{}", crate::statusbar_cmd::RESTORE_TITLE);
    let _ = std::io::stdout().flush();
}

// trace:STORY-994 | ai:codex
fn native_restore_title_command(
    terminal: &TerminalIdentity,
    previous_title: &Option<String>,
) -> Option<(String, Vec<String>)> {
    let previous = previous_title.as_deref()?;
    match terminal.emulator.as_deref() {
        Some("tmux") => Some((
            "tmux".to_string(),
            vec!["rename-window".to_string(), previous.to_string()],
        )),
        Some("wezterm") => {
            let mut args = vec![
                "cli".to_string(),
                "set-tab-title".to_string(),
                previous.to_string(),
            ];
            if let Some(pane) = terminal.wezterm_pane.as_deref() {
                args.extend(["--pane-id".to_string(), pane.to_string()]);
            }
            Some(("wezterm".to_string(), args))
        }
        Some("terminator") => terminal.terminator_uuid.as_deref().map(|uuid| {
            (
                "remotinator".to_string(),
                vec![
                    "set_tab_title".to_string(),
                    uuid.to_string(),
                    previous.to_string(),
                ],
            )
        }),
        _ => None,
    }
}

// trace:STORY-994 | ai:codex
fn current_native_title(terminal: &TerminalIdentity) -> Option<String> {
    match terminal.emulator.as_deref() {
        Some("tmux") => command_output_trim("tmux", &["display-message", "-p", "#W"]),
        Some("wezterm") => {
            let mut args = vec!["cli", "get-tab-title"];
            if let Some(pane) = terminal.wezterm_pane.as_deref() {
                args.extend(["--pane-id", pane]);
            }
            command_output_trim("wezterm", &args)
        }
        Some("terminator") => terminal
            .terminator_uuid
            .as_deref()
            .and_then(|uuid| command_output_trim("remotinator", &["get_tab_title", uuid])),
        _ => None,
    }
}

fn command_output_trim(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output_retrying_etxtbsy()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(unix)]
fn current_tty() -> Option<String> {
    std::fs::read_link("/proc/self/fd/0")
        .ok()
        .map(|p| p.display().to_string())
        .filter(|s| s.starts_with("/dev/"))
}

#[cfg(unix)]
fn process_tty(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/fd/0"))
        .ok()
        .map(|p| p.display().to_string())
        .filter(|s| s.starts_with("/dev/"))
}

#[cfg(unix)]
// trace:STORY-791 | ai:codex
fn pid_has_ancestor(mut child_pid: u32, ancestor_pid: u32) -> bool {
    while let Some(parent) = parent_pid(child_pid) {
        if parent == ancestor_pid {
            return true;
        }
        if parent == 0 || parent == child_pid {
            return false;
        }
        child_pid = parent;
    }
    false
}

#[cfg(unix)]
fn parent_pid(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(") ")?.1;
        after_comm.split_whitespace().nth(1)?.parse().ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        // trace:BUG-1021 | ai:codex
        let out = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
}

#[cfg(unix)]
fn role_enter_shell_pid() -> u32 {
    parent_pid(std::process::id()).unwrap_or_else(std::process::id)
}

#[cfg(not(unix))]
fn role_enter_shell_pid() -> u32 {
    std::process::id()
}

#[cfg(unix)]
fn process_cmdline(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = raw
        .split(|b| *b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part))
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(unix)]
pub(crate) fn session_start_agent_pid(child_pid: u32, agent_type: &str) -> Option<u32> {
    #[cfg(test)]
    if hermetic_ancestry_seam::active() {
        return None;
    }
    session_start_agent_pid_in(child_pid, agent_type, parent_pid, process_cmdline)
}

/// Test-only: makes this thread's vendor-ancestor walk see no agent client,
/// as on a CI runner, so a test run from inside a live agent session cannot
/// resolve that session's registry binding. Production has no such switch.
/// Use through [`crate::test_env::AgentSessionIsolation`].
// trace:TASK-2349 trace:TASK-2345 | ai:claude
#[cfg(test)]
pub(crate) mod hermetic_ancestry_seam {
    use std::cell::Cell;

    thread_local! {
        static ACTIVE: Cell<bool> = const { Cell::new(false) };
    }

    pub(crate) fn active() -> bool {
        ACTIVE.with(Cell::get)
    }

    /// Active until the guard drops, restoring the previous state.
    pub(crate) struct Guard(bool);

    pub(crate) fn enter() -> Guard {
        Guard(ACTIVE.with(|a| a.replace(true)))
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.with(|a| a.set(self.0));
        }
    }
}

/// The nearest ancestor of `child_pid` whose cmdline is the agent's, over an
/// injectable process tree so the walk is testable without `/proc`.
// trace:TASK-2272 | ai:claude
#[cfg(unix)]
fn session_start_agent_pid_in(
    mut child_pid: u32,
    agent_type: &str,
    parent_of: impl Fn(u32) -> Option<u32>,
    cmdline_of: impl Fn(u32) -> Option<String>,
) -> Option<u32> {
    while let Some(parent) = parent_of(child_pid) {
        if parent == 0 || parent == child_pid {
            return None;
        }
        if cmdline_of(parent).is_some_and(|cmd| cmdline_matches_agent(&cmd, agent_type)) {
            return Some(parent);
        }
        child_pid = parent;
    }
    None
}

#[cfg(unix)]
fn cmdline_matches_agent(cmdline: &str, agent_type: &str) -> bool {
    // Only the executable or an interpreter's script identifies a client.
    // Hook arguments (`--client agy`) and prompt text are not processes.
    // trace:STORY-1843.ac0eef56 | ai:codex
    let mut args = cmdline.split_whitespace();
    let Some(program) = args.next() else {
        return false;
    };
    let basename = |path: &str| {
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let program_name = basename(program);
    let interpreted = matches!(
        program_name.as_str(),
        "node" | "nodejs" | "bun" | "deno" | "sh" | "bash" | "dash" | "zsh" | "ksh"
    ) || program_name.starts_with("python");
    let candidate = if interpreted {
        let mut script = None;
        for arg in args {
            if matches!(arg, "-c" | "-lc" | "-e" | "--eval" | "--command") {
                return false;
            }
            if !arg.starts_with('-') {
                script = Some(arg);
                break;
            }
        }
        let Some(script) = script else {
            return false;
        };
        script
    } else {
        program
    };
    let lower = candidate.to_ascii_lowercase();
    let name = basename(candidate);
    match agent_type {
        "claude" => {
            name == "claude"
                || lower.contains("claude-code")
                || lower.contains("@anthropic-ai/claude")
        }
        "codex" => name == "codex",
        "antigravity" => {
            name == "agy"
                || name == "gemini"
                || lower.contains("antigravity")
                || lower.contains("gemini-cli")
        }
        _ => false,
    }
}

#[cfg(not(unix))]
pub(crate) fn session_start_agent_pid(_child_pid: u32, _agent_type: &str) -> Option<u32> {
    None
}

#[cfg(not(unix))]
fn pid_has_ancestor(_child_pid: u32, _ancestor_pid: u32) -> bool {
    false
}

/// The parent of `pid`, when this process can see it.
// trace:STORY-1843 | ai:claude
pub(crate) fn parent_pid_of(pid: u32) -> Option<u32> {
    #[cfg(unix)]
    {
        parent_pid(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// How many generations `ancestor` sits above `pid` (0 when they are the
/// same process), or `None` when it is not an ancestor.
// trace:STORY-1843 | ai:claude
pub(crate) fn ancestor_depth(pid: u32, ancestor: u32) -> Option<usize> {
    let mut current = pid;
    for depth in 0..64 {
        if current == ancestor {
            return Some(depth);
        }
        match parent_pid_of(current) {
            Some(parent) if parent != current && parent > 0 => current = parent,
            _ => return None,
        }
    }
    None
}

#[cfg(not(unix))]
fn current_tty() -> Option<String> {
    None
}

#[cfg(not(unix))]
fn process_tty(_pid: u32) -> Option<String> {
    None
}

// trace:TASK-3787 | ai:codex
#[cfg(test)]
#[path = "tests/agent_registry_inline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/bug_b6b_151_registry_heartbeat_tests.rs"]
mod bug_151_heartbeat_tests;

// trace:TASK-3784 | ai:codex
#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "tests/story_2745_seat_registration_tests.rs"]
mod story_2745_seat_registration_tests;
