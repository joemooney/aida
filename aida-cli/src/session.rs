//! `aida session list / resume` — enriched wrapper around Claude Code's
//! resume-session picker.
//!
//! Claude Code stores each session as `~/.claude/projects/<encoded-cwd>/
//! <session-id>.jsonl`. Each `.jsonl` carries an `ai-title` event with
//! the auto-generated subject and (when AIDA's TASK-1-022 SessionStart
//! hook fired with an active role) the role name in a system-message
//! body. We harvest both, render an enriched picker, and exec
//! `claude --resume <id>` on the user's choice.
//!
//! Subject text isn't user-controllable in Claude Code — this module
//! exists so the AIDA role can sit alongside the auto-subject without
//! waiting for an upstream feature.
//!
//! trace:FR-1-043 | ai:claude

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub age_seconds: u64,
    pub role: Option<String>,
    pub spec: Option<String>,
    pub title: Option<String>,
    /// Timestamp of the first event in the .jsonl. Used for launch-log
    /// correlation in FR-1-044 — matches the session to a `aida session
    /// new` record so the user-chosen title and authoritative role can
    /// override the grep heuristic.
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Most-recent cwd recorded in the .jsonl. Each Claude Code message
    /// stores the cwd at message time; we sample the LAST one we see in
    /// our parse window so worktree switches mid-session show up.
    /// trace:STORY-59 | ai:claude
    pub last_cwd: Option<String>,
    /// Branch the worktree at `last_cwd` was on when we ran. Computed on
    /// demand at table-print time (one cheap `git branch --show-current`
    /// per unique cwd) — None if we can't resolve a branch (cwd missing,
    /// not a repo, etc.).
    /// trace:STORY-59 | ai:claude
    pub branch: Option<String>,
    /// Most-recent spec this session worked — the RECENT FOCUS column.
    /// Filled by `fill_recent_focus` from AIDA's per-session activity log
    /// (newest-first) with the manifest's planned items as fallback. None
    /// when AIDA never tracked the session (renders `-`). Distinct from
    /// `spec`, which stays the *first*-mentioned (launch) spec — together
    /// they show the session's spec evolution.
    /// trace:BUG-112 | ai:claude
    pub recent_focus: Option<String>,
}

/// STORY-59: liveness inferred from activity recency. Visual indicator
/// only — no PID tracking (Claude Code forks; child PIDs ≠ launch PID,
/// so PID-based liveness is unreliable across reads).
/// trace:STORY-59 | ai:claude
fn liveness_indicator(age_seconds: u64) -> &'static str {
    if age_seconds < 5 * 60 {
        "●" // live (active in last 5 minutes)
    } else if age_seconds < 60 * 60 {
        "◐" // recent (last hour)
    } else {
        " " // idle
    }
}

const RECENT_WINDOW_SECS: u64 = 24 * 60 * 60;

pub fn list(limit: usize, no_color: bool, all: bool) -> Result<()> {
    if no_color {
        colored::control::set_override(false);
    }
    let cwd = std::env::current_dir().context("could not determine cwd")?;
    let mut here = collect_sessions_from_cwd(&cwd, limit)?;

    // STORY-58: when this cwd is inside an active session worktree, also
    // walk the parent project's Claude Code session storage so the user
    // gets a merged view (their parent shell's sessions + this worktree's
    // sessions) instead of half a story.
    // trace:STORY-58 | ai:claude
    let parent_root = crate::parent_project_root_for_session(&cwd);
    let mut parent: Vec<SessionMeta> = match parent_root.as_ref() {
        Some(root) if root != &cwd => collect_sessions_from_cwd(root, limit).unwrap_or_default(),
        _ => Vec::new(),
    };

    let total_here = here.len();
    let total_parent = parent.len();
    // STORY-59: by default, hide sessions with no activity in the last
    // 24h — long-tail abandoned sessions clutter the everyday view. The
    // `--all` flag bypasses; we still note how many were elided.
    if !all {
        here.retain(|s| s.age_seconds < RECENT_WINDOW_SECS);
        parent.retain(|s| s.age_seconds < RECENT_WINDOW_SECS);
    }
    let hidden = (total_here - here.len()) + (total_parent - parent.len());

    if here.is_empty() && parent.is_empty() {
        if hidden > 0 {
            eprintln!(
                "{}",
                format!(
                    "(no sessions active in the last 24h; {} older session{} hidden — pass --all to see them)",
                    hidden,
                    if hidden == 1 { "" } else { "s" }
                )
                .dimmed()
            );
        } else {
            eprintln!("{}", "(no past sessions in this directory)".dimmed());
        }
        print_leases_hint();
        return Ok(());
    }
    fill_branches(&mut here);
    fill_branches(&mut parent);
    normalize_specs(&mut here);
    normalize_specs(&mut parent);
    fill_recent_focus(&mut here);
    fill_recent_focus(&mut parent);

    // STORY-58: when there's nothing to merge, render the classic single
    // table — keep the existing one-group output untouched. Only switch
    // to grouped headers once we actually have a parent group to show.
    if parent.is_empty() {
        print_table(&here);
    } else {
        // Compute widths over the union so both tables align column-for-column.
        let widths = TableWidths::compute(here.iter().chain(parent.iter()));
        let here_label = group_label(&cwd);
        let parent_label = parent_root
            .as_ref()
            .map(|p| group_label(p))
            .unwrap_or_else(|| "parent".to_string());
        if !here.is_empty() {
            print_group_header(&format!("This worktree ({})", here_label));
            print_table_with_widths(&here, &widths);
            println!();
        }
        print_group_header(&format!("Parent project ({})", parent_label));
        print_table_with_widths(&parent, &widths);
    }
    if hidden > 0 {
        eprintln!(
            "{}",
            format!(
                "({} older session{} hidden — pass --all to see them)",
                hidden,
                if hidden == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    // BUG-112: RECENT FOCUS replaced the old INITIAL TOPIC column. It
    // tracks the latest spec each session worked (AIDA's per-session
    // activity log, newest-first) so it stays current as work moves,
    // instead of drifting like the conversation-start title did. A `-`
    // means AIDA isn't tracking a spec for that session.
    // trace:BUG-112 | ai:claude
    eprintln!(
        "{}",
        "(RECENT FOCUS is the latest spec a session worked — it updates as work moves; \
         `-` = no spec tracked)"
            .dimmed()
    );
    print_leases_hint();
    Ok(())
}

/// BUG-98: append a one-line nudge pointing at `aida session leases`
/// when the project has any active scoped leases. `aida session list`
/// shows historical .jsonl conversations — not the same set as the
/// scoped-lease view — so users reaching here for "what's active"
/// otherwise miss the leases entirely. We only print when there's
/// something to point at; an empty-leases project gets no noise.
/// trace:BUG-98 | ai:claude
fn print_leases_hint() {
    let count = crate::active_lease_count_for_cwd();
    if count == 0 {
        return;
    }
    let label = if count == 1 { "lease" } else { "leases" };
    eprintln!(
        "{}",
        format!(
            "({} active session {} — run `aida session leases` for the live view)",
            count, label
        )
        .dimmed()
    );
}

/// STORY-58: a friendly label for a project-root path — the basename when
/// available, the full path otherwise. Used in `── This worktree (foo) ──`
/// headers so the user sees which dir each group represents.
/// trace:STORY-58 | ai:claude
fn group_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// STORY-58: render a `── Label ──` separator above each group. Dimmed so
/// it doesn't compete with the table content.
/// trace:STORY-58 | ai:claude
fn print_group_header(label: &str) {
    println!("{}", format!("── {} ──", label).dimmed());
}

/// STORY-59: resolve `branch` per session by running `git -C <cwd>
/// branch --show-current` once per unique cwd. Cheap (one fork/exec per
/// distinct worktree), and the result is cached across sessions sharing
/// TASK-237: rewrite each session's stored SPEC value to its agreed-id
/// (short form) when the spec resolves to a requirement that carries
/// one. Sessions recorded before `aida db merge-gate` ran keep the
/// long-form node-aware id (`FR-1-042`); this normalizes the column to
/// the short form (`FR-42`) so it matches how `aida list` / `aida show`
/// / `aida history` render specs. A spec that no longer resolves to a
/// requirement is left as-is with an `(unresolved)` suffix.
/// trace:TASK-237 | ai:claude
fn normalize_specs(sessions: &mut [SessionMeta]) {
    if sessions.iter().all(|s| s.spec.is_none()) {
        return;
    }
    let Ok(root) = crate::find_project_root() else {
        return;
    };
    let Some(store) = crate::load_store_for_lookup(&root) else {
        return;
    };
    normalize_specs_with_store(sessions, &store);
}

/// TASK-237: the store-pure half of [`normalize_specs`] — split out so
/// the resolution rules are unit-testable without a project on disk.
/// trace:TASK-237 | ai:claude
fn normalize_specs_with_store(sessions: &mut [SessionMeta], store: &aida_core::RequirementsStore) {
    for s in sessions.iter_mut() {
        let Some(spec) = s.spec.as_deref() else {
            continue;
        };
        match store.get_requirement_by_spec_id(spec) {
            Some(req) => {
                if let Some(agreed) = req.agreed_id.as_deref() {
                    s.spec = Some(agreed.to_string());
                }
            }
            None => s.spec = Some(format!("{} (unresolved)", spec)),
        }
    }
}

/// BUG-112: populate `recent_focus` — the most-recent spec each session
/// worked, for the RECENT FOCUS column. Joins each Claude session to its
/// AIDA manifest on `claude_session_id`, then reads the session activity
/// log for the live "current spec", falling back to the manifest's most
/// recently picked-up planned item. Sessions AIDA never tracked keep
/// `recent_focus = None` and render `-` — absent signal, not the
/// misleading stale title BUG-112 set out to remove.
/// trace:BUG-112 | ai:claude
fn fill_recent_focus(sessions: &mut [SessionMeta]) {
    if sessions.is_empty() {
        return;
    }
    let Ok(root) = crate::find_project_root() else {
        return;
    };
    let manifests = crate::session_manifest::list_all(&root);
    if manifests.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        // The manifest's `claude_session_id` is the only reliable join
        // back to a Claude session — lease ids and Claude UUIDs are
        // distinct id spaces. trace:TASK-112 | ai:claude
        let Some(manifest) = manifests
            .iter()
            .find(|m| m.claude_session_id.as_deref() == Some(s.id.as_str()))
        else {
            continue;
        };
        let activity_recent = crate::session_log_recent_spec(&root, &manifest.session_id);
        s.recent_focus = recent_focus_from(manifest, activity_recent.as_deref());
    }
}

/// BUG-112: the store-pure half of [`fill_recent_focus`] — derive the
/// RECENT FOCUS value from a session's manifest plus the optional
/// most-recent spec from its activity log. The activity log is the live
/// signal (actual `aida` spec interactions, newest-first); the manifest's
/// most recently picked-up planned item — latest `started_at`, else
/// highest `position` — is the fallback. Split out so the precedence is
/// unit-testable without a project on disk. trace:BUG-112 | ai:claude
fn recent_focus_from(
    manifest: &crate::session_manifest::SessionManifest,
    activity_recent: Option<&str>,
) -> Option<String> {
    if let Some(spec) = activity_recent {
        return Some(spec.to_string());
    }
    manifest
        .items
        .iter()
        .max_by(|a, b| match (a.started_at, b.started_at) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => a.position.cmp(&b.position),
        })
        .map(|it| it.spec_id.clone())
}

#[cfg(test)]
mod normalize_specs_tests {
    use super::*;

    fn meta(spec: Option<&str>) -> SessionMeta {
        SessionMeta {
            id: "deadbeef".to_string(),
            age_seconds: 10,
            role: None,
            spec: spec.map(|s| s.to_string()),
            title: None,
            started_at: None,
            last_cwd: None,
            branch: None,
            recent_focus: None,
        }
    }

    fn req(spec_id: &str, agreed_id: Option<&str>) -> aida_core::Requirement {
        let mut r = aida_core::Requirement::new(format!("test {spec_id}"), String::new());
        r.spec_id = Some(spec_id.to_string());
        r.agreed_id = agreed_id.map(|s| s.to_string());
        r
    }

    fn store(reqs: Vec<aida_core::Requirement>) -> aida_core::RequirementsStore {
        let mut s = aida_core::RequirementsStore::default();
        s.requirements = reqs;
        s
    }

    /// Long-form node-aware id rewrites to the agreed short id.
    #[test]
    fn long_form_resolves_to_agreed_id() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("FR-1-042"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-42"));
    }

    /// A spec already stored in short form (the agreed id) resolves and
    /// stays in short form — `get_requirement_by_spec_id` matches the
    /// agreed id too.
    #[test]
    fn short_form_stays_short() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("FR-42"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-42"));
    }

    /// A requirement with no agreed id keeps its long-form spec_id —
    /// that is the correct fallback, not an error.
    #[test]
    fn no_agreed_id_keeps_long_form() {
        let st = store(vec![req("FR-1-042", None)]);
        let mut sessions = vec![meta(Some("FR-1-042"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("FR-1-042"));
    }

    /// A spec that doesn't resolve (deleted, or never existed) is marked
    /// `(unresolved)` rather than silently shown as a live id.
    #[test]
    fn unresolvable_spec_is_flagged() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(Some("BUG-999"))];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec.as_deref(), Some("BUG-999 (unresolved)"));
    }

    /// A session with no spec is left untouched.
    #[test]
    fn missing_spec_is_left_alone() {
        let st = store(vec![req("FR-1-042", Some("FR-42"))]);
        let mut sessions = vec![meta(None)];
        normalize_specs_with_store(&mut sessions, &st);
        assert_eq!(sessions[0].spec, None);
    }
}

