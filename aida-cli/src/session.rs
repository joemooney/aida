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
    pub size_bytes: u64,
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
    let metas = entries
        .into_iter()
        .filter_map(|(path, mtime)| parse_session_meta(&path, mtime, now).ok())
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
    let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
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
    for (i, line) in reader.lines().enumerate() {
        if i >= MAX_LINES && title.is_some() && role.is_some() {
            break;
        }
        let Ok(line) = line else { continue };

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
        size_bytes,
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
}
