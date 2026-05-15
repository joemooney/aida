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
    exec_claude_resume(&target)
}

/// `aida session new` — capture role + title up-front, append a record
/// to `~/.aida/session-launches.log`, then exec `claude
/// --permission-mode <mode>`. Subsequent `aida session list` calls read
/// the launches log and join it with the .jsonl files (cwd + start-time
/// match) to surface the user-chosen title and authoritative role —
/// instead of falling back to the grep heuristic.
/// trace:FR-1-044 | ai:claude
pub fn new_session(
    title: Option<String>,
    permission_mode: &str,
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

    append_launch_log(&role, permission_mode, &title)?;

    let name_for_log = display_name.as_deref().unwrap_or("(auto)");
    eprintln!(
        "{} {} → claude --permission-mode {} (name: {})",
        "▶".green().bold(),
        format!("session new (role:{}, title:{:?})", role, title).dimmed(),
        permission_mode,
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
fn exec_claude_new(permission_mode: &str, name: Option<&str>) -> Result<()> {
    exec_claude(permission_mode, name, None)
}

/// STORY-42: replace this process with `claude --permission-mode <mode>
/// [--name <n>] [<initial_prompt>]`. The initial prompt becomes claude's
/// first message — pass `/aida-pickup` / `/aida-review --pr N` so the
/// agent routes into the right skill on launch with no extra typing.
/// trace:STORY-42 | ai:claude
pub fn exec_claude_with_prompt(
    permission_mode: &str,
    name: Option<&str>,
    initial_prompt: &str,
) -> Result<()> {
    exec_claude(permission_mode, name, Some(initial_prompt))
}

fn exec_claude(
    permission_mode: &str,
    name: Option<&str>,
    initial_prompt: Option<&str>,
) -> Result<()> {
    use std::process::Command;
    let mut cmd = Command::new("claude");
    cmd.args(["--permission-mode", permission_mode]);
    if let Some(n) = name {
        cmd.args(["--name", n]);
    }
    if let Some(p) = initial_prompt {
        // Positional first-message — claude treats trailing positionals
        // as the initial prompt for the session.
        cmd.arg(p);
    }
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
    let encoded = s.replace('/', "-");
    let home = dirs::home_dir().context("HOME not set; cannot locate Claude project dir")?;
    Ok(home.join(".claude/projects").join(encoded))
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
                    if !name.is_empty() {
                        role = Some(name);
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
            "TITLE",
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
        let title = s.title.as_deref().unwrap_or("(untitled)");
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
            title.dimmed(),
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
                s.title.as_deref().unwrap_or("(untitled)"),
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

/// Replace this process with `claude --resume <id>`. Falls back to spawn
/// + wait on platforms without exec semantics.
fn exec_claude_resume(id: &str) -> Result<()> {
    use std::process::Command;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new("claude").args(["--resume", id]).exec();
        // exec only returns on failure
        anyhow::bail!("failed to exec claude: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = Command::new("claude")
            .args(["--resume", id])
            .status()
            .context("failed to spawn claude")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