/// the same cwd.
/// trace:STORY-59 | ai:claude
fn fill_branches(sessions: &mut [SessionMeta]) {
    use std::collections::HashMap;
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    for s in sessions.iter_mut() {
        let Some(cwd) = s.last_cwd.clone() else {
            continue;
        };
        let branch = cache
            .entry(cwd.clone())
            .or_insert_with(|| resolve_branch(&cwd))
            .clone();
        s.branch = branch;
    }
}

fn resolve_branch(cwd: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub fn resume(id: Option<String>, limit: usize) -> Result<()> {
    let target = match id {
        Some(prefix) => resolve_id(&prefix)?,
        None => pick_interactive(limit)?,
    };
    exec_claude_resume(&target, None)
}

/// `aida session new` — capture role + title up-front, append a record
/// to `~/.aida/session-launches.log`, then exec `claude
/// --permission-mode <mode>`. Subsequent `aida session list` calls read
/// the launches log and join it with the .jsonl files (cwd + start-time
/// match) to surface the user-chosen title and authoritative role —
/// instead of falling back to the grep heuristic.
/// trace:FR-1-044 | ai:claude
///
/// STORY-495: `permission_mode` is now `Option<&str>`. `None` means honor
/// Claude's native permission posture — no `--permission-mode` is injected
/// (the faithful-launcher default). `Some(mode)` injects it explicitly.
pub fn new_session(
    title: Option<String>,
    permission_mode: Option<&str>,
    role_override: Option<String>,
    display_name: Option<String>,
) -> Result<()> {
    let role = role_override
        .or_else(|| std::env::var("AIDA_SESSION_ROLE").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".to_string());

    let title = match title {
        Some(t) if !t.trim().is_empty() => t,
        _ => inquire::Text::new("Title for this session?")
            .with_help_message("Shown in `aida session list`. Leave blank to skip.")
            .prompt()
            .unwrap_or_default(),
    };

    // STORY-495: record `native` in the launch-log when no mode is injected.
    append_launch_log(&role, permission_mode.unwrap_or("native"), &title)?;

    let name_for_log = display_name.as_deref().unwrap_or("(auto)");
    let mode_display = permission_mode
        .map(|m| format!("--permission-mode {}", m))
        .unwrap_or_else(|| "(native permission posture)".to_string());
    eprintln!(
        "{} {} → claude {} (name: {})",
        "▶".green().bold(),
        format!("session new (role:{}, title:{:?})", role, title).dimmed(),
        mode_display,
        name_for_log,
    );

    exec_claude_new(permission_mode, display_name.as_deref())
}

/// TASK-31: derive a claude `--name` value from session metadata. Keeps the
/// /resume picker and terminal title legible when multiple concurrent
/// worktrees are open.
///
/// Convention:
///   - role=reviewer            → `review@<scope>`               (PR/MR work)
///   - scope=EPIC-N + batchM    → `EPIC-N:batchM`                (epic-batch)
///   - other implementer shapes → `<role-label>@<scope>:<suffix>` or
///                                `<role-label>@<scope>` if no suffix
///
/// `<suffix>` is the part of the branch name after `<scope-slug>-`. Returns
/// `None` when scope is empty (defensive — the caller already validates).
/// Result is truncated to 64 chars to fit common terminal-title budgets.
/// trace:TASK-31 | ai:claude
pub fn derive_session_name(scope: &str, branch: &str, role: &str) -> Option<String> {
    if scope.trim().is_empty() {
        return None;
    }
    let scope_display = normalize_scope_for_display(scope);
    let role_lower = role.to_ascii_lowercase();
    if role_lower == "reviewer" {
        return Some(truncate(&format!("review@{}", scope_display), 64));
    }
    let is_epic = scope_display.starts_with("EPIC-");
    if is_epic {
        if let Some(batch) = extract_batch_suffix(branch) {
            return Some(truncate(&format!("{}:{}", scope_display, batch), 64));
        }
    }
    let role_label = role_label_for(&role_lower);
    let result = match extract_branch_suffix(branch, scope) {
        Some(s) if !s.is_empty() => format!("{}@{}:{}", role_label, scope_display, s),
        _ => format!("{}@{}", role_label, scope_display),
    };
    Some(truncate(&result, 64))
}

fn normalize_scope_for_display(scope: &str) -> String {
    // Uppercase only when it looks like a SPEC-ID (TYPE-NUM); leave
    // free-form scopes (path globs, "feature:auth") alone.
    let parts: Vec<&str> = scope.splitn(2, '-').collect();
    if parts.len() == 2
        && !parts[1].is_empty()
        && parts[1].chars().all(|c| c.is_ascii_digit())
        && parts[0].chars().all(|c| c.is_ascii_alphabetic())
    {
        return scope.to_uppercase();
    }
    scope.to_string()
}

fn role_label_for(role_lower: &str) -> String {
    match role_lower {
        "implementer" => "impl".to_string(),
        "reviewer" => "review".to_string(),
        "" | "-" => "session".to_string(),
        other => other.to_string(),
    }
}

fn extract_batch_suffix(branch: &str) -> Option<&str> {
    let last = branch.rsplit('-').next()?;
    if last.len() > 5 && last.starts_with("batch") && last[5..].chars().all(|c| c.is_ascii_digit())
    {
        Some(last)
    } else {
        None
    }
}

fn extract_branch_suffix<'a>(branch: &'a str, scope: &str) -> Option<&'a str> {
    let scope_lower = scope.to_ascii_lowercase();
    let prefix = format!("{}-", scope_lower);
    let branch_lower = branch.to_ascii_lowercase();
    if branch_lower.starts_with(&prefix) {
        let tail = &branch[prefix.len()..];
        if !tail.is_empty() {
            return Some(tail);
        }
    }
    None
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

const LAUNCH_LOG_REL: &str = ".aida/session-launches.log";

fn launch_log_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("HOME not set; cannot locate launches log")?;
    Ok(home.join(LAUNCH_LOG_REL))
}

/// Append a launch record. Format: TSV with these fields, one record per
/// line (newline-terminated):
///   iso_ts \t role-or-dash \t cwd \t permission_mode \t title
/// trace:FR-1-044 | ai:claude
fn append_launch_log(role: &str, permission_mode: &str, title: &str) -> Result<()> {
    use std::io::Write;
    let path = launch_log_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\n",
        chrono::Utc::now().to_rfc3339(),
        if role.is_empty() { "-" } else { role },
        cwd.display(),
        permission_mode,
        sanitize_for_tsv(title),
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    f.write_all(line.as_bytes())?;
    Ok(())
}

fn sanitize_for_tsv(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ").replace('\r', " ")
}

/// Replace this process with `claude --permission-mode <mode>`. When `name`
/// is `Some(...)`, also passes `--name <n>` so the launched session is
/// labeled in the /resume picker and terminal title. trace:TASK-31 | ai:claude
fn exec_claude_new(permission_mode: Option<&str>, name: Option<&str>) -> Result<()> {
    exec_claude(permission_mode, name, None, None)
}

