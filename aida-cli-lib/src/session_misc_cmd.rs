//! Small self-contained session/identity command handlers lifted out of
//! `main.rs` (SPIKE-78): `aida whoami` (the session view; its rendering lives
//! in `whoami_view`) and the internal
//! `_bg-fetch` background/two-leg refresh worker. Pure movement — behavior
//! unchanged. Shared helpers (`current_user_id`, `canonical_role_name`,
//! `bg_fetch_lock_path`, `write_last_fetch_entry`, `handle_fetch_command`)
//! stay in `main.rs` and are reached via `crate::`.

use anyhow::Result;
use colored::Colorize;

use crate::command_registry::{CmdPolicy, CommandReg};
use crate::stakeholder_role::StakeholderAction;
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

/// `aida whoami --verbose`'s caller-identity block: the identity AIDA
/// resolved, each line annotated with where the value came from. This is a
/// PURE READ of the same resolvers the gating / queue / provenance code
/// already uses (`current_user_id`, `agent_registry::detect_agent_type`, and
/// the `AIDA_SESSION_*` / `AIDA_AGENT_*` / `AIDA_HEADLESS` / `AIDA_AI_TOOL`
/// env reads) — no project store is loaded and no state is written. It exists
/// to answer the recurring "why did this refuse?" (role resolved to a
/// default, not advisor, so the advisor-gate fired) and "why is my queue
/// empty?" (the shell's user/role identity differs from whatever queued the
/// items — see the BUG-89 queue-identity note in CLAUDE.md).
///
/// Rows are `(field, value, source)`; the field names are the ones the
/// pre-STORY-098 `aida whoami` printed, kept stable for scripts.
// trace:TASK-784
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
pub(crate) fn caller_identity_rows() -> Vec<(&'static str, String, String)> {
    // Helper: read an env var, returning the value + a source label that names
    // the env var when set (non-empty) and the fallback otherwise.
    fn env_with_source(key: &str, fallback: &str) -> (String, String) {
        match std::env::var(key) {
            Ok(v) if !v.trim().is_empty() => (v, format!("from {key}")),
            _ => (fallback.to_string(), "default".to_string()),
        }
    }
    let mut rows = Vec::new();

    // Role — read of $AIDA_SESSION_ROLE, canonicalized exactly the way the
    // gating/queue/routing code reads it (dialog → advisor, human casing).
    // Unset means no role is seated; the gating code treats that as the
    // implementer default, so name that explicitly — it's the value that
    // surprises people at the advisor-gate.
    rows.push(match std::env::var("AIDA_SESSION_ROLE") {
        Ok(raw) if !raw.trim().is_empty() => {
            let canonical = canonical_role_name(&raw);
            let source = if canonical != raw {
                format!("from AIDA_SESSION_ROLE={raw}, canonicalized")
            } else {
                "from AIDA_SESSION_ROLE".to_string()
            };
            ("role", canonical, source)
        }
        _ => (
            "role",
            "implementer".to_string(),
            "default — no AIDA_SESSION_ROLE seated".to_string(),
        ),
    });

    // Agent type — detect_agent_type prefers $AIDA_AGENT_TYPE, then sniffs the
    // env (CODEX_* / ANTIGRAVITY_* / GEMINI_* / CLAUDE*), else "other".
    let agent_type_source = match std::env::var("AIDA_AGENT_TYPE") {
        Ok(v) if !v.trim().is_empty() => "from AIDA_AGENT_TYPE",
        _ => "env-sniff fallback",
    };
    rows.push((
        "agent-type",
        agent_registry::detect_agent_type(),
        agent_type_source.to_string(),
    ));

    let (agent_name, src) = env_with_source("AIDA_AGENT_NAME", "(none)");
    rows.push(("agent-name", agent_name, src));

    // User id — the queue/provenance identity (BUG-89). Mirror current_user_id's
    // resolution order so the source label is precise.
    let user_source = whoami_user_source(
        std::env::var("AIDA_USER").ok().as_deref(),
        std::env::var("USER").ok().as_deref(),
        std::env::var("USERNAME").ok().as_deref(),
    );
    rows.push(("user-id", current_user_id(None), user_source.to_string()));

    // Headless flag — set to "1" by the headless launchers.
    rows.push(match std::env::var("AIDA_HEADLESS") {
        Ok(v) if !v.trim().is_empty() => ("headless", v, "from AIDA_HEADLESS".to_string()),
        _ => (
            "headless",
            "no".to_string(),
            "default — AIDA_HEADLESS unset".to_string(),
        ),
    });

    // AI tool — provenance stamp source; then the AIDA_SESSION_* family.
    for (field, key) in [
        ("ai-tool", "AIDA_AI_TOOL"),
        ("scope", "AIDA_SESSION_SCOPE"),
        ("project", "AIDA_SESSION_PROJECT"),
        ("purpose", "AIDA_SESSION_PURPOSE"),
        ("session-id", "AIDA_SESSION_ID"),
    ] {
        let (value, src) = env_with_source(key, "(none)");
        rows.push((field, value, src));
    }
    rows
}

