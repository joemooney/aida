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

use crate::leases::SessionLease;
use crate::process_retry::RetryEtxtbsy;
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
pub(crate) struct AgentRegistryEntry {
    pub(crate) id: String,
    pub(crate) agent_type: String,
    pub(crate) pid: u32,
    /// Kernel start time of `pid`, so a recycled pid does not read as the
    /// live agent. Missing on entries written by older binaries; liveness
    /// then degrades to a bare pid probe.
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
pub(crate) struct AgentBinaryIdentity {
    pub(crate) version: String,
    pub(crate) sha: String,
}

impl AgentBinaryIdentity {
    pub(crate) fn new(version: String, sha: String) -> Self {
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

fn registry_path(project_root: &Path, id: &str) -> PathBuf {
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
            && registry_entry_is_live(project_root, entry)
            && pid_has_ancestor(child_pid, entry.pid)
    })
}

pub(crate) fn touch_mcp_agent(
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

    let mut entry = existing_launcher
        .map(|(_, entry)| entry)
        .or_else(|| {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
        })
        .unwrap_or_else(|| AgentRegistryEntry {
            id: id.clone(),
            agent_type: agent_type.clone(),
            pid,
            pid_start: crate::process_probe::process_start_identity(pid),
            pid_namespace: current_pid_namespace(),
            name: None,
            description: None,
            tty: current_tty(),
            terminal: current_terminal_identity(),
            started_at: now,
            last_active_at: now,
            role: None,
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
            resumed_from: None,
        });

    if entry.source != "agent-launcher" {
        entry.id = id;
        entry.agent_type = agent_type;
        if entry.pid != pid || entry.pid_start.is_none() {
            entry.pid_start = crate::process_probe::process_start_identity(pid);
            entry.pid_namespace = current_pid_namespace();
        }
        entry.pid = pid;
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
fn lease_linked_to_session(
    project_root: &Path,
    session_id: &str,
) -> Option<(SessionLease, Option<String>)> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }
    let linked: Vec<(String, Option<String>)> = crate::session_manifest::list_all(project_root)
        .into_iter()
        .filter(|m| {
            m.claude_session_id
                .as_deref()
                .is_some_and(|sid| !sid.is_empty() && session_id.starts_with(sid))
        })
        .map(|m| (m.session_id, m.agent_name))
        .collect();
    crate::leases::list_leases(project_root)
        .into_iter()
        .find_map(|l| {
            let name = linked.iter().find(|(id, _)| *id == l.id)?.1.clone();
            Some((l, name))
        })
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
    let now = Utc::now();
    let hook_pid = std::process::id();
    let agent_type = detect_agent_type();
    let pid = session_start_agent_pid(hook_pid, &agent_type).unwrap_or(hook_pid);
    let id = agent_id(&agent_type, pid);
    let path = registry_path(project_root, &id);
    let session_id = session_id.filter(|s| !s.trim().is_empty());
    let current_start = crate::process_probe::process_start_identity(pid);
    let linked = session_id.and_then(|sid| lease_linked_to_session(project_root, sid));
    let (linked, launch_name) = match linked {
        Some((lease, name)) => (Some(lease), name),
        None => (None, None),
    };
    let env_scope = env_nonempty("AIDA_SESSION_SCOPE").filter(|s| looks_like_spec_id(s));
    let env_name = match &linked {
        Some(lease) if env_scope.as_deref() != Some(lease.scope.as_str()) => None,
        // A headless child's own handle, else its launch name.
        // trace:STORY-1843 | ai:claude
        _ => env_nonempty(crate::agent_handle::HANDLE_ENV)
            .or_else(|| env_nonempty("AIDA_AGENT_NAME")),
    };
    let mut entry = std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
        // A recycled pid's entry belongs to another process: keep it only
        // while its recorded start identity is this process's. A live
        // process that starts a new session (`/clear`, a new conversation)
        // keeps its launcher entry. Without a recorded start, fall back to
        // the session id. trace:TASK-1955 | ai:claude (BUG-2167)
        .filter(
            |old| match (old.pid_start.as_deref(), current_start.as_deref()) {
                (Some(recorded), Some(current)) => recorded == current,
                _ => match (session_id, old.claude_session_id.as_deref()) {
                    (Some(new), Some(old)) => new == old,
                    _ => true,
                },
            },
        )
        .unwrap_or_else(|| AgentRegistryEntry {
            id: id.clone(),
            agent_type: agent_type.clone(),
            pid,
            pid_start: crate::process_probe::process_start_identity(pid),
            pid_namespace: current_pid_namespace(),
            name: env_name.clone(),
            description: None,
            tty: process_tty(pid).or_else(current_tty),
            terminal: None,
            started_at: now,
            last_active_at: now,
            role: None,
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
            resumed_from: None,
        });

    entry.id = id;
    entry.agent_type = agent_type;
    if entry.pid != pid || entry.pid_start.is_none() {
        entry.pid_start = crate::process_probe::process_start_identity(pid);
        entry.pid_namespace = current_pid_namespace();
    }
    entry.pid = pid;
    entry.last_active_at = now;
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

    write_entry(project_root, &entry)?;
    Ok(entry)
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
    let now = Utc::now();
    let agent_type = normalize_agent_type(agent_type.to_string());
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start: crate::process_probe::process_start_identity(pid),
        pid_namespace: current_pid_namespace(),
        name,
        description,
        tty: current_tty(),
        terminal: current_terminal_identity(),
        started_at: now,
        last_active_at: now,
        role,
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
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        resumed_from,
    };
    write_entry(project_root, &entry)?;
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
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start: crate::process_probe::process_start_identity(pid),
        pid_namespace: current_pid_namespace(),
        name: Some(format!("role:{role}")),
        description: Some("aida role enter shell".to_string()),
        tty,
        terminal,
        started_at: now,
        last_active_at: now,
        role: Some(role.to_string()),
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
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        pid_start: crate::process_probe::process_start_identity(pid),
        pid_namespace: current_pid_namespace(),
        name,
        description,
        tty,
        terminal,
        started_at: now,
        last_active_at: now,
        role: Some(role),
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
    match role.unwrap_or("").to_ascii_lowercase().as_str() {
        "advisor" | "product" | "integrator" => "repo".to_string(),
        _ => format!("worktree:{}", worktree_path.display()),
    }
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
    let mut entry: AgentRegistryEntry =
        toml::from_str(&body).with_context(|| format!("parsing {}", path.display()))?;
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
    write_entry(project_root, &entry)?;
    Ok(Some(entry))
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