/// STORY-42 / TASK-112: replace this process with `claude
/// --permission-mode <mode> [--name <n>] --session-id <uuid>
/// <initial_prompt>`. The initial prompt becomes claude's first message
/// — pass `/aida-pickup` / `/aida-review --pr N` so the agent routes
/// into the right skill on launch with no extra typing. `aida queue
/// work` mints the UUID up front so it can record it in the session
/// manifest (and a later `--resume` can find the conversation) before
/// `exec` replaces this process. `session_id` must be a valid UUID —
/// claude rejects anything else. trace:STORY-42, TASK-112 | ai:claude
pub fn exec_claude_with_session(
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: &str,
    session_id: &str,
) -> Result<()> {
    exec_claude(
        permission_mode,
        name,
        Some(initial_prompt),
        Some(session_id),
    )
}

/// Build the argv (after the `claude` program name) for an interactive
/// `aida queue work` launch — `--permission-mode`, optional `--name` /
/// `--session-id`, and a trailing positional initial prompt. Shared by
/// `exec_claude` (process replacement) and `spawn_claude_session`
/// (spawn + wait, BUG-226) so the two launch paths can never drift.
/// trace:BUG-226 | ai:claude
///
/// STORY-495: `permission_mode` is `Option<&str>`. `None` omits
/// `--permission-mode` entirely so the spawned `claude` uses its native
/// permission posture (the faithful-launcher default). The headless launch
/// path uses a separate [`claude_headless_args`] builder that always forces
/// `bypassPermissions`, so this change never touches the unattended drain.
pub fn claude_session_args(
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(m) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(m.to_string());
    }
    if let Some(n) = name {
        args.push("--name".to_string());
        args.push(n.to_string());
    }
    // TASK-112: a caller-minted session id, so the conversation is
    // addressable for `aida queue work --resume`.
    if let Some(sid) = session_id {
        args.push("--session-id".to_string());
        args.push(sid.to_string());
    }
    if let Some(p) = initial_prompt {
        // Positional first-message — claude treats trailing positionals
        // as the initial prompt for the session.
        args.push(p.to_string());
    }
    args
}

fn exec_claude(
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: Option<&str>,
    session_id: Option<&str>,
) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("claude");
    cmd.args(claude_session_args(
        permission_mode,
        name,
        initial_prompt,
        session_id,
    ));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec claude: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn claude")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// BUG-226: spawn an interactive `claude` session (inherited stdio) and
/// wait for it, returning the exit status. The standalone reviewer path
/// needs `aida queue work` to *outlive* the launch so it can print an
/// end-of-command summary — `exec_claude` (process replacement) cannot.
/// trace:BUG-226 | ai:claude
pub fn spawn_claude_session(
    permission_mode: Option<&str>,
    name: Option<&str>,
    initial_prompt: &str,
    session_id: &str,
) -> Result<std::process::ExitStatus> {
    std::process::Command::new("claude")
        .args(claude_session_args(
            permission_mode,
            name,
            Some(initial_prompt),
            Some(session_id),
        ))
        .status()
        .context("failed to spawn claude")
}

/// BUG-226: spawn (not exec) a headless `claude -p` reviewer and wait,
/// returning the exit status. Mirrors `exec_claude_headless` exactly —
/// same `claude_headless_args` flag set, `AIDA_HEADLESS=1` in the env,
/// stdout redirected to `log_path` — but keeps the parent alive so the
/// standalone reviewer summary can read the verdict file + JSONL log.
/// trace:BUG-226 | ai:claude
///
/// TASK-307: starts a background tee thread on `log_path` so the headless
/// session's high-signal events surface to the orchestrator's terminal
/// alongside the launch banner. The tee's filter and on/off are driven by
/// `tee_opts`; even when disabled, failure events (`is_error`,
/// `permission_denials`) still stream. trace:TASK-307 | ai:claude
pub fn spawn_claude_headless(
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
) -> Result<std::process::ExitStatus> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    let status = Command::new("claude")
        .args(claude_headless_args(prompt, session_id))
        .env("AIDA_HEADLESS", "1")
        .stdout(Stdio::from(log))
        .status()
        .context("failed to spawn claude")?;
    tee.stop();
    Ok(status)
}

/// BUG-226: spawn `claude --resume <id>` and wait — the spawn counterpart
/// of `exec_claude_resume` for the standalone reviewer summary path.
/// trace:BUG-226 | ai:claude
pub fn spawn_claude_resume(
    id: &str,
    permission_mode: Option<&str>,
) -> Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new("claude");
    cmd.args(["--resume", id]);
    if let Some(m) = permission_mode {
        cmd.args(["--permission-mode", m]);
    }
    cmd.status().context("failed to spawn claude")
}

/// STORY-263: build the argv (after the `claude` program name) for a headless
/// `claude -p` launch. The flag set is SPIKE-7's mandatory list — see
/// `docs/spikes/2026-05-16-claude-headless.md`:
///   - `-p` — print mode: single turn, exits on its own, no Ctrl+D.
///   - `--permission-mode bypassPermissions` — mandatory; `acceptEdits`
///     leaves `Bash` gated and `default` auto-denies silently (spike Q2).
///   - `--output-format stream-json --verbose` — newline-delimited JSON
///     events, so a watchdog (TASK-298) can tail liveness (spike Q6).
///   - `--disallowed-tools AskUserQuestion` — structural programmatic gate:
///     under headless mode there is no human to answer the prompt, so the
///     tool must be unavailable rather than advisory. BUG-280 added skill-
///     template instructions saying "don't call AskUserQuestion under
///     `--no-human`"; BUG-327 found a reviewer reasoning past those
///     instructions and bailing without a verdict file. The flag is
///     variadic on the claude side, so we wedge it between `--verbose`
///     and `--session-id` so the next `--` token terminates its value
///     list and the prompt positional at the tail stays the prompt.
///     trace:BUG-327 | ai:claude
///   - `--session-id <uuid>` — persistence stays ON, so a killed run stays
///     resumable (spike Q9).
///
/// Never `--bare`: it strips OAuth/keychain auth and breaks login (spike Q1).
/// Pure — the flag set is unit-tested without spawning claude.
/// trace:STORY-263 | ai:claude
pub fn claude_headless_args(prompt: &str, session_id: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--disallowed-tools".to_string(),
        "AskUserQuestion".to_string(),
        "--session-id".to_string(),
        session_id.to_string(),
        // Prompt last — a positional, mirroring `exec_claude`.
        prompt.to_string(),
    ]
}

/// STORY-263: replace this process with a headless `claude -p` run — the
/// launch path behind `aida queue work --no-human` (the `--auto-complete`
/// orchestrator's reviewer phase). Claude's stream-json stdout is redirected
/// to `log_path` so the orchestrator's own stdout stays clean (it carries
/// `--json` phase events) and TASK-298's watchdog has a file to tail; stderr
/// stays inherited so Claude errors still surface. trace:STORY-263 | ai:claude
///
/// Sets `AIDA_HEADLESS=1` in the launched environment so skills can tell
/// they are running unattended — the `/aida-review` skill keys its
/// finding-filing step on it, since there is no human to triage the
/// reviewer's findings. trace:STORY-278 | ai:claude
///
/// TASK-307: the headless `claude -p` reviewer/implementer is now hosted by
/// the parent (spawn + wait + exit) instead of `exec`-replacing it, so a
/// background tee thread can tail `log_path` and surface high-signal events
/// to the operator's terminal during the run. The semantics the caller sees
/// are unchanged — the function never returns on success; the process exits
/// with the child's exit code. The pre-TASK-307 `exec()` behaviour is gone
/// because the parent must stay alive to host the tee — including for the
/// `--no-tee-headless` path, where the tee still runs so failure events
/// (`is_error`, `permission_denials`) can never hide. trace:TASK-307 | ai:claude
pub fn exec_claude_headless(
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
) -> Result<()> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    let status = Command::new("claude")
        .args(claude_headless_args(prompt, session_id))
        .env("AIDA_HEADLESS", "1")
        .stdout(Stdio::from(log))
        .status()
        .context("failed to spawn claude")?;
    tee.stop();
    std::process::exit(status.code().unwrap_or(1));
}

/// STORY-306: build the argv (after the `claude` program name) for a headless
/// `claude -p --resume <id>` launch — the advisor tier's implementer-resume
/// leg. Identical to [`claude_headless_args`] (the SPIKE-7 mandatory flag
/// set) except it `--resume`s an existing session instead of minting a new
/// `--session-id`, so the resumed implementer re-enters its punted phase-1
/// conversation with the working model it had already built. Pure — the flag
/// set is unit-tested without spawning claude. trace:STORY-306 | ai:claude
pub fn claude_headless_resume_args(prompt: &str, session_id: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        "bypassPermissions".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        // BUG-327: structural disable mirrors `claude_headless_args`.
        // Variadic flag on claude's side; the next `--resume` terminates
        // the value list so the trailing prompt positional is unharmed.
        // trace:BUG-327 | ai:claude
        "--disallowed-tools".to_string(),
        "AskUserQuestion".to_string(),
        "--resume".to_string(),
        session_id.to_string(),
        // Prompt last — a positional, mirroring `claude_headless_args`.
        prompt.to_string(),
    ]
}

/// STORY-306: spawn a headless `claude -p --resume <id>` run and wait,
/// returning the exit status. The spawn-and-wait counterpart of
/// [`spawn_claude_headless`] for the orchestrator's advisor-resume leg — the
/// parent stays alive to classify the resumed implementer's outcome. Claude's
/// stream-json stdout is redirected to `log_path`; `AIDA_HEADLESS=1` is set so
/// the resumed skill knows it is unattended.
///
/// `cwd` must be the working directory the original session was created in
/// (typically the implementer's worktree). Claude Code persists each session
/// at `~/.claude/projects/<cwd-slug>/<session-id>.jsonl` — the slug is
/// derived from cwd, so a resume from a different directory looks in the
/// wrong slug folder and fails with "No conversation found." The original
/// SPIKE-7 verification of this code missed this because the unit test
/// exercises only the argv. trace:STORY-306 | ai:claude
pub fn spawn_claude_headless_resume(
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    cwd: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
) -> Result<std::process::ExitStatus> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    let status = Command::new("claude")
        .current_dir(cwd)
        .args(claude_headless_resume_args(prompt, session_id))
        .env("AIDA_HEADLESS", "1")
        .stdout(Stdio::from(log))
        .status()
        .context("failed to spawn claude")?;
    tee.stop();
    Ok(status)
}

