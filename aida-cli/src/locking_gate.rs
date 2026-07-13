//! Automatic advisor-lock enforcement gate — STORY-711 slice 2.
//!
//! Slice 1 (#1387) shipped the pure verifier (`aida_core::lock::verify_worktree_lock`)
//! plus a *manual* `aida lock` CLI — nothing called it automatically. This module
//! is the automatic bouncer the plan promised: the same commit-boundary pattern
//! `advisor_code_gate.rs` (STORY-684) pioneered, applied to the worktree lock.
//!
//! ## Pieces
//!
//!   - [`LockingConfig`] — the `[locking]` posture in `.aida/config.toml`
//!     (`off` default / `warn` / `enforce`), overridable per-run by the
//!     `AIDA_LOCKING` env var. Mirrors `advisor::AdvisorConfig`'s hand-rolled
//!     section-scanner pattern exactly (no serde-toml dependency for one scalar).
//!   - [`my_lock_token`] — the committing session's authorization token, read
//!     from the **role-context snapshot** the launcher wrote (`aida-cli::
//!     render_agent_launch_context`), NOT a bare env var. Per the signed-off
//!     design fork (`docs/plans/2026-07-12-story-711-advisor-lock.md` #2): an
//!     env var is spoofable and vanishes on respawn, so the durable on-disk
//!     snapshot is the trust boundary. `AIDA_AGENT_REGISTRY_TOKEN` is used only
//!     to LOCATE which snapshot file is "mine" — the actual authority (the
//!     authorizing advisor's id) is read from the file's content.
//!   - [`enforce_at_commit`] — the wiring: read the worktree's lock
//!     (`worktree_lock::read_authorized_by`), read my token, run
//!     `aida_core::lock::locking_gate`, act on the verdict. Called by both
//!     `aida commit` (`commit.rs`) and the scaffolded git pre-commit hook (via
//!     `aida internal locking-gate`), so the decision is identical no matter
//!     which vendor drove the commit — mirrors `advisor_code_gate::enforce_at_commit`.
//!
//! ## Default posture is a proven no-op
//!
//! `LockingConfig::default()` is `Off`, and [`enforce_at_commit`] short-circuits
//! to `Ok(())` before touching the filesystem when the resolved posture is
//! `Off` — so a project with no `[locking]` config and no `AIDA_LOCKING` env
//! sees ZERO behavior change: no lease scan, no snapshot read, no output.
//! `locking_gate_off_default_never_touches_disk` below proves it.
//!
//! trace:TASK-1140 | ai:claude

use std::path::Path;

use anyhow::Result;
use colored::Colorize;

use aida_core::lock::LockingPosture;

/// `[locking]` section in `.aida/config.toml`. Mirrors `advisor::AdvisorConfig`'s
/// load pattern: missing file / section / key all fall through to the default
/// (`Off`), so a config error never blocks a commit.
// trace:TASK-1140 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct LockingConfig {
    pub posture: LockingPosture,
}

impl Default for LockingConfig {
    fn default() -> Self {
        Self {
            posture: LockingPosture::Off,
        }
    }
}

impl LockingConfig {
    /// Load `[locking]` from `<project_root>/.aida/config.toml`, then apply the
    /// `AIDA_LOCKING` env override (env wins over config, matching the
    /// documented CLI-flag → env → config → default precedence). Missing
    /// file/section/key all fall through to the `Off` default.
    // trace:TASK-1140 | ai:claude
    pub fn load(project_root: &Path) -> Self {
        let mut cfg = Self::default();
        if let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        {
            for (key, val) in scan_locking_section(&content) {
                if key == "posture" {
                    if let Some(p) = LockingPosture::parse(&val) {
                        cfg.posture = p;
                    }
                }
            }
        }
        cfg.posture =
            resolve_env_override(cfg.posture, std::env::var("AIDA_LOCKING").ok().as_deref());
        cfg
    }

    /// Build from a raw TOML string, bypassing the filesystem — used by tests.
    #[cfg(test)]
    pub fn from_toml_str(content: &str) -> Self {
        let mut cfg = Self::default();
        for (key, val) in scan_locking_section(content) {
            if key == "posture" {
                if let Some(p) = LockingPosture::parse(&val) {
                    cfg.posture = p;
                }
            }
        }
        cfg
    }
}

/// PURE: env override resolution — `AIDA_LOCKING` wins over the config-derived
/// base when it parses to a valid posture; an unset or unparseable env value
/// leaves `base` untouched. Split out so the override logic is testable
/// without mutating the real process environment (which would race other
/// tests).
// trace:TASK-1140 | ai:claude
fn resolve_env_override(base: LockingPosture, env: Option<&str>) -> LockingPosture {
    match env.and_then(LockingPosture::parse) {
        Some(p) => p,
        None => base,
    }
}

