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
//! the agent's worktree pins the entry to Busy regardless of recency, so a
//! parked-but-running agent attached to scope still reads as occupied.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentRegistryEntry {
    pub(crate) id: String,
    pub(crate) agent_type: String,
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tty: Option<String>,
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
    pub(crate) name: Option<String>,
    pub(crate) tty: Option<String>,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) last_active_at: DateTime<Utc>,
    pub(crate) role: Option<String>,
    pub(crate) current_spec: Option<String>,
    pub(crate) worktree_path: PathBuf,
    pub(crate) source: String,
    pub(crate) binary_version: Option<String>,
    pub(crate) build_sha: Option<String>,
    pub(crate) status: AgentStatus,
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
        }
    }
}

/// `[agent_registry]` section in `.aida/config.toml`. Sensible default
/// (30s) means a project that never writes the section gets reasonable
/// busy/idle behaviour for free; missing file / section / keys all fall
/// through to defaults — a config error never blocks `aida status`.
/// trace:STORY-435 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) busy_threshold_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            busy_threshold_secs: 30,
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
            }
        }
        cfg
    }
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

fn agent_id(agent_type: &str, pid: u32) -> String {
    format!("{agent_type}-{pid}")
}

pub(crate) fn touch_mcp_agent(
    project_root: &Path,
    binary: &AgentBinaryIdentity,
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let pid = std::process::id();
    let agent_type = detect_agent_type();
    let id = agent_id(&agent_type, pid);
    let path = registry_path(project_root, &id);

    let mut entry = std::fs::read_to_string(&path)
        .ok()
        .and_then(|body| toml::from_str::<AgentRegistryEntry>(&body).ok())
        .unwrap_or_else(|| AgentRegistryEntry {
            id: id.clone(),
            agent_type: agent_type.clone(),
            pid,
            name: None,
            tty: current_tty(),
            started_at: now,
            last_active_at: now,
            role: None,
            current_spec: None,
            worktree_path: project_root.to_path_buf(),
            source: "mcp".to_string(),
            binary_version: None,
            build_sha: None,
        });

    entry.id = id;
    entry.agent_type = agent_type;
    entry.pid = pid;
    entry.last_active_at = now;
    entry.role = env_nonempty("AIDA_SESSION_ROLE").or(entry.role);
    entry.current_spec = env_nonempty("AIDA_SESSION_SCOPE")
        .filter(|s| looks_like_spec_id(s))
        .or(entry.current_spec);
    entry.worktree_path = project_root.to_path_buf();
    entry.source = "mcp".to_string();
    entry.binary_version = Some(binary.version.clone());
    entry.build_sha = Some(binary.sha.clone());
    if entry.tty.is_none() {
        entry.tty = current_tty();
    }

    write_entry(project_root, &entry)?;
    Ok(entry)
}

/// Register an agent process spawned by `aida agent new ...`.
///
/// Multiple concurrent sessions for the same project are intentionally
/// supported: registry IDs are `<agent-type>-<pid>`, so each process gets a
/// distinct entry even when cwd/spec match.
// trace:STORY-432 | ai:codex
pub(crate) fn register_spawned_agent(
    project_root: &Path,
    agent_type: &str,
    pid: u32,
    role: Option<String>,
    current_spec: Option<String>,
    worktree_path: PathBuf,
    binary: Option<&AgentBinaryIdentity>,
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let agent_type = normalize_agent_type(agent_type.to_string());
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        name: None,
        tty: current_tty(),
        started_at: now,
        last_active_at: now,
        role,
        current_spec,
        worktree_path,
        source: "agent-launcher".to_string(),
        binary_version: binary.map(|b| b.version.clone()),
        build_sha: binary.map(|b| b.sha.clone()),
    };
    write_entry(project_root, &entry)?;
    Ok(entry)
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
) -> Result<AgentRegistryEntry> {
    let now = Utc::now();
    let agent_type = normalize_agent_type(agent_type.to_string());
    let entry = AgentRegistryEntry {
        id: agent_id(&agent_type, pid),
        agent_type,
        pid,
        name,
        tty: process_tty(pid).or_else(current_tty),
        started_at: now,
        last_active_at: now,
        role: Some(role),
        current_spec,
        worktree_path,
        source: "manual-register".to_string(),
        binary_version: None,
        build_sha: None,
    };
    write_entry(project_root, &entry)?;
    Ok(entry)
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

pub(crate) fn list_agent_views(
    project_root: &Path,
    ctx: &AgentClassifyContext,
) -> Vec<AgentRegistryView> {
    let Ok(entries) = std::fs::read_dir(agents_dir(project_root)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
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
        out.push(view_for(record, ctx));
    }
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
            let identity = if agent.source == "lease" {
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
            format!(
                "  {:<15} {:<11} {:<12} {:<5} {:<8} {}{}",
                identity,
                agent.role.as_deref().unwrap_or("(none)"),
                agent.current_spec.as_deref().unwrap_or("(none)"),
                agent.status.as_str(),
                format!("({elapsed})"),
                agent.worktree_path.display(),
                source_note
            )
        })
        .collect()
}