/// One line of `~/.aida/session-launches.log` matching the current cwd,
/// parsed back into structured form so `aida session list` can correlate
/// by timestamp. (The cwd field is dropped after the filter — every
/// LaunchRecord we keep matches the active project.)
#[derive(Debug, Clone)]
struct LaunchRecord {
    ts: chrono::DateTime<chrono::Utc>,
    role: String,
    title: String,
}

fn read_launches_for_cwd(cwd: &Path) -> Vec<LaunchRecord> {
    let Ok(path) = launch_log_path() else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let cwd_str = cwd.to_string_lossy().to_string();
    content
        .lines()
        .filter_map(|line| {
            let mut it = line.splitn(5, '\t');
            let ts = it.next()?;
            let role = it.next()?;
            let recorded_cwd = it.next()?;
            let _mode = it.next()?;
            let title = it.next().unwrap_or("");
            if recorded_cwd != cwd_str {
                return None;
            }
            let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts)
                .ok()?
                .with_timezone(&chrono::Utc);
            Some(LaunchRecord {
                ts: parsed_ts,
                role: role.to_string(),
                title: title.to_string(),
            })
        })
        .collect()
}

/// Find the closest launch record (by timestamp) that's within `window`
/// seconds before-or-after the session's start time. Uses the .jsonl's
/// FIRST event timestamp (≈ launch time), not the file mtime (which
/// updates on every event).
/// trace:FR-1-044 | ai:claude
fn match_launch(
    launches: &[LaunchRecord],
    session_started_at: chrono::DateTime<chrono::Utc>,
    window_sec: i64,
) -> Option<&LaunchRecord> {
    launches
        .iter()
        .filter(|l| (session_started_at - l.ts).num_seconds().abs() <= window_sec)
        .min_by_key(|l| (session_started_at - l.ts).num_seconds().abs())
}

/// Walk Claude Code's per-project session directory for the current cwd
/// and return the most recent N parsed `SessionMeta` entries.
fn collect_sessions(limit: usize) -> Result<Vec<SessionMeta>> {
    let cwd = std::env::current_dir().context("could not determine cwd")?;
    collect_sessions_from_cwd(&cwd, limit)
}

/// STORY-58: same as `collect_sessions` but for an arbitrary project root,
/// so `aida session list` can pull a second batch from the parent project's
/// Claude Code session storage when it's invoked inside a session worktree.
/// trace:STORY-58 | ai:claude
fn collect_sessions_from_cwd(cwd: &Path, limit: usize) -> Result<Vec<SessionMeta>> {
    let dir = claude_project_dir(cwd)?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    // Sort .jsonl files by mtime desc; only parse the top `limit`.
    let mut entries: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((path, mtime))
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(limit);

    let now = SystemTime::now();
    let launches = read_launches_for_cwd(cwd);
    let metas = entries
        .into_iter()
        .filter_map(|(path, mtime)| {
            let mut meta = parse_session_meta(&path, mtime, now).ok()?;
            // FR-1-044: try to attribute the session to a launch
            // record. The .jsonl's FIRST event timestamp is the right
            // anchor — file mtime updates on every event so it drifts
            // away from the actual launch time as the session is used.
            if let Some(started) = meta.started_at {
                if let Some(launch) = match_launch(&launches, started, 60) {
                    if !launch.title.is_empty() {
                        meta.title = Some(launch.title.clone());
                    }
                    if launch.role != "-" {
                        meta.role = Some(launch.role.clone());
                    }
                }
            }
            Some(meta)
        })
        .collect();
    Ok(metas)
}

/// Encode a project path the way Claude Code does — replace each `/`
/// with `-` (and drop the leading separator). e.g. `/home/joe/ai/aida` →
/// `-home-joe-ai-aida`.
pub(crate) fn claude_project_dir(cwd: &Path) -> Result<PathBuf> {
    let s = cwd.to_string_lossy();
    // BUG-346: Windows paths can carry backslashes even when tests use
    // Unix-shaped fixture strings elsewhere. Normalize both spellings so
    // advisor fork destinations use the same slug convention cross-platform.
    // trace:BUG-346 | ai:codex
    let encoded = s.replace('\\', "-").replace('/', "-");
    #[cfg(test)]
    let home = std::env::var_os("AIDA_TEST_HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .context("HOME not set; cannot locate Claude project dir")?;
    #[cfg(not(test))]
    let home = dirs::home_dir().context("HOME not set; cannot locate Claude project dir")?;
    Ok(home.join(".claude/projects").join(encoded))
}

/// TASK-112: scan every Claude Code project directory under
/// `~/.claude/projects/` and return recorded sessions whose first-mentioned
/// SPEC-ID matches `scope` (case-insensitive), most recent first. Used by
/// `aida queue work --resume` / `--list-sessions` to find a prior
/// conversation to continue for a given scope — sessions launched in a
/// since-removed worktree still surface, because the `.jsonl` files persist
/// after `aida session end` removes the worktree.
///
/// Bounded: across all project dirs we parse only the 150
/// most-recently-modified `.jsonl` files, so the scan stays fast even with
/// hundreds of historical sessions. trace:TASK-112 | ai:claude
pub fn list_scope_sessions(scope: &str) -> Result<Vec<SessionMeta>> {
    let home = dirs::home_dir().context("HOME not set; cannot locate Claude sessions")?;
    let projects = home.join(".claude").join("projects");
    if !projects.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(PathBuf, SystemTime)> = Vec::new();
    if let Ok(dirs) = std::fs::read_dir(&projects) {
        for dir in dirs.flatten() {
            let p = dir.path();
            if !p.is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&p) else {
                continue;
            };
            for f in files.flatten() {
                let fp = f.path();
                if fp.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Ok(mtime) = f.metadata().and_then(|m| m.modified()) {
                    entries.push((fp, mtime));
                }
            }
        }
    }
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(150);

    let now = SystemTime::now();
    let want = scope.trim();
    // BUG-447: scope the machine-global scan to the CURRENT project. Onboarding
    // seeds the same id (TASK-007) in every project, and the scan keyed on
    // spec-id alone would surface a same-id session from a DIFFERENT (or
    // since-deleted) project on this machine — resuming it would replay another
    // project's context. Filter to sessions whose recorded cwd is within the
    // current project root or a sibling `<root>-<slug>` worktree. When we can't
    // resolve a project root (not in a project), keep the old global behaviour.
    // trace:BUG-447 | ai:claude
    let project_root = crate::find_main_worktree_root().ok();
    let mut out: Vec<SessionMeta> = entries
        .into_iter()
        .filter_map(|(path, mtime)| parse_session_meta(&path, mtime, now).ok())
        .filter(|m| {
            m.spec
                .as_deref()
                .map(|s| s.eq_ignore_ascii_case(want))
                .unwrap_or(false)
        })
        .filter(|m| match &project_root {
            // No cwd recorded ⇒ can't confirm it's ours ⇒ exclude when scoped.
            Some(root) => m
                .last_cwd
                .as_deref()
                .map(|cwd| session_cwd_in_project(cwd, root))
                .unwrap_or(false),
            None => true,
        })
        .collect();
    out.sort_by_key(|m| m.age_seconds);
    Ok(out)
}

/// BUG-447: is a recorded session's `cwd` within the current project's scope —
/// the project-root subtree, or a sibling `<root>-<slug>` worktree that
/// `aida queue work` creates (`<parent>/<repo_name>-<slug>`)? `Path::starts_with`
/// is component-wise, so a sibling worktree (`…/ai-task-007`) does NOT match the
/// root (`…/ai`); the explicit sibling check handles it. trace:BUG-447 | ai:claude
fn session_cwd_in_project(cwd: &str, project_root: &Path) -> bool {
    let cwd = Path::new(cwd.trim());
    if cwd == project_root || cwd.starts_with(project_root) {
        return true;
    }
    let (Some(parent), Some(repo_name)) = (
        project_root.parent(),
        project_root.file_name().and_then(|s| s.to_str()),
    ) else {
        return false;
    };
    match (cwd.parent(), cwd.file_name().and_then(|s| s.to_str())) {
        (Some(cwd_parent), Some(cwd_name)) => {
            cwd_parent == parent && cwd_name.starts_with(&format!("{repo_name}-"))
        }
        _ => false,
    }
}

/// TASK-402: canonicalize a role name parsed out of a session's JSONL so
/// `--list-sessions` reports the project-wide identity, not a deprecated
/// alias. `dialog` is the legacy token (TASK-279) for `advisor` (TASK-586);
/// normalizing here keeps the recorded role aligned with what the orchestrator
/// actually launched (`--role implementer`/`advisor`) and removes the
/// "did the orchestrator launch the wrong role?" confusion during a
/// resume-after-failure recovery. Pure + case-insensitive on the alias.
/// trace:TASK-402 | ai:claude
fn canonical_session_role(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else {
        raw.to_string()
    }
}

/// TASK-112: one-line summary of a recorded Claude session, for the
/// `aida queue work --list-sessions` output. `<liveness> <id8>  <age>
/// <role>  <title>`. trace:TASK-112 | ai:claude
pub fn format_session_line(m: &SessionMeta) -> String {
    format!(
        "{} {:<8}  {:>4}  {:<11}  {}",
        liveness_indicator(m.age_seconds),
        &m.id[..m.id.len().min(8)],
        humanize_age(m.age_seconds),
        m.role.as_deref().unwrap_or("-"),
        m.title.as_deref().unwrap_or("(untitled)"),
    )
}