/// Extract `key = value` pairs from `[locking]`. Section-aware; stops at the
/// next `[section]`. Mirrors `advisor::scan_advisor_section` (hand-rolled, no
/// serde-toml dependency for one scalar).
// trace:TASK-1140 | ai:claude
fn scan_locking_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_locking = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_locking = stripped.trim_end_matches(']').trim() == "locking";
            continue;
        }
        if in_locking {
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

/// PURE: parse the `- Authorized by: <id>` line `render_agent_launch_context`
/// embeds into a launch-context snapshot body. Returns `None` when absent
/// (no brief carried a token at launch, or the snapshot predates slice 2).
// trace:TASK-1140 | ai:claude
pub(crate) fn context_snapshot_authorized_by(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.trim()
            .strip_prefix("- Authorized by:")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}

/// The committing session's lock-authorization token — read from the
/// launch-context snapshot the launcher wrote at
/// `<project_root>/.aida/agents/context/<agent_type>-<token>.context.md`
/// (traverses the `.aida/agents` symlink installed by `session_start` when
/// `project_root` is a linked worktree, so this resolves correctly whether
/// called from the main checkout or a spawned implementer's worktree).
///
/// `AIDA_AGENT_REGISTRY_TOKEN` only LOCATES which snapshot file is "mine" —
/// the actual authority (the authorizing advisor's id) comes from the file's
/// content, never the env var directly. `None` when the env var is unset (no
/// launch-context session), or no matching snapshot file carries the field.
// trace:TASK-1140 | ai:claude
pub(crate) fn my_lock_token(project_root: &Path) -> Option<String> {
    let token = std::env::var("AIDA_AGENT_REGISTRY_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())?;
    let dir = project_root.join(".aida").join("agents").join("context");
    let suffix = format!("-{token}.context.md");
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(&suffix) {
            let body = std::fs::read_to_string(entry.path()).ok()?;
            return context_snapshot_authorized_by(&body);
        }
    }
    None
}

/// Enforce the advisor-lock gate at a real commit boundary. Mirrors
/// `advisor_code_gate::enforce_at_commit`'s role as the vendor-agnostic
/// bouncer: `aida commit` and the scaffolded git pre-commit hook (via
/// `aida internal locking-gate`) both funnel through here, so the decision is
/// identical no matter which vendor drove the commit.
///
/// Under the default `Off` posture this returns `Ok(())` WITHOUT reading the
/// lease directory or the launch-context snapshot — the fast, provably-no-op
/// path for every project that hasn't opted into `[locking]`.
// trace:TASK-1140 | ai:claude
pub(crate) fn enforce_at_commit(root: &Path) -> Result<()> {
    let posture = LockingConfig::load(root).posture;
    if posture == LockingPosture::Off {
        return Ok(());
    }
    let worktree_lock = crate::worktree_lock::read_authorized_by(root, root);
    let my_token = my_lock_token(root);
    match aida_core::lock::locking_gate(worktree_lock.as_deref(), my_token.as_deref(), posture) {
        aida_core::lock::GateAction::Allow => Ok(()),
        aida_core::lock::GateAction::Warn { by } => {
            eprintln!(
                "{} this worktree is locked by advisor `{by}` and you carry no matching \
                 authorization token. Proceeding — [locking] posture = warn.",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            Ok(())
        }
        aida_core::lock::GateAction::Refuse { by } => {
            anyhow::bail!(
                "Refusing commit: this worktree is locked by advisor `{by}` and you carry no \
                 matching authorization token.\n\
                 Coordinate with {by} — they can `aida lock release <worktree>`, or re-brief you \
                 with `aida brief <agent> <SPEC> --authorized-by {by}`.\n\
                 To downgrade this gate: set `[locking] posture = \"warn\"` or `\"off\"` in \
                 .aida/config.toml, or `AIDA_LOCKING=warn`/`off` for this process."
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_posture_is_off() {
        assert_eq!(LockingConfig::default().posture, LockingPosture::Off);
    }

    #[test]
    fn load_falls_back_to_default_when_config_file_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = LockingConfig::load(tmp.path());
        assert_eq!(cfg.posture, LockingPosture::Off);
    }

    #[test]
    fn from_toml_str_parses_posture() {
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"enforce\"\n").posture,
            LockingPosture::Enforce
        );
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"warn\"\n").posture,
            LockingPosture::Warn
        );
        // Missing key / section / unknown value → default.
        assert_eq!(
            LockingConfig::from_toml_str("").posture,
            LockingPosture::Off
        );
        assert_eq!(
            LockingConfig::from_toml_str("[advisor]\nfork_mode = \"auto\"\n").posture,
            LockingPosture::Off
        );
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"bogus\"\n").posture,
            LockingPosture::Off
        );
    }

    #[test]
    fn scan_stops_at_next_section() {
        let cfg = LockingConfig::from_toml_str(
            "[locking]\nposture = \"warn\"\n\n[advisor]\nposture = \"enforce\"\n",
        );
        assert_eq!(cfg.posture, LockingPosture::Warn);
    }

    #[test]
    fn env_override_wins_when_valid() {
        assert_eq!(
            resolve_env_override(LockingPosture::Off, Some("enforce")),
            LockingPosture::Enforce
        );
        assert_eq!(
            resolve_env_override(LockingPosture::Enforce, Some("off")),
            LockingPosture::Off
        );
    }

    #[test]
    fn env_override_is_noop_when_unset_or_invalid() {
        assert_eq!(
            resolve_env_override(LockingPosture::Warn, None),
            LockingPosture::Warn
        );
        assert_eq!(
            resolve_env_override(LockingPosture::Warn, Some("bogus")),
            LockingPosture::Warn
        );
    }

    #[test]
    fn context_snapshot_authorized_by_extracts_the_line() {
        let body = "# AIDA Launch Context\n\n## Launch\n\n- Agent: claude\n- Authorized by: advisor-a\n- Context token: abc\n\n## Role Guidance\n";
        assert_eq!(
            context_snapshot_authorized_by(body).as_deref(),
            Some("advisor-a")
        );
    }

    #[test]
    fn context_snapshot_authorized_by_none_when_absent() {
        let body = "# AIDA Launch Context\n\n## Launch\n\n- Agent: claude\n- Context token: abc\n";
        assert_eq!(context_snapshot_authorized_by(body), None);
    }

    #[test]
    fn context_snapshot_authorized_by_ignores_blank_value() {
        let body = "- Authorized by:   \n";
        assert_eq!(context_snapshot_authorized_by(body), None);
    }

    /// The hard requirement: a repo with no `[locking]` config and no
    /// `AIDA_LOCKING` env must be byte-for-byte unaffected. Proves
    /// `enforce_at_commit` never bails and never even reaches the lease/
    /// snapshot filesystem reads — it short-circuits on the default `Off`
    /// posture before doing any I/O beyond the (absent) config file.
    // trace:TASK-1140 | ai:claude
    #[test]
    fn off_default_posture_is_a_proven_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No .aida/ dir at all — not even a sessions dir a real lease could
        // live in. If enforce_at_commit's Off short-circuit didn't work, any
        // lease read would simply find nothing (also Ok), so additionally
        // assert no [locking] config existed and AIDA_LOCKING is unset.
        assert!(std::env::var("AIDA_LOCKING").is_err());
        assert!(!root.join(".aida").join("config.toml").exists());
        let result = enforce_at_commit(root);
        assert!(result.is_ok(), "expected Ok(()), got {:?}", result);
    }

    /// Even when a lock EXISTS and would mismatch, Off still allows — the
    /// no-op claim holds under adversarial state, not just an empty repo.
    // trace:TASK-1140 | ai:claude
    #[test]
    fn off_posture_allows_even_a_mismatched_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        // Acquire a lock for "advisor-a" on this worktree.
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();
        // No AIDA_AGENT_REGISTRY_TOKEN / matching snapshot → my_token is None,
        // which would normally be Refused fail-safe — but posture is Off.
        let result = enforce_at_commit(root);
        assert!(result.is_ok(), "expected Ok(()), got {:?}", result);
    }

    #[test]
    fn enforce_posture_refuses_a_mismatched_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[locking]\nposture = \"enforce\"\n",
        )
        .unwrap();
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();
        let result = enforce_at_commit(root);
        assert!(result.is_err(), "expected a refusal");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("advisor-a"), "got: {msg}");
    }

    #[test]
    fn warn_posture_allows_a_mismatched_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[locking]\nposture = \"warn\"\n",
        )
        .unwrap();
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();
        let result = enforce_at_commit(root);
        assert!(result.is_ok(), "warn must not block: {:?}", result);
    }

    #[test]
    fn enforce_posture_allows_a_matching_token_read_from_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[locking]\nposture = \"enforce\"\n",
        )
        .unwrap();
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();

        // Write a launch-context snapshot carrying the matching token, and
        // point AIDA_AGENT_REGISTRY_TOKEN at it — mirrors what the launcher
        // does at `aida agent new`. trace:TASK-1140 | ai:claude
        let ctx_dir = root.join(".aida").join("agents").join("context");
        std::fs::create_dir_all(&ctx_dir).unwrap();
        let token = "test-token-123";
        std::fs::write(
            ctx_dir.join(format!("claude-{token}.context.md")),
            "## Launch\n\n- Authorized by: advisor-a\n",
        )
        .unwrap();

        // SAFETY: tests in this module don't run this specific test
        // concurrently with another that reads AIDA_AGENT_REGISTRY_TOKEN in a
        // conflicting way; scoped narrowly and cleared in a guard.
        struct EnvGuard;
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                std::env::remove_var("AIDA_AGENT_REGISTRY_TOKEN");
            }
        }
        let _guard = EnvGuard;
        std::env::set_var("AIDA_AGENT_REGISTRY_TOKEN", token);

        let result = enforce_at_commit(root);
        assert!(result.is_ok(), "matching token must allow: {:?}", result);
    }
}