fn write_entry(project_root: &Path, entry: &AgentRegistryEntry) -> Result<()> {
    let path = registry_path(project_root, &entry.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating agent registry dir {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(entry).context("serialising agent registry entry")?;
    aida_core::write_atomic(&path, body.as_bytes())
        .with_context(|| format!("writing agent registry entry {}", path.display()))
}

fn view_for(entry: AgentRegistryEntry, ctx: &AgentClassifyContext) -> AgentRegistryView {
    let pid_alive = crate::process_probe::pid_is_alive(entry.pid);
    let status = classify_status(&entry, pid_alive, ctx);
    AgentRegistryView {
        id: entry.id,
        agent_type: entry.agent_type,
        pid: entry.pid,
        name: entry.name,
        tty: entry.tty,
        started_at: entry.started_at,
        last_active_at: entry.last_active_at,
        role: entry.role,
        current_spec: entry.current_spec,
        worktree_path: entry.worktree_path,
        source: entry.source,
        binary_version: entry.binary_version,
        build_sha: entry.build_sha,
        status,
    }
}

// STORY-435 busy/idle: freshness signal from the MCP heartbeat. The lease-
// correlation branch deliberately keeps a parked-but-running agent attached
// to a Live lease as Busy — an idle process still "occupies" active scope
// from an operator's POV. trace:STORY-435 | ai:claude
fn classify_status(
    entry: &AgentRegistryEntry,
    pid_alive: bool,
    ctx: &AgentClassifyContext,
) -> AgentStatus {
    if !pid_alive {
        return AgentStatus::Stale;
    }
    let elapsed = elapsed_secs_clamped(ctx.now, entry.last_active_at);
    if elapsed < ctx.threshold_secs as i64 {
        return AgentStatus::Busy;
    }
    if covers(&ctx.live_lease_worktrees, &entry.worktree_path) {
        return AgentStatus::Busy;
    }
    AgentStatus::Idle
}

/// Elapsed seconds between `now` and `at`, clamped to `>= 0` so a clock
/// that ticked backwards (NTP step, suspend/resume) doesn't prematurely
/// flip a fresh entry to Idle. trace:STORY-435 | ai:claude
fn elapsed_secs_clamped(now: DateTime<Utc>, at: DateTime<Utc>) -> i64 {
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

fn humanize_elapsed(secs: i64) -> String {
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

pub(crate) fn detect_agent_type() -> String {
    if let Some(v) = env_nonempty("AIDA_AGENT_TYPE").map(normalize_agent_type) {
        return v;
    }
    if std::env::vars().any(|(k, _)| k.starts_with("CODEX_")) {
        return "codex".to_string();
    }
    if std::env::vars().any(|(k, _)| k.starts_with("ANTIGRAVITY_") || k.starts_with("GEMINI_")) {
        return "antigravity".to_string();
    }
    if std::env::vars().any(|(k, _)| k.starts_with("CLAUDE")) {
        return "claude".to_string();
    }
    "other".to_string()
}

pub(crate) fn normalize_agent_type(raw: String) -> String {
    match raw.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "claude" | "claudecode" => "claude".to_string(),
        "codex" => "codex".to_string(),
        "antigravity" | "gemini" => "antigravity".to_string(),
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
            name: None,
            tty: Some("/dev/pts/1".to_string()),
            started_at: last_active_at,
            last_active_at,
            role: Some("implementer".to_string()),
            current_spec: None,
            worktree_path: PathBuf::from("/tmp/aida-story"),
            source: "mcp".to_string(),
            binary_version: Some("0.9.1".to_string()),
            build_sha: Some("abc123".to_string()),
        }
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

    // STORY-435: stale activity but a Live lease covers the worktree → Busy.
    #[test]
    fn classify_status_live_lease_is_busy_despite_stale_activity() {
        let now = Utc::now();
        let e = entry_with(42, now - Duration::minutes(5));
        let c = ctx(now, 30, vec![PathBuf::from("/tmp/aida-story")]);
        assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
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

    #[test]
    fn format_agent_status_lines_appends_freshness_hint() {
        let now = Utc::now();
        let view = AgentRegistryView {
            id: "codex-42".to_string(),
            agent_type: "codex".to_string(),
            pid: 42,
            name: None,
            tty: None,
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
        };

        let lines = format_agent_status_lines(&[view]);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        // Strict column-shape assertion for the static part; the (Xs)
        // trailing hint is loose because Utc::now() inside the formatter
        // can tick a second on a slow runner.
        assert!(
            line.starts_with("  codex#42        implementer STORY-431    busy  ("),
            "unexpected line: {line:?}"
        );
        assert!(
            line.ends_with(" /tmp/aida-story-431"),
            "unexpected line: {line:?}"
        );
        assert!(line.contains("s)"), "expected (Xs) hint, got: {line:?}");
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

        let cfg = Config::from_toml_str("[other]\nfoo = 1\n");
        assert_eq!(cfg.busy_threshold_secs, 30);
    }

    #[test]
    fn config_parses_busy_threshold_secs() {
        let cfg = Config::from_toml_str("[agent_registry]\nbusy_threshold_secs = 90\n");
        assert_eq!(cfg.busy_threshold_secs, 90);
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

    #[test]
    fn looks_like_spec_id_accepts_canonical_ids_only() {
        assert!(looks_like_spec_id("STORY-431"));
        assert!(looks_like_spec_id("TASK-498"));
        assert!(!looks_like_spec_id("story-431"));
        assert!(!looks_like_spec_id("branch-name"));
    }
}
