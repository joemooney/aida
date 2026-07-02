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

/// Render a registry glyph honoring the active profile. Default Unicode profile
/// reproduces the historical literals byte-for-byte. trace:TASK-840 | ai:claude
fn glyph(g: crate::glyphs::Glyph) -> &'static str {
    crate::glyphs::get(g, crate::find_project_root().ok().as_deref())
}

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
        "●" // live (active in last 5 minutes) — `●` is not a registry glyph.
    } else if age_seconds < 60 * 60 {
        // recent (last hour) — route the partial marker through the registry.
        glyph(crate::glyphs::Glyph::InFlight)
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
        aida_core::RequirementsStore {
            requirements: reqs,
            ..Default::default()
        }
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
    exec_claude_resume(&target, None, false)
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
    contained: bool,
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
    let mode_display = if contained {
        "contained sandbox".to_string()
    } else {
        permission_mode
            .map(|m| format!("--permission-mode {}", m))
            .unwrap_or_else(|| "(native permission posture)".to_string())
    };
    eprintln!(
        "{} {} → claude {} (name: {})",
        glyph(crate::glyphs::Glyph::FlowActive).green().bold(),
        format!("session new (role:{}, title:{:?})", role, title).dimmed(),
        mode_display,
        name_for_log,
    );

    exec_claude(
        permission_mode,
        display_name.as_deref(),
        None,
        None,
        contained,
    )
}

/// TASK-31: derive a claude `--name` value from session metadata. Keeps the
/// /resume picker and terminal title legible when multiple concurrent
/// worktrees are open.
///
/// Convention:
///   - role=reviewer            → `review@<scope>`               (PR/MR work)
///   - scope=EPIC-N + batchM    → `EPIC-N:batchM`                (epic-batch)
///   - other implementer shapes → `<role-label>@<scope>:<suffix>` or
///     `<role-label>@<scope>` if no suffix
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
    s.replace(['\t', '\n', '\r'], " ")
}

/// Replace this process with `claude --permission-mode <mode>`. When `name`
/// is `Some(...)`, also passes `--name <n>` so the launched session is
/// labeled in the /resume picker and terminal title. trace:TASK-31 | ai:claude
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
    contained: bool,
) -> Result<()> {
    exec_claude(
        permission_mode,
        name,
        Some(initial_prompt),
        Some(session_id),
        contained,
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
    contained: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(m) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(m.to_string());
    }
    if contained {
        args.extend(claude_contained_flags());
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
    contained: bool,
) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("claude");
    cmd.args(claude_session_args(
        permission_mode,
        name,
        initial_prompt,
        session_id,
        contained,
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
    contained: bool,
) -> Result<std::process::ExitStatus> {
    std::process::Command::new("claude")
        .args(claude_session_args(
            permission_mode,
            name,
            Some(initial_prompt),
            Some(session_id),
            contained,
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
    contained: bool,
) -> Result<std::process::ExitStatus> {
    // STORY-683: resolve the vendor (default Claude) and delegate. Keeping the
    // `spawn_claude_headless` name + signature means every existing call site is
    // unchanged and an un-configured drain is byte-identical to before — the
    // vendor only diverges when `AIDA_HEADLESS_VENDOR` / `[orchestrator]
    // headless_vendor` selects Codex. trace:STORY-683 | ai:claude
    let vendor = resolve_headless_vendor(&headless_worktree_root());
    spawn_vendor_headless(vendor, prompt, session_id, log_path, tee_opts, contained)
}

/// STORY-683: spawn (not exec) a headless run of `vendor`'s CLI and wait,
/// returning the exit status. The vendor-neutral generalization of
/// [`spawn_claude_headless`]: it builds the right argv per vendor
/// ([`headless_vendor_args`] — `claude -p …` vs `codex exec …`), applies the
/// opt-in OS-boundary wrapper (`bwrap`, STORY-612) around whichever program, and
/// sets `AIDA_HEADLESS=1` in the env. The Claude path is unchanged from the
/// pre-STORY-683 behavior. trace:STORY-683 | ai:claude
pub fn spawn_vendor_headless(
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // STORY-683: surface a non-default vendor so an operator watching a drain
    // knows it is running on Codex, not Claude. The default (Claude) launch
    // stays silent so existing output is unchanged. trace:STORY-683 | ai:claude
    if vendor != HeadlessVendor::Claude {
        eprintln!(
            "{} headless drain phase on vendor `{}`",
            "Vendor:".cyan().bold(),
            vendor.as_str()
        );
    }
    // STORY-612: apply the opt-in OS-boundary wrapper (`bwrap`) around the whole
    // headless process when `[contained] os_wrap` is on. STORY-683: the wrapped
    // program is the vendor binary (`claude` / `codex`), not hardcoded `claude`.
    // trace:STORY-612 trace:STORY-683 | ai:claude
    let worktree = headless_worktree_root();
    // TASK-1081: route the vendor binary through the mock-substitution resolver
    // before wrapping — `AIDA_AGENT_CMD` swaps the program, argv unchanged; unset
    // yields the native vendor binary (byte-identical). trace:TASK-1081
    let (program, args) = os_wrapped_program_and_args(
        &worktree,
        &resolve_agent_program(vendor.program()),
        headless_vendor_args(vendor, prompt, session_id, contained),
    )?;
    let status = Command::new(program)
        .args(args)
        .env("AIDA_HEADLESS", "1")
        .stdout(Stdio::from(log))
        .status()
        .with_context(|| format!("failed to spawn {}", vendor.program()))?;
    tee.stop();
    Ok(status)
}

/// STORY-612: the worktree root a non-resume headless drain runs in — its
/// `.aida-store` sibling and the worktree itself are the rw surfaces the OS
/// wrapper binds. Resolve the project root from cwd, falling back to cwd itself
/// (and `.` if even that fails) so the wrapper never panics on a stray env.
/// trace:STORY-612 | ai:claude
fn headless_worktree_root() -> PathBuf {
    crate::find_project_root()
        .ok()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// BUG-226: spawn `claude --resume <id>` and wait — the spawn counterpart
/// of `exec_claude_resume` for the standalone reviewer summary path.
/// trace:BUG-226 | ai:claude
pub fn spawn_claude_resume(
    id: &str,
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    let mut cmd = std::process::Command::new("claude");
    cmd.args(["--resume", id]);
    if let Some(m) = permission_mode {
        cmd.args(["--permission-mode", m]);
    }
    if contained {
        cmd.args(claude_contained_flags());
    }
    cmd.status().context("failed to spawn claude")
}

/// TASK-895: build the argv (after the `codex` program name) for an interactive
/// Codex session hosted in an `aida tui` tab. Codex's interactive CLI takes the
/// initial prompt as a trailing positional (`codex [PROMPT]`) — the analogue of
/// claude's positional first message. Unlike claude there is no caller-minted
/// `--session-id` and no AIDA-addressable `--resume`, so the argv carries only
/// the prompt; AIDA hosts a fresh Codex session per tab (resume-parity is a
/// follow-up). Faithful-launcher posture: no forced approval/sandbox bypass —
/// Codex prompts natively, matching the STORY-495 native default. Pure — the
/// flag set is unit-tested without spawning codex.
// trace:TASK-895 | ai:claude
pub fn codex_session_args(initial_prompt: &str) -> Vec<String> {
    vec![initial_prompt.to_string()]
}

/// TASK-895: replace this process with an interactive `codex <prompt>` session —
/// the Codex analogue of [`exec_claude_with_session`], for a Codex tab hosted by
/// the AIDA TUI. The hosted `aida queue work` process is replaced by `codex`, so
/// all lease / worktree / manifest setup has already run by the time this is
/// reached.
// trace:TASK-895 | ai:claude
pub fn exec_codex_session(initial_prompt: &str) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("codex");
    cmd.args(codex_session_args(initial_prompt));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        anyhow::bail!("failed to exec codex: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = cmd.status().context("failed to spawn codex")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// STORY-683: which vendor's headless CLI drives an orchestrator drain phase.
/// The autonomous drain (`burndown` / `queue work --auto-complete --no-human`)
/// used to hardcode `claude -p`; this enum lets the same spawn path launch
/// `codex exec` instead, so a sustained headless drain can run on Codex.
///
/// `Claude` is the default everywhere — selecting `Codex` is an explicit opt-in
/// (via `AIDA_HEADLESS_VENDOR=codex` or `[orchestrator] headless_vendor =
/// "codex"`), so an un-configured drain is byte-identical to the pre-STORY-683
/// behavior. Prior art for the `codex exec` adapter is `compete.rs::vendor_adapter`.
/// trace:STORY-683 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessVendor {
    /// `claude -p …` — the default. The SPIKE-7 mandatory flag set.
    Claude,
    /// `codex exec …` — the Codex headless CLI. trace:STORY-683
    Codex,
}

impl HeadlessVendor {
    /// The PATH binary this vendor spawns (`claude` / `codex`).
    pub fn program(self) -> &'static str {
        match self {
            HeadlessVendor::Claude => "claude",
            HeadlessVendor::Codex => "codex",
        }
    }

    /// The canonical lowercase token (`claude` / `codex`).
    pub fn as_str(self) -> &'static str {
        match self {
            HeadlessVendor::Claude => "claude",
            HeadlessVendor::Codex => "codex",
        }
    }

    /// Parse a vendor token. Case-insensitive, surrounding whitespace tolerated.
    /// `None` for an unrecognized token so the caller can fall through to the
    /// default rather than launch an unknown binary. trace:STORY-683 | ai:claude
    pub fn parse(raw: &str) -> Option<HeadlessVendor> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(HeadlessVendor::Claude),
            "codex" => Some(HeadlessVendor::Codex),
            _ => None,
        }
    }
}

/// STORY-683: resolve the vendor a headless drain spawn should use. Resolution
/// order, highest precedence first:
///   1. `AIDA_HEADLESS_VENDOR` env (per-host / per-invocation override) — mirrors
///      the `AIDA_OS_WRAP` precedence convention; an unrecognized value is ignored.
///   2. `[orchestrator] headless_vendor` in the project config.
///   3. `Claude` (default) — so an un-configured drain is unchanged.
/// `worktree_root` roots the config read. trace:STORY-683 | ai:claude
pub(crate) fn resolve_headless_vendor(worktree_root: &Path) -> HeadlessVendor {
    if let Some(raw) = std::env::var("AIDA_HEADLESS_VENDOR").ok() {
        if let Some(v) = HeadlessVendor::parse(&raw) {
            return v;
        }
    }
    let cfg = crate::read_project_config_value(worktree_root);
    crate::config_lookup(cfg.as_ref(), "orchestrator", "headless_vendor")
        .and_then(|v| v.as_str())
        .and_then(HeadlessVendor::parse)
        .unwrap_or(HeadlessVendor::Claude)
}

/// STORY-683: build the argv (after the program name) for a headless launch of
/// the given vendor. `Claude` reuses the SPIKE-7 mandatory flag set
/// ([`claude_headless_args_with_posture`]); `Codex` builds the `codex exec`
/// argv (prior art: `compete.rs::vendor_adapter`), with the prompt as the final
/// positional. Pure — both arms are unit-tested without spawning. The `contained`
/// posture only affects the Claude arm today (Codex carries its own sandbox via
/// `--dangerously-bypass-approvals-and-sandbox`). trace:STORY-683 | ai:claude
pub fn headless_vendor_args(
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
) -> Vec<String> {
    match vendor {
        HeadlessVendor::Claude => claude_headless_args_with_posture(prompt, session_id, contained),
        HeadlessVendor::Codex => codex_headless_args(prompt),
    }
}

/// STORY-683: the `codex exec` argv (after the `codex` program name) for a
/// one-shot headless run. Mirrors the working `compete.rs` adapter:
/// `exec --dangerously-bypass-approvals-and-sandbox <prompt>`. Codex's headless
/// `exec` is single-shot and exits on its own (the orchestrator analogue of
/// claude's `-p`); approvals are bypassed so the run is unattended. The prompt
/// is the final positional. Unlike claude there is no `--session-id` /
/// `--output-format stream-json` (codex has no matching resumable session model),
/// so the codex arm does not thread `session_id`. trace:STORY-683 | ai:claude
pub fn codex_headless_args(prompt: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        prompt.to_string(),
    ]
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
    claude_headless_args_with_posture(prompt, session_id, false)
}