    let live_entries = load_live_entries(project_root);
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
        if !crate::process_probe::process_identity_is_alive(agent.pid, Some(start_time.as_str())) {
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
            let advisor = crate::advisor_authority_from(server_role, false, false);
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
    let path = registry_path(project_root, &entry.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating agent registry dir {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(entry).context("serialising agent registry entry")?;
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
    AgentRegistryView {
        id: entry.id,
        agent_type: entry.agent_type,
        pid: entry.pid,
        pid_start: entry.pid_start,
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
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self/ns/pid").ok().map(|m| m.ino())
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
    observer
        .can_observe(entry.pid_namespace)
        .then(|| registry_entry_is_live(project_root, entry))
}

/// Inode of Linux's initial PID namespace.
// trace:STORY-1843 | ai:claude
pub(crate) const INITIAL_PID_NAMESPACE: u64 = 0xEFFF_FFFC;

/// How long a row whose process this observer cannot see stays in a human
/// LISTING, from its last heartbeat. Display only: guards never apply a
/// heartbeat bound, because session-start-hook rows heartbeat exactly once
/// (at SessionStart), so any finite bound makes every live host session look
/// stale from a sandbox eventually — the guard must fail closed instead.
// trace:BUG-2212 | ai:claude
// trace:TASK-2272 | ai:claude
pub(crate) const UNOBSERVED_HEARTBEAT_TTL_SECS: i64 = 24 * 60 * 60;

/// Raw `/proc` facts that say whether this process sits in a PID namespace
/// of its own (a sandboxed shell sees only its own few pids).
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PidNamespaceProbe {
    pub(crate) self_ns: Option<u64>,
    /// `None` when `/proc/1/ns/pid` is unreadable (pid 1 owned by another user).
    pub(crate) pid1_ns: Option<u64>,
    /// Entries on the `NSpid:` line of `/proc/self/status`.
    pub(crate) nspid_levels: usize,
    pub(crate) pid1_comm: Option<String>,
}

impl PidNamespaceProbe {
    pub(crate) fn read() -> Self {
        use std::os::unix::fs::MetadataExt;
        let ns = |path: &str| std::fs::metadata(path).ok().map(|m| m.ino());
        let nspid_levels = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|body| {
                body.lines()
                    .find_map(|l| l.strip_prefix("NSpid:"))
                    .map(|rest| rest.split_whitespace().count())
            })
            .unwrap_or(0);
        Self {
            self_ns: ns("/proc/self/ns/pid"),
            pid1_ns: ns("/proc/1/ns/pid"),
            nspid_levels,
            pid1_comm: std::fs::read_to_string("/proc/1/comm")
                .ok()
                .map(|c| c.trim().to_string()),
        }
    }

    /// Isolated when pid 1 lives in another namespace or the kernel reports
    /// more than one pid level. Only when `/proc/1/ns/pid` is unreadable does
    /// pid 1's name decide: anything but a known init means a sandbox.
    // trace:TASK-2272 | ai:claude
    pub(crate) fn is_isolated(&self) -> bool {
        const INITS: [&str; 10] = [
            "systemd",
            "init",
            "tini",
            "dumb-init",
            "docker-init",
            "catatonit",
            "runit",
            "openrc-init",
            "s6-svscan",
            "busybox",
        ];
        match (self.self_ns, self.pid1_ns) {
            (Some(a), Some(b)) if a != b => return true,
            _ => {}
        }
        if self.nspid_levels > 1 {
            return true;
        }
        if self.pid1_ns.is_some() {
            return false;
        }
        self.pid1_comm
            .as_deref()
            .is_some_and(|comm| !INITS.contains(&comm))
    }
}

/// The PID view a liveness check runs from: which namespace this process is
/// in, and whether that namespace is an isolated sandbox.
// trace:BUG-2212 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PidObserver {
    pub(crate) namespace: Option<u64>,
    pub(crate) isolated: bool,
}

impl PidObserver {
    pub(crate) fn current() -> Self {
        Self::from_probe(&PidNamespaceProbe::read())
    }

    pub(crate) fn from_probe(probe: &PidNamespaceProbe) -> Self {
        Self {
            namespace: probe.self_ns,
            isolated: probe.is_isolated(),
        }
    }

    /// A row's process is visible only from the namespace it was stamped in;
    /// an unstamped legacy row only from a non-isolated initial namespace.
    pub(crate) fn can_observe(&self, recorded: Option<u64>) -> bool {
        match (recorded, self.namespace) {
            (Some(recorded), Some(current)) => recorded == current,
            (None, Some(INITIAL_PID_NAMESPACE)) => !self.isolated,
            _ => false,
        }
    }
}

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
    if observer.can_observe(entry.pid_namespace) {
        return (
            registry_entry_is_live(project_root, entry),
            LivenessBasis::Process,
        );
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
    crate::process_probe::process_identity_is_alive(entry.pid, entry.pid_start.as_deref())
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
                && entry_process_alive(&entry)
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
    match raw.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "claude" | "claudecode" => "claude".to_string(),
        "codex" => "codex".to_string(),
        "antigravity" | "gemini" => "antigravity".to_string(),
        "shell" => "shell".to_string(),
        "web" => "web".to_string(),
        _ => "other".to_string(),
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn looks_like_spec_id(s: &str) -> bool {
    let Some((prefix, number)) = s.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_uppercase())
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit())
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    fn entry_with(pid: u32, last_active_at: DateTime<Utc>) -> AgentRegistryEntry {
        AgentRegistryEntry {
            id: agent_id("codex", pid),
            agent_type: "codex".to_string(),
            pid,
            pid_start: None,
            pid_namespace: current_pid_namespace(),
            name: None,
            description: None,
            tty: Some("/dev/pts/1".to_string()),
            terminal: None,
            started_at: last_active_at,
            last_active_at,
            role: Some("implementer".to_string()),
            current_spec: None,
            worktree_path: PathBuf::from("/tmp/aida-story"),
            source: "mcp".to_string(),
            binary_version: Some("0.9.1".to_string()),
            build_sha: Some("abc123".to_string()),
            availability: Availability::Available,
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            claude_session_id: None,
            ended_at: None,
            spec_status_at_end: None,
            resumed_from: None,
        }
    }

    // STORY-1420: a named seat is live only while an un-ended entry with a
    // live pid carries its name; a dead pid or an `ended_at` reads as exited;
    // a name the registry never saw is unknown.
    // trace:STORY-1420 | ai:claude
    #[test]
    fn named_agent_liveness_reads_dead_pid_and_ended_at_as_exited() {
        let tmp = TempDir::new().unwrap();
        let mut live = entry_with(std::process::id(), Utc::now());
        live.id = "live".to_string();
        live.name = Some("Reviewer-Live".to_string());
        write_entry(tmp.path(), &live).unwrap();

        let mut dead = entry_with(u32::MAX - 1, Utc::now());
        dead.id = "dead".to_string();
        dead.name = Some("reviewer-dead".to_string());
        write_entry(tmp.path(), &dead).unwrap();

        let mut ended = entry_with(std::process::id(), Utc::now());
        ended.id = "ended".to_string();
        ended.name = Some("reviewer-ended".to_string());
        ended.ended_at = Some(Utc::now());
        write_entry(tmp.path(), &ended).unwrap();

        assert_eq!(
            named_agent_liveness(tmp.path(), "reviewer-live"),
            Some(true)
        );
        assert_eq!(
            named_agent_liveness(tmp.path(), "reviewer-dead"),
            Some(false)
        );
        assert_eq!(
            named_agent_liveness(tmp.path(), "reviewer-ended"),
            Some(false)
        );
        assert_eq!(named_agent_liveness(tmp.path(), "never-registered"), None);
    }

    #[test]
    fn terminal_identity_captures_known_emulator_env() {
        let terminator = terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("TERMINATOR_UUID", "term-1")],
        )
        .unwrap();
        assert_eq!(terminator.emulator.as_deref(), Some("terminator"));
        assert_eq!(terminator.terminator_uuid.as_deref(), Some("term-1"));

