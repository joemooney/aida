//! Fork-from-live advisor — STORY-360.
//!
//! The `--no-human=both` orchestrator routes design-fork punts to a headless
//! advisor. By default that advisor cold-boots (a fresh `claude -p`) and only
//! sees the persistent substrate. SPIKE-11 verified that we can instead
//! **fork** a live advisor's session — copy the JSONL transcript and
//! `claude --resume` the copy — so the headless judgement boots with the full
//! in-flight context the live advisor built up.
//!
//! This module owns the pieces that decision needs:
//!
//!   - [`AdvisorConfig`] — `[advisor]` section in `.aida/config.toml`
//!     (`fork_mode`, `allow_mtime_fallback`, `keep_fork_jsonls`,
//!     `max_source_size_mb`).
//!   - [`AdvisorRegistration`] — the `~/.aida/advisor.toml` record the user
//!     writes with `aida advisor register` (uuid + project slug + pid).
//!   - [`discover_live_advisor_session`] — the discovery cascade: registration
//!     → `AIDA_ADVISOR_SESSION_UUID` env var → optional mtime fallback.
//!   - [`plan_fork`] — the top-level decision, returning a [`ForkPlan`] when
//!     the orchestrator should fork or `None` when it should cold-boot.
//!
//! Cold-boot remains the fallback whenever discovery returns `None`, the
//! source JSONL exceeds the cost ceiling, the registered session is dead, or
//! `fork_mode = "never"`.
//!
//! trace:STORY-360 | ai:claude

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::process_probe;
use crate::session;

/// How aggressively to fork the live advisor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkMode {
    /// Fork when a live advisor is registered; cold-boot otherwise. The
    /// default — the user opts into the cost by running `aida advisor
    /// register`, and the orchestrator falls back gracefully when they
    /// haven't.
    Auto,
    /// Fork whenever the source JSONL exists. Useful for the calibration
    /// loop (STORY-347) where every punt should produce a fork verdict.
    Always,
    /// Never fork — always cold-boot. The pre-STORY-360 behaviour.
    Never,
}

impl ForkMode {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(ForkMode::Auto),
            "always" => Some(ForkMode::Always),
            "never" => Some(ForkMode::Never),
            _ => None,
        }
    }
}

/// STORY-347 calibration mode — measure how often a cold-boot advisor and a
/// fork-from-live advisor *disagree* on the same punt. With it ON, every
/// punt produces both verdicts; cold-boot drives the drain, the fork is
/// shadow-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationMode {
    /// No calibration shadow-fork. Behaviour is byte-identical to today's
    /// drains — the default.
    Off,
    /// On every punt, fire a cold-boot advisor *and* (when a live advisor
    /// is registered) a fork-from-live advisor. Record both verdicts to
    /// `.aida/punts/<punt-id>/calibration.yaml`; the cold-boot verdict
    /// drives the drain, the fork is shadow only.
    On,
}

impl CalibrationMode {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "false" | "0" | "no" => Some(CalibrationMode::Off),
            "on" | "true" | "1" | "yes" => Some(CalibrationMode::On),
            _ => None,
        }
    }

    pub fn is_on(self) -> bool {
        matches!(self, CalibrationMode::On)
    }
}

/// `[advisor]` section in `.aida/config.toml`. Each field has a sensible
/// default so a project that has never written the section still has a
/// well-defined behaviour (the default `auto` + no live registration falls
/// through to today's cold-boot path).
#[derive(Debug, Clone)]
pub struct AdvisorConfig {
    pub fork_mode: ForkMode,
    pub allow_mtime_fallback: bool,
    pub keep_fork_jsonls: bool,
    pub max_source_size_mb: u64,
    /// trace:STORY-347 | ai:claude
    pub calibration_mode: CalibrationMode,
}

impl Default for AdvisorConfig {
    fn default() -> Self {
        Self {
            fork_mode: ForkMode::Auto,
            allow_mtime_fallback: false,
            keep_fork_jsonls: true,
            max_source_size_mb: 10,
            calibration_mode: CalibrationMode::Off,
        }
    }
}