pub fn claude_headless_args_with_posture(
    prompt: &str,
    session_id: &str,
    contained: bool,
) -> Vec<String> {
    let permission_mode = if contained {
        "dontAsk"
    } else {
        "bypassPermissions"
    };
    let mut args = vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        permission_mode.to_string(),
    ];
    if contained {
        args.extend(claude_contained_flags());
    }
    args.extend([
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--disallowed-tools".to_string(),
        "AskUserQuestion".to_string(),
        "--session-id".to_string(),
        session_id.to_string(),
        prompt.to_string(),
    ]);
    args
}

pub fn claude_contained_flags() -> Vec<String> {
    vec![
        "--setting-sources".to_string(),
        "project".to_string(),
        "--settings".to_string(),
        claude_contained_settings_json(),
    ]
}

fn claude_contained_settings_json() -> String {
    // Egress allowlist is strictly opt-in via `[contained] allowed_hosts`. When
    // the project root or config can't be resolved we fall back to an empty
    // allowlist — i.e. the pre-STORY-605 behavior. trace:STORY-605 | ai:claude
    let root = crate::find_project_root().ok();
    let allowed_hosts = root
        .as_ref()
        .map(|r| contained_allowed_hosts(r))
        .unwrap_or_default();
    let managed_only = root
        .as_ref()
        .map(|r| contained_managed_domains_only(r))
        .unwrap_or(false);
    contained_settings_json(&allowed_hosts, managed_only)
}

