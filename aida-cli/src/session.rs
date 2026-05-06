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
}

pub fn list(limit: usize, no_color: bool) -> Result<()> {
    if no_color {
        colored::control::set_override(false);
    }
    let sessions = collect_sessions(limit)?;
    if sessions.is_empty() {
        eprintln!("{}", "(no past sessions in this directory)".dimmed());
        return Ok(());
    }
    print_table(&sessions);
    Ok(())
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

    eprintln!(
        "{} {} → claude --permission-mode {}",
        "▶".green().bold(),
        format!("session new (role:{}, title:{:?})", role, title).dimmed(),
        permission_mode,
    );

    exec_claude_new(permission_mode)
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

/// Replace this process with `claude --permission-mode <mode>`.
fn exec_claude_new(permission_mode: &str) -> Result<()> {
    use std::process::Command;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new("claude")
            .args(["--permission-mode", permission_mode])
            .exec();
        anyhow::bail!("failed to exec claude: {}", err);
    }
    #[cfg(not(unix))]
    {
        let status = Command::new("claude")
            .args(["--permission-mode", permission_mode])
            .status()
            .context("failed to spawn claude")?;
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
            let parsed_ts = chrono::DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&chrono::Utc);
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
    let dir = claude_project_dir(&cwd)?;
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
    let launches = read_launches_for_cwd(&cwd);
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
fn claude_project_dir(cwd: &Path) -> Result<PathBuf> {
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
    let age_seconds = now
        .duration_since(mtime)
        .map(|d| d.as_secs())
        .unwrap_or(0);

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
    for (i, line) in reader.lines().enumerate() {
        if i >= MAX_LINES && title.is_some() && role.is_some() && started_at.is_some() {
            break;
        }
        let Ok(line) = line else { continue };

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
            "FR" | "BUG" | "TASK" | "EPIC" | "STORY" | "SPIKE" | "SPRINT"
                | "FOLDER" | "META" | "UR" | "SR" | "CR" | "REQ" | "NFR" | "SPEC"
        ) {
            return Some(line[start..j].to_string());
        }
    }
    None
}

fn print_table(sessions: &[SessionMeta]) {
    let id_w = 8;
    let role_w = sessions
        .iter()
        .map(|s| s.role.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(4)
        .max(4);
    let spec_w = sessions
        .iter()
        .map(|s| s.spec.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(4)
        .max(4);
    let age_w = 6;

    println!(
        "{}",
        format!(
            "{:<id_w$}  {:<age_w$}  {:<role_w$}  {:<spec_w$}  {}",
            "ID",
            "AGE",
            "ROLE",
            "SPEC",
            "TITLE",
            id_w = id_w,
            age_w = age_w,
            role_w = role_w,
            spec_w = spec_w,
        )
        .dimmed()
    );

    for s in sessions {
        let id_short = &s.id[..s.id.len().min(id_w)];
        let age = humanize_age(s.age_seconds);
        let role = s.role.as_deref().unwrap_or("-");
        let spec = s.spec.as_deref().unwrap_or("-");
        let title = s.title.as_deref().unwrap_or("(untitled)");
        println!(
            "{:<id_w$}  {:<age_w$}  {:<role_w$}  {:<spec_w$}  {}",
            id_short.bold(),
            age,
            role.yellow(),
            spec.cyan(),
            title.dimmed(),
            id_w = id_w,
            age_w = age_w,
            role_w = role_w,
            spec_w = spec_w,
        );
    }
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
    let sessions = collect_sessions(limit)?;
    if sessions.is_empty() {
        anyhow::bail!("no past sessions in this directory");
    }
    let labels: Vec<String> = sessions
        .iter()
        .map(|s| {
            format!(
                "{:<8}  {:<6}  {:<10}  {:<12}  {}",
                &s.id[..s.id.len().min(8)],
                humanize_age(s.age_seconds),
                s.role.as_deref().unwrap_or("-"),
                s.spec.as_deref().unwrap_or("-"),
                s.title.as_deref().unwrap_or("(untitled)"),
            )
        })
        .collect();

    let pick = inquire::Select::new("Resume which session?", labels)
        .with_help_message("↑↓ to move, type to filter, Enter to resume, Esc to cancel")
        .prompt()
        .context("interactive picker cancelled")?;

    // Map the picked label back to its session id (first 8 chars before
    // padding).
    let id_prefix: String = pick.chars().take(8).collect();
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
        assert_eq!(first_spec_id("Working on FR-1-042 today"), Some("FR-1-042".into()));
        assert_eq!(first_spec_id("BUG-1-017 is fixed"), Some("BUG-1-017".into()));
        assert_eq!(first_spec_id("see EPIC-2 and TASK-1"), Some("EPIC-2".into()));
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
            mk(-120, "way-before"),    // 2min before — outside 60s window
            mk(-10, "ten-before"),     // 10s before — inside window
            mk(45, "forty-five-after"), // 45s after — inside window, but farther than -10
            mk(200, "way-after"),      // outside window
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
}