impl AdvisorConfig {
    /// Load `[advisor]` from `<project_root>/.aida/config.toml`. Missing
    /// file / section / keys all fall through to defaults — a config error
    /// never blocks the orchestrator.
    pub fn load(project_root: &Path) -> Self {
        let mut cfg = AdvisorConfig::default();
        let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        else {
            return cfg;
        };
        for (key, val) in scan_advisor_section(&content) {
            apply_key(&mut cfg, &key, &val);
        }
        cfg
    }

    /// Build from a raw TOML string — used by the tests so they don't have
    /// to touch the filesystem.
    #[cfg(test)]
    pub fn from_toml_str(content: &str) -> Self {
        let mut cfg = AdvisorConfig::default();
        for (key, val) in scan_advisor_section(content) {
            apply_key(&mut cfg, &key, &val);
        }
        cfg
    }
}

fn apply_key(cfg: &mut AdvisorConfig, key: &str, val: &str) {
    match key {
        "fork_mode" => {
            if let Some(m) = ForkMode::parse(val) {
                cfg.fork_mode = m;
            }
        }
        "allow_mtime_fallback" => {
            if let Some(b) = parse_bool(val) {
                cfg.allow_mtime_fallback = b;
            }
        }
        "keep_fork_jsonls" => {
            if let Some(b) = parse_bool(val) {
                cfg.keep_fork_jsonls = b;
            }
        }
        "max_source_size_mb" => {
            if let Ok(n) = val.parse::<u64>() {
                cfg.max_source_size_mb = n;
            }
        }
        "calibration_mode" => {
            if let Some(m) = CalibrationMode::parse(val) {
                cfg.calibration_mode = m;
            }
        }
        _ => {}
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Extract `key = value` pairs from `[advisor]`. Section-aware; stops at the
/// next `[section]`. Mirrors the hand-rolled scanner already used by
/// `workflow_hints` and `aida-tui::config` so we don't pull a serde TOML
/// dependency for four scalars.
fn scan_advisor_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_advisor = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_advisor = stripped.trim_end_matches(']').trim() == "advisor";
            continue;
        }
        if in_advisor {
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

// --- Registration ---------------------------------------------------------

/// The record written by `aida advisor register` to `~/.aida/advisor.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorRegistration {
    /// Claude Code session UUID (the JSONL filename).
    pub uuid: String,
    /// The encoded project slug under `~/.claude/projects/<slug>/`, e.g.
    /// `-home-joe-ai-aida`. Used to locate the source JSONL.
    pub project_slug: String,
    /// Absolute path the live advisor session was launched in. Informational
    /// — `aida advisor status` shows it.
    pub project_root: String,
    /// RFC3339 timestamp the registration was written.
    pub registered_at: String,
    /// Best-effort: the live `claude` PID at registration time. The
    /// liveness check verifies the PID is still in the process table; falls
    /// back to JSONL mtime when the PID is missing or stale.
    pub claude_pid: Option<u32>,
}

/// Path to the per-user registration file (`~/.aida/advisor.toml`).
pub fn registration_path() -> Result<PathBuf> {
    let home = advisor_home_dir().context("HOME not set; cannot locate ~/.aida/advisor.toml")?;
    Ok(home.join(".aida").join("advisor.toml"))
}

fn advisor_home_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(home) = std::env::var_os("AIDA_TEST_HOME") {
        return Some(PathBuf::from(home));
    }
    dirs::home_dir()
}

/// Read the registration if it exists and parses. Returns `None` (not Err)
/// on missing-file / parse-failure so discovery can fall through cleanly.
pub fn read_registration() -> Option<AdvisorRegistration> {
    let path = registration_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str(&content).ok()
}