/// Read `[contained] allowed_hosts` (a string array) from the project config.
/// Empty when unset, absent, or malformed — the network-egress restriction is
/// strictly OPT-IN, so an absent/typo'd key never silently restricts a drain.
/// Section is `[contained]` (not `[sandbox]`) to avoid colliding with the
/// `aida sandbox` throwaway-store command's vocabulary. trace:STORY-605 | ai:claude
pub(crate) fn contained_allowed_hosts(project_root: &std::path::Path) -> Vec<String> {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "allowed_hosts")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// STORY-615: read `[contained] managed_domains_only` (bool, default false).
/// When true, the contained settings add `sandbox.network.allowManagedDomainsOnly`
/// so a HEADLESS drain default-DENIES egress (to the managed set + any
/// `allowed_hosts`) WITHOUT the approval prompt the allowlist-only path (STORY-605)
/// hits — which a `claude -p` drain can't answer. Default OFF so a building drain
/// that needs crates.io / github.com / npm isn't silently cut off; opt in only
/// when the drain's egress is fully covered by the managed set + allowed_hosts.
/// trace:STORY-615 | ai:claude
pub(crate) fn contained_managed_domains_only(project_root: &std::path::Path) -> bool {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "managed_domains_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// STORY-617: read `[contained] read_allowlist` (a string array of absolute
/// paths) from the project config. EMPTY when unset, absent, or malformed — the
/// strict read-confinement is strictly OPT-IN (default-ABSENT key), so an
/// absent/typo'd key never silently narrows the readable filesystem. Mirrors the
/// slice-1 `allowed_hosts` rule. When non-empty the os_wrap path binds ONLY
/// these paths (+ the essential system/toolchain paths + the worktree) ro,
/// instead of `--ro-bind / /`. trace:STORY-617 | ai:claude
pub(crate) fn contained_read_allowlist(project_root: &std::path::Path) -> Vec<String> {
    let cfg = crate::read_project_config_value(project_root);
    crate::config_lookup(cfg.as_ref(), "contained", "read_allowlist")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// TASK-809: build the Claude Code MANAGED-settings JSON for a headless drain.
/// STORY-615 already emits `sandbox.network.allowManagedDomainsOnly` in the
/// project `--settings` JSON, but Claude Code only HARD-BLOCKS (deny without a
/// prompt) when that flag arrives via the MANAGED settings tier — the project /
/// `--settings` tier still PROMPTS for a non-allowlisted domain, which a headless
/// `claude -p` drain cannot answer. This is the remaining slice: a managed-tier
/// settings document (delivered by bind-mounting it over the wrapped process's
/// `/etc/claude-code/managed-settings.json` inside the bwrap namespace — no host
/// root, no host pollution) so the headless drain's egress is truly default-deny.
///
/// Per Claude Code's docs, when `allowManagedDomainsOnly` is set in managed
/// settings, ONLY `allowedDomains` from the MANAGED settings are honored — so the
/// operator's `allowed_hosts` are mirrored here too. Pure + total so the shape is
/// unit-tested without spawning. trace:TASK-809 | ai:claude
pub(crate) fn managed_settings_json(allowed_hosts: &[String]) -> String {
    let mut network = serde_json::Map::new();
    network.insert(
        "allowManagedDomainsOnly".to_string(),
        serde_json::json!(true),
    );
    if !allowed_hosts.is_empty() {
        network.insert(
            "allowedDomains".to_string(),
            serde_json::json!(allowed_hosts),
        );
    }
    serde_json::json!({
        "sandbox": {
            "network": serde_json::Value::Object(network)
        }
    })
    .to_string()
}

/// Build the contained-mode `--settings` JSON. With a NON-EMPTY `allowed_hosts`
/// egress allowlist, a `sandbox.network.allowedDomains` key is added so Claude
/// Code's own sandbox (bubblewrap + an out-of-sandbox proxy on Linux) default-
/// denies network egress except to those hosts (STORY-605, SPIKE-61). With an
/// EMPTY allowlist the `network` key is OMITTED entirely — the output is the
/// same shape as the pre-STORY-605 contained settings, so the posture is
/// unchanged unless the operator opts in via `[contained] allowed_hosts`.
///
/// NOTE (slice-1 limitation): a non-allowlisted domain PROMPTS for approval by
/// default; a headless `claude -p` drain can't answer that prompt. True block-
/// without-prompt needs `network.allowManagedDomainsOnly` via MANAGED settings
/// (not this project `--settings`) and is a follow-up — see STORY-605. Pure +
/// total so the "empty → unchanged" invariant is unit-tested. trace:STORY-605
pub(crate) fn contained_settings_json(
    allowed_hosts: &[String],
    managed_domains_only: bool,
) -> String {
    let mut sandbox = serde_json::json!({
        "enabled": true,
        "failIfUnavailable": true,
        "autoAllowBashIfSandboxed": true,
        "allowUnsandboxedCommands": false
    });
    // STORY-615: the `network` key is added when EITHER an allowlist is set OR
    // managed-domains-only is on; with neither it's omitted entirely so the
    // posture is unchanged (the pre-STORY-605 shape). trace:STORY-615
    if !allowed_hosts.is_empty() || managed_domains_only {
        let mut network = serde_json::Map::new();
        if !allowed_hosts.is_empty() {
            network.insert(
                "allowedDomains".to_string(),
                serde_json::json!(allowed_hosts),
            );
        }
        if managed_domains_only {
            network.insert(
                "allowManagedDomainsOnly".to_string(),
                serde_json::json!(true),
            );
        }
        sandbox["network"] = serde_json::Value::Object(network);
    }
    serde_json::json!({
        "permissions": {
            "allow": [
                "Edit(/**)"
            ],
            "deny": destructive_command_deny_rules()
        },
        "sandbox": sandbox
    })
    .to_string()
}

fn destructive_command_deny_rules() -> Vec<&'static str> {
    vec![
        "Bash(rm -rf / *)",
        "Bash(rm -rf /)",
        "Bash(rm -rf ~ *)",
        "Bash(rm -rf ~)",
        "Bash(rm -rf .. *)",
        "Bash(rm -rf ../*)",
        "Bash(git reset --hard *)",
        "Bash(git reset --hard)",
        "Bash(git clean -fd *)",
        "Bash(git clean -fd)",
        "Bash(git clean -fx *)",
        "Bash(git clean -fx)",
        "Bash(git push --force *)",
        "Bash(git push -f *)",
        "Bash(git branch -D *)",
        "Bash(git checkout -- *)",
        "Bash(git restore --source * -- *)",
    ]
}

// ───────────────────────── STORY-612: OS-boundary wrapper ─────────────────────
//
// Slice 2 of the sandbox-execution work (SPIKE-61). The `contained` posture
// (STORY-567/605) turns on Claude Code's *own* Bash sandbox, but Edit/Write/MCP
// run unconfined and there is no OS boundary around the `claude` process itself.
// This wraps the headless `claude -p` spawn in **bubblewrap** so the WHOLE
// process — every tool it drives — is confined at the OS level.
//
// Model = WRITE-confinement (operator decision 2026-06-13): `--ro-bind / /`
// makes the whole filesystem READABLE but read-only, then rw-binds ONLY the code
// worktree, the `.aida-store` worktree, and the build/auth caches. So every
// OS-level WRITE by Edit/Write/MCP/Bash lands inside the worktree; a rogue or
// injected drain cannot `rm -rf ~`, tamper with `~/.ssh`, or scribble outside
// its tree. Network stays SHARED — we never `--unshare-net`, because `claude`
// itself must reach `api.anthropic.com`. Egress is therefore bounded by the
// slice-1 `[contained] allowed_hosts` allowlist (Claude's Bash-sandbox proxy),
// NOT by this FS confinement; READS stay broad. That read-exfil gap is the
// documented limitation a slice-3 strict read-confinement follow-up closes.
//
// Strictly opt-in via `[contained] os_wrap = true` (the slice-1 allowlist lives
// in the same `[contained]` block). Fail-CLOSED: if os_wrap is requested but
// `bwrap` is not on PATH we error rather than silently run unconfined — asking
// for an OS boundary and not getting one must never pass silently.
// trace:STORY-612 | ai:claude

/// Resolve the opt-in `[contained] os_wrap` flag from the project config rooted
/// at `worktree_root`. Defaults to `false` (today's behavior) when unset,
/// absent, malformed, or the config can't be read — the OS boundary is strictly
/// opt-in, mirroring the slice-1 `allowed_hosts` rule. trace:STORY-612 | ai:claude
/// trace:TASK-866 | ai:claude
///
/// The `AIDA_OS_WRAP` environment variable takes PRECEDENCE over the config
/// value (TASK-876): `bwrap` availability is a per-MACHINE property, so its
/// enable switch must be per-machine too. Committing `os_wrap = true` to the
/// tracked `.aida/config.toml` would enable it repo-wide and, being fail-closed,
/// would ERROR on every clone whose host lacks working bwrap. The env override
/// lets an operator enable confinement per-host (`export AIDA_OS_WRAP=1` in a
/// shell / `.bashrc`) with NO shared-config change and NO risk to other clones.
/// Accepts `1`/`true`/`yes` (on) and `0`/`false`/`no` (off), case-insensitive;
/// an unrecognized value is ignored (falls through to config).
/// trace:TASK-876 | ai:claude
pub(crate) fn os_wrap_enabled(worktree_root: &Path) -> bool {
    if let Some(over) = os_wrap_env_override() {
        return over;
    }
    let cfg = crate::read_project_config_value(worktree_root);
    crate::config_lookup(cfg.as_ref(), "contained", "os_wrap")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Parse the `AIDA_OS_WRAP` per-host override. Returns `Some(true)`/`Some(false)`
/// for a recognized truthy/falsey value, `None` when the var is unset or holds
/// an unrecognized value (so the config value is consulted). Case-insensitive;
/// surrounding whitespace tolerated. trace:TASK-876 | ai:claude
fn os_wrap_env_override() -> Option<bool> {
    let raw = std::env::var("AIDA_OS_WRAP").ok()?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Build the bubblewrap argv that goes BETWEEN the `bwrap` program name and the
/// wrapped `claude` command — i.e. the confinement flags only. Pure + total so
/// the write-confinement invariants (ro root, rw worktree, never `--unshare-net`)
/// are unit-tested without spawning anything.
///
/// `rw_paths` are additional read-WRITE bind-mounts (the store worktree, cargo /
/// npm caches, the `~/.claude` auth dir + `~/.claude.json`). They use the
/// `--bind-try` form so a missing path is skipped rather than erroring — only
/// the worktree itself is a hard `--bind` (it always exists; the drain runs in
/// it). trace:STORY-612 | ai:claude
// STORY-617: the default (no read-allowlist) convenience entry — preserved as
// the STORY-612 public signature; production now calls the `_inner` variant
// directly to thread the opt-in allowlist. Still used by the bwrap unit tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn bwrap_confinement_args(worktree: &Path, rw_paths: &[PathBuf]) -> Vec<String> {
    bwrap_confinement_args_inner(worktree, rw_paths, &[])
}

/// STORY-617: the essential system paths a strict read-confinement allowlist
/// always binds (read-only) on top of the operator's `read_allowlist`, because
/// `claude` / `node` / `cargo` cannot run without them. Kept minimal and
/// READ-ONLY: the toolchains live under `/usr` and `/nix`, dynamic linking +
/// shared libs under `/lib*`, system config (incl. resolv.conf and the
/// `/etc/claude-code` managed-settings drop the TASK-809 path mounts over) and
/// TLS roots under `/etc`. `/dev`, `/proc`, `/tmp` are provided separately by
/// the bwrap `--dev`/`--proc`/`--tmpfs` flags. trace:STORY-617 | ai:claude
fn strict_read_essential_paths() -> &'static [&'static str] {
    &[
        "/usr", "/bin", "/sbin", "/lib", "/lib64", "/lib32", "/etc", "/opt", "/nix", "/run", "/var",
    ]
}

/// Build the bubblewrap confinement argv. STORY-617 adds an OPTIONAL strict
/// read-confinement allowlist: when `read_allowlist` is NON-EMPTY the whole-
/// filesystem `--ro-bind / /` base is REPLACED by an enumerated set of ro
/// binds — the essential system/toolchain paths (`strict_read_essential_paths`)
/// plus the operator's allowlist plus the worktree — so host secrets outside
/// the allowlist (`~/.ssh`, `~/.aws`, browser cookies) are simply NOT PRESENT
/// in the sandbox, not merely read-only. When `read_allowlist` is EMPTY the
/// output is byte-for-byte the pre-STORY-617 write-confinement base (ro root),
/// so the default posture is unchanged. The allowlist entries use `--ro-bind-try`
/// so a listed-but-absent path is skipped rather than aborting the launch.
/// Pure + total so both arms are unit-tested without spawning. trace:STORY-617
pub(crate) fn bwrap_confinement_args_inner(
    worktree: &Path,
    rw_paths: &[PathBuf],
    read_allowlist: &[String],
) -> Vec<String> {
    let wt = worktree.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    if read_allowlist.is_empty() {
        // Default (unchanged) posture: whole filesystem readable but READ-ONLY —
        // the STORY-612 write-confinement base.
        args.push("--ro-bind".into());
        args.push("/".into());
        args.push("/".into());
    } else {
        // STORY-617 strict read-confinement: default-ABSENT filesystem. Bind only
        // the essential system/toolchain paths + the operator allowlist (all ro
        // via `--ro-bind-try` so a missing path is skipped, not fatal). The
        // worktree itself is bound rw below.
        for p in strict_read_essential_paths() {
            args.push("--ro-bind-try".into());
            args.push((*p).to_string());
            args.push((*p).to_string());
        }
        for p in read_allowlist {
            args.push("--ro-bind-try".into());
            args.push(p.clone());
            args.push(p.clone());
        }
    }
    args.extend([
        // Fresh /dev, /proc, writable /tmp so toolchains have a sane env.
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        // The code worktree is the one always-present rw surface.
        "--bind".into(),
        wt.clone(),
        wt.clone(),
    ]);
    for p in rw_paths {
        // `--bind-try`: skip silently if the path is absent (caches/auth dirs
        // may not exist on a fresh machine) rather than aborting the launch.
        args.push("--bind-try".into());
        let s = p.to_string_lossy().into_owned();
        args.push(s.clone());
        args.push(s);
    }
    // Run inside the worktree (the parent's cwd may differ, e.g. resume), and
    // tear the sandbox down with the parent so a killed drain leaves nothing.
    args.push("--chdir".into());
    args.push(wt);
    args.push("--die-with-parent".into());
    args
}

/// The single resolver for the PROGRAM a HEADLESS agent spawn actually invokes.
/// Every headless spawn site (drain phase, headless resume, advisor tier, the
/// `claude agents --json` liveness query) passes the vendor binary name it would
/// otherwise hardcode (`claude` / `codex`) through here.
///
/// - `AIDA_AGENT_CMD` unset (or empty/whitespace) → returns the vendor program
///   unchanged, so an un-configured drain is byte-identical to today. This is the
///   faithful-launcher invariant — the override is the ONE explicit opt-in, no
///   hidden behavior change.
/// - `AIDA_AGENT_CMD` set → returns that program in place of the vendor binary.
///   The caller passes the ORIGINAL argv (all flags + the brief/prompt positional)
///   through unchanged, so a mock substitute receives exactly what the real vendor
///   CLI would. Per-vendor overrides are intentionally NOT supported — one redirect
///   is all the mock substrate needs (PRIN-2).
///
/// Interactive TTY launches are out of scope — they resolve their program
/// independently and are not routed through this resolver.
// trace:TASK-1081
pub(crate) fn resolve_agent_program(vendor_program: &str) -> String {
    match std::env::var("AIDA_AGENT_CMD") {
        Ok(cmd) if !cmd.trim().is_empty() => cmd,
        _ => vendor_program.to_string(),
    }
}

/// Compose the program + argv to actually exec for a headless `claude` launch,
/// applying the STORY-612 OS-boundary wrapper when `[contained] os_wrap` is on.
///
/// - os_wrap OFF (default) → `("claude", claude_args)` unchanged.
/// - os_wrap ON  → `("bwrap", [confinement-flags…, "claude", claude_args…])`,
///   binding the worktree + store + cargo/npm/claude caches rw, everything else
///   ro. Errors (fail-closed) if `bwrap` is not on PATH.
///
/// TASK-1081: both callers are HEADLESS (`exec_claude_headless`,
/// `spawn_claude_headless_resume`), so the wrapped program is routed through
/// `resolve_agent_program` — an `AIDA_AGENT_CMD` override swaps the vendor binary
/// for a mock while the argv is unchanged. Unset → the native `claude` as before.
///
/// `worktree_root` is the code worktree the drain runs in (its `.aida-store`
/// sibling is bound rw).
// trace:STORY-612 trace:TASK-1081 | ai:claude
fn claude_program_and_args(
    worktree_root: &Path,
    claude_args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    os_wrapped_program_and_args(worktree_root, &resolve_agent_program("claude"), claude_args)
}

/// Generalized form of `claude_program_and_args` that wraps an ARBITRARY program
/// (the headless paths pass the bare `"claude"`; the interactive `aida agent new`
/// path passes the PATH-resolved claude binary). Same fail-closed contract:
///
/// - os_wrap OFF (default, incl. no `AIDA_OS_WRAP`) → `(program, args)` unchanged,
///   so a normal launch is byte-identical to today's behavior.
/// - os_wrap ON  → `("bwrap", [confinement-flags…, program, args…])`. Errors
///   (fail-closed) if `bwrap` is not on PATH or the userns preflight fails —
///   never launches unconfined when the OS boundary was requested.
///
/// trace:TASK-864 | ai:claude
pub(crate) fn os_wrapped_program_and_args(
    worktree_root: &Path,
    program: &str,
    program_args: Vec<String>,
) -> Result<(String, Vec<String>)> {
    if !os_wrap_enabled(worktree_root) {
        return Ok((program.to_string(), program_args));
    }
    let claude_args = program_args;
    if which_on_path("bwrap").is_none() {
        anyhow::bail!(
            "[contained] os_wrap is enabled but `bwrap` (bubblewrap) was not found on PATH — \
             install bubblewrap or unset os_wrap. Refusing to launch the drain unconfined."
        );
    }
    // Fail-closed preflight: `bwrap` can be installed yet unable to set up a user
    // namespace (the Ubuntu 23.10+/24.04 AppArmor `apparmor_restrict_unprivileged_userns`
    // restriction — SPIKE-61 §7.7). Catch it here with an actionable message
    // instead of letting the drain die on a cryptic `uid map: Permission denied`.
    bwrap_preflight()?;
    let store = worktree_root.join(".aida-store");
    let mut rw_paths = vec![store];
    if let Some(home) = dirs::home_dir() {
        // Cargo/npm registry + build caches must stay writable or cargo/npm fail
        // mid-build; `~/.claude` + `~/.claude.json` hold Claude Code's session
        // state and auth, which it writes during a run.
        let cargo_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cargo"));
        rw_paths.push(cargo_home);
        rw_paths.push(home.join(".npm"));
        rw_paths.push(home.join(".claude"));
        rw_paths.push(home.join(".claude.json"));
    }
    // STORY-617: opt-in strict read-confinement. Default-ABSENT => empty =>
    // `bwrap_confinement_args_inner` falls back to the unchanged `--ro-bind / /`.
    let read_allowlist = contained_read_allowlist(worktree_root);
    let mut full = bwrap_confinement_args_inner(worktree_root, &rw_paths, &read_allowlist);
    // TASK-809: when `[contained] managed_domains_only` is on, deliver the hard
    // default-deny egress via the MANAGED-settings tier — Claude Code only
    // blocks-without-prompt when the flag arrives there, not via the project
    // `--settings` STORY-615 emits. We write the managed doc under `.aida/`
    // (gitignored runtime state) and bind it READ-ONLY over the wrapped
    // process's `/etc/claude-code/managed-settings.json`, so the host's `/etc`
    // is never touched and the policy can never be overridden from inside the
    // sandbox. trace:TASK-809 | ai:claude
    if contained_managed_domains_only(worktree_root) {
        let allowed_hosts = contained_allowed_hosts(worktree_root);
        let json = managed_settings_json(&allowed_hosts);
        let aida_dir = worktree_root.join(".aida");
        std::fs::create_dir_all(&aida_dir).with_context(|| {
            format!(
                "failed to create {} for managed settings",
                aida_dir.display()
            )
        })?;
        let managed_path = aida_dir.join("contained-managed-settings.json");
        std::fs::write(&managed_path, &json).with_context(|| {
            format!(
                "failed to write contained managed settings {}",
                managed_path.display()
            )
        })?;
        // `--ro-bind` (hard, not -try): if managed_domains_only is requested we
        // MUST get the policy in front of claude — fail-closed if the bind can't
        // be set up rather than silently launching with egress un-hard-blocked.
        full.push("--ro-bind".to_string());
        full.push(managed_path.to_string_lossy().into_owned());
        full.push("/etc/claude-code/managed-settings.json".to_string());
    }
    full.push(program.to_string());
    full.extend(claude_args);
    Ok(("bwrap".to_string(), full))
}

/// Run a trivial `bwrap … true` to confirm bubblewrap can actually create the
/// user namespace + uid map on this host BEFORE wrapping the real drain. On
/// failure (typically the AppArmor unprivileged-userns restriction) bail with
/// the remediation rather than letting the drain hit a cryptic bwrap error —
/// and never fall through to an unconfined launch. trace:STORY-612 | ai:claude
fn bwrap_preflight() -> Result<()> {
    use std::process::{Command, Stdio};
    let ran = Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "true",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match ran {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!(
            "[contained] os_wrap is enabled and `bwrap` is installed, but a sandbox self-test \
             failed — the kernel is refusing the unprivileged user namespace bwrap needs. On \
             Ubuntu 23.10+/24.04 this is AppArmor's unprivileged-userns restriction. Remediate \
             with ONE of:\n  \
             - sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   (host-wide)\n  \
             - install an AppArmor profile granting bwrap userns (see /etc/apparmor.d)\n\
             Refusing to launch the drain unconfined — unset [contained] os_wrap to opt out."
        ),
    }
}

/// Availability of the bubblewrap (`bwrap`) OS sandbox on this host, as
/// reported by `aida doctor` / `aida init`. A read-only probe — it does NOT
/// enable `os_wrap` (the `[contained] os_wrap` config knob, exposed separately
/// by TASK-866); it only tells the user whether userns confinement *could* be
/// used. trace:TASK-865 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BwrapAvailability {
    /// `bwrap` is on PATH and the userns self-test passes.
    Ok,
    /// `bwrap` is not installed / not on PATH.
    NotInstalled,
    /// `bwrap` is installed but the kernel refuses the unprivileged user
    /// namespace it needs — carries the remediation hint from `bwrap_preflight`.
    UsernsBlocked { hint: String },
}

