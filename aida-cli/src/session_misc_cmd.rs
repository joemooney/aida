//! Small self-contained session/identity command handlers lifted out of
//! `main.rs` (SPIKE-78): `aida whoami` (caller-identity read) and the internal
//! `_bg-fetch` background/two-leg refresh worker. Pure movement — behavior
//! unchanged. Shared helpers (`current_user_id`, `canonical_role_name`,
//! `bg_fetch_lock_path`, `write_last_fetch_entry`, `handle_fetch_command`)
//! stay in `main.rs` and are reached via `crate::`.

use anyhow::Result;
use colored::Colorize;

use crate::*;

pub(crate) fn whoami_user_source(
    aida_user: Option<&str>,
    user: Option<&str>,
    username: Option<&str>,
) -> &'static str {
    fn set(o: Option<&str>) -> bool {
        o.is_some_and(|v| !v.is_empty())
    }
    if set(aida_user) {
        "from AIDA_USER"
    } else if set(user) {
        "from USER"
    } else if set(username) {
        "from USERNAME"
    } else {
        "default"
    }
}

/// `aida whoami` — print the caller identity AIDA resolved, each line
/// annotated with where the value came from. This is a PURE READ of the same
/// resolvers the gating / queue / provenance code already uses
/// (`current_user_id`, `agent_registry::detect_agent_type`, and the
/// `AIDA_SESSION_*` / `AIDA_AGENT_*` / `AIDA_HEADLESS` / `AIDA_AI_TOOL` env
/// reads) — no project store is loaded and no state is written. It exists to
/// answer the recurring "why did this refuse?" (role resolved to a default,
/// not advisor, so the advisor-gate fired) and "why is my queue empty?" (the
/// shell's user/role identity differs from whatever queued the items — see the
/// BUG-89 queue-identity note in CLAUDE.md).
// trace:TASK-784
pub(crate) fn handle_whoami_command() -> Result<()> {
    // Helper: read an env var, returning the value + a source label that names
    // the env var when set (non-empty) and the fallback otherwise.
    fn env_with_source(key: &str, fallback: &str) -> (String, String) {
        match std::env::var(key) {
            Ok(v) if !v.trim().is_empty() => (v, format!("from {key}")),
            _ => (fallback.to_string(), "default".to_string()),
        }
    }

    println!("{}", "AIDA caller identity".bold());

    // Role — read of $AIDA_SESSION_ROLE, canonicalized exactly the way the
    // gating/queue/routing code reads it (dialog → advisor, human casing).
    // Unset means no role is seated; the gating code treats that as the
    // implementer default, so name that explicitly — it's the value that
    // surprises people at the advisor-gate.
    match std::env::var("AIDA_SESSION_ROLE") {
        Ok(raw) if !raw.trim().is_empty() => {
            let canonical = canonical_role_name(&raw);
            if canonical != raw {
                println!(
                    "  role:       {} (from AIDA_SESSION_ROLE={}, canonicalized)",
                    canonical, raw
                );
            } else {
                println!("  role:       {} (from AIDA_SESSION_ROLE)", canonical);
            }
        }
        _ => println!("  role:       implementer (default — no AIDA_SESSION_ROLE seated)"),
    }

    // Agent type — detect_agent_type prefers $AIDA_AGENT_TYPE, then sniffs the
    // env (CODEX_* / ANTIGRAVITY_* / GEMINI_* / CLAUDE*), else "other".
    let agent_type = agent_registry::detect_agent_type();
    let agent_type_source = match std::env::var("AIDA_AGENT_TYPE") {
        Ok(v) if !v.trim().is_empty() => "from AIDA_AGENT_TYPE".to_string(),
        _ => "env-sniff fallback".to_string(),
    };
    println!("  agent-type: {} ({})", agent_type, agent_type_source);

    // Agent name.
    let (agent_name, agent_name_src) = env_with_source("AIDA_AGENT_NAME", "(none)");
    println!("  agent-name: {} ({})", agent_name, agent_name_src);

    // User id — the queue/provenance identity (BUG-89). Mirror current_user_id's
    // resolution order so the source label is precise.
    let user_id = current_user_id(None);
    let user_source = whoami_user_source(
        std::env::var("AIDA_USER").ok().as_deref(),
        std::env::var("USER").ok().as_deref(),
        std::env::var("USERNAME").ok().as_deref(),
    );
    println!("  user-id:    {} ({})", user_id, user_source);

    // Headless flag — set to "1" by the headless launchers.
    match std::env::var("AIDA_HEADLESS") {
        Ok(v) if !v.trim().is_empty() => {
            println!("  headless:   {} (from AIDA_HEADLESS)", v)
        }
        _ => println!("  headless:   no (default — AIDA_HEADLESS unset)"),
    }

    // AI tool — provenance stamp source.
    let (ai_tool, ai_tool_src) = env_with_source("AIDA_AI_TOOL", "(none)");
    println!("  ai-tool:    {} ({})", ai_tool, ai_tool_src);

    // Active session / scope — the AIDA_SESSION_* family.
    let (scope, scope_src) = env_with_source("AIDA_SESSION_SCOPE", "(none)");
    println!("  scope:      {} ({})", scope, scope_src);
    let (project, project_src) = env_with_source("AIDA_SESSION_PROJECT", "(none)");
    println!("  project:    {} ({})", project, project_src);
    let (purpose, purpose_src) = env_with_source("AIDA_SESSION_PURPOSE", "(none)");
    println!("  purpose:    {} ({})", purpose, purpose_src);
    let (session_id, session_id_src) = env_with_source("AIDA_SESSION_ID", "(none)");
    println!("  session-id: {} ({})", session_id, session_id_src);

    Ok(())
}

pub(crate) fn handle_bg_fetch_command(store_path: &std::path::Path) -> Result<()> {
    // Always try to clean up the lockfile, even on early-exit paths.
    // Drop guard via a small helper struct so panics / early returns
    // never leave a stale lock.
    struct LockGuard(Option<std::path::PathBuf>);
    impl Drop for LockGuard {
        fn drop(&mut self) {
            if let Some(p) = &self.0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let _guard = LockGuard(bg_fetch_lock_path(store_path));

    if !aida_core::git_ops::is_git_repo(store_path) {
        // No git repo → record an error so the user gets `cache:?` not
        // a false-fresh next render. trace:STORY-79 | ai:claude
        let _ = write_last_fetch_entry(store_path, "error: not a git repo");
        return Ok(());
    }
    if !aida_core::git_ops::has_remote(store_path, "origin") {
        // No origin → record an error rather than letting the statusline
        // re-spawn us in a tight loop. Shared cache key with
        // `handle_fetch_command`. trace:STORY-79 TASK-107 | ai:claude
        let _ = write_last_fetch_entry(store_path, "error: no origin remote");
        return Ok(());
    }
    // Reuse the shared `aida fetch --store-only --quiet` path so the
    // background fetcher and the user-facing verb stay in lock-step on
    // git args, error reporting, and cache stamping. TASK-107 acceptance
    // criterion: "at least one existing caller refactored to use aida
    // fetch". trace:TASK-107 | ai:claude
    let _ = handle_fetch_command(
        store_path, /* code_only */ false, /* store_only */ true, /* quiet */ true,
    );
    Ok(())
}