/// Write the registration, creating `~/.aida/` if needed.
pub fn write_registration(reg: &AdvisorRegistration) -> Result<()> {
    let path = registration_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let body = toml::to_string_pretty(reg).context("serialising advisor registration")?;
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Remove the registration. Idempotent — a missing file is not an error.
pub fn clear_registration() -> Result<()> {
    let path = registration_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

// --- Discovery ------------------------------------------------------------

/// A discovered live advisor session — what `discover_live_advisor_session`
/// found, ready to fork.
#[derive(Debug, Clone)]
pub struct LiveAdvisor {
    pub uuid: String,
    /// Source project slug under `~/.claude/projects/<slug>/` — recorded for
    /// the audit trail (the fork is written under the *spec's* slug, not
    /// this one, so a debugger can see exactly where the JSONL came from).
    #[allow(dead_code)]
    pub project_slug: String,
    pub source_jsonl: PathBuf,
    pub jsonl_size_bytes: u64,
    /// How the session was discovered — surfaced by the orchestrator's fork
    /// banner so an operator can see *why* a given session is being treated
    /// as live.
    pub discovery: Discovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discovery {
    Registration,
    EnvVar,
    MtimeFallback,
}

impl Discovery {
    pub fn label(self) -> &'static str {
        match self {
            Discovery::Registration => "registration",
            Discovery::EnvVar => "env-var",
            Discovery::MtimeFallback => "mtime-fallback",
        }
    }
}

/// JSONLs older than this are treated as a dead session by the freshness
/// check. Matches `process_probe::RECENT_JSONL_WINDOW` (one minute is short
/// enough that a quiescent session won't trick us into forking, long enough
/// to absorb a normal inter-tool-call gap).
pub const ALIVE_JSONL_WINDOW: Duration = Duration::from_secs(120);

/// Walk the discovery cascade and return the first live candidate. Returns
/// `None` to mean "cold-boot fallback."
///
/// 1. `~/.aida/advisor.toml` — if the JSONL exists and is alive.
/// 2. `AIDA_ADVISOR_SESSION_UUID` env var — when no registration is set.
/// 3. mtime fallback under the project root's slug — only when
///    `allow_mtime_fallback = true`.
pub fn discover_live_advisor_session(
    config: &AdvisorConfig,
    fallback_project_root: Option<&Path>,
) -> Option<LiveAdvisor> {
    if let Some(reg) = read_registration() {
        if let Some(advisor) = locate_session(&reg.uuid, &reg.project_slug, Discovery::Registration)
        {
            if is_alive(&advisor, reg.claude_pid) {
                return Some(advisor);
            }
        }
    }

    if let Ok(uuid) = std::env::var("AIDA_ADVISOR_SESSION_UUID") {
        let uuid = uuid.trim().to_string();
        if !uuid.is_empty() {
            // Without a recorded slug we have to scan every project dir for
            // a matching JSONL — relatively cheap (one stat per project).
            if let Some(advisor) = locate_session_by_uuid(&uuid, Discovery::EnvVar) {
                if is_alive(&advisor, None) {
                    return Some(advisor);
                }
            }
        }
    }

    if config.allow_mtime_fallback {
        if let Some(root) = fallback_project_root {
            if let Some(advisor) = latest_jsonl_under_project(root, Discovery::MtimeFallback) {
                if is_alive(&advisor, None) {
                    return Some(advisor);
                }
            }
        }
    }

    None
}

fn locate_session(uuid: &str, project_slug: &str, discovery: Discovery) -> Option<LiveAdvisor> {
    let home = advisor_home_dir()?;
    let jsonl = home
        .join(".claude")
        .join("projects")
        .join(project_slug)
        .join(format!("{}.jsonl", uuid));
    let meta = std::fs::metadata(&jsonl).ok()?;
    Some(LiveAdvisor {
        uuid: uuid.to_string(),
        project_slug: project_slug.to_string(),
        jsonl_size_bytes: meta.len(),
        source_jsonl: jsonl,
        discovery,
    })
}

fn locate_session_by_uuid(uuid: &str, discovery: Discovery) -> Option<LiveAdvisor> {
    let home = advisor_home_dir()?;
    let projects = home.join(".claude").join("projects");
    let dirs = std::fs::read_dir(&projects).ok()?;
    for d in dirs.flatten() {
        let candidate = d.path().join(format!("{}.jsonl", uuid));
        if let Ok(meta) = std::fs::metadata(&candidate) {
            let slug = d.file_name().to_string_lossy().to_string();
            return Some(LiveAdvisor {
                uuid: uuid.to_string(),
                project_slug: slug,
                jsonl_size_bytes: meta.len(),
                source_jsonl: candidate,
                discovery,
            });
        }
    }
    None
}

fn latest_jsonl_under_project(project_root: &Path, discovery: Discovery) -> Option<LiveAdvisor> {
    let slug = process_probe::encode_cwd_for_projects(project_root);
    let home = advisor_home_dir()?;
    let dir = home.join(".claude").join("projects").join(&slug);
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut newest: Option<(SystemTime, PathBuf, u64)> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let meta = entry.metadata().ok()?;
        let mtime = meta.modified().ok()?;
        let size = meta.len();
        if newest
            .as_ref()
            .map(|(prev, _, _)| mtime > *prev)
            .unwrap_or(true)
        {
            newest = Some((mtime, p, size));
        }
    }
    let (_, path, size) = newest?;
    let uuid = path.file_stem()?.to_string_lossy().to_string();
    Some(LiveAdvisor {
        uuid,
        project_slug: slug,
        jsonl_size_bytes: size,
        source_jsonl: path,
        discovery,
    })
}

/// Liveness check: the JSONL was touched recently OR the recorded `claude_pid`
/// is still alive. Either signal alone is enough — claude can spend longer
/// than the mtime window inside a single tool call.
pub fn is_alive(advisor: &LiveAdvisor, claude_pid: Option<u32>) -> bool {
    if let Some(pid) = claude_pid {
        if process_probe::pid_is_alive(pid) {
            return true;
        }
    }
    let Ok(meta) = std::fs::metadata(&advisor.source_jsonl) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(mtime)
        .map(|age| age <= ALIVE_JSONL_WINDOW)
        .unwrap_or(true)
}

// --- Fork planning --------------------------------------------------------

/// What the orchestrator needs to execute a fork: the source JSONL to copy,
/// the destination JSONL path under the spec's worktree project slug, and
/// the new UUID for `claude --resume`.
#[derive(Debug, Clone)]
pub struct ForkPlan {
    pub source_jsonl: PathBuf,
    pub fork_uuid: String,
    pub fork_jsonl: PathBuf,
    /// The discovered live advisor — kept for telemetry / status messages
    /// ("forked from registered session abc123…").
    pub live: LiveAdvisor,
}

/// Decide whether to fork. Returns `Some(ForkPlan)` when the orchestrator
/// should fork, `None` to fall back to cold-boot.
///
/// `spec_worktree` is where the orchestrator will `claude --resume` — its
/// encoded slug is where the fork JSONL must land so `claude --resume`
/// finds it on disk.
pub fn plan_fork(spec_worktree: &Path, config: &AdvisorConfig) -> Option<ForkPlan> {
    if matches!(config.fork_mode, ForkMode::Never) {
        return None;
    }
    let live = discover_live_advisor_session(config, Some(spec_worktree))?;
    let max_bytes = config.max_source_size_mb.saturating_mul(1024 * 1024);
    if max_bytes > 0 && live.jsonl_size_bytes > max_bytes {
        return None;
    }
    let fork_uuid = uuid::Uuid::now_v7().to_string();
    let dest_dir = session::claude_project_dir(spec_worktree).ok()?;
    let fork_jsonl = dest_dir.join(format!("{}.jsonl", fork_uuid));
    Some(ForkPlan {
        source_jsonl: live.source_jsonl.clone(),
        fork_uuid,
        fork_jsonl,
        live,
    })
}

/// Execute the fork — copy `source_jsonl` → `fork_jsonl`, creating the
/// destination project-slug directory if it doesn't yet exist. Returns the
/// number of bytes copied so the orchestrator can log it. Failures bubble up
/// so the caller decides whether to fall back to cold-boot.
pub fn execute_fork(plan: &ForkPlan) -> Result<u64> {
    if let Some(dir) = plan.fork_jsonl.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating fork project dir {}", dir.display()))?;
    }
    std::fs::copy(&plan.source_jsonl, &plan.fork_jsonl).with_context(|| {
        format!(
            "copying advisor JSONL {} → {}",
            plan.source_jsonl.display(),
            plan.fork_jsonl.display()
        )
    })
}

