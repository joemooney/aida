//! Phase-1 agent process registry.
//!
//! STORY-431 deliberately keeps process liveness separate from session leases:
//! leases describe scope ownership, while `.aida/agents/*.toml` describes an
//! observable process. Phase 1 only registers MCP-serving processes because
//! AIDA does not yet launch every agent; Phase 2/3 launchers will widen
//! coverage to interactive sessions that never call MCP tools.

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

pub(crate) fn list_agent_views(project_root: &Path) -> Vec<AgentRegistryView> {
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
        out.push(view_for(record));
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
    agents
        .iter()
        .map(|agent| {
            format!(
                "  {:<15} {:<11} {:<12} {:<5} {}",
                format!("{}#{}", agent.agent_type, agent.pid),
                agent.role.as_deref().unwrap_or("(none)"),
                agent.current_spec.as_deref().unwrap_or("(none)"),
                agent.status.as_str(),
                agent.worktree_path.display()
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

fn view_for(entry: AgentRegistryEntry) -> AgentRegistryView {
    let status = classify_status(&entry, crate::process_probe::pid_is_alive(entry.pid));
    AgentRegistryView {
        id: entry.id,
        agent_type: entry.agent_type,
        pid: entry.pid,
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

fn classify_status(entry: &AgentRegistryEntry, pid_alive: bool) -> AgentStatus {
    if !pid_alive {
        AgentStatus::Stale
    } else if entry.current_spec.is_some() {
        AgentStatus::Busy
    } else {
        AgentStatus::Idle
    }
}

fn detect_agent_type() -> String {
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

fn normalize_agent_type(raw: String) -> String {
    match raw.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
        "claude" | "claudecode" => "claude".to_string(),
        "codex" => "codex".to_string(),
        "antigravity" | "gemini" => "antigravity".to_string(),
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

#[cfg(not(unix))]
fn current_tty() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(pid: u32, spec: Option<&str>) -> AgentRegistryEntry {
        AgentRegistryEntry {
            id: agent_id("codex", pid),
            agent_type: "codex".to_string(),
            pid,
            tty: Some("/dev/pts/1".to_string()),
            started_at: Utc::now(),
            last_active_at: Utc::now(),
            role: Some("implementer".to_string()),
            current_spec: spec.map(str::to_string),
            worktree_path: PathBuf::from("/tmp/aida-story"),
            source: "mcp".to_string(),
            binary_version: Some("0.9.1".to_string()),
            build_sha: Some("abc123".to_string()),
        }
    }

    #[test]
    fn registry_path_lives_under_aida_agents() {
        let root = Path::new("/tmp/project");
        assert_eq!(
            registry_path(root, "codex-123"),
            PathBuf::from("/tmp/project/.aida/agents/codex-123.toml")
        );
    }

    #[test]
    fn status_classification_marks_stale_before_busy() {
        assert_eq!(
            classify_status(&entry(42, Some("STORY-431")), false),
            AgentStatus::Stale
        );
        assert_eq!(
            classify_status(&entry(42, Some("STORY-431")), true),
            AgentStatus::Busy
        );
        assert_eq!(classify_status(&entry(42, None), true), AgentStatus::Idle);
    }

    #[test]
    fn list_agent_views_reads_toml_and_computes_stale_status() {
        let tmp = TempDir::new().unwrap();
        let record = entry(u32::MAX - 1, Some("STORY-431"));
        write_entry(tmp.path(), &record).unwrap();

        let views = list_agent_views(tmp.path());
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].agent_type, "codex");
        assert_eq!(views[0].status, AgentStatus::Stale);
    }

    #[test]
    fn format_agent_status_lines_matches_status_section_columns() {
        let view = AgentRegistryView {
            id: "codex-42".to_string(),
            agent_type: "codex".to_string(),
            pid: 42,
            tty: None,
            started_at: Utc::now(),
            last_active_at: Utc::now(),
            role: Some("implementer".to_string()),
            current_spec: Some("STORY-431".to_string()),
            worktree_path: PathBuf::from("/tmp/aida-story-431"),
            source: "mcp".to_string(),
            binary_version: None,
            build_sha: None,
            status: AgentStatus::Busy,
        };

        let lines = format_agent_status_lines(&[view]);
        assert_eq!(
            lines,
            vec!["  codex#42        implementer STORY-431    busy  /tmp/aida-story-431"]
        );
    }

    #[test]
    fn looks_like_spec_id_accepts_canonical_ids_only() {
        assert!(looks_like_spec_id("STORY-431"));
        assert!(looks_like_spec_id("TASK-498"));
        assert!(!looks_like_spec_id("story-431"));
        assert!(!looks_like_spec_id("branch-name"));
    }
}