/// Probe whether the bubblewrap OS sandbox is available on this host: is
/// `bwrap` on PATH, and does the trivial userns self-test pass? Reuses
/// `which_on_path` + `bwrap_preflight` so the doctor/init report matches what
/// the launch path actually checks. Read-only — never enables confinement.
/// trace:TASK-865 | ai:claude
pub(crate) fn bwrap_availability() -> BwrapAvailability {
    if which_on_path("bwrap").is_none() {
        return BwrapAvailability::NotInstalled;
    }
    match bwrap_preflight() {
        Ok(()) => BwrapAvailability::Ok,
        Err(e) => BwrapAvailability::UsernsBlocked {
            hint: bwrap_userns_remediation_hint(&e.to_string()),
        },
    }
}

/// Distil `bwrap_preflight`'s long fail-closed message down to the one-line
/// remediation a doctor/init status row wants. Falls back to the full text if
/// the expected `Remediate with ONE of:` marker is absent. trace:TASK-865 | ai:claude
fn bwrap_userns_remediation_hint(full: &str) -> String {
    // The preflight message lists remediations after a "Remediate with ONE of:"
    // marker; surface the first concrete `sysctl` line as the short hint.
    if let Some(rest) = full.split("Remediate with ONE of:").nth(1) {
        for line in rest.lines() {
            // The source lines are `  - sudo sysctl … (host-wide)\n  ` — strip
            // the leading bullet, the trailing soft-wrap whitespace, and any
            // parenthetical aside so the one-liner is a clean copy-paste hint.
            let trimmed = line.trim().trim_start_matches('-').trim();
            let trimmed = trimmed.split("   ").next().unwrap_or(trimmed).trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "the kernel is refusing the unprivileged user namespace bwrap needs (on \
     Ubuntu 23.10+/24.04, AppArmor's apparmor_restrict_unprivileged_userns); \
     run `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` or \
     install an AppArmor profile granting bwrap userns"
        .to_string()
}

/// The exact command that lifts the kernel's unprivileged-userns restriction
/// for the CURRENT boot (does not survive a reboot). Single source of truth so
/// the doctor remediation, the guided setup printer, and the docs can't drift.
/// trace:STORY-665 | ai:claude
pub(crate) const BWRAP_USERNS_SYSCTL_RUNTIME: &str =
    "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0";

/// The command that PERSISTS the userns sysctl across reboots via a
/// `/etc/sysctl.d` drop-in. trace:STORY-665 | ai:claude
pub(crate) const BWRAP_USERNS_SYSCTL_PERSIST: &str =
    "echo 'kernel.apparmor_restrict_unprivileged_userns=0' \
     | sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf";

/// The Debian/Ubuntu install command for bubblewrap. trace:STORY-665 | ai:claude
pub(crate) const BWRAP_INSTALL_DEBIAN: &str = "sudo apt install bubblewrap";

/// Minimal PATH lookup for an executable name (avoids pulling in a `which`
/// crate). Returns the first matching path, or `None`. trace:STORY-612 | ai:claude
fn which_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
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
    contained: bool,
) -> Result<()> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // STORY-612: OS-boundary wrapper, same as spawn_claude_headless.
    let worktree = headless_worktree_root();
    let (program, args) = claude_program_and_args(
        &worktree,
        claude_headless_args_with_posture(prompt, session_id, contained),
    )?;
    let status = Command::new(program)
        .args(args)
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
    claude_headless_resume_args_with_posture(prompt, session_id, false)
}

pub fn claude_headless_resume_args_with_posture(
    prompt: &str,
    session_id: &str,
    contained: bool,
) -> Vec<String> {
    let permission_mode = if contained {
        "dontAsk"
    } else {
        "bypassPermissions"
    };
    let mut args = vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        permission_mode.to_string(),
    ];
    if contained {
        args.extend(claude_contained_flags());
    }
    args.extend([
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
        prompt.to_string(),
    ]);
    args
}