        let tmux = terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("TMUX_PANE", "%3"), ("TMUX", "/tmp/tmux.sock,1,0")],
        )
        .unwrap();
        assert_eq!(tmux.emulator.as_deref(), Some("tmux"));
        assert_eq!(tmux.tmux_pane.as_deref(), Some("%3"));
        assert_eq!(tmux.tmux_socket.as_deref(), Some("/tmp/tmux.sock"));

        for (vars, emulator) in [
            (vec![("WEZTERM_PANE", "12")], "wezterm"),
            (vec![("KITTY_WINDOW_ID", "9")], "kitty"),
            (vec![("WINDOWID", "123")], "gnome-terminal"),
        ] {
            let ident = terminal_identity_from_env(Some("/dev/pts/4".to_string()), vars).unwrap();
            assert_eq!(ident.tty.as_deref(), Some("/dev/pts/4"));
            assert_eq!(ident.emulator.as_deref(), Some(emulator));
            match emulator {
                "wezterm" => assert_eq!(ident.wezterm_pane.as_deref(), Some("12")),
                "kitty" => assert_eq!(ident.kitty_window_id.as_deref(), Some("9")),
                "gnome-terminal" => assert_eq!(ident.windowid.as_deref(), Some("123")),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn terminal_identity_with_missing_env_is_only_tty() {
        let ident =
            terminal_identity_from_env(Some("/dev/pts/9".to_string()), Vec::<(&str, &str)>::new())
                .unwrap();
        assert_eq!(ident.tty.as_deref(), Some("/dev/pts/9"));
        assert_eq!(ident.emulator, None);
        assert_eq!(ident.terminator_uuid, None);
        assert_eq!(ident.tmux_pane, None);
        assert_eq!(ident.wezterm_pane, None);
        assert_eq!(ident.windowid, None);
        assert_eq!(ident.kitty_window_id, None);
    }

    #[test]
    fn launch_title_includes_role_optional_spec_and_short_id() {
        assert_eq!(
            launch_title("implementer", Some("STORY-993"), "01a079fe-dead-beef"),
            "aida · implementer · STORY-993 · 01a079fe"
        );
        assert_eq!(
            launch_title("advisor", None, "abcdef012345"),
            "aida · advisor · abcdef01"
        );
    }

    #[test]
    fn native_restore_title_command_restores_supported_emulators() {
        let previous = Some("previous title".to_string());

        let tmux = terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("TMUX_PANE", "%3"), ("TMUX", "/tmp/tmux.sock,1,0")],
        )
        .unwrap();
        assert_eq!(
            native_restore_title_command(&tmux, &previous),
            Some((
                "tmux".to_string(),
                vec!["rename-window".to_string(), "previous title".to_string()]
            ))
        );

        let wezterm = terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("WEZTERM_PANE", "12")],
        )
        .unwrap();
        assert_eq!(
            native_restore_title_command(&wezterm, &previous),
            Some((
                "wezterm".to_string(),
                vec![
                    "cli".to_string(),
                    "set-tab-title".to_string(),
                    "previous title".to_string(),
                    "--pane-id".to_string(),
                    "12".to_string()
                ]
            ))
        );

        let terminator = terminal_identity_from_env(
            Some("/dev/pts/4".to_string()),
            vec![("TERMINATOR_UUID", "term-994")],
        )
        .unwrap();
        assert_eq!(
            native_restore_title_command(&terminator, &previous),
            Some((
                "remotinator".to_string(),
                vec![
                    "set_tab_title".to_string(),
                    "term-994".to_string(),
                    "previous title".to_string()
                ]
            ))
        );

        assert_eq!(native_restore_title_command(&tmux, &None), None);
    }

    // `cmdline_matches_agent` reads /proc-style cmdlines and is unix-gated.
    #[cfg(unix)]
    #[test]
    fn session_start_cmdline_match_detects_agent_binary() {
        assert!(cmdline_matches_agent(
            "/home/joe/.local/bin/codex --cd /repo",
            "codex"
        ));
        assert!(cmdline_matches_agent(
            "/usr/bin/node /usr/local/bin/claude",
            "claude"
        ));
        assert!(cmdline_matches_agent(
            "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            "claude"
        ));
        assert!(cmdline_matches_agent("gemini --model pro", "antigravity"));
        // trace:TASK-2272 | ai:claude
        assert!(cmdline_matches_agent(
            "/home/joe/.local/bin/agy --resume",
            "antigravity"
        ));
        assert!(!cmdline_matches_agent(
            "aida awaiting --notice",
            "antigravity"
        ));
        assert!(!cmdline_matches_agent(
            "bash .claude/hooks/aida-mail-notice.sh",
            "claude"
        ));
    }

    // trace:STORY-1843.ac0eef56 | ai:codex
    #[cfg(unix)]
    #[test]
    fn story_1843_pr3_hook_arguments_do_not_identify_a_client() {
        let tree: std::collections::HashMap<u32, (u32, &str)> = [
            (400, (300, "aida awaiting --notice")),
            (300, (200, "sh /repo/.agents/hooks/aida-mail-notice.sh --client agy --event SessionStart")),
            (200, (100, "sh -c h=/repo/.agents/hooks/aida-mail-notice.sh; $h --client agy --event SessionStart")),
            (100, (1, "/home/joe/.local/bin/agy --prompt claude")),
            (1, (0, "/sbin/init")),
        ].into_iter().collect();
        assert_eq!(
            session_start_agent_pid_in(
                400,
                "antigravity",
                |pid| tree.get(&pid).map(|(p, _)| *p),
                |pid| tree.get(&pid).map(|(_, command)| command.to_string())
            ),
            Some(100)
        );
        assert!(!cmdline_matches_agent("agy --prompt claude", "claude"));
        assert!(!cmdline_matches_agent(
            "python3 /fixture/claude agy",
            "antigravity"
        ));
        assert!(cmdline_matches_agent("python3 /fixture/agy", "antigravity"));
    }

    // TASK-2272 (TASK-2282, TASK-1-463): the Antigravity SessionStart hook
    // runs as `aida awaiting --notice` under a hook shell under `agy`. The
    // registry row must name the `agy` process, not the short-lived hook.
    // trace:TASK-2272 | ai:claude
    #[cfg(unix)]
    #[test]
    fn session_start_agent_pid_finds_the_agy_ancestor_of_the_hook() {
        let tree: std::collections::HashMap<u32, (u32, &str)> = [
            (300, (200, "aida awaiting --notice")),
            (200, (100, "sh -c aida awaiting --notice")),
            (100, (1, "/home/joe/.local/bin/agy")),
            (1, (0, "/sbin/init")),
        ]
        .into_iter()
        .collect();
        let parent_of = |pid: u32| tree.get(&pid).map(|(p, _)| *p);
        let cmdline_of = |pid: u32| tree.get(&pid).map(|(_, c)| c.to_string());
        assert_eq!(
            session_start_agent_pid_in(300, "antigravity", parent_of, cmdline_of),
            Some(100)
        );
        // No agent ancestor: None, so the caller falls back to the hook pid.
        assert_eq!(
            session_start_agent_pid_in(300, "codex", parent_of, cmdline_of),
            None
        );
    }

    #[test]
    fn session_start_hook_touch_writes_terminal_block() {
        let tmp = TempDir::new().unwrap();
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "codex"),
            ("AIDA_SESSION_ROLE", "advisor"),
            ("AIDA_SESSION_SCOPE", "STORY-994"),
            ("TERMINATOR_UUID", "term-994"),
        ]);

        let binary = AgentBinaryIdentity::new("0.14.0".into(), "abc123".into());
        let entry =
            touch_session_start_agent(tmp.path(), tmp.path(), Some("claude-sid"), &binary).unwrap();

        assert_eq!(entry.agent_type, "codex");
        assert_eq!(entry.role.as_deref(), Some("advisor"));
        assert_eq!(entry.current_spec.as_deref(), Some("STORY-994"));
        assert_eq!(entry.claude_session_id.as_deref(), Some("claude-sid"));
        let terminal = entry.terminal.as_ref().expect("terminal block");
        assert_eq!(terminal.emulator.as_deref(), Some("terminator"));
        assert_eq!(terminal.terminator_uuid.as_deref(), Some("term-994"));
    }

    /// Write lease `id` (scope `scope`, worktree `worktree`) and its
    /// manifest linking Claude session `sid`, as `agent new --bg` does.
    // trace:TASK-1955 | ai:claude (BUG-2167)
    fn bug_2167_linked_lease(root: &Path, id: &str, scope: &str, worktree: &Path, sid: &str) {
        let dir = root.join(".aida").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"s\"\nowner = \"t\"\nworktree_path = '{}'\nbranch = \"b\"\nstarted_at = \"2026-10-20T00:00:00Z\"\nhostname = \"h\"\nrole = \"implementer\"\n",
                worktree.display()
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join(format!("{id}.manifest.toml")),
            format!(
                "session_id = \"{id}\"\nplanned_at = \"2026-10-20T00:00:00Z\"\nplan_source = \"agent new --bg\"\nclaude_session_id = \"{sid}\"\nitems = []\n"
            ),
        )
        .unwrap();
    }

    /// BUG-2167: a `--bg` session in a recycled spare carrying an earlier
    /// launch's env is registered from the lease linked to its own session
    /// id (spec, role, worktree), and the stale env name is not used.
    // trace:TASK-1955 | ai:claude (BUG-2167)
    #[test]
    fn bg_session_start_uses_the_linked_lease_not_the_spares_stale_env() {
        let tmp = TempDir::new().unwrap();
        let pool = tmp.path().join("pool-7");
        bug_2167_linked_lease(tmp.path(), "leaseb00000001", "TASK-2000", &pool, "abcd1234");
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "claude"),
            ("AIDA_AGENT_NAME", "fix-task-1784"),
            ("AIDA_SESSION_SCOPE", "TASK-1784"),
        ]);
        let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        let entry = touch_session_start_agent(
            tmp.path(),
            tmp.path(),
            Some("abcd1234-0000-4000-8000-000000000000"),
            &binary,
        )
        .unwrap();
        assert_eq!(entry.current_spec.as_deref(), Some("TASK-2000"));
        assert_eq!(entry.worktree_path, pool);
        assert_eq!(entry.role.as_deref(), Some("implementer"));
        assert_ne!(entry.name.as_deref(), Some("fix-task-1784"));

        // The launcher then names it from its own plan.
        let n = adopt_bg_session_identity(
            tmp.path(),
            "abcd1234",
            "rebase-2673",
            Some("TASK-2000"),
            Some("implementer"),
            &pool,
        );
        assert_eq!(n, 1);
        let views: Vec<_> = load_entries(tmp.path())
            .into_iter()
            .map(|(_, e)| e)
            .collect();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].name.as_deref(), Some("rebase-2673"));
    }

    /// TASK-2102: a SessionStart hook that fires AFTER `link_bg_session`
    /// saved the manifest names the row from the launch, even when the
    /// spare's env contradicts the lease and its pid already had an entry
    /// with this start identity carrying a stale name.
    // trace:TASK-1955 | ai:claude (TASK-2102)
    #[test]
    fn bg_session_start_after_the_link_takes_the_launch_name() {
        let tmp = TempDir::new().unwrap();
        let pool = tmp.path().join("pool-8");
        bug_2167_linked_lease(tmp.path(), "leaseb00000002", "TASK-2001", &pool, "feed5678");
        let manifest = tmp
            .path()
            .join(".aida")
            .join("sessions")
            .join("leaseb00000002.manifest.toml");
        let body = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            body.replace("items = []", "agent_name = \"rebase-2678\"\nitems = []"),
        )
        .unwrap();
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "claude"),
            ("AIDA_AGENT_NAME", "fix-task-1784"),
            ("AIDA_SESSION_SCOPE", "TASK-1784"),
        ]);
        let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        let sid = "feed5678-0000-4000-8000-000000000000";
        // The spare's first session already left a same-start entry named
        // from its stale env.
        let first = touch_session_start_agent(tmp.path(), tmp.path(), Some(sid), &binary).unwrap();
        let mut stale = first.clone();
        stale.name = Some("fix-task-1784".into());
        write_entry(tmp.path(), &stale).unwrap();

        let entry = touch_session_start_agent(tmp.path(), tmp.path(), Some(sid), &binary).unwrap();
        assert_eq!(entry.name.as_deref(), Some("rebase-2678"));
        assert_eq!(entry.current_spec.as_deref(), Some("TASK-2001"));
        assert_eq!(first.name.as_deref(), Some("rebase-2678"));
    }

    /// BUG-2167: a pid's entry left by an earlier session never lends its
    /// name to the next session on that pid; an unlinked hook records its
    /// own worktree, not the main checkout.
    // trace:TASK-1955 | ai:claude (BUG-2167)
    #[test]
    fn recycled_pid_session_start_does_not_inherit_the_earlier_name() {
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("wt");
        let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        {
            let _env = crate::test_env::EnvVarsGuard::set(&[
                ("AIDA_AGENT_TYPE", "codex"),
                ("AIDA_AGENT_NAME", "first-seat"),
            ]);
            let mut old =
                touch_session_start_agent(tmp.path(), &worktree, Some("sid-one"), &binary).unwrap();
            // The entry an earlier process with this pid left behind.
            old.pid_start = Some("linux-starttime:1".into());
            write_entry(tmp.path(), &old).unwrap();
        }
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "codex"),
            ("AIDA_AGENT_NAME", "second-seat"),
        ]);
        let entry =
            touch_session_start_agent(tmp.path(), &worktree, Some("sid-two"), &binary).unwrap();
        assert_eq!(entry.name.as_deref(), Some("second-seat"));
        assert_eq!(entry.worktree_path, worktree);
    }

    /// PR-2809 r1: the same live process starting a new session (`/clear`,
    /// a new conversation) keeps its `agent new` launcher entry.
    // trace:TASK-1955 | ai:claude (BUG-2167)
    #[test]
    fn same_process_new_session_keeps_its_launcher_entry() {
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("wt");
        let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "codex"),
            ("AIDA_AGENT_NAME", "lane-7"),
        ]);
        let mut launched =
            touch_session_start_agent(tmp.path(), &worktree, Some("sid-one"), &binary).unwrap();
        launched.source = "agent-launcher".into();
        launched.description = Some("lane seven".into());
        write_entry(tmp.path(), &launched).unwrap();

        let entry =
            touch_session_start_agent(tmp.path(), &worktree, Some("sid-two"), &binary).unwrap();
        assert_eq!(entry.source, "agent-launcher");
        assert_eq!(entry.name.as_deref(), Some("lane-7"));
        assert_eq!(entry.description.as_deref(), Some("lane seven"));
        assert_eq!(entry.claude_session_id.as_deref(), Some("sid-two"));
    }

    // trace:STORY-994 | ai:codex
    #[test]
    fn role_enter_registration_writes_terminal_block() {
        let tmp = TempDir::new().unwrap();
        let worktree = tmp.path().join("worktree");
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_SESSION_SCOPE", "STORY-994"),
            ("TERMINATOR_UUID", "role-term-994"),
        ]);

        let entry = register_role_enter_agent(tmp.path(), "advisor", worktree.clone()).unwrap();

        assert_eq!(entry.agent_type, "shell");
        assert_eq!(entry.source, "role-enter");
        assert_eq!(entry.role.as_deref(), Some("advisor"));
        assert_eq!(entry.current_spec.as_deref(), Some("STORY-994"));
        assert_eq!(entry.worktree_path, worktree);
        let terminal = entry.terminal.as_ref().expect("terminal block");
        assert_eq!(terminal.emulator.as_deref(), Some("terminator"));
        assert_eq!(terminal.terminator_uuid.as_deref(), Some("role-term-994"));

        let persisted = std::fs::read_to_string(registry_path(tmp.path(), &entry.id)).unwrap();
        assert!(
            persisted.contains("[terminal]"),
            "role-enter registry entry must persist a [terminal] block: {persisted}"
        );
        assert!(persisted.contains("terminator_uuid = \"role-term-994\""));
        assert!(persisted.contains("emulator = \"terminator\""));
    }

    // trace:BUG-1156 | ai:codex
    #[cfg(unix)]
    #[test]
    fn role_enter_shell_liveness_requires_live_descendant_session() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let worktree = tmp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();

        let shell_pid = std::process::id();
        let mut role_entry = entry_with(shell_pid, now);
        role_entry.id = agent_id("shell", shell_pid);
        role_entry.agent_type = "shell".to_string();
        role_entry.source = "role-enter".to_string();
        role_entry.name = Some("role:implementer".to_string());
        role_entry.worktree_path = worktree.clone();
        write_entry(tmp.path(), &role_entry).unwrap();

        let mut child = std::process::Command::new("sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn child session stand-in");
        let child_pid = child.id();

        let mut session_entry = entry_with(child_pid, now);
        session_entry.id = agent_id("codex", child_pid);
        session_entry.source = "session-start-hook".to_string();
        session_entry.worktree_path = worktree;
        write_entry(tmp.path(), &session_entry).unwrap();

        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
        let shell = views
            .iter()
            .find(|view| view.id == role_entry.id)
            .expect("role-enter view");
        assert_ne!(shell.status, AgentStatus::Stale);

        child.kill().expect("kill child session stand-in");
        child.wait().expect("reap child session stand-in");

        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
        let shell = views
            .iter()
            .find(|view| view.id == role_entry.id)
            .expect("role-enter view");
        assert_eq!(shell.status, AgentStatus::Stale);
    }

    fn ctx(now: DateTime<Utc>, threshold_secs: u64, leases: Vec<PathBuf>) -> AgentClassifyContext {
        AgentClassifyContext::new(now, threshold_secs, leases)
    }

    #[test]
    fn registry_path_lives_under_aida_agents() {
        let root = Path::new("/tmp/project");
        assert_eq!(
            registry_path(root, "codex-123"),
            PathBuf::from("/tmp/project/.aida/agents/codex-123.toml")
        );
    }

    // STORY-435: dead pid wins over every other signal.
    #[test]
    fn classify_status_dead_pid_is_stale() {
        let now = Utc::now();
        let e = entry_with(42, now);
        let c = ctx(now, 30, vec![PathBuf::from("/tmp/aida-story")]);
        assert_eq!(classify_status(&e, false, &c), AgentStatus::Stale);
    }

    // STORY-435: fresh activity (within threshold) → Busy.
    /// BUG-1701 AC5: the CPU figure that distinguishes a finished seat from a working one.
    // trace:BUG-1701 | ai:claude
    #[test]
    fn parse_ps_cpu_time_covers_every_ps_shape() {
        assert_eq!(parse_ps_cpu_time("00:00"), Some(0));
        // The observed stranded seats: 12s and 46s of CPU across hours.
        assert_eq!(parse_ps_cpu_time("00:12"), Some(12));
        assert_eq!(parse_ps_cpu_time("00:00:46"), Some(46));
        assert_eq!(parse_ps_cpu_time("01:02:03"), Some(3723));
        assert_eq!(parse_ps_cpu_time(" 2-03:04:05 "), Some(183_845));
        // Unavailable or unparseable stays None rather than guessing zero, which would read as
        // "definitely idle".
        assert_eq!(parse_ps_cpu_time(""), None);
        assert_eq!(parse_ps_cpu_time("   "), None);
        assert_eq!(parse_ps_cpu_time("not-a-time"), None);
        assert_eq!(parse_ps_cpu_time("1:2:3:4"), None);
    }

    #[test]
    fn classify_status_fresh_activity_is_busy() {
        let now = Utc::now();
        let e = entry_with(42, now - Duration::seconds(5));
        let c = ctx(now, 30, vec![]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
    }

    // STORY-435: stale activity + no lease → Idle.
    #[test]
    fn classify_status_stale_activity_is_idle() {
        let now = Utc::now();
        let e = entry_with(42, now - Duration::minutes(5));
        let c = ctx(now, 30, vec![]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Idle);
    }

    // trace:STORY-790 | ai:codex
    #[test]
    fn ended_resumable_agent_resolves_by_name_or_spec() {
        let tmp = TempDir::new().unwrap();
        let dead_pid = 4_294_967_294u32;
        let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
        register_spawned_agent(
            tmp.path(),
            "claude",
            dead_pid,
            Some("advisor".into()),
            Some("STORY-790".into()),
            tmp.path().into(),
            Some(&binary),
            Some("claude-advisor-1".into()),
            Some("018f-session".into()),
            None,
            None,
        )
        .unwrap();
        mark_agent_ended(tmp.path(), "claude", dead_pid).unwrap();

        let by_name = resolve_resumable_ended_agent(tmp.path(), "claude-advisor-1").unwrap();
        assert_eq!(by_name.native_session_id.as_deref(), Some("018f-session"));
        assert!(by_name.ended_at.is_some());

        let by_spec = resolve_resumable_ended_agent(tmp.path(), "STORY-790").unwrap();
        assert_eq!(by_spec.id, by_name.id);

        let views =
            ended_resumable_agent_views(tmp.path(), &ctx(Utc::now(), 30, vec![tmp.path().into()]));
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].native_session_id.as_deref(), Some("018f-session"));
    }

    /// TASK-1750: an entry whose live pid carries a different recorded start
    /// time is a recycled pid and reads dead; the matching start time (and a
    /// legacy entry with none) reads live.
    // trace:TASK-1750 | ai:claude
    #[test]
    fn recycled_pid_entry_is_not_live() {
        let Some(ours) = crate::process_probe::process_start_identity(std::process::id()) else {
            return; // platform cannot report start times; nothing to distinguish
        };
        let tmp = tempfile::tempdir().unwrap();
        let mut e = entry_with(std::process::id(), Utc::now());
        e.source = "mcp".to_string();
        assert!(
            registry_entry_is_live(tmp.path(), &e),
            "legacy entry: bare pid probe"
        );
        e.pid_start = Some(ours);
        assert!(registry_entry_is_live(tmp.path(), &e));
        e.pid_start = Some("2000-01-01T00:00:00+00:00".to_string());
        assert!(
            !registry_entry_is_live(tmp.path(), &e),
            "recycled pid must read dead"
        );
    }

    // trace:STORY-790 | ai:codex
    #[test]
    fn ended_marker_wins_over_live_pid_probe() {
        let now = Utc::now();
        let mut e = entry_with(std::process::id(), now);
        e.ended_at = Some(now);
        e.native_session_id = Some("session-id".to_string());
        let c = ctx(now, 30, vec![]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Stale);
    }

    // STORY-435: stale activity but a Live lease covers the worktree → Busy.
    //
    // BUG-1727 AC5: pinned in the shape production uses. The only caller that passes a
    // non-empty lease list also attaches a work probe (`lib.rs` builds the context with
    // `.with_work_probe(..)`), so a `proc_tree: None` context can never reach the lease arm
    // in the CLI: every other call site passes `Vec::new()` and `covers` is false for an
    // empty list. Asserting STORY-435 through `Some(..)` keeps this test on the live path.
    // The `None` fallback arm is covered by `classify_status_live_lease_without_proc_tree_is_busy`.
    // trace:STORY-435 | ai:claude
    #[test]
    fn classify_status_live_lease_is_busy_despite_stale_activity() {
        let now = Utc::now();
        let e = entry_with(42, now - Duration::minutes(5));
        // The lease path comes from the entry itself, so no tmp-path literal is duplicated.
        let c = ctx(now, 30, vec![e.worktree_path.clone()]).with_work_probe(bug1704_tree(120), 10);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn ages_show_work_counts_a_descendant_inside_the_window() {
        assert!(ages_show_work(&[120], 600, 10));
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn ages_show_work_ignores_session_long_service_children() {
        assert!(!ages_show_work(&[3600], 600, 10));
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn ages_show_work_ignores_momentary_hook_children() {
        assert!(!ages_show_work(&[2], 600, 10));
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn ages_show_work_is_false_when_grace_window_is_empty() {
        assert!(!ages_show_work(&[7], 10, 10));
        assert!(!ages_show_work(&[7], 5, 10));
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn ages_show_work_is_time_invariant_for_a_child_started_after_the_heartbeat() {
        for (secs_since_heartbeat, age) in [
            (120, 110),
            (1_200, 1_190),
            (36_000, 35_990),
            (360_000, 359_990),
        ] {
            assert!(ages_show_work(&[age], secs_since_heartbeat, 10));
        }

        // This is BUG-1726's shape: heartbeat after child start excludes it, intentionally.
        for (secs_since_heartbeat, age) in [(600, 601), (36_000, 36_001)] {
            assert!(!ages_show_work(&[age], secs_since_heartbeat, 10));
        }
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn seat_has_own_work_is_true_when_seat_pid_is_absent_from_snapshot() {
        assert!(seat_has_own_work(
            &ProcTree::from_nodes(vec![]),
            42,
            600,
            10
        ));
    }

    /// BUG-1727 AC3: the grace floor's cost, pinned rather than assumed. A child younger
    /// than `grace_secs` is not counted, so the seat reads Idle while that child runs —
    /// and the window closes on its own. Both halves use the SAME heartbeat staleness and
    /// the SAME child; only its age advances, which is what makes this a self-heal and not
    /// two unrelated cases.
    // trace:BUG-1727 | ai:claude
    #[test]
    fn ages_show_work_self_heals_when_a_young_child_reaches_the_grace_floor() {
        assert!(!ages_show_work(&[5], 31, 10));
        assert!(ages_show_work(&[10], 31, 10));
    }

    /// BUG-1727 AC1 + AC2: a live descendant the probe could not age reads Busy, the same
    /// way a missing root does. Without the unsampled count this tree yields an empty age
    /// list, indistinguishable from a seat with no children at all, and reads Idle.
    // trace:BUG-1727 | ai:claude
    #[test]
    fn seat_has_own_work_is_true_when_a_descendant_cannot_be_aged() {
        let tree = ProcTree::from_nodes(vec![
            aida_core::liveness::ProcNode {
                pid: 42,
                ppid: None,
                age_secs: Some(700),
            },
            aida_core::liveness::ProcNode {
                pid: 50,
                ppid: Some(42),
                age_secs: None,
            },
        ]);
        assert!(seat_has_own_work(&tree, 42, 600, 10));
    }

    /// …and the fail-safe stays narrow: a fully sampled subtree with nothing in the work
    /// window still reads Idle, so AC1 did not turn every covered seat Busy.
    // trace:BUG-1727 | ai:claude
    #[test]
    fn seat_has_own_work_is_false_when_a_sampled_subtree_shows_no_work() {
        assert!(!seat_has_own_work(&bug1704_tree(3600), 42, 600, 10));
    }

    // trace:BUG-1704 | ai:codex
    fn bug1704_tree(child_age: i64) -> ProcTree {
        ProcTree::from_nodes(vec![
            aida_core::liveness::ProcNode {
                pid: 42,
                ppid: None,
                age_secs: Some(700),
            },
            aida_core::liveness::ProcNode {
                pid: 50,
                ppid: Some(42),
                age_secs: Some(700),
            },
            aida_core::liveness::ProcNode {
                pid: 51,
                ppid: Some(50),
                age_secs: Some(child_age),
            },
        ])
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn classify_status_live_lease_without_proc_tree_is_busy() {
        let now = Utc::now();
        let entry = entry_with(42, now - Duration::minutes(5));
        // The lease path comes from the entry itself: the rule under test is "a live lease
        // covers this seat's worktree", so duplicating a literal here would add a non-portable
        // tmp path for no gain. trace:BUG-1704 | ai:claude
        let ctx = ctx(now, 30, vec![entry.worktree_path.clone()]);
        assert_eq!(classify_status(&entry, true, &ctx), AgentStatus::Busy);
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn classify_status_live_lease_with_quiet_subtree_is_idle() {
        let now = Utc::now();
        let entry = entry_with(42, now - Duration::minutes(5));
        let ctx =
            ctx(now, 30, vec![entry.worktree_path.clone()]).with_work_probe(bug1704_tree(3600), 10);
        assert_eq!(classify_status(&entry, true, &ctx), AgentStatus::Idle);
    }

    // STORY-435: lease worktree that's an ancestor of the agent worktree
    // also counts (operator working in a subdir of the lease's root).
    #[test]
    fn classify_status_live_lease_covers_ancestor() {
        let now = Utc::now();
        let mut e = entry_with(42, now - Duration::minutes(5));
        e.worktree_path = PathBuf::from("/tmp/aida-story/subdir");
        let c = ctx(now, 30, vec![PathBuf::from("/tmp/aida-story")]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
    }

    // STORY-435: empty lease worktree (advisory lock) must NOT match every
    // agent — mirrors TASK-474's lease_covers_cwd short-circuit.
    #[test]
    fn classify_status_ignores_empty_lease_worktree() {
        let now = Utc::now();
        let e = entry_with(42, now - Duration::minutes(5));
        let c = ctx(now, 30, vec![PathBuf::from("")]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Idle);
    }

    // STORY-435: a clock that ticked backwards (NTP step, suspend/resume)
    // must not prematurely flip a fresh entry to Idle.
    #[test]
    fn classify_status_clamps_negative_elapsed() {
        let now = Utc::now();
        let e = entry_with(42, now + Duration::seconds(1));
        let c = ctx(now, 30, vec![]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
    }

    // STORY-435: spec-mandated regression — three agents with different
    // activity patterns each classify correctly off the same context.
    #[test]
    fn three_agents_with_different_activity_patterns_classify_correctly() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();

        // (1) live pid + fresh activity → Busy.
        let mut busy = entry_with(std::process::id(), now - Duration::seconds(2));
        busy.id = agent_id("codex", busy.pid);
        write_entry(tmp.path(), &busy).unwrap();

        // (2) live pid + stale activity + no lease → Idle.
        let mut idle = entry_with(std::process::id(), now - Duration::minutes(5));
        idle.agent_type = "claude".to_string();
        idle.id = agent_id("claude", idle.pid);
        write_entry(tmp.path(), &idle).unwrap();

        // (3) dead pid → Stale (regardless of last_active_at).
        let mut stale = entry_with(u32::MAX - 1, now - Duration::seconds(1));
        stale.agent_type = "antigravity".to_string();
        stale.id = agent_id("antigravity", stale.pid);
        write_entry(tmp.path(), &stale).unwrap();

        let c = ctx(now, 30, vec![]);
        let views = list_agent_views(tmp.path(), &c);
        assert_eq!(views.len(), 3);

        // Sorted alphabetically by agent_type: antigravity, claude, codex.
        let by_type: std::collections::HashMap<_, _> = views
            .iter()
            .map(|v| (v.agent_type.as_str(), v.status))
            .collect();
        assert_eq!(by_type["codex"], AgentStatus::Busy);
        assert_eq!(by_type["claude"], AgentStatus::Idle);
        assert_eq!(by_type["antigravity"], AgentStatus::Stale);
    }

    #[test]
    fn list_agent_views_reads_toml_and_computes_stale_status() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let record = entry_with(u32::MAX - 1, now);
        write_entry(tmp.path(), &record).unwrap();

        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].agent_type, "codex");
        assert_eq!(views[0].status, AgentStatus::Stale);
    }

    // STORY-791: descriptions are durable registry metadata and render in the
    // agent-status row instead of forcing operators to infer purpose from role.
    #[test]
    fn spawned_agent_description_roundtrips_to_view_and_status_line() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
        let entry = register_spawned_agent(
            tmp.path(),
            "codex",
            std::process::id(),
            Some("advisor".to_string()),
            None,
            tmp.path().to_path_buf(),
            Some(&binary),
            Some("codex-advisor-1".to_string()),
            None,
            None,
            Some("repo triage".to_string()),
        )
        .unwrap();
        assert_eq!(entry.description.as_deref(), Some("repo triage"));

        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
        assert_eq!(views[0].description.as_deref(), Some("repo triage"));
        let lines = format_agent_status_lines(&views);
        assert!(lines[0].contains("process"));
        assert!(lines[0].contains("repo triage"));
    }

    // STORY-791: the singleton role set has a safe default but remains
    // configurable under [agent_registry].
    #[test]
    fn config_parses_singleton_roles() {
        let cfg = Config::from_toml_str(
            "[agent_registry]\nsingleton_roles = \"advisor, product, integrator\"\n",
        );
        assert!(cfg.role_is_singleton("advisor"));
        assert!(cfg.role_is_singleton("product"));
        assert!(!cfg.role_is_singleton("implementer"));
    }

    // STORY-791: launches are singleton per role per derived scope. A repo-wide
    // advisor conflicts with another advisor, while a different role does not.
    #[test]
    fn same_scope_conflict_detects_live_same_role_scope() {
        let tmp = TempDir::new().unwrap();
        let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
        register_spawned_agent(
            tmp.path(),
            "codex",
            std::process::id(),
            Some("advisor".to_string()),
            None,
            tmp.path().to_path_buf(),
            Some(&binary),
            Some("codex-advisor-1".to_string()),
            None,
            None,
            Some("repo master advisor".to_string()),
        )
        .unwrap();
        let cfg = Config::default();

        let conflict = same_scope_conflict(tmp.path(), &cfg, Some("advisor"), None, tmp.path());
        assert!(conflict.is_some());
        assert!(
            same_scope_conflict(tmp.path(), &cfg, Some("reviewer"), None, tmp.path()).is_none()
        );
    }

    // STORY-791: spec-scoped doer roles conflict only on the same spec scope.
    #[test]
    fn same_scope_conflict_is_spec_scoped_for_implementers() {
        let tmp = TempDir::new().unwrap();
        let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
        register_spawned_agent(
            tmp.path(),
            "codex",
            std::process::id(),
            Some("implementer".to_string()),
            Some("TASK-1".to_string()),
            tmp.path().join("task-1"),
            Some(&binary),
            Some("codex-implementer-1".to_string()),
            None,
            None,
            None,
        )
        .unwrap();
        let cfg = Config::default();

        assert!(same_scope_conflict(
            tmp.path(),
            &cfg,
            Some("implementer"),
            Some("TASK-1"),
            &tmp.path().join("other")
        )
        .is_some());
        assert!(same_scope_conflict(
            tmp.path(),
            &cfg,
            Some("implementer"),
            Some("TASK-2"),
            &tmp.path().join("task-2")
        )
        .is_none());
    }

    // TASK-1184: dead rows without a resumable native session are pruned.
    #[test]
    fn gc_dead_agents_removes_dead_non_resumable_entries() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let record = entry_with(u32::MAX - 1, now);
        let path = registry_path(tmp.path(), &record.id);
        write_entry(tmp.path(), &record).unwrap();

        let report = gc_dead_agents(tmp.path(), false, None).unwrap();

        assert_eq!(report.removed_count(), 1);
        assert!(report.retained.is_empty());
        assert!(!path.exists());
    }

    // TASK-1184: resumable rows are retained by default, but an explicit
    // age threshold can prune old tombstones.
    #[test]
    fn gc_dead_agents_retains_resumable_until_older_than_matches() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut record = entry_with(u32::MAX - 1, now - Duration::days(10));
        record.native_session_id = Some("native-session-1".to_string());
        let path = registry_path(tmp.path(), &record.id);
        write_entry(tmp.path(), &record).unwrap();

        let retained = gc_dead_agents(tmp.path(), false, None).unwrap();
        assert!(retained.removed.is_empty());
        assert_eq!(retained.retained.len(), 1);
        assert!(path.exists());

        let removed = gc_dead_agents(tmp.path(), false, Some(7)).unwrap();
        assert_eq!(removed.removed_count(), 1);
        assert!(removed.retained.is_empty());
        assert!(!path.exists());
    }

    // STORY-1843 (TASK-2177): a dead resumable row stays for resume but is
    // stamped ended, so a sandboxed mail sender stops counting it as a reader.
    // trace:STORY-1843 | ai:claude
    // trace:STORY-1843.ac97b025 | ai:claude
    #[test]
    fn gc_dead_agents_marks_dead_resumable_rows_ended_and_keeps_them() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut record = entry_with(u32::MAX - 1, now - Duration::minutes(5));
        record.native_session_id = Some("native-session-1".to_string());
        let path = registry_path(tmp.path(), &record.id);
        write_entry(tmp.path(), &record).unwrap();

        let preview = gc_dead_agents(tmp.path(), true, None).unwrap();
        assert_eq!(preview.marked_ended, 1);
        assert!(load_entries(tmp.path())[0].1.ended_at.is_none());

        let report = gc_dead_agents(tmp.path(), false, None).unwrap();
        assert_eq!(report.marked_ended, 1);
        assert_eq!(report.retained.len(), 1);
        assert!(path.exists());
        let ended = load_entries(tmp.path()).remove(0).1;
        assert!(ended.ended_at.is_some());
        assert_eq!(ended.native_session_id.as_deref(), Some("native-session-1"));
        assert_eq!(ended.last_active_at, record.last_active_at);

        assert_eq!(ended.ended_at, Some(record.last_active_at));

        let again = gc_dead_agents(tmp.path(), false, None).unwrap();
        assert_eq!(again.marked_ended, 0, "ending is idempotent");
    }

    // STORY-1843: a crash gc discovers late keeps its last-heartbeat end, so a
    // session that ended recently stays first for resume-latest and the picker.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn gc_ended_old_crash_does_not_outrank_a_recent_end_for_resume() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut crashed = entry_with(u32::MAX - 1, now - Duration::days(3));
        crashed.native_session_id = Some("old-crash".to_string());
        write_entry(tmp.path(), &crashed).unwrap();
        let mut recent = entry_with(u32::MAX - 2, now - Duration::minutes(10));
        recent.native_session_id = Some("recent-end".to_string());
        recent.ended_at = Some(now - Duration::minutes(10));
        write_entry(tmp.path(), &recent).unwrap();

        let report = gc_dead_agents(tmp.path(), false, None).unwrap();
        assert_eq!(report.marked_ended, 1);

        let views =
            ended_resumable_agent_views(tmp.path(), &ctx(Utc::now(), 30, vec![tmp.path().into()]));
        let order: Vec<_> = views
            .iter()
            .map(|v| v.native_session_id.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(order, vec!["recent-end", "old-crash"]);
        assert_eq!(views[1].ended_at, Some(crashed.last_active_at));
    }

    // STORY-1843: gc in another PID namespace cannot see the row's process, so
    // it must neither prune nor end it.
    // trace:STORY-1843 | ai:claude
    #[test]
    fn gc_dead_agents_leaves_rows_from_unobservable_namespaces_untouched() {
        let Some(observer) = current_pid_namespace() else {
            return;
        };
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut resumable = entry_with(u32::MAX - 1, now);
        resumable.pid_namespace = Some(observer ^ 1);
        resumable.native_session_id = Some("native-session-1".to_string());
        write_entry(tmp.path(), &resumable).unwrap();
        let mut plain = entry_with(u32::MAX - 2, now);
        plain.pid_namespace = Some(observer ^ 1);
        write_entry(tmp.path(), &plain).unwrap();
        let before: Vec<_> = load_entries(tmp.path())
            .into_iter()
            .map(|(p, _)| std::fs::read(p).unwrap())
            .collect();

        let report = gc_dead_agents(tmp.path(), false, None).unwrap();

        assert_eq!(report, AgentGcReport::default());
        let after: Vec<_> = load_entries(tmp.path())
            .into_iter()
            .map(|(p, _)| std::fs::read(p).unwrap())
            .collect();
        assert_eq!(before, after);
    }

    fn probe(self_ns: u64, pid1_ns: Option<u64>, nspid: usize, comm: &str) -> PidNamespaceProbe {
        PidNamespaceProbe {
            self_ns: Some(self_ns),
            pid1_ns,
            nspid_levels: nspid,
            pid1_comm: Some(comm.to_string()),
        }
    }

    // BUG-2212: each signal alone marks the caller isolated; a host shell
    // with systemd as pid 1 in its own namespace is not.
    // trace:BUG-2212 | ai:claude
    #[test]
    fn pid_namespace_probe_detects_isolated_callers() {
        let host = INITIAL_PID_NAMESPACE;
        assert!(!probe(host, Some(host), 1, "systemd").is_isolated());
        assert!(!probe(host, None, 1, "init").is_isolated());
        assert!(probe(7, Some(host), 1, "systemd").is_isolated());
        assert!(probe(7, None, 2, "systemd").is_isolated());
        // bubblewrap: pid 1 unreadable from the sandbox user, comm is not init.
        assert!(probe(7, None, 1, "3").is_isolated());
        assert!(!PidNamespaceProbe::default().is_isolated());
    }

    // TASK-2272 (TASK-2281): a host whose pid 1 is runit/openrc/s6 shares its
    // namespace with pid 1, so the inode comparison settles it and the name is
    // never consulted. The name decides only when /proc/1/ns/pid is unreadable.
    // trace:TASK-2272 | ai:claude
    #[test]
    fn pid_namespace_probe_prefers_the_namespace_inode_over_pid1_comm() {
        let host = INITIAL_PID_NAMESPACE;
        for comm in ["runit", "openrc-init", "s6-svscan", "my-custom-init"] {
            assert!(!probe(host, Some(host), 1, comm).is_isolated(), "{comm}");
        }
        for comm in ["runit", "openrc-init", "s6-svscan", "dumb-init", "tini"] {
            assert!(!probe(host, None, 1, comm).is_isolated(), "{comm}");
        }
        assert!(probe(host, None, 1, "bwrap").is_isolated());
        assert!(probe(7, Some(host), 1, "runit").is_isolated());
    }

    // trace:BUG-2212 | ai:claude
    #[test]
    fn pid_observer_sees_only_its_own_namespace_and_unstamped_rows_from_the_host() {
        let host = PidObserver::from_probe(&probe(INITIAL_PID_NAMESPACE, None, 1, "systemd"));
        let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));
        assert!(host.can_observe(Some(INITIAL_PID_NAMESPACE)));
        assert!(host.can_observe(None));
        assert!(!host.can_observe(Some(7)));
        assert!(sandbox.can_observe(Some(7)));
        assert!(!sandbox.can_observe(Some(INITIAL_PID_NAMESPACE)));
        assert!(!sandbox.can_observe(None));
        // An isolated caller cannot vouch for unstamped rows even when its
        // namespace inode reads as the initial one.
        let masked = PidObserver {
            namespace: Some(INITIAL_PID_NAMESPACE),
            isolated: true,
        };
        assert!(!masked.can_observe(None));
    }

    // BUG-2212: from a sandboxed shell a live host session's pid is invisible.
    // agent ls must list it on its fresh heartbeat, labelled as such, and only
    // age it out of the DISPLAY listing once the heartbeat is past the window.
    // trace:BUG-2212 | ai:claude
    #[test]
    fn list_agent_views_falls_back_to_heartbeat_for_unobservable_rows() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut fresh = entry_with(u32::MAX - 1, now - chrono::Duration::minutes(5));
        fresh.pid_namespace = Some(INITIAL_PID_NAMESPACE);
        write_entry(tmp.path(), &fresh).unwrap();
        let mut old = entry_with(u32::MAX - 2, now - chrono::Duration::hours(25));
        old.pid_namespace = Some(INITIAL_PID_NAMESPACE);
        write_entry(tmp.path(), &old).unwrap();
        let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));

        let views = list_agent_views(
            tmp.path(),
            &ctx(now, 30, vec![]).with_observer(sandbox).for_display(),
        );
        let by_pid: std::collections::HashMap<u32, &AgentRegistryView> =
            views.iter().map(|v| (v.pid, v)).collect();
        assert_eq!(by_pid[&(u32::MAX - 1)].status, AgentStatus::Idle);
        assert_eq!(by_pid[&(u32::MAX - 1)].liveness, LivenessBasis::Heartbeat);
        assert_eq!(by_pid[&(u32::MAX - 2)].status, AgentStatus::Stale);
        assert_eq!(by_pid[&(u32::MAX - 2)].liveness, LivenessBasis::Heartbeat);

        // From the host the same dead pids read Stale on process evidence.
        let host = PidObserver {
            namespace: Some(INITIAL_PID_NAMESPACE),
            isolated: false,
        };
        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]).with_observer(host));
        assert!(views
            .iter()
            .all(|v| v.status == AgentStatus::Stale && v.liveness == LivenessBasis::Process));
    }

    // TASK-2272 (TASK-2280): guards FAIL CLOSED. A session-start-hook row
    // heartbeats exactly once, so from a sandboxed launcher every live host
    // session's heartbeat goes arbitrarily old. A row whose pid this observer
    // cannot check and that has no `ended_at` therefore counts as live
    // (unverified) however old its heartbeat — the duplicate launch is
    // refused. Only a PROVABLY dead row (ended_at, or a checkable dead pid)
    // frees the seat.
    // trace:TASK-2272 | ai:claude
    #[test]
    fn guards_fail_closed_on_unverifiable_rows_and_open_only_on_provable_death() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        // A host row whose only heartbeat is its session start, 17h ago.
        let mut unverifiable = entry_with(u32::MAX - 1, now - chrono::Duration::hours(17));
        unverifiable.pid_namespace = Some(INITIAL_PID_NAMESPACE);
        unverifiable.role = Some("advisor".to_string());
        write_entry(tmp.path(), &unverifiable).unwrap();
        let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));

        let guard = list_agent_views(tmp.path(), &ctx(now, 30, vec![]).with_observer(sandbox));
        let row = guard.iter().find(|v| v.pid == u32::MAX - 1).unwrap();
        assert_ne!(row.status, AgentStatus::Stale);
        assert_eq!(row.liveness, LivenessBasis::Heartbeat);
        let dup = crate::agent_launch::matching_live_duplicate(
            &guard,
            &unverifiable.agent_type,
            Some("advisor"),
            None,
        )
        .expect("an unverifiable host row refuses the duplicate launch");
        assert_eq!(dup.pid, u32::MAX - 1);

        // An ended row is provably dead from any namespace: launch allowed.
        let mut ended = entry_with(u32::MAX - 2, now - chrono::Duration::minutes(1));
        ended.pid_namespace = Some(INITIAL_PID_NAMESPACE);
        ended.role = Some("advisor".to_string());
        ended.ended_at = Some(now);
        write_entry(tmp.path(), &ended).unwrap();
        // A checkable dead pid (our own namespace) is provably dead too.
        let mut dead = entry_with(u32::MAX - 3, now - chrono::Duration::minutes(1));
        dead.pid_namespace = Some(7);
        dead.role = Some("advisor".to_string());
        write_entry(tmp.path(), &dead).unwrap();

        let sandbox_ctx = ctx(now, 30, vec![]).with_observer(sandbox);
        let provably_dead: Vec<_> = list_agent_views(tmp.path(), &sandbox_ctx)
            .into_iter()
            .filter(|v| v.pid != u32::MAX - 1)
            .collect();
        assert!(provably_dead.iter().all(|v| v.status == AgentStatus::Stale));
        assert!(crate::agent_launch::matching_live_duplicate(
            &provably_dead,
            &unverifiable.agent_type,
            Some("advisor"),
            None,
        )
        .is_none());
    }

    // trace:BUG-2212 | ai:claude
    #[test]
    fn observer_liveness_treats_ended_rows_as_dead_from_any_namespace() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut ended = entry_with(u32::MAX - 1, now);
        ended.pid_namespace = Some(INITIAL_PID_NAMESPACE);
        ended.ended_at = Some(now);
        let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));
        assert_eq!(
            observer_liveness(tmp.path(), &ended, sandbox, now, None),
            (false, LivenessBasis::Process)
        );
    }

    // TASK-543: raw-registered agents use the same stale-PID transition as
    // launcher/MCP entries.
    #[test]
    fn manual_registered_entry_with_dead_pid_is_stale() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut record = entry_with(u32::MAX - 1, now);
        record.source = "manual-register".to_string();
        record.name = Some("codex-raw".to_string());
        write_entry(tmp.path(), &record).unwrap();

        let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].source, "manual-register");
        assert_eq!(views[0].name.as_deref(), Some("codex-raw"));
        assert_eq!(views[0].status, AgentStatus::Stale);
    }

    /// BUG-416: live_agents_covering_cwd counts only LIVE agents whose
    /// worktree covers the cwd (equal or ancestor), skipping dead PIDs. A
    /// worktree with ≥2 live agents reads as "shared" (the add-hint suppressor
    /// fires); a lone agent or an unrelated dir does not. trace:BUG-416
    #[test]
    fn live_agents_covering_cwd_counts_shared_live_worktrees_only() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let wt_a = root.join("aida-a");
        let wt_b = root.join("aida-b");
        let nested = wt_a.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&wt_b).unwrap();

        let live = |id: &str, wt: &Path| {
            let mut e = entry_with(std::process::id(), Utc::now());
            e.id = id.to_string();
            e.worktree_path = wt.to_path_buf();
            write_entry(root, &e).unwrap();
        };
        // Two live agents share wt_a.
        live("a-codex-1", &wt_a);
        live("a-claude-2", &wt_a);
        // One live agent alone in wt_b.
        live("b-codex-solo", &wt_b);
        // A dead agent also recorded in wt_b — must be skipped.
        let mut dead = entry_with(u32::MAX - 1, Utc::now());
        dead.id = "b-codex-dead".to_string();
        dead.worktree_path = wt_b.clone();
        write_entry(root, &dead).unwrap();

        // wt_a shared by 2 live agents.
        assert_eq!(live_agents_covering_cwd(root, &wt_a), 2);
        // A descendant of wt_a is still "covered" by both.
        assert_eq!(live_agents_covering_cwd(root, &nested), 2);
        // wt_b: 1 live + 1 dead → 1 (lone occupant; dead skipped).
        assert_eq!(live_agents_covering_cwd(root, &wt_b), 1);
        // The parent dir is no agent's worktree → 0 (human-alone case hints).
        assert_eq!(live_agents_covering_cwd(root, root), 0);
    }

    #[test]
    fn format_agent_status_lines_appends_freshness_hint() {
        let now = Utc::now();
        let view = AgentRegistryView {
            id: "codex-42".to_string(),
            agent_type: "codex".to_string(),
            pid: 42,
            pid_start: None,
            name: None,
            description: None,
            tty: None,
            terminal: None,
            started_at: now,
            // Freshness column is computed relative to `Utc::now()` inside
            // `format_agent_status_lines`. Anchor it at "now - 0s" so the
            // rendered hint is "(0s)" deterministically.
            last_active_at: now,
            role: Some("implementer".to_string()),
            current_spec: Some("STORY-431".to_string()),
            worktree_path: PathBuf::from("/tmp/aida-story-431"),
            source: "mcp".to_string(),
            binary_version: None,
            build_sha: None,
            status: AgentStatus::Busy,
            availability: Availability::Available,
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            ended_at: None,
            resumed_from: None,
            liveness: LivenessBasis::Process,
        };

        let lines = format_agent_status_lines(&[view]);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // Strict column-shape assertion for the static part; the (Xs)
        // trailing hint is loose because Utc::now() inside the formatter
        // can tick a second on a slow runner.
        assert!(
            line.starts_with(
                "  codex#42        process  implementer STORY-431    spec:STORY-431     busy  ("
            ),
            "unexpected line: {line:?}"
        );
        assert!(
            line.ends_with(" /tmp/aida-story-431"),
            "unexpected line: {line:?}"
        );
        assert!(line.contains("s)"), "expected (Xs) hint, got: {line:?}");
    }

    #[test]
    fn mcp_authority_status_lines_name_role_and_relaunch_command() {
        let now = Utc::now();
        let view = AgentRegistryView {
            id: "codex-42".to_string(),
            agent_type: "codex".to_string(),
            pid: 42,
            pid_start: None,
            name: None,
            description: None,
            tty: None,
            started_at: now,
            last_active_at: now,
            role: Some("implementer".to_string()),
            current_spec: Some("BUG-1043".to_string()),
            terminal: None,
            worktree_path: PathBuf::from("/tmp/aida-bug-1043"),
            source: "mcp".to_string(),
            binary_version: None,
            build_sha: None,
            status: AgentStatus::Busy,
            availability: Availability::Available,
            paused_since: None,
            paused_reason: None,
            expected_back: None,
            native_session_id: None,
            ended_at: None,
            resumed_from: None,
            liveness: LivenessBasis::Process,
        };

        let lines = mcp_authority_status_lines(&[view]);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert!(line.contains("MCP authority:"), "{line}");
        // ADR-66: the line names the server's granted seat and points at TTY
        // issuance, never an env-var relaunch. trace:STORY-1473 | ai:claude
        assert!(line.contains("seat=implementer"), "{line}");
        assert!(line.contains("advisor authority: no"), "{line}");
        assert!(
            line.contains("enter advisor at a TTY, then restart MCP"),
            "{line}"
        );
        assert!(!line.contains("AIDA_SESSION_ROLE"), "{line}");
    }

    #[test]
    fn humanize_elapsed_thresholds() {
        assert_eq!(humanize_elapsed(0), "0s");
        assert_eq!(humanize_elapsed(59), "59s");
        assert_eq!(humanize_elapsed(60), "1m");
        assert_eq!(humanize_elapsed(90), "1m");
        assert_eq!(humanize_elapsed(3599), "59m");
        assert_eq!(humanize_elapsed(3600), "1h");
        assert_eq!(humanize_elapsed(7200), "2h");
        assert_eq!(humanize_elapsed(86_399), "23h");
        assert_eq!(humanize_elapsed(86_400), "1d");
        assert_eq!(humanize_elapsed(90_000), "1d");
        // Negative inputs clamp to 0s.
        assert_eq!(humanize_elapsed(-5), "0s");
    }

    #[test]
    fn config_load_defaults_when_section_missing() {
        let cfg = Config::from_toml_str("");
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.busy_threshold_secs, 30);
        assert_eq!(cfg.work_grace_secs, 10);

        let cfg = Config::from_toml_str("[other]\nfoo = 1\n");
        assert_eq!(cfg.busy_threshold_secs, 30);
    }

    #[test]
    fn config_parses_busy_threshold_secs() {
        let cfg = Config::from_toml_str("[agent_registry]\nbusy_threshold_secs = 90\n");
        assert_eq!(cfg.busy_threshold_secs, 90);
    }

    // trace:BUG-1704 | ai:codex
    #[test]
    fn config_parses_work_grace_secs() {
        let cfg = Config::from_toml_str("[agent_registry]\nwork_grace_secs = 25\n");
        assert_eq!(cfg.work_grace_secs, 25);
        assert_eq!(Config::default().work_grace_secs, 10);
    }

    #[test]
    fn config_ignores_unknown_keys() {
        let body = "[agent_registry]\nbusy_threshold_secs = 45\nunknown = \"x\"\n";
        let cfg = Config::from_toml_str(body);
        assert_eq!(cfg.busy_threshold_secs, 45);
    }

    #[test]
    fn config_load_reads_from_disk() {
        let tmp = TempDir::new().unwrap();
        let aida = tmp.path().join(".aida");
        std::fs::create_dir_all(&aida).unwrap();
        std::fs::write(
            aida.join("config.toml"),
            "[agent_registry]\nbusy_threshold_secs = 7\n",
        )
        .unwrap();
        assert_eq!(Config::load(tmp.path()).busy_threshold_secs, 7);
    }

    // STORY-528: a registry record written before the availability field
    // existed must still deserialize — defaulting to Available with no pause
    // metadata. This is the backward-compatibility guarantee.
    #[test]
    fn legacy_record_without_availability_reads_as_available() {
        let legacy = r#"
id = "codex-42"
agent_type = "codex"
pid = 42
started_at = "2026-06-07T10:00:00Z"
last_active_at = "2026-06-07T10:00:00Z"
worktree_path = "/tmp/aida-story"
source = "mcp"
"#;
        let entry: AgentRegistryEntry = toml::from_str(legacy).unwrap();
        assert_eq!(entry.availability, Availability::Available);
        assert!(entry.paused_since.is_none());
        assert!(entry.paused_reason.is_none());
        assert!(entry.expected_back.is_none());
        assert!(entry.description.is_none());
    }

    #[cfg(unix)]
    // STORY-791: MCP child registration can attach to the launcher row by
    // walking process ancestry rather than creating a second row.
    #[test]
    fn pid_has_ancestor_detects_live_child_parent() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let child_pid = child.id();
        assert!(pid_has_ancestor(child_pid, std::process::id()));
        let _ = child.kill();
        let _ = child.wait();
    }

    // STORY-528: pause then resume roundtrips through the on-disk record.
    #[test]
    fn pause_then_resume_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let mut e = entry_with(std::process::id(), Utc::now());
        e.name = Some("codex-impl-1".to_string());
        write_entry(tmp.path(), &e).unwrap();

        let paused = pause_agent(
            tmp.path(),
            "codex-impl-1",
            PauseReason::Budget,
            Some(Utc::now() + Duration::hours(2)),
        )
        .unwrap();
        assert_eq!(paused.availability, Availability::Paused);
        assert_eq!(paused.paused_reason, Some(PauseReason::Budget));
        assert!(paused.paused_since.is_some());
        assert!(paused.expected_back.is_some());

        // Reloaded from disk it's still paused.
        let ctx = ctx(Utc::now(), 30, vec![]);
        let views = list_agent_views(tmp.path(), &ctx);
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].availability, Availability::Paused);

        let resumed = resume_agent(tmp.path(), "codex-impl-1").unwrap();
        assert_eq!(resumed.availability, Availability::Available);
        assert!(resumed.paused_since.is_none());
        assert!(resumed.paused_reason.is_none());
        assert!(resumed.expected_back.is_none());
    }

    // STORY-528: pausing resolves via the synthetic <type>#<pid> id too.
    #[test]
    fn pause_resolves_via_hash_id() {
        let tmp = TempDir::new().unwrap();
        let e = entry_with(std::process::id(), Utc::now());
        write_entry(tmp.path(), &e).unwrap();
        let target = format!("codex#{}", std::process::id());
        let paused = pause_agent(tmp.path(), &target, PauseReason::RateLimit, None).unwrap();
        assert_eq!(paused.availability, Availability::Paused);
        assert_eq!(paused.paused_reason, Some(PauseReason::RateLimit));
    }

    // STORY-528: an unknown target errors rather than silently no-op'ing.
    #[test]
    fn pause_unknown_target_errors() {
        let tmp = TempDir::new().unwrap();
        let err = pause_agent(tmp.path(), "nope-9999", PauseReason::Manual, None).unwrap_err();
        assert!(err.to_string().contains("no active agent"));
    }

    // STORY-528: brief-time guard WARNS (returns Some) for a paused agent and
    // is silent (None) for an available agent / unknown target.
    #[test]
    fn paused_warning_fires_only_for_paused_target() {
        let tmp = TempDir::new().unwrap();
        let mut e = entry_with(std::process::id(), Utc::now());
        e.name = Some("codex-impl-1".to_string());
        write_entry(tmp.path(), &e).unwrap();

        // Available → no warning.
        assert!(paused_warning_for_target(tmp.path(), "codex-impl-1").is_none());
        // Unknown target → no warning.
        assert!(paused_warning_for_target(tmp.path(), "ghost").is_none());

        pause_agent(tmp.path(), "codex-impl-1", PauseReason::Budget, None).unwrap();
        let warning = paused_warning_for_target(tmp.path(), "codex-impl-1").unwrap();
        assert!(warning.contains("paused"));
        assert!(warning.contains("budget"));
    }

    // STORY-528: the status-line formatter appends the ⏸ glyph for a paused
    // agent and leaves an available agent's line glyph-free.
    #[test]
    fn format_status_line_appends_pause_glyph() {
        let now = Utc::now();
        let mk = |availability: Availability, reason: Option<PauseReason>| AgentRegistryView {
            id: "codex-42".to_string(),
            agent_type: "codex".to_string(),
            pid: 42,
            pid_start: None,
            name: None,
            description: None,
            tty: None,
            terminal: None,
            started_at: now,
            last_active_at: now,
            role: Some("implementer".to_string()),
            current_spec: Some("STORY-528".to_string()),
            worktree_path: PathBuf::from("/tmp/aida-story-528"),
            source: "mcp".to_string(),
            binary_version: None,
            build_sha: None,
            status: AgentStatus::Idle,
            availability,
            paused_since: reason.map(|_| now),
            paused_reason: reason,
            expected_back: None,
            native_session_id: None,
            ended_at: None,
            resumed_from: None,
            liveness: LivenessBasis::Process,
        };

        let avail_line = &format_agent_status_lines(&[mk(Availability::Available, None)])[0];
        assert!(!avail_line.contains("⏸"), "available agent: {avail_line:?}");

        let paused_line =
            &format_agent_status_lines(&[mk(Availability::Paused, Some(PauseReason::Budget))])[0];
        assert!(paused_line.contains("⏸ paused (budget)"), "{paused_line:?}");
    }

    // STORY-528: pause_detail formats the ~back HH:MM fragment when an
    // expected-back time is present, and omits it otherwise.
    #[test]
    fn pause_detail_formats_expected_back() {
        assert_eq!(pause_detail(PauseReason::Budget, None), "paused (budget)");
        let detail = pause_detail(PauseReason::RateLimit, Some(Utc::now()));
        assert!(detail.starts_with("paused (rate-limit, ~back "));
    }

    // STORY-528: reason parsing accepts the documented spellings.
    #[test]
    fn pause_reason_parse_accepts_known_reasons() {
        assert_eq!(PauseReason::parse("budget"), Some(PauseReason::Budget));
        assert_eq!(
            PauseReason::parse("RATE-LIMIT"),
            Some(PauseReason::RateLimit)
        );
        assert_eq!(
            PauseReason::parse("rate_limit"),
            Some(PauseReason::RateLimit)
        );
        assert_eq!(PauseReason::parse("manual"), Some(PauseReason::Manual));
        assert_eq!(PauseReason::parse("unknown"), Some(PauseReason::Unknown));
        assert_eq!(PauseReason::parse("bogus"), None);
    }

    #[test]
    fn looks_like_spec_id_accepts_canonical_ids_only() {
        assert!(looks_like_spec_id("STORY-431"));
        assert!(looks_like_spec_id("TASK-498"));
        assert!(!looks_like_spec_id("story-431"));
        assert!(!looks_like_spec_id("branch-name"));
    }

    // trace:BUG-1920 | ai:claude
    #[test]
    fn detect_agent_type_prefers_live_session_over_api_key() {
        assert_eq!(
            detect_agent_type_from_env([("GEMINI_API_KEY", "x"), ("CLAUDECODE", "1")]),
            "claude"
        );
        assert_eq!(
            detect_agent_type_from_env([
                ("GEMINI_API_KEY", "x"),
                ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ]),
            "claude"
        );
    }

    // trace:BUG-1920 | ai:claude
    #[test]
    fn detect_agent_type_api_key_never_decides() {
        assert_eq!(
            detect_agent_type_from_env([
                ("ANTHROPIC_API_KEY", "x"),
                ("ANTIGRAVITY_SESSION_ID", "s1"),
            ]),
            "antigravity"
        );
        assert_eq!(
            detect_agent_type_from_env([("GEMINI_API_KEY", "x")]),
            "other"
        );
        assert_eq!(
            detect_agent_type_from_env([("CLAUDE_CODE_OAUTH_TOKEN", "x")]),
            "other"
        );
    }

    // trace:BUG-1920 | ai:claude
    #[test]
    fn detect_agent_type_live_marker_beats_stale_weak_prefix() {
        assert_eq!(
            detect_agent_type_from_env([("GEMINI_MODEL", "pro"), ("CLAUDECODE", "1")]),
            "claude"
        );
        assert_eq!(
            detect_agent_type_from_env([("GEMINI_CLI", "1")]),
            "antigravity"
        );
        assert_eq!(
            detect_agent_type_from_env([("CLAUDE_CONFIG_DIR", "/x")]),
            "claude"
        );
        assert_eq!(
            detect_agent_type_from_env(Vec::<(String, String)>::new()),
            "other"
        );
    }

    // trace:BUG-1920 | ai:claude
    #[test]
    fn detect_agent_type_explicit_override_wins() {
        assert_eq!(
            detect_agent_type_from_env([
                ("CLAUDECODE", "1"),
                ("GEMINI_API_KEY", "x"),
                ("AIDA_AGENT_TYPE", "codex"),
            ]),
            "codex"
        );
        // A blank override is ignored rather than forcing "other".
        assert_eq!(
            detect_agent_type_from_env([("AIDA_AGENT_TYPE", "  "), ("CLAUDECODE", "1")]),
            "claude"
        );
    }
}