fn parse_session_meta(path: &Path, mtime: SystemTime, now: SystemTime) -> Result<SessionMeta> {
    use std::io::{BufRead, BufReader};
    let file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string();
    let age_seconds = now.duration_since(mtime).map(|d| d.as_secs()).unwrap_or(0);

    // Parse only the first chunk of lines — `ai-title` appears within
    // the first few hundred events and the SessionStart hook output
    // (the source of role + first @SPEC mentions) is even earlier. Reads
    // stay sub-millisecond per file even on multi-MB session logs.
    const MAX_LINES: usize = 400;
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut role: Option<String> = None;
    let mut title: Option<String> = None;
    let mut spec: Option<String> = None;
    let mut started_at: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut last_cwd: Option<String> = None;
    for (i, line) in reader.lines().enumerate() {
        if i >= MAX_LINES && title.is_some() && role.is_some() && started_at.is_some() {
            break;
        }
        let Ok(line) = line else { continue };

        // STORY-59: capture the most recent cwd we see in the parse
        // window. Each event in Claude Code's .jsonl carries `"cwd":"..."`
        // — we read the last occurrence so a session that switched
        // worktrees mid-flight reports its current cwd, not its launch
        // cwd. Cheap because it's just a substring scan per line.
        if let Some(cwd) = extract_str(&line, "\"cwd\":\"") {
            last_cwd = Some(cwd);
        }

        // First event with a `"timestamp":"..."` field — gives us a
        // close-to-launch-time anchor for FR-1-044's launches.log
        // correlation. Skip the file-history-snapshot's timestamp
        // (which has its own, slightly different timestamp).
        if started_at.is_none() {
            if let Some(ts_str) = extract_str(&line, "\"timestamp\":\"") {
                if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&ts_str) {
                    started_at = Some(parsed.with_timezone(&chrono::Utc));
                }
            }
        }

        // ai-title event — what Claude Code's resume picker shows.
        if title.is_none() {
            if let Some(t) = extract_str(&line, "\"aiTitle\":\"") {
                title = Some(t);
            }
        }

        // Role markers — Claude Code doesn't log the SessionStart hook
        // output as a discrete event, but commands like `aida role show`
        // and shell echos of $AIDA_SESSION_ROLE that ran early in the
        // session leave reliable strings:
        //   - `Role: implementer`     (from aida role show)
        //   - `AIDA_SESSION_ROLE=implementer`
        // Both are checked; the first plausible match wins.
        // trace:FR-1-043 | ai:claude
        if role.is_none() {
            for marker in ["AIDA_SESSION_ROLE=", "Role: "] {
                if let Some(idx) = line.find(marker) {
                    let after = &line[idx + marker.len()..];
                    let name: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect();
                    // Reject empty matches and the literal `(none active)`
                    // / `${role}` cases that show up in shell-template
                    // output or unset-env echos.
                    //
                    // TASK-402 (friction #2): a session the orchestrator
                    // launched with `--role implementer` could be tagged
                    // `dialog` in `--list-sessions` because the early JSONL
                    // scan caught the deprecated alias (a not-yet-migrated
                    // role file / shell echo). Canonicalize the parsed name
                    // so the recorded role reflects the project-wide identity
                    // (`dialog` is the deprecated alias for `advisor`) instead
                    // of confusing the operator mid-recovery.
                    // trace:TASK-402 | ai:claude
                    if !name.is_empty() {
                        role = Some(canonical_session_role(&name));
                        break;
                    }
                }
            }
        }

        // First mentioned SPEC-ID (heuristic: regex-ish scan for typical
        // patterns FR-N, BUG-N-M, TASK-N, etc. in a user message body).
        if spec.is_none() {
            if let Some(s) = first_spec_id(&line) {
                spec = Some(s);
            }
        }

        if i >= MAX_LINES {
            break;
        }
    }

    Ok(SessionMeta {
        id: file_name,
        age_seconds,
        role,
        spec,
        title,
        started_at,
        last_cwd,
        branch: None,
        recent_focus: None,
    })
}