/// TASK-894: build the `(program, args)` for one launch of the headless
/// **advisor tier** (`/aida-advise`) the orchestrator spawns to resolve an
/// implementer's punt under `--auto-complete --no-human=both`. Vendor-neutral
/// generalization of the STORY-306 spawn, mirroring STORY-683's drain-phase
/// generalization.
///
/// - `is_fork` is the orchestrator's fork-from-live decision for the Claude
///   path: when `true` the Claude arm resumes the forked live-advisor session
///   (`claude … --resume <advisor_uuid> /aida-advise`, the context-rich path);
///   when `false` it cold-boots with the substrate-seeded prompt.
/// - `seeded_prompt` is the cold-boot prompt (the live-advisor-context prepend
///   the assess cold-boot uses); `advisor_uuid` is the resume/session id.
///
/// **Codex has no `--resume` / session model** (see [`codex_headless_args`]), so
/// a Codex advisor tier ignores `is_fork` and always hosts a *fresh* `codex exec`
/// per punt against the seeded prompt — no resume, the per-punt-fresh-spawn
/// trade-off noted in STORY-683's follow-ups. The caller is responsible for
/// forcing the pass to cold-boot for a non-Claude vendor so the fork-from-live
/// JSONL machinery (Claude-specific) is never exercised.
///
/// Claude's `is_fork`/cold-boot arms are byte-identical to the pre-existing
/// inline construction, so an un-configured drain (vendor = Claude) is unchanged.
/// Pure — both arms are unit-tested without spawning.
// trace:TASK-894 | ai:claude
pub fn advisor_tier_program_and_args(
    vendor: HeadlessVendor,
    is_fork: bool,
    seeded_prompt: &str,
    advisor_uuid: &str,
) -> (String, Vec<String>) {
    match vendor {
        HeadlessVendor::Claude => {
            let args = if is_fork {
                // Fork branch inherits the live advisor's context via --resume.
                claude_headless_resume_args("/aida-advise", advisor_uuid)
            } else {
                claude_headless_args(seeded_prompt, advisor_uuid)
            };
            // TASK-1081: route the vendor binary through the mock resolver — an
            // `AIDA_AGENT_CMD` override swaps the program, argv unchanged; unset
            // yields the native binary. trace:TASK-1081
            (
                resolve_agent_program(HeadlessVendor::Claude.program()),
                args,
            )
        }
        HeadlessVendor::Codex => {
            // No resume model — a fresh spawn per punt against the seeded prompt.
            // `is_fork` is intentionally ignored here. trace:TASK-894 | ai:claude
            // TASK-1081: same mock resolver as the Claude arm. trace:TASK-1081
            (
                resolve_agent_program(HeadlessVendor::Codex.program()),
                codex_headless_args(seeded_prompt),
            )
        }
    }
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
    contained: bool,
) -> Result<std::process::ExitStatus> {
    use std::process::{Command, Stdio};
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // STORY-612: OS-boundary wrapper. The resume path runs in `cwd` (the
    // original implementer's worktree), so that is the rw bind scope.
    // trace:STORY-612 | ai:claude
    let (program, args) = claude_program_and_args(
        cwd,
        claude_headless_resume_args_with_posture(prompt, session_id, contained),
    )?;
    let status = Command::new(program)
        .current_dir(cwd)
        .args(args)
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
    let encoded = s.replace(['\\', '/'], "-");
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
        // The "recent" marker routes through the registry, so compare against
        // its rendered form rather than a hard-coded literal. trace:TASK-840
        let recent = glyph(crate::glyphs::Glyph::InFlight);
        let live_colored = if live == "●" {
            live.green().bold().to_string()
        } else if live == recent {
            live.yellow().to_string()
        } else {
            live.dimmed().to_string()
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
        .with_help_message("arrows to move, type to filter, Enter to resume, Esc to cancel")
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

/// Replace this process with `claude --resume <id>`. Falls back to spawn
/// + wait on platforms without exec semantics. `permission_mode`, when
///   given, is passed through so a resumed `aida queue work` session keeps
///   the same permission posture as a fresh one. trace:TASK-112 | ai:claude
pub fn exec_claude_resume(id: &str, permission_mode: Option<&str>, contained: bool) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("claude");
    cmd.args(["--resume", id]);
    if let Some(m) = permission_mode {
        cmd.args(["--permission-mode", m]);
    }
    if contained {
        cmd.args(claude_contained_flags());
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

    // BUG-581: every test that reads or mutates os_wrap touches the
    // process-global `AIDA_OS_WRAP` env var (via `os_wrap_env_override`). cargo
    // runs tests in parallel threads sharing ONE process environment, so a
    // mutator test mid-flight (with AIDA_OS_WRAP set) leaks into reader tests
    // that assume a clean baseline and they fail intermittently in CI. This
    // BUG-697: the `OsWrapEnvGuard` RAII helper below acquires the ONE shared
    // process-global env lock (crate::test_env::env_lock) so os_wrap swaps
    // can't race a read/swap under any other test helper. trace:BUG-581

    /// RAII guard for the os_wrap env tests (BUG-581). On construction it locks
    /// the shared `OS_WRAP_ENV_LOCK` (recovering from a poisoned lock so one
    /// panicking test can't cascade), saves the ambient `AIDA_OS_WRAP`, and
    /// REMOVES it so the test starts from a clean baseline. On drop it restores
    /// the saved value, so the rest of the suite is unaffected. Every os_wrap
    /// test acquires this at its top; mutators still set/remove the var within
    /// their body — those changes are also undone by the drop restore.
    /// trace:BUG-581 | ai:claude
    struct OsWrapEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Option<std::ffi::OsString>,
    }

    impl OsWrapEnvGuard {
        fn acquire() -> Self {
            let lock = crate::test_env::env_lock(); // BUG-697: shared env lock
            let saved = std::env::var_os("AIDA_OS_WRAP");
            std::env::remove_var("AIDA_OS_WRAP");
            Self { _lock: lock, saved }
        }
    }

    impl Drop for OsWrapEnvGuard {
        fn drop(&mut self) {
            match &self.saved {
                Some(v) => std::env::set_var("AIDA_OS_WRAP", v),
                None => std::env::remove_var("AIDA_OS_WRAP"),
            }
        }
    }

    /// RAII guard for the `AIDA_AGENT_CMD` resolver tests. It shares the
    /// `OS_WRAP_ENV_LOCK` so it is mutually exclusive with the os_wrap
    /// program-resolution tests — those call `claude_program_and_args`, which now
    /// reads `AIDA_AGENT_CMD`, so a concurrently-set override must never leak into
    /// their clean-baseline `program == "claude"` assertions. Saves + clears both
    /// `AIDA_AGENT_CMD` and `AIDA_OS_WRAP` on construct (a clean, os_wrap-off
    /// baseline), restores both on drop.
    // trace:TASK-1081 | ai:claude
    struct AgentCmdEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved_cmd: Option<std::ffi::OsString>,
        saved_wrap: Option<std::ffi::OsString>,
    }

    impl AgentCmdEnvGuard {
        fn acquire() -> Self {
            let lock = crate::test_env::env_lock(); // BUG-697: shared env lock
            let saved_cmd = std::env::var_os("AIDA_AGENT_CMD");
            let saved_wrap = std::env::var_os("AIDA_OS_WRAP");
            std::env::remove_var("AIDA_AGENT_CMD");
            std::env::remove_var("AIDA_OS_WRAP");
            Self {
                _lock: lock,
                saved_cmd,
                saved_wrap,
            }
        }
    }

    impl Drop for AgentCmdEnvGuard {
        fn drop(&mut self) {
            match &self.saved_cmd {
                Some(v) => std::env::set_var("AIDA_AGENT_CMD", v),
                None => std::env::remove_var("AIDA_AGENT_CMD"),
            }
            match &self.saved_wrap {
                Some(v) => std::env::set_var("AIDA_OS_WRAP", v),
                None => std::env::remove_var("AIDA_OS_WRAP"),
            }
        }
    }

    /// With `AIDA_AGENT_CMD` unset the resolver yields the native vendor binary
    /// (faithful-launcher invariant); set at a trivial fake exe it yields the
    /// fake, and the spawn site passes the ORIGINAL argv through unchanged. No
    /// real claude/codex is spawned — the resolver never launches the program,
    /// it only names it.
    // trace:TASK-1081 | ai:claude
    #[test]
    fn agent_cmd_override_swaps_program_keeps_argv() {
        let _env = AgentCmdEnvGuard::acquire();

        // Unset → the native vendor binaries, byte-identical to today.
        assert_eq!(resolve_agent_program("claude"), "claude");
        assert_eq!(resolve_agent_program("codex"), "codex");

        // Set at a trivial fake exe → that program replaces every vendor binary.
        let fake = "/tmp/aida-fake-agent-task-1081";
        std::env::set_var("AIDA_AGENT_CMD", fake);
        assert_eq!(resolve_agent_program("claude"), fake);
        assert_eq!(resolve_agent_program("codex"), fake);

        // The headless spawn site swaps only the PROGRAM; the argv (all flags +
        // the trailing prompt positional) is passed through unchanged. os_wrap is
        // off (guard cleared AIDA_OS_WRAP; the temp root has no config).
        let tmp = tempfile::tempdir().unwrap();
        let claude_args = claude_headless_args("/aida-review", "sid");
        let (program, args) = claude_program_and_args(tmp.path(), claude_args.clone())
            .expect("unwrapped launch never errors");
        assert_eq!(program, fake, "override swaps the vendor program");
        assert_eq!(args, claude_args, "argv is passed through unchanged");

        // Empty / whitespace override is treated as unset — fall back to native.
        std::env::set_var("AIDA_AGENT_CMD", "   ");
        assert_eq!(resolve_agent_program("claude"), "claude");
        std::env::remove_var("AIDA_AGENT_CMD");
        assert_eq!(resolve_agent_program("claude"), "claude");
    }

    // TASK-865: the doctor/init bwrap row distils bwrap_preflight's long
    // fail-closed message down to a single clean remediation line — the first
    // concrete `sysctl` step, with the parenthetical aside and soft-wrap
    // whitespace stripped. trace:TASK-865 | ai:claude
    #[test]
    fn bwrap_remediation_hint_extracts_first_concrete_step() {
        let full = "[contained] os_wrap is enabled and `bwrap` is installed, but a sandbox \
                    self-test failed. Remediate with ONE of:\n  \
                    - sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0   (host-wide)\n  \
                    - install an AppArmor profile granting bwrap userns";
        let hint = bwrap_userns_remediation_hint(full);
        assert_eq!(
            hint,
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
        );
    }

    // STORY-665: the shared remediation constants are the single source of
    // truth for the doctor remediation, the `--fix-sandbox` printer, and the
    // docs. Pin their exact text so a copy-paste from any surface is correct
    // and they can't silently drift apart. trace:STORY-665 | ai:claude
    #[test]
    fn bwrap_remediation_constants_are_exact_and_copy_pasteable() {
        assert_eq!(
            BWRAP_USERNS_SYSCTL_RUNTIME,
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0"
        );
        assert_eq!(
            BWRAP_USERNS_SYSCTL_PERSIST,
            "echo 'kernel.apparmor_restrict_unprivileged_userns=0' \
             | sudo tee /etc/sysctl.d/99-aida-bwrap-userns.conf"
        );
        assert_eq!(BWRAP_INSTALL_DEBIAN, "sudo apt install bubblewrap");
        // The persist drop-in must land under /etc/sysctl.d so it survives a
        // reboot, and the runtime form must be the non-persisting `sysctl -w`.
        assert!(BWRAP_USERNS_SYSCTL_PERSIST.contains("/etc/sysctl.d/"));
        assert!(BWRAP_USERNS_SYSCTL_RUNTIME.contains("sysctl -w"));
    }

    // TASK-865: when the expected marker is absent, fall back to a complete
    // self-contained hint rather than returning an empty string.
    #[test]
    fn bwrap_remediation_hint_falls_back_without_marker() {
        let hint = bwrap_userns_remediation_hint("some unexpected error text");
        assert!(hint.contains("apparmor_restrict_unprivileged_userns"));
        assert!(!hint.is_empty());
    }

    // STORY-605: an EMPTY egress allowlist must leave the contained settings
    // unchanged — the network-egress restriction is strictly opt-in. The
    // parsed sandbox object must carry exactly the four pre-STORY-605 keys and
    // NO `network` key. (If this fails, the default contained posture changed.)
    // trace:STORY-605 | ai:claude
    #[test]
    fn empty_egress_allowlist_omits_network_key_unchanged() {
        let json = contained_settings_json(&[], false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let sandbox = v.get("sandbox").and_then(|s| s.as_object()).unwrap();
        assert!(
            sandbox.get("network").is_none(),
            "empty allowlist must omit the network key (unchanged posture): {json}"
        );
        assert_eq!(sandbox.get("enabled"), Some(&serde_json::json!(true)));
        assert_eq!(
            sandbox.get("allowUnsandboxedCommands"),
            Some(&serde_json::json!(false))
        );
        // The unchanged set is exactly these four keys.
        let mut keys: Vec<&String> = sandbox.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "allowUnsandboxedCommands",
                "autoAllowBashIfSandboxed",
                "enabled",
                "failIfUnavailable",
            ]
        );
    }

    // STORY-615: managed_domains_only=true adds
    // `sandbox.network.allowManagedDomainsOnly`; composes with the allowlist;
    // default (false) + empty allowlist still omits the network key entirely.
    // trace:STORY-615 | ai:claude
    #[test]
    fn managed_domains_only_adds_allow_managed_flag() {
        // managed-only with NO allowlist → network carries only the flag.
        let json = contained_settings_json(&[], true);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        assert!(
            v.pointer("/sandbox/network/allowedDomains").is_none(),
            "no allowlist → no allowedDomains key: {json}"
        );
        // composes with an allowlist (both keys present).
        let json = contained_settings_json(&["github.com".to_string()], true);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        assert!(v.pointer("/sandbox/network/allowedDomains").is_some());
        // default off + empty allowlist → network omitted (unchanged posture).
        let json = contained_settings_json(&[], false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v.pointer("/sandbox/network").is_none());
    }

    // TASK-809: the MANAGED-settings doc carries the hard default-deny flag
    // `sandbox.network.allowManagedDomainsOnly = true`, and mirrors the operator
    // `allowed_hosts` into `sandbox.network.allowedDomains` (managed-only honors
    // only the managed allowedDomains). With an empty allowlist the
    // allowedDomains key is omitted but the hard-block flag is still present.
    // trace:TASK-809 | ai:claude
    #[test]
    fn managed_settings_json_hard_blocks_and_mirrors_hosts() {
        // empty allowlist → flag present, no allowedDomains key.
        let json = managed_settings_json(&[]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true)),
            "managed doc must hard-block (deny without prompt): {json}"
        );
        assert!(
            v.pointer("/sandbox/network/allowedDomains").is_none(),
            "no allowlist → no allowedDomains key: {json}"
        );
        // non-empty allowlist → mirrored into the MANAGED allowedDomains.
        let hosts = vec!["github.com".to_string(), "*.crates.io".to_string()];
        let json = managed_settings_json(&hosts);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v.pointer("/sandbox/network/allowManagedDomainsOnly"),
            Some(&serde_json::json!(true))
        );
        let got: Vec<&str> = v
            .pointer("/sandbox/network/allowedDomains")
            .and_then(|d| d.as_array())
            .expect("managed allowedDomains present")
            .iter()
            .filter_map(|d| d.as_str())
            .collect();
        assert_eq!(got, vec!["github.com", "*.crates.io"]);
    }

    // TASK-809: the os_wrap launch binds the generated managed-settings doc over
    // `/etc/claude-code/managed-settings.json` (hard `--ro-bind`) ONLY when
    // `[contained] managed_domains_only = true`; absent the flag the launch has
    // no such bind (unchanged STORY-612 posture). trace:TASK-809 | ai:claude
    #[test]
    fn os_wrap_binds_managed_settings_only_when_opted_in() {
        // Clean, serialized os_wrap env baseline (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // Helper: build a worktree with the given config, return the bwrap argv
        // (or None when the host can't create a userns / bwrap is absent — then
        // the launch fails closed and we can't inspect the argv, so we skip).
        fn wrapped_args(cfg: &str) -> Option<Vec<String>> {
            let tmp = tempfile::tempdir().unwrap();
            let wt = tmp.path();
            std::fs::create_dir_all(wt.join(".aida")).unwrap();
            std::fs::write(wt.join(".aida/config.toml"), cfg).unwrap();
            if which_on_path("bwrap").is_none() || bwrap_preflight().is_err() {
                return None;
            }
            let dummy = claude_headless_args("/aida-review", "sid");
            let (program, args) =
                claude_program_and_args(wt, dummy).expect("wrapped launch builds");
            assert_eq!(program, "bwrap");
            Some(args)
        }

        const MANAGED_DEST: &str = "/etc/claude-code/managed-settings.json";

        // managed_domains_only ON → a hard --ro-bind onto the managed dest.
        if let Some(args) =
            wrapped_args("[contained]\nos_wrap = true\nmanaged_domains_only = true\n")
        {
            let bound = args
                .windows(3)
                .any(|w| w[0] == "--ro-bind" && w[2] == MANAGED_DEST);
            assert!(
                bound,
                "managed_domains_only must --ro-bind the managed settings doc: {args:?}"
            );
        }

        // os_wrap on but managed_domains_only OFF → NO managed-settings bind.
        if let Some(args) = wrapped_args("[contained]\nos_wrap = true\n") {
            assert!(
                !args.iter().any(|a| a == MANAGED_DEST),
                "no managed bind without managed_domains_only (unchanged posture): {args:?}"
            );
        }
    }

    // STORY-617: with an EMPTY read_allowlist the confinement args are
    // byte-for-byte the pre-STORY-617 base — whole-fs `--ro-bind / /`, no
    // `--ro-bind-try` for system paths. (If this fails the default read posture
    // changed.) trace:STORY-617 | ai:claude
    #[test]
    fn empty_read_allowlist_keeps_ro_root_unchanged() {
        let wt = Path::new("/home/joe/ai/aida-story-617");
        let store = wt.join(".aida-store");
        let with_helper = bwrap_confinement_args(wt, std::slice::from_ref(&store));
        let with_inner = bwrap_confinement_args_inner(wt, std::slice::from_ref(&store), &[]);
        assert_eq!(
            with_helper, with_inner,
            "empty allowlist must equal the no-allowlist helper output"
        );
        // ro-root present, first.
        let ro = with_inner
            .windows(3)
            .position(|w| w == ["--ro-bind", "/", "/"])
            .expect("must --ro-bind / / when no read_allowlist");
        assert_eq!(ro, 0, "ro-root must come first: {with_inner:?}");
        // no enumerated essential-path binds in the unchanged posture.
        assert!(
            !with_inner.iter().any(|a| a == "--ro-bind-try"),
            "default posture must not enumerate system paths: {with_inner:?}"
        );
    }

    // STORY-617: a NON-EMPTY read_allowlist replaces `--ro-bind / /` with an
    // enumerated set — essential system paths + the allowlist (all ro via
    // --ro-bind-try) — while still rw-binding the worktree and NEVER unsharing
    // net. Host secrets outside the allowlist are simply absent. trace:STORY-617
    #[test]
    fn nonempty_read_allowlist_enumerates_and_drops_ro_root() {
        let wt = Path::new("/home/joe/ai/aida-story-617");
        let store = wt.join(".aida-store");
        let allow = vec![
            "/home/joe/.config/special".to_string(),
            "/data/shared".to_string(),
        ];
        let flags = bwrap_confinement_args_inner(wt, std::slice::from_ref(&store), &allow);

        // (a) the broad ro-root is GONE — strict default-absent filesystem.
        assert!(
            !flags.windows(3).any(|w| w == ["--ro-bind", "/", "/"]),
            "strict mode must NOT --ro-bind / /: {flags:?}"
        );

        // (b) each allowlist entry is bound ro via --ro-bind-try (skips if absent).
        for p in &allow {
            assert!(
                flags
                    .windows(3)
                    .any(|w| w[0] == "--ro-bind-try" && w[1] == *p && w[2] == *p),
                "allowlist path {p} must be ro-bind-try'd: {flags:?}"
            );
        }
        // (c) the essential toolchain paths are present (so claude/cargo run).
        for must in ["/usr", "/etc", "/lib"] {
            assert!(
                flags
                    .windows(3)
                    .any(|w| w[0] == "--ro-bind-try" && w[1] == must && w[2] == must),
                "essential path {must} must be bound: {flags:?}"
            );
        }
        // (d) a host-secret path NOT in the allowlist is absent entirely.
        assert!(
            !flags.iter().any(|a| a.contains(".ssh")),
            "non-allowlisted ~/.ssh must not appear: {flags:?}"
        );

        // worktree still rw-bound; net still shared.
        let wt_s = wt.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind" && w[1] == wt_s && w[2] == wt_s),
            "worktree must stay rw --bind in strict mode: {flags:?}"
        );
        assert!(
            !flags.iter().any(|a| a == "--unshare-net"),
            "strict read-confinement must NOT unshare net: {flags:?}"
        );
    }

    // STORY-617: the config reader is opt-in — absent key => empty => unchanged.
    // trace:STORY-617 | ai:claude
    #[test]
    fn read_allowlist_config_is_opt_in() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        // absent config => empty.
        assert!(contained_read_allowlist(wt).is_empty());
        // present array => parsed.
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        std::fs::write(
            wt.join(".aida/config.toml"),
            "[contained]\nread_allowlist = [\"/data/a\", \"/data/b\"]\n",
        )
        .unwrap();
        assert_eq!(
            contained_read_allowlist(wt),
            vec!["/data/a".to_string(), "/data/b".to_string()]
        );
    }

    // STORY-605: a non-empty allowlist adds `sandbox.network.allowedDomains`
    // (Claude Code's verified egress schema) with the given hosts, leaving the
    // base sandbox keys intact. trace:STORY-605 | ai:claude
    #[test]
    fn nonempty_egress_allowlist_adds_allowed_domains() {
        let hosts = vec!["github.com".to_string(), "*.crates.io".to_string()];
        let json = contained_settings_json(&hosts, false);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let domains = v
            .pointer("/sandbox/network/allowedDomains")
            .and_then(|d| d.as_array())
            .expect("network.allowedDomains present");
        let got: Vec<&str> = domains.iter().filter_map(|d| d.as_str()).collect();
        assert_eq!(got, vec!["github.com", "*.crates.io"]);
        // Base sandbox keys still intact.
        assert_eq!(
            v.pointer("/sandbox/enabled"),
            Some(&serde_json::json!(true))
        );
    }

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

    /// STORY-59: liveness indicator buckets — `●` live (<5min), a recent
    /// marker (<1h), space for idle. The widths are visual; the test
    /// guards the bucket boundaries. The recent marker routes through the
    /// glyph registry (TASK-840). trace:STORY-59 | ai:claude
    #[test]
    fn liveness_indicator_buckets() {
        let recent = crate::glyphs::Glyph::InFlight.render(crate::glyphs::active_profile(None));
        assert_eq!(liveness_indicator(0), "●");
        assert_eq!(liveness_indicator(4 * 60 + 59), "●");
        assert_eq!(liveness_indicator(5 * 60), recent);
        assert_eq!(liveness_indicator(59 * 60), recent);
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
        let args = claude_session_args(None, None, Some("/aida-pickup"), Some("sid"), false);
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
        let args = claude_session_args(Some("bypassPermissions"), None, None, None, false);
        let pos = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("--permission-mode present");
        assert_eq!(
            args.get(pos + 1).map(String::as_str),
            Some("bypassPermissions")
        );
    }

    // TASK-895: the interactive Codex argv is just the prompt positional —
    // Codex has no caller-minted `--session-id` / TUI-addressable `--resume`,
    // and the faithful-launcher default forces no approval/sandbox bypass.
    // trace:TASK-895 | ai:claude
    #[test]
    fn codex_session_args_is_just_the_prompt_positional() {
        let args = codex_session_args("/aida-pickup");
        assert_eq!(args, vec!["/aida-pickup".to_string()]);
        assert!(!args.iter().any(|a| a == "--session-id"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
        assert!(
            !args
                .iter()
                .any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
            "interactive codex must not force the bypass: {args:?}"
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
        let interactive = claude_session_args(None, None, Some("/aida-review"), Some("sid"), false);
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

    #[test]
    fn contained_headless_args_use_strict_sandbox_settings() {
        let args = claude_headless_args_with_posture("/aida-review", "sid", true);
        let pos = args
            .iter()
            .position(|a| a == "--permission-mode")
            .expect("contained headless must set a permission mode");
        assert_eq!(args.get(pos + 1).map(String::as_str), Some("dontAsk"));
        assert!(args.contains(&"--setting-sources".to_string()));
        assert!(args.contains(&"project".to_string()));
        let settings_pos = args
            .iter()
            .position(|a| a == "--settings")
            .expect("contained launch must pass inline settings");
        let settings: serde_json::Value =
            serde_json::from_str(args.get(settings_pos + 1).unwrap()).unwrap();
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["sandbox"]["failIfUnavailable"], true);
        assert_eq!(settings["sandbox"]["allowUnsandboxedCommands"], false);
        assert!(settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String("Edit(/**)".to_string())));
        assert!(settings["permissions"]["deny"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(
                "Bash(git reset --hard *)".to_string()
            )));
    }

    // STORY-683 / BUG-697: serialize the tests that mutate the process-global
    // `AIDA_HEADLESS_VENDOR` env var on the ONE shared env lock
    // (crate::test_env::env_lock) — same parallel-env hazard as os_wrap.

    struct HeadlessVendorEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Option<std::ffi::OsString>,
    }

    impl HeadlessVendorEnvGuard {
        fn acquire() -> Self {
            let lock = crate::test_env::env_lock(); // BUG-697: shared env lock
            let saved = std::env::var_os("AIDA_HEADLESS_VENDOR");
            std::env::remove_var("AIDA_HEADLESS_VENDOR");
            Self { _lock: lock, saved }
        }
    }

    impl Drop for HeadlessVendorEnvGuard {
        fn drop(&mut self) {
            match &self.saved {
                Some(v) => std::env::set_var("AIDA_HEADLESS_VENDOR", v),
                None => std::env::remove_var("AIDA_HEADLESS_VENDOR"),
            }
        }
    }

    /// STORY-683: the vendor selector builds the correct command for each vendor.
    /// Claude reuses the SPIKE-7 `claude -p` flag set; Codex builds `codex exec`.
    /// This is the core vendor-dispatch invariant. trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_args_builds_correct_command_per_vendor() {
        let prompt = "/aida-review --pr 7";
        let sid = "019e0000-0000-7000-8000-000000000000";

        // Claude arm: -p print mode + the prompt survives, NOT a codex command.
        let claude = headless_vendor_args(HeadlessVendor::Claude, prompt, sid, false);
        assert!(claude.contains(&"-p".to_string()), "claude -p: {claude:?}");
        assert!(
            claude.contains(&"bypassPermissions".to_string()),
            "claude bypass: {claude:?}"
        );
        assert!(claude.contains(&prompt.to_string()), "{claude:?}");
        assert!(
            !claude.contains(&"exec".to_string()),
            "claude arm must not be a codex exec: {claude:?}"
        );
        // Identical to the dedicated claude builder — the default path is unchanged.
        assert_eq!(
            claude,
            claude_headless_args_with_posture(prompt, sid, false)
        );

        // Codex arm: `codex exec --dangerously-bypass-approvals-and-sandbox <prompt>`,
        // with the prompt as the final positional and NO claude `-p`.
        let codex = headless_vendor_args(HeadlessVendor::Codex, prompt, sid, false);
        assert_eq!(codex.first().map(String::as_str), Some("exec"), "{codex:?}");
        assert!(
            codex.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()),
            "codex bypass: {codex:?}"
        );
        assert_eq!(codex.last().map(String::as_str), Some(prompt), "{codex:?}");
        assert!(
            !codex.contains(&"-p".to_string()),
            "codex arm must not carry claude's -p: {codex:?}"
        );
    }

    /// TASK-894: the advisor-tier spawn builds the correct command per vendor.
    /// Claude resumes the fork (`--resume <uuid> /aida-advise`) or cold-boots the
    /// seeded prompt; Codex always hosts a fresh `codex exec <seeded>` (no resume,
    /// `is_fork` ignored). The Claude arms are byte-identical to the dedicated
    /// builders so an un-configured drain is unchanged.
    // trace:TASK-894 | ai:claude
    #[test]
    fn advisor_tier_program_and_args_builds_correct_command_per_vendor() {
        let seeded = "advisor-context\n\n/aida-advise";
        let advisor_uuid = "019e0000-0000-7000-8000-000000000aaa";

        // Claude cold-boot: `claude` + the seeded prompt via the dedicated builder.
        let (prog, args) =
            advisor_tier_program_and_args(HeadlessVendor::Claude, false, seeded, advisor_uuid);
        assert_eq!(prog, "claude");
        assert_eq!(args, claude_headless_args(seeded, advisor_uuid));
        assert!(args.contains(&"-p".to_string()), "{args:?}");
        assert!(args.contains(&seeded.to_string()), "{args:?}");
        assert!(!args.contains(&"--resume".to_string()), "{args:?}");

        // Claude fork: `claude … --resume <uuid> /aida-advise`, NOT the seeded prompt.
        let (prog, args) =
            advisor_tier_program_and_args(HeadlessVendor::Claude, true, seeded, advisor_uuid);
        assert_eq!(prog, "claude");
        assert_eq!(
            args,
            claude_headless_resume_args("/aida-advise", advisor_uuid)
        );
        assert!(args.contains(&"--resume".to_string()), "{args:?}");
        assert!(args.contains(&advisor_uuid.to_string()), "{args:?}");
        assert!(args.contains(&"/aida-advise".to_string()), "{args:?}");

        // Codex: a fresh `codex exec <seeded>` per punt, ignoring `is_fork` —
        // codex has no resume model. trace:TASK-894
        for is_fork in [false, true] {
            let (prog, args) =
                advisor_tier_program_and_args(HeadlessVendor::Codex, is_fork, seeded, advisor_uuid);
            assert_eq!(prog, "codex", "is_fork={is_fork}");
            assert_eq!(args, codex_headless_args(seeded), "is_fork={is_fork}");
            assert_eq!(args.first().map(String::as_str), Some("exec"), "{args:?}");
            assert_eq!(args.last().map(String::as_str), Some(seeded), "{args:?}");
            // Never carries claude's resume / session flags.
            assert!(!args.contains(&"--resume".to_string()), "{args:?}");
            assert!(!args.contains(&advisor_uuid.to_string()), "{args:?}");
            assert!(!args.contains(&"-p".to_string()), "{args:?}");
        }
    }

    /// STORY-683: the program a vendor spawns is its own binary.
    /// trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_program_maps_to_binary() {
        assert_eq!(HeadlessVendor::Claude.program(), "claude");
        assert_eq!(HeadlessVendor::Codex.program(), "codex");
    }

    /// STORY-683: vendor-token parsing is case-insensitive, whitespace-tolerant,
    /// and rejects unknowns (so the caller falls through to the default).
    /// trace:STORY-683 | ai:claude
    #[test]
    fn headless_vendor_parse_is_lenient_and_rejects_unknown() {
        assert_eq!(
            HeadlessVendor::parse("claude"),
            Some(HeadlessVendor::Claude)
        );
        assert_eq!(
            HeadlessVendor::parse(" Codex "),
            Some(HeadlessVendor::Codex)
        );
        assert_eq!(HeadlessVendor::parse("CODEX"), Some(HeadlessVendor::Codex));
        assert_eq!(HeadlessVendor::parse("gemini"), None);
        assert_eq!(HeadlessVendor::parse(""), None);
    }

    /// STORY-683: with no env override and no config, the resolver defaults to
    /// Claude — an un-configured drain is byte-identical to the pre-STORY-683
    /// behavior. trace:STORY-683 | ai:claude
    #[test]
    fn resolve_headless_vendor_defaults_to_claude() {
        let _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);
    }

    /// STORY-683: `AIDA_HEADLESS_VENDOR=codex` selects Codex (env precedence);
    /// an unrecognized value is ignored and falls through to the Claude default.
    /// trace:STORY-683 | ai:claude
    #[test]
    fn resolve_headless_vendor_env_override() {
        let _env = HeadlessVendorEnvGuard::acquire();
        let tmp = tempfile::tempdir().unwrap();

        std::env::set_var("AIDA_HEADLESS_VENDOR", "codex");
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Codex);

        std::env::set_var("AIDA_HEADLESS_VENDOR", "claude");
        assert_eq!(resolve_headless_vendor(tmp.path()), HeadlessVendor::Claude);

        std::env::set_var("AIDA_HEADLESS_VENDOR", "nonsense");
        assert_eq!(
            resolve_headless_vendor(tmp.path()),
            HeadlessVendor::Claude,
            "unrecognized env value must fall through to the default"
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

    // STORY-612: the write-confinement bwrap flags must (a) make the root
    // read-only, (b) rw-bind the worktree, (c) chdir into it, and crucially
    // (d) NEVER `--unshare-net` — `claude` needs api.anthropic.com.
    // trace:STORY-612 | ai:claude
    #[test]
    fn bwrap_confinement_is_ro_root_rw_worktree_shared_net() {
        let wt = Path::new("/home/joe/ai/aida-story-612");
        let store = wt.join(".aida-store");
        let flags = bwrap_confinement_args(wt, std::slice::from_ref(&store));

        // (a) read-only root: the tokens "--ro-bind", "/", "/" appear in order.
        let ro = flags
            .windows(3)
            .position(|w| w == ["--ro-bind", "/", "/"])
            .expect("must --ro-bind / / (read-only root)");
        assert_eq!(
            ro, 0,
            "ro-root bind must come first so later binds override"
        );

        // (b) the worktree is hard-bound rw.
        let wt_s = wt.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind" && w[1] == wt_s && w[2] == wt_s),
            "worktree must be rw --bind: {flags:?}"
        );
        // store is rw via --bind-try (skipped if absent).
        let store_s = store.to_string_lossy().to_string();
        assert!(
            flags
                .windows(3)
                .any(|w| w[0] == "--bind-try" && w[1] == store_s && w[2] == store_s),
            "store must be rw --bind-try: {flags:?}"
        );

        // (c) chdir into the worktree.
        let chdir = flags.iter().position(|a| a == "--chdir").expect("--chdir");
        assert_eq!(
            flags.get(chdir + 1).map(String::as_str),
            Some(wt_s.as_str())
        );

        // (d) network is SHARED — no net unshare anywhere.
        assert!(
            !flags.iter().any(|a| a == "--unshare-net"),
            "must NOT unshare net (claude needs the API): {flags:?}"
        );
        // tears down with the parent.
        assert!(flags.iter().any(|a| a == "--die-with-parent"), "{flags:?}");
    }

    // STORY-612: when os_wrap is OFF (the default — no `[contained] os_wrap`
    // in this repo's config), the launch is the bare `claude` argv, byte-for-
    // byte unchanged. This is the "unchanged posture" invariant mirroring the
    // slice-1 empty-allowlist test. trace:STORY-612 | ai:claude
    #[test]
    fn os_wrap_off_leaves_launch_unwrapped() {
        // Clean, serialized os_wrap env baseline (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // A project root with no `[contained] os_wrap` (here: an empty temp dir
        // with no config at all) must return ("claude", <args unchanged>).
        let tmp = tempfile::tempdir().unwrap();
        assert!(!os_wrap_enabled(tmp.path()), "absent config => os_wrap off");
        let claude_args = claude_headless_args("/aida-review", "sid");
        let (program, args) = claude_program_and_args(tmp.path(), claude_args.clone())
            .expect("unwrapped launch never errors");
        assert_eq!(
            program, "claude",
            "default posture must exec claude directly"
        );
        assert_eq!(
            args, claude_args,
            "argv must be unchanged when os_wrap is off"
        );
    }

    /// STORY-612 — the automated live-verify gate. Adapts to the host:
    ///
    /// - On a host where bubblewrap CAN create an unprivileged user namespace
    ///   (e.g. GitHub `ubuntu-latest` runners), this spawns `bwrap` with the
    ///   EXACT confinement flags the launcher emits and proves the write-
    ///   confinement property live: a write INSIDE the worktree succeeds and a
    ///   write OUTSIDE (the read-only root) is blocked. If `claude` is on PATH
    ///   it also runs `claude --version` inside the wrapper (best-effort — the
    ///   binary is absent on CI runners). This is the live gate the PR's merge
    ///   is blocked on.
    /// - On a userns-RESTRICTED host (this dev box; or any non-Linux where
    ///   `bwrap` is absent) it instead asserts the FAIL-CLOSED contract: an
    ///   os_wrap-enabled launch returns an Err carrying the remediation, and
    ///   never silently falls through to an unconfined run.
    ///
    /// trace:STORY-612 | ai:claude
    #[test]
    fn bwrap_write_confinement_live_or_fail_closed() {
        // Clean, serialized os_wrap env baseline (BUG-581) — a leaked
        // AIDA_OS_WRAP=0 would otherwise force this on-config launch off.
        // trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();
        // A worktree with `[contained] os_wrap = true`.
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        std::fs::create_dir_all(wt.join(".aida")).unwrap();
        std::fs::write(
            wt.join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();
        assert!(os_wrap_enabled(wt), "config must enable os_wrap");

        let dummy = claude_headless_args("/aida-review", "sid");

        // Can bubblewrap actually set up a userns on this host?
        if bwrap_preflight().is_err() {
            // CI sets AIDA_REQUIRE_BWRAP_LIVE=1 to force the live arm so a green
            // check can never come from the fail-closed branch silently. If the
            // runner can't create a userns, that is a CI-environment failure, not
            // a pass. trace:STORY-612 | ai:claude
            assert!(
                std::env::var_os("AIDA_REQUIRE_BWRAP_LIVE").is_none(),
                "AIDA_REQUIRE_BWRAP_LIVE=1 but bwrap cannot create an unprivileged user \
                 namespace here — the live-confinement gate could not run. Install bubblewrap \
                 and/or `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`."
            );
            // Restricted host (no force): the launch MUST fail closed with remediation.
            let err = claude_program_and_args(wt, dummy)
                .expect_err("userns-restricted host must NOT yield a runnable wrapped command");
            let msg = format!("{err:#}");
            assert!(
                msg.contains("Refusing to launch the drain unconfined"),
                "fail-closed error must spell out it won't run unconfined: {msg}"
            );
            return;
        }

        // Live path: the launcher wraps with bwrap…
        let (program, _args) = claude_program_and_args(wt, dummy).expect("wrapped launch builds");
        assert_eq!(program, "bwrap", "userns-capable host must wrap with bwrap");

        // …and the EXACT flags it emits actually confine writes. Probe inside
        // the worktree (rw bind → succeeds) vs. a path on the read-only root
        // (blocked). Pass paths via env; bwrap forwards the environment.
        let inside = wt.join("probe_in");
        let outside = format!("/aida-oswrap-probe-out-{}", std::process::id());
        let flags = bwrap_confinement_args(wt, &[]);
        let probe = "touch \"$AIDA_PROBE_IN\" && \
             (touch \"$AIDA_PROBE_OUT\" 2>/dev/null && echo LEAK || echo BLOCKED)";
        let out = std::process::Command::new("bwrap")
            .args(&flags)
            .args(["sh", "-c", probe])
            .env("AIDA_PROBE_IN", &inside)
            .env("AIDA_PROBE_OUT", &outside)
            .output()
            .expect("spawn bwrap probe");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("BLOCKED") && !stdout.contains("LEAK"),
            "write to the read-only root must be blocked: stdout={stdout:?} stderr={:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            inside.exists(),
            "write inside the rw-bound worktree must persist to the host"
        );
        assert!(
            !Path::new(&outside).exists(),
            "the blocked outside write must not have leaked onto the host"
        );

        // Best-effort: if claude is installed, confirm it launches inside the
        // wrapper (auth dir bound, binary resolves). Skipped on CI runners.
        if which_on_path("claude").is_some() {
            let mut full = bwrap_confinement_args(wt, &[]);
            full.push("claude".into());
            full.push("--version".into());
            let st = std::process::Command::new("bwrap")
                .args(&full)
                .output()
                .expect("spawn claude --version under bwrap");
            assert!(
                st.status.success(),
                "claude --version must run inside the wrapper: {:?}",
                String::from_utf8_lossy(&st.stderr)
            );
        }
    }

    /// TASK-876: the `AIDA_OS_WRAP` per-host override takes PRECEDENCE over the
    /// tracked config value, in BOTH directions, and recognizes the documented
    /// truthy/falsey spellings (case-insensitive). An unrecognized value falls
    /// through to the config. These tests mutate a process-global env var, so
    /// they run serially under a shared mutex. trace:TASK-876 | ai:claude
    #[test]
    fn aida_os_wrap_env_overrides_config() {
        // Serialize env mutation across the os_wrap tests + start clean; the
        // guard's drop restores the ambient AIDA_OS_WRAP (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();

        // Config says OFF (no config at all).
        let off_dir = tempfile::tempdir().unwrap();
        // Config says ON.
        let on_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(on_dir.path().join(".aida")).unwrap();
        std::fs::write(
            on_dir.path().join(".aida/config.toml"),
            "[contained]\nos_wrap = true\n",
        )
        .unwrap();

        // Override ON forces true even when config is off.
        for truthy in ["1", "true", "TRUE", "Yes", " yes "] {
            std::env::set_var("AIDA_OS_WRAP", truthy);
            assert!(
                os_wrap_enabled(off_dir.path()),
                "AIDA_OS_WRAP={truthy:?} must force os_wrap ON over an off config"
            );
        }

        // Override OFF forces false even when config is on.
        for falsey in ["0", "false", "FALSE", "No", " no "] {
            std::env::set_var("AIDA_OS_WRAP", falsey);
            assert!(
                !os_wrap_enabled(on_dir.path()),
                "AIDA_OS_WRAP={falsey:?} must force os_wrap OFF over an on config"
            );
        }

        // Unrecognized value → ignored → config wins.
        std::env::set_var("AIDA_OS_WRAP", "maybe");
        assert!(
            !os_wrap_enabled(off_dir.path()),
            "garbage AIDA_OS_WRAP must fall through to the (off) config"
        );
        assert!(
            os_wrap_enabled(on_dir.path()),
            "garbage AIDA_OS_WRAP must fall through to the (on) config"
        );

        // Unset → config wins in both directions.
        std::env::remove_var("AIDA_OS_WRAP");
        assert!(!os_wrap_enabled(off_dir.path()));
        assert!(os_wrap_enabled(on_dir.path()));
    }

    /// TASK-864: the INTERACTIVE launch path (which calls
    /// `os_wrapped_program_and_args` with the PATH-resolved claude binary) is
    /// byte-identical to a bare exec when os_wrap is OFF, and produces a
    /// `bwrap … <binary> …` wrap (or a fail-closed Err on a userns-restricted
    /// host) when os_wrap is ON. trace:TASK-864 | ai:claude
    #[test]
    fn interactive_path_wraps_when_enabled_unchanged_when_off() {
        // Serialize + clean env baseline; the guard restores it on drop
        // (BUG-581). trace:BUG-581
        let _env = OsWrapEnvGuard::acquire();

        let resolved_binary = "/usr/local/bin/claude";
        let args = vec![
            "--dangerously-skip-permissions".to_string(),
            "--settings".to_string(),
            "/tmp/x.json".to_string(),
        ];

        // OFF (no config, no env) → bare program + args unchanged.
        let off_dir = tempfile::tempdir().unwrap();
        let (program, out_args) =
            os_wrapped_program_and_args(off_dir.path(), resolved_binary, args.clone())
                .expect("off path never errors");
        assert_eq!(
            program, resolved_binary,
            "os_wrap OFF must exec the resolved binary directly"
        );
        assert_eq!(out_args, args, "args unchanged when off");

        // ON via the env override.
        std::env::set_var("AIDA_OS_WRAP", "1");
        let on_dir = tempfile::tempdir().unwrap();
        match os_wrapped_program_and_args(on_dir.path(), resolved_binary, args.clone()) {
            Ok((program, out_args)) => {
                // userns-capable host: bwrap wrap with the resolved binary inside.
                assert_eq!(program, "bwrap", "enabled => wrap with bwrap");
                assert!(
                    out_args.iter().any(|a| a == resolved_binary),
                    "wrapped argv must invoke the resolved binary: {out_args:?}"
                );
                let bin_pos = out_args.iter().position(|a| a == resolved_binary).unwrap();
                assert_eq!(
                    &out_args[bin_pos + 1..],
                    &args[..],
                    "the claude args must follow the binary unchanged"
                );
                assert!(
                    out_args.iter().any(|a| a == "--die-with-parent"),
                    "confinement flags must be present: {out_args:?}"
                );
            }
            Err(e) => {
                // userns-restricted host: fail-closed, never a silent unconfined run.
                let msg = format!("{e:#}");
                assert!(
                    msg.contains("Refusing to launch the drain unconfined")
                        || msg.contains("os_wrap"),
                    "enabled-but-unrunnable must fail closed with remediation: {msg}"
                );
            }
        }
    }
}