/// `aida whoami` / `aida node whoami`: the compact session view (handle,
/// node, mail, working, goal, worktree), plus the caller-identity block under
/// `--verbose`. Human at a TTY, flat lines when piped, `--json` on request.
// trace:TASK-784
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
pub(crate) fn handle_whoami_command(flags: &WhoamiFlags) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let worktree = find_aida_project_root_from(&cwd).unwrap_or(cwd);
    let main_root = main_worktree_root_from(&worktree);
    let facts = collect_mail_identity_facts_in(Some(&worktree));
    let view = whoami_view::collect(&main_root, &worktree, &facts, chrono::Utc::now());
    let identity = flags.verbose.then(caller_identity_rows);

    if flags.json {
        let mut value = serde_json::to_value(&view)?;
        if let Some(rows) = &identity {
            let map: serde_json::Map<String, serde_json::Value> = rows
                .iter()
                .map(|(field, value, source)| {
                    (
                        field.to_string(),
                        serde_json::json!({ "value": value, "source": source }),
                    )
                })
                .collect();
            value["identity"] = serde_json::Value::Object(map);
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let agent = agent_output_mode();
    print!("{}", view.render(agent));
    if let Some(rows) = identity {
        println!();
        if !agent {
            println!("{}", "AIDA caller identity".bold());
        }
        for (field, value, source) in rows {
            let label = format!("{field}:");
            println!("  {label:<11} {value} ({source})");
        }
    }
    Ok(())
}

/// (internal) Background fetch worker spawned by `aida statusline`.
/// Fetches `origin/<branch>` for the orphan store at <store-path>,
/// updates `~/.aida/cache/last-fetch.toml`, and removes the
/// per-project lockfile. Not intended for direct invocation —
/// statusline forks this as a detached process with stdio nulled.
// trace:STORY-79 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "_bg-fetch", hide = true)]
pub(crate) struct BgFetchArgs {
    /// Absolute path to the orphan-store worktree (`.aida-store/`).
    pub store_path: std::path::PathBuf,
}

// Spawned detached by `aida statusline`; it only fetches the orphan store and
// stamps the fetch cache, never a requirement write.
// trace:STORY-1728 | ai:claude
pub(crate) static BG_FETCH: CommandReg = CommandReg {
    name: "_bg-fetch",
    help_after: "whoami",
    command: <BgFetchArgs as clap::CommandFactory>::command,
    policy: |_| CmdPolicy {
        stakeholder: StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::command_registry::Handler::Early(|matches, _| {
        let args = <BgFetchArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
        handle_bg_fetch_command(&args.store_path)
    }),
};

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

/// Who this session is: mail handle, node, unread mail, current spec, goal
/// and worktree.
///
/// One compact block from local state only; never marks mail seen.
/// `--verbose` adds the caller-identity details (role,
/// agent-type, agent-name, user-id, headless, ai-tool, scope, project,
/// purpose, session-id), each annotated with where it came from.
///
/// Also available as `aida node whoami`.
// trace:TASK-784
// trace:TASK-851 | ai:claude
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "whoami")]
pub(crate) struct WhoamiArgs {
    #[clap(flatten)]
    pub flags: WhoamiFlags,
}

/// Flags shared by `aida whoami` and `aida node whoami`.
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
#[derive(clap::Args, Debug, Clone, Default)]
pub(crate) struct WhoamiFlags {
    /// Machine-readable JSON output.
    #[clap(long)]
    pub json: bool,
    /// Also print the caller-identity details and where each came from.
    #[clap(long, short = 'v')]
    pub verbose: bool,
}

// TASK-784: a read of local state only (env, registry, leases, mailbox
// files, queue YAML, read-only cache). No store load, no network.
// trace:TASK-784
fn run_whoami(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <WhoamiArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_whoami_command(&args.flags)
}

// trace:STORY-1728 | ai:claude
pub(crate) static WHOAMI: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "whoami",
        help_after: "solo",
        command: <WhoamiArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_whoami),
    };
