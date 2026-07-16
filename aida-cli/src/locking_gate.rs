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
//!     (`context-aware` default / `off` / `warn` / `enforce`), overridable
//!     per-run by the `AIDA_LOCKING` env var. Mirrors `advisor::AdvisorConfig`'s
//!     hand-rolled section-scanner pattern exactly (no serde-toml dependency
//!     for one scalar).
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
//! ## Default posture is context-aware, never a solo hard-block (TASK-958)
//!
//! `LockingConfig::default()` is `ContextAware` (TASK-958, replacing the old
//! `Off` default). On a `Refused` verdict it hard-blocks the commit ONLY under
//! a corroborated live drain (`orchestrator::detect(root).is_orchestrated()` —
//! a committing child under a genuinely-live drain/fan-out, the BUG-637
//! duplicate-dispatch hazard); a solo/manual commit — or one carrying a
//! stale/leaked `AIDA_AUTO_COMPLETE` — is downgraded to a warning and always
//! proceeds. `Unlocked`/`Authorized`
//! verdicts stay a silent no-op, so the gate only ever evaluates the fast
//! lock-state read and only acts on a real conflict. The explicit `off` posture
//! still short-circuits [`enforce_at_commit`] to `Ok(())` before any I/O for a
//! project that wants the pre-TASK-958 zero-gating stance.
//!
//! trace:TASK-1140 | ai:claude
//! trace:TASK-958 | ai:claude

use std::path::Path;

use anyhow::Result;
use colored::Colorize;

use aida_core::lock::LockingPosture;

/// `[locking]` section in `.aida/config.toml`. Mirrors `advisor::AdvisorConfig`'s
/// load pattern: missing file / section / key all fall through to the default
/// (`ContextAware`, TASK-958), so a config error never hard-blocks a solo
/// commit.
// trace:TASK-1140 | ai:claude
// trace:TASK-958 | ai:claude
#[derive(Debug, Clone, Copy)]
pub struct LockingConfig {
    pub posture: LockingPosture,
}