/// Find a JSON string-value for `"<key>":"...` and return the (unescaped
/// only for backslash-quote) value up to the closing quote. Good enough
/// for the simple `"aiTitle":"..."` event we emit ourselves; not a full
/// JSON parser.
fn extract_str(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let mut end = start;
    let bytes = line.as_bytes();
    while end < bytes.len() {
        let b = bytes[end];
        if b == b'\\' && end + 1 < bytes.len() {
            end += 2;
            continue;
        }
        if b == b'"' {
            break;
        }
        end += 1;
    }
    Some(line[start..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Pull the first SPEC-ID-shaped token (FR-1-042, BUG-1-038, TASK-001,
/// EPIC-2, …) from a line. Skips negatives like ANSI codes and `1-2`
/// number ranges.
fn first_spec_id(line: &str) -> Option<String> {
    // Manual scan: walk the line looking for an alpha run of length ≥2,
    // followed immediately by `-` and digits. Optionally followed by `-`
    // and digits again (node-aware form). Return the first match.
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        if !bytes[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i - start < 2 {
            continue;
        }
        if i >= n || bytes[i] != b'-' {
            continue;
        }
        let after_dash = i + 1;
        if after_dash >= n || !bytes[after_dash].is_ascii_digit() {
            continue;
        }
        // Walk through digits, optionally another -digits segment.
        let mut j = after_dash;
        while j < n && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j < n && bytes[j] == b'-' && j + 1 < n && bytes[j + 1].is_ascii_digit() {
            j += 1;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
        }
        // Reject very short prefixes like "v-1" or "x-2" — require ≥2
        // alpha chars (already guaranteed) AND the prefix to be one of
        // the conventional SPEC-ID prefixes. Cheap check:
        let prefix = &line[start..start + (i - start)];
        if matches!(
            prefix,
            "FR" | "BUG"
                | "TASK"
                | "EPIC"
                | "STORY"
                | "SPIKE"
                | "SPRINT"
                | "FOLDER"
                | "META"
                | "UR"
                | "SR"
                | "CR"
                | "REQ"
                | "NFR"
                | "SPEC"
        ) {
            return Some(line[start..j].to_string());
        }
    }
    None
}

/// STORY-58: column widths for the session-list table, computed over a
/// chosen set of rows. Lifted out of `print_table` so the grouped path can
/// compute one shared width set from the union of "this worktree" + parent
/// rows, keeping columns aligned across both sections.
/// trace:STORY-58 | ai:claude
struct TableWidths {
    id_w: usize,
    age_w: usize,
    role_w: usize,
    spec_w: usize,
    worktree_w: usize,
}

impl TableWidths {
    /// Worktree column = `<basename> @ <branch>` (truncated). 28 chars is
    /// enough for "aida-epic-20 @ epic-20-batch3" without squeezing TITLE.
    const WORKTREE_W: usize = 28;

    fn compute<'a, I: Iterator<Item = &'a SessionMeta>>(sessions: I) -> Self {
        let mut role_w = 4usize;
        let mut spec_w = 4usize;
        for s in sessions {
            role_w = role_w.max(s.role.as_deref().unwrap_or("-").len());
            spec_w = spec_w.max(s.spec.as_deref().unwrap_or("-").len());
        }
        Self {
            id_w: 8,
            age_w: 6,
            role_w,
            spec_w,
            worktree_w: Self::WORKTREE_W,
        }
    }
}

fn print_table(sessions: &[SessionMeta]) {
    let widths = TableWidths::compute(sessions.iter());
    print_table_with_widths(sessions, &widths);
}

/// STORY-58: render the session-list table using caller-supplied widths
/// (so two grouped sections can share one column layout).
/// trace:STORY-58 | ai:claude
fn print_table_with_widths(sessions: &[SessionMeta], w: &TableWidths) {
    println!(
        "{}",
        format!(
            " {:<id_w$}  {:<age_w$}  {:<role_w$}  {:<spec_w$}  {:<wt_w$}  {}",
            "ID",
            "AGE",
            "ROLE",
            "SPEC",
            "WORKTREE",
            "RECENT FOCUS",
            id_w = w.id_w,
            age_w = w.age_w,
            role_w = w.role_w,
            spec_w = w.spec_w,
            wt_w = w.worktree_w,
        )
        .dimmed()
    );

    for s in sessions {
        let id_short = &s.id[..s.id.len().min(w.id_w)];
        let age = humanize_age(s.age_seconds);
        let role = s.role.as_deref().unwrap_or("-");
        let spec = s.spec.as_deref().unwrap_or("-");
        let focus = s.recent_focus.as_deref().unwrap_or("-");
        let worktree =
            format_worktree_label(s.last_cwd.as_deref(), s.branch.as_deref(), w.worktree_w);
        let live = liveness_indicator(s.age_seconds);
        // Color the indicator: bright green when truly live, yellow for
        // recent, dim for idle. Width of the indicator slot is one cell.
        let live_colored = match live {
            "●" => live.green().bold().to_string(),
            "◐" => live.yellow().to_string(),
            _ => live.dimmed().to_string(),
        };
        println!(
            "{} {:<id_w$}  {:<age_w$}  {:<role_w$}  {:<spec_w$}  {:<wt_w$}  {}",
            live_colored,
            id_short.bold(),
            age,
            role.yellow(),
            spec.cyan(),
            worktree.cyan(),
            focus.cyan(),
            id_w = w.id_w,
            age_w = w.age_w,
            role_w = w.role_w,
            spec_w = w.spec_w,
            wt_w = w.worktree_w,
        );
    }
}

/// STORY-59: render `<basename> @ <branch>` with truncation. Empty/no
/// cwd shows a dash; missing branch shows just the basename.
/// trace:STORY-59 | ai:claude
fn format_worktree_label(cwd: Option<&str>, branch: Option<&str>, max: usize) -> String {
    let Some(cwd) = cwd else {
        return "-".to_string();
    };
    let basename = std::path::Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd);
    let label = match branch {
        Some(b) => format!("{} @ {}", basename, b),
        None => basename.to_string(),
    };
    if label.chars().count() <= max {
        return label;
    }
    let mut out: String = label.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn humanize_age(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else {
        format!("{}w", secs / (7 * 86_400))
    }
}

fn pick_interactive(limit: usize) -> Result<String> {
    let mut sessions = collect_sessions(limit)?;
    if sessions.is_empty() {
        anyhow::bail!("no past sessions in this directory");
    }
    fill_branches(&mut sessions);
    fill_recent_focus(&mut sessions);
    let labels: Vec<String> = sessions
        .iter()
        .map(|s| {
            format!(
                "{} {:<8}  {:<6}  {:<10}  {:<12}  {:<24}  {}",
                liveness_indicator(s.age_seconds),
                &s.id[..s.id.len().min(8)],
                humanize_age(s.age_seconds),
                s.role.as_deref().unwrap_or("-"),
                s.spec.as_deref().unwrap_or("-"),
                format_worktree_label(s.last_cwd.as_deref(), s.branch.as_deref(), 24),
                s.recent_focus.as_deref().unwrap_or("-"),
            )
        })
        .collect();

    let pick = inquire::Select::new("Resume which session?", labels)
        .with_help_message("↑↓ to move, type to filter, Enter to resume, Esc to cancel")
        .prompt()
        .context("interactive picker cancelled")?;

    // Map the picked label back to its session id. Labels start with a
    // 1-char liveness glyph + space; the id sits at chars [2..10].
    // STORY-59: account for the leading liveness column.
    let id_prefix: String = pick.chars().skip(2).take(8).collect();
    let chosen = sessions
        .iter()
        .find(|s| s.id.starts_with(id_prefix.trim()))
        .ok_or_else(|| anyhow::anyhow!("could not match picked label back to a session id"))?;
    Ok(chosen.id.clone())
}

/// Resolve a (possibly truncated) session id prefix to a full id. Errors
/// when the prefix matches zero or multiple sessions.
fn resolve_id(prefix: &str) -> Result<String> {
    // Allow a generous walk — user might have written down the id from a
    // weeks-old session; collect everything and filter.
    let all = collect_sessions(usize::MAX)?;
    let matches: Vec<&SessionMeta> = all.iter().filter(|s| s.id.starts_with(prefix)).collect();
    match matches.len() {
        0 => anyhow::bail!("no session matches id prefix `{}`", prefix),
        1 => Ok(matches[0].id.clone()),
        _ => anyhow::bail!(
            "{} sessions match `{}` — use a longer prefix",
            matches.len(),
            prefix
        ),
    }
}

/// TASK-264: a spec status that means "work is still in flight" — the
/// `forget` anchor-spec guard refuses (without `--force`) to drop a
/// session anchored to one. `Done` counts because it means "finished on
/// a branch, not yet merged" (STORY-86) — the merge auto-bumps it to
/// `Completed`, so a `Done` spec still has unmerged artifacts to protect.
/// trace:TASK-264 | ai:claude
fn spec_status_in_flight(status: &aida_core::RequirementStatus) -> bool {
    matches!(
        status,
        aida_core::RequirementStatus::InProgress
            | aida_core::RequirementStatus::Done
            // STORY-332: a punted spec was being worked and may carry
            // partial unmerged branch work — keep its session out of a
            // bulk `forget`.
            | aida_core::RequirementStatus::NeedsAttention
    )
}

/// TASK-264: `aida session forget <id>` — explicit removal of one tracked
/// Claude Code session. Deletes the session's `.jsonl` metadata file so
/// it drops out of `aida session list`. Sibling of `aida session prune`
/// (bulk, age-based) — `forget` is single-target, addressed by id. Both
/// share the `.aida/session-prune.log` audit trail.
///
/// Two guards protect mid-work artifacts, each overridable with `--force`:
///   1. the session currently running this command (CLAUDE_CODE_SESSION_ID)
///   2. any session whose anchor spec is still in flight (In Progress, or
///      Done-but-not-yet-merged)
///
/// trace:TASK-264 | ai:claude
pub fn forget(id_query: &str, force: bool, dry_run: bool, yes: bool) -> Result<()> {
    let id_query = id_query.trim();
    if id_query.is_empty() {
        anyhow::bail!("a session id (or 8-char prefix) is required");
    }
    let cwd = std::env::current_dir().context("could not determine cwd")?;

    // Search the same dirs `aida session list` walks: this cwd's encoded
    // project dir, plus the parent project's when run inside a worktree —
    // so an id picked off either table in `list` resolves here.
    let mut search_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(d) = claude_project_dir(&cwd) {
        if d.is_dir() {
            search_dirs.push(d);
        }
    }
    if let Some(parent) = crate::parent_project_root_for_session(&cwd) {
        if parent != cwd {
            if let Ok(d) = claude_project_dir(&parent) {
                if d.is_dir() && !search_dirs.contains(&d) {
                    search_dirs.push(d);
                }
            }
        }
    }
    if search_dirs.is_empty() {
        anyhow::bail!("no Claude Code session storage found for this project");
    }

    // Collect every .jsonl whose id starts with the query prefix.
    let now = SystemTime::now();
    let mut matches: Vec<(SessionMeta, PathBuf, u64)> = Vec::new();
    for dir in &search_dirs {
        let Ok(read) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in read.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if !stem.starts_with(id_query) {
                continue;
            }
            let Ok(fs_meta) = entry.metadata() else {
                continue;
            };
            let Ok(mtime) = fs_meta.modified() else {
                continue;
            };
            if let Ok(meta) = parse_session_meta(&path, mtime, now) {
                matches.push((meta, path, fs_meta.len()));
            }
        }
    }

    let (meta, path, size) = match matches.len() {
        0 => anyhow::bail!(
            "no tracked session matches id `{}` — check `aida session list`",
            id_query
        ),
        1 => matches.into_iter().next().unwrap(),
        n => {
            eprintln!("{} sessions match `{}`:", n, id_query);
            for (m, _, _) in &matches {
                eprintln!("  {}", &m.id[..m.id.len().min(12)]);
            }
            anyhow::bail!("ambiguous id `{}` — use a longer prefix", id_query);
        }
    };

    // Enrich the single match with its most-recent spec (RECENT FOCUS),
    // so the anchor-spec guard and the confirmation use the live signal.
    let mut one = vec![meta];
    fill_recent_focus(&mut one);
    let meta = one.pop().unwrap();
    let id8 = &meta.id[..meta.id.len().min(8)];

    // Guard 1: the session currently running this command. Claude Code
    // exports its conversation id as CLAUDE_CODE_SESSION_ID, and that id
    // is the .jsonl file stem — so a direct equality test identifies the
    // live session. trace:TASK-264 | ai:claude
    let is_active = std::env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .is_some_and(|active| active == meta.id);
    if is_active && !force {
        anyhow::bail!(
            "session {} is the one running this command — \
             pass --force to forget it anyway",
            id8
        );
    }

    // Resolve the anchor spec (most-recent focus, else the launch spec)
    // and its status from the requirement store.
    let anchor_spec = meta.recent_focus.clone().or_else(|| meta.spec.clone());
    let mut anchor_display = anchor_spec.clone();
    let mut anchor_status: Option<aida_core::RequirementStatus> = None;
    if let Some(spec) = anchor_spec.as_deref() {
        if let Ok(root) = crate::find_project_root() {
            if let Some(store) = crate::load_store_for_lookup(&root) {
                if let Some(req) = store.get_requirement_by_spec_id(spec) {
                    anchor_display = Some(
                        req.agreed_id
                            .clone()
                            .or_else(|| req.spec_id.clone())
                            .unwrap_or_else(|| spec.to_string()),
                    );
                    anchor_status = Some(req.status.clone());
                }
            }
        }
    }

    // Guard 2: anchor spec still in flight — forgetting would drop
    // metadata for unmerged work. trace:TASK-264 | ai:claude
    let in_flight = anchor_status.as_ref().is_some_and(spec_status_in_flight);
    if in_flight && !force {
        anyhow::bail!(
            "session {}'s anchor spec {} is still in flight ({}) — \
             pass --force to forget it anyway",
            id8,
            anchor_display.as_deref().unwrap_or("?"),
            anchor_status
                .as_ref()
                .map(|s| s.to_string())
                .unwrap_or_default()
        );
    }

    // Confirmation summary: role, anchor spec + status, age, last activity
    // — enough for the user to verify they're not deleting live work.
    let session_age = meta
        .started_at
        .map(|t| (chrono::Utc::now() - t).num_seconds().max(0) as u64)
        .map(humanize_age)
        .unwrap_or_else(|| "?".to_string());
    println!(
        "{} Forgetting session {} ({}, age {}, last active {} ago)",
        "ℹ".cyan(),
        id8.yellow(),
        meta.role.as_deref().unwrap_or("unknown role"),
        session_age,
        humanize_age(meta.age_seconds),
    );
    let anchor_line = match (&anchor_display, &anchor_status) {
        (Some(s), Some(st)) => format!("{} [{}]", s, st),
        (Some(s), None) => format!("{} (status unknown)", s),
        (None, _) => "(none tracked)".to_string(),
    };
    println!("   {}  {}", "anchor spec:".dimmed(), anchor_line);
    println!("   {}  {}", "session file:".dimmed(), path.display());
    if is_active {
        println!(
            "   {} this is the session running `aida session forget` — --force given",
            "⚠".yellow()
        );
    }
    if in_flight {
        println!(
            "   {} anchor spec is still in flight — forgetting unmerged work metadata, --force given",
            "⚠".yellow()
        );
    }

    if dry_run {
        println!("{}", "(--dry-run; nothing removed)".dimmed());
        return Ok(());
    }

    if !yes {
        use std::io::Write;
        print!("Continue? [y/N] ");
        std::io::stdout().flush().ok();
        let mut ans = String::new();
        if std::io::stdin().read_line(&mut ans).is_err()
            || !matches!(ans.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("could not delete session file {}", path.display()))?;

    // Audit trail — append to the same `.aida/session-prune.log`, in the
    // same `<iso>\t<size>\t<age_seconds>\t<path>` shape `aida session
    // prune` writes, so both removal paths read uniformly.
    if let Ok(root) = crate::find_project_root() {
        let log_path = root.join(".aida").join("session-prune.log");
        if let Some(parent) = log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = writeln!(
                f,
                "{}\t{}\t{}\t{}",
                chrono::Utc::now().to_rfc3339(),
                size,
                meta.age_seconds,
                path.display()
            );
        }
    }

    println!("{} session {} forgotten", "✓".green(), id8);
    Ok(())
}

/// Replace this process with `claude --resume <id>`. Falls back to spawn
/// + wait on platforms without exec semantics. `permission_mode`, when
/// given, is passed through so a resumed `aida queue work` session keeps
/// the same permission posture as a fresh one. trace:TASK-112 | ai:claude
pub fn exec_claude_resume(id: &str, permission_mode: Option<&str>) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("claude");
    cmd.args(["--resume", id]);
    if let Some(m) = permission_mode {
        cmd.args(["--permission-mode", m]);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        // exec only returns on failure
        anyhow::bail!("failed to exec claude: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn claude")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TASK-402 (friction #2): a session JSONL that echoed the deprecated
    // `dialog` alias must report the canonical `advisor` role in
    // `--list-sessions`, so a recovery operator doesn't think the orchestrator
    // launched the wrong role. Other roles pass through unchanged.
    // trace:TASK-402 | ai:claude
    #[test]
    fn canonical_session_role_normalizes_dialog_alias() {
        assert_eq!(canonical_session_role("dialog"), "advisor");
        assert_eq!(canonical_session_role("Dialog"), "advisor");
        assert_eq!(canonical_session_role("DIALOG"), "advisor");
        // Real roles are untouched.
        assert_eq!(canonical_session_role("implementer"), "implementer");
        assert_eq!(canonical_session_role("advisor"), "advisor");
        assert_eq!(canonical_session_role("reviewer"), "reviewer");
    }

    #[test]
    fn session_cwd_scoping_separates_sibling_project_trees() {
        // BUG-447: the reported bleed — a TASK-007 session in the deleted
        // `~/ai` project must NOT match a `~/ai/dummy1` lookup.
        let root = Path::new("/home/joe/ai/dummy1");
        // main worktree (cwd == root) and a subdir of it are in scope
        assert!(session_cwd_in_project("/home/joe/ai/dummy1", root));
        assert!(session_cwd_in_project("/home/joe/ai/dummy1/src", root));
        // the stale sibling-tree session is OUT of scope
        assert!(!session_cwd_in_project("/home/joe/ai-task-007", root));
        assert!(!session_cwd_in_project("/home/joe/ai", root));
    }

    #[test]
    fn session_cwd_scoping_includes_sibling_worktree() {
        // `aida queue work` creates `<parent>/<repo_name>-<slug>` worktrees;
        // their sessions belong to this project and must stay in scope.
        let root = Path::new("/home/joe/ai/dummy1");
        assert!(session_cwd_in_project("/home/joe/ai/dummy1-task-007", root));
        assert!(session_cwd_in_project(
            "/home/joe/ai/dummy1-task-007-some-title",
            root
        ));
        // a genuinely different sibling project (no shared `<root>-` prefix)
        // is out of scope.
        assert!(!session_cwd_in_project("/home/joe/ai/other", root));
    }

    /// BUG-112: build a manifest with `(spec, position, started_at)` items
    /// for the RECENT FOCUS precedence tests.
    fn manifest_with(
        items: &[(&str, u32, Option<chrono::DateTime<chrono::Utc>>)],
    ) -> crate::session_manifest::SessionManifest {
        use crate::session_manifest::{ManifestItem, SessionManifest};
        SessionManifest {
            session_id: "lease01".to_string(),
            planned_at: chrono::Utc::now(),
            plan_source: "test".to_string(),
            claude_session_id: None,
            batch_name: None,
            plan: None,
            items: items
                .iter()
                .map(|(spec, pos, started)| ManifestItem {
                    spec_id: spec.to_string(),
                    position: *pos,
                    status_at_plan: "Approved".to_string(),
                    started_at: *started,
                    completed_at: None,
                    note: None,
                })
                .collect(),
        }
    }

    /// BUG-112: the activity log is the live signal — its most-recent
    /// spec wins over the manifest's planned items.
    #[test]
    fn recent_focus_prefers_activity_log_over_manifest() {
        let m = manifest_with(&[("STORY-1", 1, None)]);
        assert_eq!(
            recent_focus_from(&m, Some("BUG-9")).as_deref(),
            Some("BUG-9")
        );
    }

    /// BUG-112: with no activity log, fall back to the most recently
    /// picked-up planned item — latest `started_at`, else highest
    /// `position`, and a started item always outranks an unstarted one.
    #[test]
    fn recent_focus_falls_back_to_latest_manifest_item() {
        let early: chrono::DateTime<chrono::Utc> = "2026-05-17T10:00:00Z".parse().unwrap();
        let late: chrono::DateTime<chrono::Utc> = "2026-05-17T12:00:00Z".parse().unwrap();
        let by_time = manifest_with(&[("TASK-1", 1, Some(early)), ("TASK-2", 2, Some(late))]);
        assert_eq!(recent_focus_from(&by_time, None).as_deref(), Some("TASK-2"));

        let by_position = manifest_with(&[("TASK-3", 1, None), ("TASK-4", 2, None)]);
        assert_eq!(
            recent_focus_from(&by_position, None).as_deref(),
            Some("TASK-4")
        );

        let started_beats_unstarted =
            manifest_with(&[("TASK-5", 1, Some(early)), ("TASK-6", 9, None)]);
        assert_eq!(
            recent_focus_from(&started_beats_unstarted, None).as_deref(),
            Some("TASK-5")
        );
    }

    /// BUG-112: a manifest with no items yields no focus → renders `-`.
    #[test]
    fn recent_focus_none_for_empty_manifest() {
        let m = manifest_with(&[]);
        assert_eq!(recent_focus_from(&m, None), None);
    }

    #[test]
    fn extract_str_basic() {
        let line = r#"{"type":"ai-title","aiTitle":"Hello world","sessionId":"abc"}"#;
        assert_eq!(
            extract_str(line, "\"aiTitle\":\""),
            Some("Hello world".into())
        );
    }

    #[test]
    fn extract_str_handles_escaped_quotes() {
        let line = r#"{"aiTitle":"Said \"hi\" to bob"}"#;
        assert_eq!(
            extract_str(line, "\"aiTitle\":\""),
            Some(r#"Said "hi" to bob"#.into())
        );
    }

    #[test]
    fn first_spec_id_finds_node_aware_form() {
        assert_eq!(
            first_spec_id("Working on FR-1-042 today"),
            Some("FR-1-042".into())
        );
        assert_eq!(
            first_spec_id("BUG-1-017 is fixed"),
            Some("BUG-1-017".into())
        );
        assert_eq!(
            first_spec_id("see EPIC-2 and TASK-1"),
            Some("EPIC-2".into())
        );
    }

    #[test]
    fn first_spec_id_skips_non_specs() {
        assert_eq!(first_spec_id("version 1-2 compatible"), None);
        assert_eq!(first_spec_id("not-spec-1 here"), None);
        // X is too short / not in the prefix list
        assert_eq!(first_spec_id("X-1 isn't a spec"), None);
    }

    /// TASK-264: the `forget` anchor-spec guard fires for in-flight work
    /// (In Progress, Done-not-merged) and stays quiet for everything else.
    #[test]
    fn spec_status_in_flight_covers_unmerged_work() {
        use aida_core::RequirementStatus::*;
        assert!(spec_status_in_flight(&InProgress));
        assert!(spec_status_in_flight(&Done));
        // STORY-332: a punted spec is still in flight.
        assert!(spec_status_in_flight(&NeedsAttention));
        for safe in [Draft, Approved, Planned, Completed, Rejected] {
            assert!(
                !spec_status_in_flight(&safe),
                "{safe} should not block forget",
            );
        }
    }

    #[test]
    fn humanize_age_buckets() {
        assert_eq!(humanize_age(5), "5s");
        assert_eq!(humanize_age(120), "2m");
        assert_eq!(humanize_age(7200), "2h");
        assert_eq!(humanize_age(2 * 86_400), "2d");
        assert_eq!(humanize_age(2 * 7 * 86_400), "2w");
    }

    #[test]
    fn claude_project_dir_encodes_path() {
        // We can only assert the encoded suffix since the dir prefix
        // depends on $HOME at test time.
        let dir = claude_project_dir(Path::new("/home/joe/ai/aida")).unwrap();
        assert!(dir.to_string_lossy().ends_with("-home-joe-ai-aida"));
    }

    /// FR-1-044: launch-log correlation finds the closest record within
    /// the time window and ignores records outside it.
    #[test]
    fn match_launch_picks_closest_within_window() {
        let base: chrono::DateTime<chrono::Utc> = "2026-05-04T18:00:00Z".parse().unwrap();
        let mk = |secs_offset: i64, title: &str| LaunchRecord {
            ts: base + chrono::Duration::seconds(secs_offset),
            role: "implementer".into(),
            title: title.into(),
        };
        let launches = vec![
            mk(-120, "way-before"),     // 2min before — outside 60s window
            mk(-10, "ten-before"),      // 10s before — inside window
            mk(45, "forty-five-after"), // 45s after — inside window, but farther than -10
            mk(200, "way-after"),       // outside window
        ];
        let session_started = base; // exactly base
        let m = match_launch(&launches, session_started, 60).unwrap();
        assert_eq!(m.title, "ten-before");
    }

    #[test]
    fn match_launch_returns_none_when_all_outside_window() {
        let base: chrono::DateTime<chrono::Utc> = "2026-05-04T18:00:00Z".parse().unwrap();
        let launches = vec![LaunchRecord {
            ts: base + chrono::Duration::seconds(-200),
            role: "implementer".into(),
            title: "old".into(),
        }];
        assert!(match_launch(&launches, base, 60).is_none());
    }

    #[test]
    fn sanitize_for_tsv_drops_tabs_newlines() {
        assert_eq!(sanitize_for_tsv("a\tb\nc"), "a b c");
        assert_eq!(sanitize_for_tsv("plain"), "plain");
    }

    /// STORY-59: liveness indicator buckets — `●` live (<5min), `◐`
    /// recent (<1h), space for idle. The widths are visual; the test
    /// guards the bucket boundaries.
    /// trace:STORY-59 | ai:claude
    #[test]
    fn liveness_indicator_buckets() {
        assert_eq!(liveness_indicator(0), "●");
        assert_eq!(liveness_indicator(4 * 60 + 59), "●");
        assert_eq!(liveness_indicator(5 * 60), "◐");
        assert_eq!(liveness_indicator(59 * 60), "◐");
        assert_eq!(liveness_indicator(60 * 60), " ");
        assert_eq!(liveness_indicator(86_400), " ");
    }

    /// STORY-59: worktree label = "<basename> @ <branch>", truncated
    /// with `…` when it overflows the column width. Empty cwd → "-".
    /// Missing branch → just the basename.
    /// trace:STORY-59 | ai:claude
    #[test]
    fn worktree_label_formatting() {
        assert_eq!(format_worktree_label(None, None, 28), "-");
        assert_eq!(
            format_worktree_label(Some("/home/joe/ai/aida"), Some("main"), 28),
            "aida @ main"
        );
        assert_eq!(
            format_worktree_label(Some("/home/joe/ai/aida-epic-20"), None, 28),
            "aida-epic-20"
        );
        let truncated = format_worktree_label(
            Some("/home/joe/ai/aida-epic-20"),
            Some("epic-20-batch3-followups-with-more"),
            28,
        );
        assert!(truncated.chars().count() <= 28);
        assert!(truncated.ends_with('…'));
    }

    /// STORY-59: extract_str pulls cwd out of a Claude Code event line.
    /// We piggyback on the existing `extract_str` helper; this test
    /// guards the marker we use.
    /// trace:STORY-59 | ai:claude
    #[test]
    fn extract_cwd_from_event_line() {
        let line = r#"{"type":"user","cwd":"/home/joe/ai/aida-epic-20","message":{"role":"user"}}"#;
        assert_eq!(
            extract_str(line, "\"cwd\":\""),
            Some("/home/joe/ai/aida-epic-20".into())
        );
    }

    /// TASK-31: reviewer sessions get `review@<scope>` regardless of branch.
    #[test]
    fn derive_session_name_reviewer_uses_review_prefix() {
        assert_eq!(
            derive_session_name("PR-10", "pr-10", "reviewer"),
            Some("review@PR-10".to_string())
        );
        assert_eq!(
            derive_session_name("MR-7", "mr-7-anything", "reviewer"),
            Some("review@MR-7".to_string())
        );
    }

    /// TASK-31: implementer on epic-batch shape collapses to EPIC-N:batchM.
    #[test]
    fn derive_session_name_epic_batch_shape() {
        assert_eq!(
            derive_session_name("EPIC-20", "epic-20-batch11", "implementer"),
            Some("EPIC-20:batch11".to_string())
        );
        // Case-insensitive scope; batch must be `batch<digits>`.
        assert_eq!(
            derive_session_name("epic-20", "epic-20-batch12", "implementer"),
            Some("EPIC-20:batch12".to_string())
        );
    }

    /// TASK-31: implementer with branch tail beyond the scope-slug uses
    /// `<role>@<scope>:<suffix>`.
    #[test]
    fn derive_session_name_implementer_with_suffix() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42-spike", "implementer"),
            Some("impl@FR-42:spike".to_string())
        );
    }

    /// TASK-31: when the branch matches the scope-slug exactly, fall back
    /// to the no-suffix form.
    #[test]
    fn derive_session_name_implementer_no_suffix() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42", "implementer"),
            Some("impl@FR-42".to_string())
        );
    }

    /// TASK-31: long names are truncated to 64 chars.
    #[test]
    fn derive_session_name_truncates() {
        let scope = "feature:".to_string() + &"x".repeat(80);
        let name = derive_session_name(&scope, "feature-x", "implementer").unwrap();
        assert!(name.len() <= 64, "got len {}: {}", name.len(), name);
    }

    /// TASK-31: empty/sentinel role falls back to `session@<scope>`.
    #[test]
    fn derive_session_name_empty_role_uses_session_label() {
        assert_eq!(
            derive_session_name("FR-42", "fr-42-misc", "-"),
            Some("session@FR-42:misc".to_string())
        );
    }

    /// TASK-31: free-form scopes (not TYPE-NUM) keep their original case.
    #[test]
    fn derive_session_name_free_form_scope_keeps_case() {
        assert_eq!(
            derive_session_name("feature:auth", "feature-auth-login", "implementer"),
            Some("impl@feature:auth".to_string())
        );
    }

    // --- Headless launch (STORY-263) --------------------------------------

    /// SPIKE-7's mandatory flag set must all be present — a headless run that
    /// is missing any of these silently does nothing or hangs.
    #[test]
    fn claude_headless_args_has_spike7_mandatory_flags() {
        let args = claude_headless_args(
            "/aida-review --pr 7",
            "019e0000-0000-7000-8000-000000000000",
        );
        assert!(args.contains(&"-p".to_string()), "print mode: {args:?}");
        assert!(
            args.contains(&"bypassPermissions".to_string()),
            "permission mode (spike Q2): {args:?}"
        );
        assert!(
            args.contains(&"stream-json".to_string()),
            "stream-json output (spike Q6): {args:?}"
        );
        assert!(args.contains(&"--verbose".to_string()), "verbose: {args:?}");
        assert!(
            args.contains(&"--session-id".to_string()),
            "session id (spike Q9): {args:?}"
        );
        // The prompt and the session id both survive into the argv.
        assert!(args.contains(&"/aida-review --pr 7".to_string()));
        assert!(args.contains(&"019e0000-0000-7000-8000-000000000000".to_string()));
    }

    /// STORY-495: a native interactive launch (`permission_mode = None`)
    /// injects NO `--permission-mode` — Claude uses its own default posture.
    /// trace:STORY-495 | ai:claude
    #[test]
    fn claude_session_args_native_omits_permission_mode() {
        let args = claude_session_args(None, None, Some("/aida-pickup"), Some("sid"));
        assert!(
            !args.iter().any(|a| a == "--permission-mode"),
            "native launch must not inject --permission-mode: {args:?}"
        );
        // The session-id and prompt still thread through.
        assert!(args.contains(&"--session-id".to_string()), "{args:?}");
        assert!(args.contains(&"/aida-pickup".to_string()), "{args:?}");
    }

    /// STORY-495: an explicit mode is injected as `--permission-mode <m>`.
    /// trace:STORY-495 | ai:claude
    #[test]
    fn claude_session_args_some_injects_permission_mode() {
        let args = claude_session_args(Some("bypassPermissions"), None, None, None);
        let pos = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("--permission-mode present");
        assert_eq!(
            args.get(pos + 1).map(String::as_str),
            Some("bypassPermissions")
        );
    }

    /// STORY-495 safety invariant: the headless argv ALWAYS forces
    /// `bypassPermissions` regardless of the interactive faithful default —
    /// a prompting (`default`) headless child has no TTY to answer and would
    /// hang the unattended drain forever. This is structurally separate from
    /// `claude_session_args`, so flipping the interactive default can never
    /// reach it. trace:STORY-495 | ai:claude
    #[test]
    fn headless_args_force_bypass_regardless_of_interactive_default() {
        // The interactive builder is now native-by-default…
        let interactive = claude_session_args(None, None, Some("/aida-review"), Some("sid"));
        assert!(!interactive.iter().any(|a| a == "--permission-mode"));
        // …yet the headless builder still hard-forces bypass.
        let headless = claude_headless_args("/aida-review", "sid");
        let pos = headless
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("headless must inject --permission-mode");
        assert_eq!(
            headless.get(pos + 1).map(String::as_str),
            Some("bypassPermissions"),
            "headless must force bypassPermissions: {headless:?}"
        );
    }

    /// `--bare` strips OAuth/keychain auth and breaks login (spike Q1) — the
    /// headless launch must never use it.
    #[test]
    fn claude_headless_args_never_uses_bare() {
        let args = claude_headless_args("/aida-review --pr 7", "sid");
        assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
        // Persistence stays ON — no `--no-session-persistence` either (Q9).
        assert!(
            !args.iter().any(|a| a == "--no-session-persistence"),
            "{args:?}"
        );
    }

    /// STORY-306: the headless-resume argv `--resume`s the punted session
    /// instead of minting a fresh `--session-id`, and keeps every SPIKE-7
    /// mandatory flag.
    #[test]
    fn claude_headless_resume_args_has_resume_and_no_session_id() {
        let sid = "019e0000-0000-7000-8000-000000000000";
        let args = claude_headless_resume_args("proceed with OAuth", sid);
        // Resumes the existing session — `--resume <id>`, not `--session-id`.
        assert!(args.contains(&"--resume".to_string()), "{args:?}");
        assert!(
            !args.iter().any(|a| a == "--session-id"),
            "resume must not mint a new session id: {args:?}"
        );
        assert!(args.contains(&sid.to_string()), "{args:?}");
        // SPIKE-7 mandatory flags intact.
        assert!(args.contains(&"-p".to_string()), "{args:?}");
        assert!(args.contains(&"bypassPermissions".to_string()), "{args:?}");
        assert!(args.contains(&"stream-json".to_string()), "{args:?}");
        assert!(args.contains(&"--verbose".to_string()), "{args:?}");
        assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
        // The resume prompt survives into the argv.
        assert!(args.contains(&"proceed with OAuth".to_string()), "{args:?}");
    }

    /// BUG-327: both headless argv builders must structurally disable
    /// `AskUserQuestion` so a headless reviewer / implementer / advisor
    /// cannot reason past the skill-template instruction added in BUG-280
    /// and bail without writing the verdict file. The flag is variadic on
    /// the claude side (`--disallowed-tools <tools...>`), so the *placement*
    /// matters: the next argv element after the value must start with `--`
    /// to terminate the value list — otherwise the prompt positional at
    /// the tail would be consumed as a second "disallowed tool". This test
    /// pins both the presence and the safe placement.
    /// trace:BUG-327 | ai:claude
    #[test]
    fn headless_argv_disables_askuserquestion_structurally() {
        for (label, args) in [
            ("fresh", claude_headless_args("/aida-review --pr 7", "sid")),
            ("resume", claude_headless_resume_args("proceed", "sid")),
        ] {
            let pos = args
                .iter()
                .position(|a| a == "--disallowed-tools")
                .unwrap_or_else(|| panic!("[{label}] --disallowed-tools missing: {args:?}"));
            assert_eq!(
                args.get(pos + 1).map(String::as_str),
                Some("AskUserQuestion"),
                "[{label}] AskUserQuestion must immediately follow --disallowed-tools: {args:?}",
            );
            // The variadic-terminator: the token after "AskUserQuestion"
            // must start with "--" or "-", otherwise the prompt at the
            // tail gets gobbled into the disallowed-tool list.
            let terminator = args
                .get(pos + 2)
                .unwrap_or_else(|| panic!("[{label}] value list runs off the end: {args:?}"));
            assert!(
                terminator.starts_with('-'),
                "[{label}] next argv element must be a flag (variadic terminator), \
                 got {terminator:?}: {args:?}",
            );
            // Prompt positional still lands at the tail.
            assert_eq!(
                args.last().map(String::as_str),
                if label == "fresh" {
                    Some("/aida-review --pr 7")
                } else {
                    Some("proceed")
                },
                "[{label}] prompt must remain the trailing positional: {args:?}",
            );
        }
    }
}