/// Rough $/fork cost estimate from a source JSONL size. The transcript is
/// loaded into cache; cache-creation pricing dominates (~$15/MTok on Opus
/// 4.7). Used by `aida advisor status` to surface "this'll cost ~$X per
/// punt at the current transcript size." Not load-bearing — purely a
/// visibility hint. trace:STORY-360 | ai:claude
pub fn estimated_fork_cost_usd(jsonl_size_bytes: u64) -> f64 {
    // SPIKE-11 data point: 1.3 MB JSONL ≈ 225K cache-creation tokens
    // → roughly 173K tokens / MB. Opus 4.7 cache-creation $15/MTok.
    let mb = jsonl_size_bytes as f64 / (1024.0 * 1024.0);
    let tokens = mb * 173_000.0;
    (tokens / 1_000_000.0) * 15.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_advisor_toml(content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
        std::fs::write(dir.path().join(".aida").join("config.toml"), content).unwrap();
        dir
    }

    #[test]
    fn config_defaults_when_no_section() {
        let cfg = AdvisorConfig::from_toml_str("[hints]\nworkflow_hints = true\n");
        assert_eq!(cfg.fork_mode, ForkMode::Auto);
        assert!(!cfg.allow_mtime_fallback);
        assert!(cfg.keep_fork_jsonls);
        assert_eq!(cfg.max_source_size_mb, 10);
        // STORY-347: calibration mode defaults Off — the spec promises
        // byte-identical behaviour when the user has not opted in.
        assert_eq!(cfg.calibration_mode, CalibrationMode::Off);
        assert!(!cfg.calibration_mode.is_on());
    }

    #[test]
    fn config_reads_calibration_mode_on() {
        let toml = "\
[advisor]
calibration_mode = \"on\"
";
        let cfg = AdvisorConfig::from_toml_str(toml);
        assert_eq!(cfg.calibration_mode, CalibrationMode::On);
        assert!(cfg.calibration_mode.is_on());
    }

    #[test]
    fn calibration_mode_parse_accepts_known_values() {
        assert_eq!(CalibrationMode::parse("on"), Some(CalibrationMode::On));
        assert_eq!(CalibrationMode::parse("ON"), Some(CalibrationMode::On));
        assert_eq!(CalibrationMode::parse("true"), Some(CalibrationMode::On));
        assert_eq!(CalibrationMode::parse("1"), Some(CalibrationMode::On));
        assert_eq!(CalibrationMode::parse("off"), Some(CalibrationMode::Off));
        assert_eq!(CalibrationMode::parse("false"), Some(CalibrationMode::Off));
        assert_eq!(CalibrationMode::parse("garbage"), None);
    }

    #[test]
    fn config_reads_all_advisor_keys() {
        let toml = "\
[advisor]
fork_mode = \"always\"
allow_mtime_fallback = true
keep_fork_jsonls = false
max_source_size_mb = 25
";
        let cfg = AdvisorConfig::from_toml_str(toml);
        assert_eq!(cfg.fork_mode, ForkMode::Always);
        assert!(cfg.allow_mtime_fallback);
        assert!(!cfg.keep_fork_jsonls);
        assert_eq!(cfg.max_source_size_mb, 25);
    }

    #[test]
    fn config_ignores_other_sections() {
        let toml = "\
[advisor]
fork_mode = \"never\"

[hints]
workflow_hints = false
";
        let cfg = AdvisorConfig::from_toml_str(toml);
        assert_eq!(cfg.fork_mode, ForkMode::Never);
    }

    #[test]
    fn config_loads_from_disk() {
        let dir = write_advisor_toml(
            "\
[advisor]
fork_mode = \"always\"
max_source_size_mb = 50
",
        );
        let cfg = AdvisorConfig::load(dir.path());
        assert_eq!(cfg.fork_mode, ForkMode::Always);
        assert_eq!(cfg.max_source_size_mb, 50);
    }

    #[test]
    fn fork_mode_parse_accepts_known_values() {
        assert_eq!(ForkMode::parse("auto"), Some(ForkMode::Auto));
        assert_eq!(ForkMode::parse("Always"), Some(ForkMode::Always));
        assert_eq!(ForkMode::parse("  NEVER  "), Some(ForkMode::Never));
        assert_eq!(ForkMode::parse("garbage"), None);
    }

    #[test]
    fn never_mode_short_circuits_discovery() {
        // Even with a perfectly viable registration in place, fork_mode=never
        // must skip the discovery cascade entirely.
        let cfg = AdvisorConfig {
            fork_mode: ForkMode::Never,
            ..AdvisorConfig::default()
        };
        let spec_worktree = std::env::temp_dir();
        // No registration written; with Never mode this returns None
        // regardless of what discovery would have found.
        let plan = plan_fork(&spec_worktree, &cfg);
        assert!(plan.is_none(), "fork_mode=never must never plan a fork");
    }

    #[test]
    fn discovery_returns_none_when_unregistered() {
        // With no registration, no env var, no mtime fallback, discovery
        // returns None — the orchestrator's signal to cold-boot.
        // Sandbox HOME so we don't accidentally read the developer's real
        // ~/.aida/advisor.toml.
        let home = tempfile::tempdir().unwrap();
        with_home(home.path(), || {
            let cfg = AdvisorConfig::default();
            let found = discover_live_advisor_session(&cfg, None);
            assert!(found.is_none());
        });
    }

    #[test]
    fn discovery_picks_registered_session_when_alive() {
        let home = tempfile::tempdir().unwrap();
        let slug = "-test-spec-worktree";
        let uuid = "019e0000-0000-7000-8000-000000000abc";

        // Build a fake live session JSONL under HOME.
        let session_dir = home.path().join(".claude").join("projects").join(slug);
        std::fs::create_dir_all(&session_dir).unwrap();
        let jsonl = session_dir.join(format!("{}.jsonl", uuid));
        let mut f = std::fs::File::create(&jsonl).unwrap();
        writeln!(f, "{{\"type\":\"user\"}}").unwrap();
        drop(f);

        // Write the registration.
        let reg = AdvisorRegistration {
            uuid: uuid.to_string(),
            project_slug: slug.to_string(),
            project_root: "/tmp/x".to_string(),
            registered_at: "2026-05-21T00:00:00Z".to_string(),
            claude_pid: None,
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let cfg = AdvisorConfig::default();
            let found = discover_live_advisor_session(&cfg, None).expect("registered+fresh → some");
            assert_eq!(found.uuid, uuid);
            assert_eq!(found.project_slug, slug);
            assert_eq!(found.discovery, Discovery::Registration);
        });
    }

    #[test]
    fn discovery_falls_through_when_registered_jsonl_missing() {
        let home = tempfile::tempdir().unwrap();
        let reg = AdvisorRegistration {
            uuid: "019e0000-0000-7000-8000-000000000def".to_string(),
            project_slug: "-no-such-slug".to_string(),
            project_root: "/tmp/x".to_string(),
            registered_at: "2026-05-21T00:00:00Z".to_string(),
            claude_pid: None,
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let cfg = AdvisorConfig::default();
            assert!(discover_live_advisor_session(&cfg, None).is_none());
        });
    }

    #[test]
    fn discovery_treats_stale_jsonl_as_dead() {
        let home = tempfile::tempdir().unwrap();
        let slug = "-test-stale";
        let uuid = "019e0000-0000-7000-8000-0000000staaa";

        let session_dir = home.path().join(".claude").join("projects").join(slug);
        std::fs::create_dir_all(&session_dir).unwrap();
        let jsonl = session_dir.join(format!("{}.jsonl", uuid));
        std::fs::write(&jsonl, "{}").unwrap();

        // Backdate the JSONL well past ALIVE_JSONL_WINDOW.
        let stale = SystemTime::now() - Duration::from_secs(60 * 60);
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&jsonl)
            .unwrap();
        let times = std::fs::FileTimes::new().set_modified(stale);
        f.set_times(times).unwrap();

        let reg = AdvisorRegistration {
            uuid: uuid.to_string(),
            project_slug: slug.to_string(),
            project_root: "/tmp/x".to_string(),
            registered_at: "2026-05-21T00:00:00Z".to_string(),
            claude_pid: None,
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let cfg = AdvisorConfig::default();
            assert!(
                discover_live_advisor_session(&cfg, None).is_none(),
                "stale JSONL should be treated as a dead session"
            );
        });
    }

    #[test]
    fn plan_fork_destination_uses_spec_worktree_slug_not_source_slug() {
        let home = tempfile::tempdir().unwrap();
        let source_slug = "-home-joe-ai-aida"; // the live advisor's project
        let spec_worktree = PathBuf::from("/home/joe/ai/aida-story-360");
        let expected_dest_slug = process_probe::encode_cwd_for_projects(&spec_worktree);
        let uuid = "019e0000-0000-7000-8000-0000000fork1";

        let session_dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join(source_slug);
        std::fs::create_dir_all(&session_dir).unwrap();
        let jsonl = session_dir.join(format!("{}.jsonl", uuid));
        std::fs::write(&jsonl, "{}\n").unwrap();

        let reg = AdvisorRegistration {
            uuid: uuid.to_string(),
            project_slug: source_slug.to_string(),
            project_root: "/home/joe/ai/aida".to_string(),
            registered_at: "2026-05-21T00:00:00Z".to_string(),
            claude_pid: None,
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let cfg = AdvisorConfig::default();
            let plan =
                plan_fork(&spec_worktree, &cfg).expect("registered+fresh + spec_worktree → plan");
            // The fork JSONL must land under the spec's slug, not the source's.
            let parent_slug = plan
                .fork_jsonl
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            assert_eq!(parent_slug, expected_dest_slug);
            assert_ne!(parent_slug, source_slug);
            assert_eq!(plan.source_jsonl, jsonl);
        });
    }

    #[test]
    fn plan_fork_respects_size_ceiling() {
        let home = tempfile::tempdir().unwrap();
        let slug = "-test-too-big";
        let uuid = "019e0000-0000-7000-8000-0000000bigggg";

        let session_dir = home.path().join(".claude").join("projects").join(slug);
        std::fs::create_dir_all(&session_dir).unwrap();
        let jsonl = session_dir.join(format!("{}.jsonl", uuid));
        // 2 MB of zeros — comfortably above a 1 MB cap.
        let big = vec![0u8; 2 * 1024 * 1024];
        std::fs::write(&jsonl, &big).unwrap();

        let reg = AdvisorRegistration {
            uuid: uuid.to_string(),
            project_slug: slug.to_string(),
            project_root: "/tmp/x".to_string(),
            registered_at: "2026-05-21T00:00:00Z".to_string(),
            claude_pid: None,
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let cfg = AdvisorConfig {
                max_source_size_mb: 1,
                ..AdvisorConfig::default()
            };
            assert!(
                plan_fork(&PathBuf::from("/tmp/spec-x"), &cfg).is_none(),
                "JSONL above max_source_size_mb must skip the fork"
            );
        });
    }

    #[test]
    fn registration_roundtrip() {
        let home = tempfile::tempdir().unwrap();
        let reg = AdvisorRegistration {
            uuid: "019e1111-1111-7111-8111-111111111111".to_string(),
            project_slug: "-home-joe-ai-aida".to_string(),
            project_root: "/home/joe/ai/aida".to_string(),
            registered_at: "2026-05-21T17:30:00Z".to_string(),
            claude_pid: Some(12345),
        };

        with_home(home.path(), || {
            write_registration(&reg).unwrap();
            let read = read_registration().expect("registration roundtrips");
            assert_eq!(read, reg);
            clear_registration().unwrap();
            assert!(read_registration().is_none());
            // clear_registration is idempotent.
            clear_registration().unwrap();
        });
    }

    #[test]
    fn estimated_cost_scales_with_size() {
        let one_mb = estimated_fork_cost_usd(1024 * 1024);
        let four_mb = estimated_fork_cost_usd(4 * 1024 * 1024);
        // Linear-ish in MB; 4 MB ≈ 4× 1 MB.
        assert!(one_mb > 0.0);
        assert!(four_mb > 3.5 * one_mb && four_mb < 4.5 * one_mb);
    }

    // --- env helper -------------------------------------------------------
    //
    // The discovery tests need to sandbox HOME so they don't read the
    // developer's real ~/.aida/advisor.toml. Tests can run in parallel
    // within a process, so we serialise HOME swaps with a mutex.

    fn with_home(home: &Path, f: impl FnOnce()) {
        let value = home.to_str().unwrap();
        with_env_vars(
            &[
                ("AIDA_TEST_HOME", Some(value)),
                ("HOME", Some(value)),
                ("USERPROFILE", Some(value)),
            ],
            f,
        );
    }

    fn with_env_vars(keys: &[(&str, Option<&str>)], f: impl FnOnce()) {
        // BUG-697: serialise on the ONE shared process-global env lock, not a
        // module-local mutex — env swaps under different locks still data-race.
        let _guard = crate::test_env::env_lock();
        let prior: Vec<(&str, Option<std::ffi::OsString>)> = keys
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        // SAFETY: serialised by ENV_LOCK so no other test mutates these keys.
        for (key, value) in keys {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (key, value) in prior {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        if let Err(p) = result {
            std::panic::resume_unwind(p);
        }
    }
}