impl Default for LockingConfig {
    fn default() -> Self {
        Self {
            // TASK-958: the context-aware posture is the default — a Refused
            // commit hard-blocks only under an active drain, warns when solo.
            posture: LockingPosture::ContextAware,
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

/// PURE: does this orchestrator verdict count as an active drain for the
/// context-aware lock gate? Only a **corroborated** live orchestrator run
/// (`Orchestrated` — `AIDA_AUTO_COMPLETE` set AND its `AIDA_AUTO_COMPLETE_TOKEN`
/// names a live drain-state PID) counts. A bare/uncorroborated
/// `AIDA_AUTO_COMPLETE` (`Uncorroborated` — no token, or a token no live
/// orchestrator owns) does NOT, and neither does an ordinary session
/// (`Interactive`). So a stale/leaked `AIDA_AUTO_COMPLETE` env var never
/// hard-blocks a legit commit — only a genuinely-live fan-out does. This is the
/// one bit the `ContextAware` posture consults: a `Refused` commit hard-blocks
/// under a live drain, warns when solo. Factored out so the corroboration→gate
/// mapping is unit-testable without mutating the real process environment.
// trace:TASK-958 | ai:claude
fn under_live_drain(ctx: crate::orchestrator::OrchestratorContext) -> bool {
    ctx.is_orchestrated()
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
/// The default posture is `ContextAware` (TASK-958): on a `Refused` verdict it
/// hard-blocks only under a **corroborated live drain**
/// (`orchestrator::detect(root).is_orchestrated()` — `AIDA_AUTO_COMPLETE` set
/// AND its token names a live orchestrator run) and warns otherwise, so a
/// solo/manual commit (or one carrying a stale/leaked `AIDA_AUTO_COMPLETE`) is
/// never hard-blocked by the default.
/// The explicit `Off` posture still short-circuits to `Ok(())` WITHOUT reading
/// the lease directory or the launch-context snapshot — the fast, provably-no-op
/// path for a project that has opted out of `[locking]`. Under every other
/// posture the gate does a fast lock-state read and acts ONLY on a `Refused`
/// verdict (`Unlocked`/`Authorized` stay a silent `Allow`).
// trace:TASK-1140 | ai:claude
// trace:TASK-958 | ai:claude
pub(crate) fn enforce_at_commit(root: &Path) -> Result<()> {
    let posture = LockingConfig::load(root).posture;
    if posture == LockingPosture::Off {
        return Ok(());
    }
    let worktree_lock = crate::worktree_lock::read_authorized_by(root, root);
    let my_token = my_lock_token(root);
    // Corroborated live-drain signal (TASK-958): only a genuinely-live
    // orchestrator run hard-blocks — a bare/stale/leaked `AIDA_AUTO_COMPLETE`
    // is treated as solo (warn, never block). `detect` reads the env +
    // drain-state file; `is_orchestrated()` is true only when the token names a
    // live orchestrator PID.
    let under_drain = under_live_drain(crate::orchestrator::detect(root));
    match aida_core::lock::locking_gate(
        worktree_lock.as_deref(),
        my_token.as_deref(),
        posture,
        under_drain,
    ) {
        aida_core::lock::GateAction::Allow => Ok(()),
        aida_core::lock::GateAction::Warn { by } => {
            eprintln!(
                "{} this worktree is locked by advisor `{by}` and you carry no matching \
                 authorization token. Proceeding (warn-only — not under an active drain).",
                crate::glyph(crate::glyphs::Glyph::Warning).yellow()
            );
            Ok(())
        }
        aida_core::lock::GateAction::Refuse { by } => {
            anyhow::bail!(
                "Refusing commit: this worktree is locked by advisor `{by}` and you carry no \
                 matching authorization token, and this commit is landing under an active \
                 drain/fan-out.\n\
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
    fn default_config_posture_is_context_aware() {
        // TASK-958: the default flipped from Off to ContextAware.
        assert_eq!(
            LockingConfig::default().posture,
            LockingPosture::ContextAware
        );
    }

    #[test]
    fn load_falls_back_to_default_when_config_file_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = LockingConfig::load(tmp.path());
        assert_eq!(cfg.posture, LockingPosture::ContextAware);
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
        // Explicit `off` opt-out still parses.
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"off\"\n").posture,
            LockingPosture::Off
        );
        // Explicit `context-aware` parses (round-trips the default).
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"context-aware\"\n").posture,
            LockingPosture::ContextAware
        );
        // Missing key / section / unknown value → default (TASK-958: ContextAware).
        assert_eq!(
            LockingConfig::from_toml_str("").posture,
            LockingPosture::ContextAware
        );
        assert_eq!(
            LockingConfig::from_toml_str("[advisor]\nfork_mode = \"auto\"\n").posture,
            LockingPosture::ContextAware
        );
        assert_eq!(
            LockingConfig::from_toml_str("[locking]\nposture = \"bogus\"\n").posture,
            LockingPosture::ContextAware
        );
    }

    #[test]
    fn under_live_drain_only_for_a_corroborated_orchestrator() {
        use crate::orchestrator::{OrchestratorContext, UncorroboratedReason};
        // A corroborated live drain (token names a live orchestrator PID) is
        // the ONLY verdict that hard-blocks.
        assert!(under_live_drain(OrchestratorContext::Orchestrated));
        // A bare/uncorroborated AIDA_AUTO_COMPLETE → NOT a drain (warn, no hard
        // block): no token, or a token no live orchestrator owns.
        assert!(!under_live_drain(OrchestratorContext::Uncorroborated(
            UncorroboratedReason::NoToken
        )));
        assert!(!under_live_drain(OrchestratorContext::Uncorroborated(
            UncorroboratedReason::DeadOrchestrator
        )));
        // An ordinary interactive session is never a drain.
        assert!(!under_live_drain(OrchestratorContext::Interactive));
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

    /// The context-aware default (TASK-958) on a repo with no lock present:
    /// the verdict is `Unlocked`, so the gate is a silent no-op regardless of
    /// drain state. A repo with no `[locking]` config and no `AIDA_LOCKING`
    /// env is byte-for-byte unaffected when nothing is locked.
    // trace:TASK-958 | ai:claude
    #[test]
    fn context_aware_default_with_no_lock_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // No [locking] config → the ContextAware default; no lease → Unlocked.
        assert!(std::env::var("AIDA_LOCKING").is_err());
        assert!(!root.join(".aida").join("config.toml").exists());
        let result = enforce_at_commit(root);
        assert!(result.is_ok(), "expected Ok(()), got {:?}", result);
    }

    /// The load-bearing safety property (TASK-958): under the context-aware
    /// DEFAULT, a solo/manual commit against a mismatched lock WARNS and
    /// proceeds — it is never hard-blocked. `my_token` is None here (fail-safe
    /// Refused), which under a corroborated live drain would hard-block — but
    /// with no live orchestrator it only warns.
    // trace:TASK-958 | ai:claude
    #[test]
    fn context_aware_default_solo_mismatched_lock_warns_not_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        // No [locking] config → ContextAware default. AIDA_AUTO_COMPLETE unset
        // in the test runner → `orchestrator::detect` returns Interactive →
        // not a live drain → warn, not block.
        assert!(std::env::var("AIDA_LOCKING").is_err());
        assert!(std::env::var("AIDA_AUTO_COMPLETE").is_err());
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();
        let result = enforce_at_commit(root);
        assert!(
            result.is_ok(),
            "a solo commit must never be hard-blocked by the default: {:?}",
            result
        );
    }

    /// The explicit `off` opt-out short-circuits before any I/O even when a
    /// lock EXISTS and would mismatch — the pre-TASK-958 zero-gating stance.
    // trace:TASK-1140 | ai:claude
    // trace:TASK-958 | ai:claude
    #[test]
    fn off_posture_allows_even_a_mismatched_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root).unwrap();
        std::fs::create_dir_all(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida").join("config.toml"),
            "[locking]\nposture = \"off\"\n",
        )
        .unwrap();
        // Acquire a lock for "advisor-a" on this worktree.
        crate::worktree_lock::acquire(root, root, "advisor-a").unwrap();
        // No AIDA_AGENT_REGISTRY_TOKEN / matching snapshot → my_token is None,
        // which would normally be Refused fail-safe — but explicit posture Off.
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
