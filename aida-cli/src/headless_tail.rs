//! `aida headless tail` — clean tailer for `.aida/headless-logs/<spec>-<lease>.jsonl`.
//!
//! Wraps the right JSONL filtering so the user doesn't have to discover (and
//! re-discover) the non-obvious jq pipeline
//! `select(.type=="assistant") | .message.content[] | select(.type=="text") | .text`.
//! Stays a thin wrapper — picks the latest log by mtime when no argument is
//! given, surfaces text and tool-use blocks cleanly, tolerates malformed
//! JSONL lines. No daemon, no buffering, no cache writes.
//!
//! trace:TASK-398

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use colored::Colorize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Polling interval for `--follow` mode. Short enough to feel live but not
/// so short it busy-spins.
const FOLLOW_POLL: Duration = Duration::from_millis(250);

/// Truncation width for a tool-call input preview (acceptance criterion).
const TOOL_INPUT_PREVIEW_CHARS: usize = 120;

/// Caller-supplied options for the tail command.
#[derive(Debug, Clone)]
pub struct TailOptions {
    /// Positional argument: a SPEC-ID, a lease/session id (or prefix), or `None`.
    pub selector: Option<String>,
    /// Just list available logs and exit (no follow, no streaming).
    pub list: bool,
    /// Include tool invocations interleaved with assistant text.
    pub with_tools: bool,
    /// Only tool invocations, no assistant text.
    pub tools_only: bool,
    /// Include user-typed `type=="user"` messages (tool_result content).
    pub include_user: bool,
    /// `tail -f`-style follow. `false` matches the `--no-follow` / `-n` flag.
    pub follow: bool,
    /// Discard entries older than this duration. Compared against
    /// `message.timestamp` when present; entries with no timestamp pass through.
    pub since: Option<Duration>,
    /// Colorize the output (auto-disabled when stdout is piped, or `NO_COLOR` set).
    pub color: bool,
}

impl Default for TailOptions {
    fn default() -> Self {
        Self {
            selector: None,
            list: false,
            with_tools: false,
            tools_only: false,
            include_user: false,
            follow: true,
            since: None,
            color: true,
        }
    }
}

/// One log file under `.aida/headless-logs/`.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub path: PathBuf,
    pub filename: String,
    pub mtime: SystemTime,
    pub size: u64,
    /// `task`, `advise`, `resume`, `other` — derived from filename prefix.
    pub kind: String,
    /// Spec id parsed out of the filename (uppercased), best-effort.
    pub spec: Option<String>,
    /// Last hyphen-separated token before `.jsonl` — the lease/session id.
    pub lease: Option<String>,
}

/// Discover every `.jsonl` under `.aida/headless-logs/`, newest first.
pub fn discover_logs(dir: &Path) -> Result<Vec<LogEntry>> {
    if !dir.exists() {
        bail!(
            "no headless logs directory at {} — has a `--no-human` drain run from this project?",
            dir.display()
        );
    }
    let mut out: Vec<LogEntry> = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        let (kind, spec, lease) = parse_filename(&filename);
        out.push(LogEntry {
            path,
            filename,
            mtime,
            size: meta.len(),
            kind,
            spec,
            lease,
        });
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    Ok(out)
}

/// Parse a log filename into (kind, spec_id, lease_id).
///
/// Patterns observed in `.aida/headless-logs/`:
///
/// - `<branch>-<session-uuid>.jsonl` — implementer / reviewer drain.
///   Branch is typically `task-1`, `task-398`, `bug-89`, `story-86`, `pr-65`.
///   Session UUID is v7-style with 4 hyphens.
/// - `advise-SPEC-ID-<short-id>.jsonl` — STORY-306 advisor sub-session.
/// - `resume-SPEC-ID-<short-id>.jsonl` — advisor-resumed implementer.
pub fn parse_filename(filename: &str) -> (String, Option<String>, Option<String>) {
    let stem = filename.strip_suffix(".jsonl").unwrap_or(filename);
    if let Some(rest) = stem.strip_prefix("advise-") {
        return parse_prefixed("advise", rest);
    }
    if let Some(rest) = stem.strip_prefix("resume-") {
        return parse_prefixed("resume", rest);
    }
    // `<branch>-<uuid>.jsonl`. The UUID's last 5 hyphen-separated tokens are
    // `xxxxxxxx`, `xxxx`, `xxxx`, `xxxx`, `xxxxxxxxxxxx`. Anything before
    // that is the branch.
    let tokens: Vec<&str> = stem.split('-').collect();
    if tokens.len() >= 6 && looks_like_uuid_tail(&tokens[tokens.len() - 5..]) {
        let branch = tokens[..tokens.len() - 5].join("-");
        let lease = tokens[tokens.len() - 5..].join("-");
        return ("task".to_string(), branch_to_spec(&branch), Some(lease));
    }
    // Fallback: a single trailing token is the lease, everything before is
    // the branch (matches the `pr-65-abc` shape used in unit tests).
    if let Some((branch, lease)) = stem.rsplit_once('-') {
        return (
            "task".to_string(),
            branch_to_spec(branch),
            Some(lease.to_string()),
        );
    }
    ("other".to_string(), None, None)
}

fn parse_prefixed(kind: &str, rest: &str) -> (String, Option<String>, Option<String>) {
    // `SPEC-ID-<short-id>` — the short id is the LAST hyphen-separated token,
    // the rest is the spec.
    if let Some((spec, lease)) = rest.rsplit_once('-') {
        (
            kind.to_string(),
            Some(spec.to_string()),
            Some(lease.to_string()),
        )
    } else {
        (kind.to_string(), Some(rest.to_string()), None)
    }
}

fn looks_like_uuid_tail(toks: &[&str]) -> bool {
    // v7 UUID: 8-4-4-4-12 hex chars.
    let widths = [8, 4, 4, 4, 12];
    if toks.len() != 5 {
        return false;
    }
    for (t, w) in toks.iter().zip(widths.iter()) {
        if t.len() != *w || !t.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
    }
    true
}

fn branch_to_spec(branch: &str) -> Option<String> {
    if branch.is_empty() {
        return None;
    }
    Some(branch.to_uppercase())
}

/// Select the right log file given the user's positional argument.
pub fn select_log<'a>(entries: &'a [LogEntry], selector: Option<&str>) -> Result<&'a LogEntry> {
    if entries.is_empty() {
        bail!("no log files under .aida/headless-logs/");
    }
    let Some(sel) = selector else {
        return Ok(&entries[0]);
    };
    let want_spec = looks_like_spec_id(sel);
    let sel_upper = sel.to_uppercase();

    // Filter candidates.
    let matches: Vec<&LogEntry> = entries
        .iter()
        .filter(|e| {
            if want_spec {
                // Match either the parsed spec or a substring of the filename
                // (covers `task-398-` in the branch and `TASK-398-` in
                // advise/resume forms).
                e.spec.as_deref().map(|s| s.eq_ignore_ascii_case(sel)) == Some(true)
                    || e.filename.to_uppercase().contains(&sel_upper)
            } else {
                // Lease / session id — match on the parsed lease prefix or
                // any substring of the filename.
                e.lease.as_deref().map(|l| l.starts_with(sel)) == Some(true)
                    || e.filename.contains(sel)
            }
        })
        .collect();

    match matches.len() {
        0 => bail!(
            "no log files match `{}` — try `aida headless tail --list` to see what is available.",
            sel
        ),
        1 => Ok(matches[0]),
        _ if want_spec => {
            // Multiple drains touched the same spec — `select_log` returns the
            // newest, matching the acceptance criterion "<spec-id> picks the
            // latest log for that spec".
            Ok(matches[0])
        }
        _ => {
            let mut names: Vec<&str> = matches.iter().map(|e| e.filename.as_str()).collect();
            names.sort();
            bail!(
                "selector `{}` is ambiguous — matches {} log files: {}. Use a longer lease prefix or pass the full filename.",
                sel,
                matches.len(),
                names.join(", ")
            );
        }
    }
}

fn looks_like_spec_id(s: &str) -> bool {
    // Crude SPEC-ID detector: starts with uppercase letters, has at least one
    // digit segment after a hyphen. Matches `TASK-398`, `STORY-86`,
    // `TASK-1-001`, `FR-1-042`.
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    if !s.contains('-') {
        return false;
    }
    let after_first_dash = s.split_once('-').map(|x| x.1).unwrap_or("");
    after_first_dash
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Top-level entry point — invoked from main.rs dispatch.
pub fn handle_tail(project_root: &Path, opts: &TailOptions) -> Result<()> {
    if opts.tools_only && !opts.with_tools {
        // --tools-only implies --with-tools (otherwise nothing would print).
        // Treat them as compatible silently rather than erroring.
    }
    let dir = project_root.join(".aida").join("headless-logs");
    let entries = discover_logs(&dir)?;

    if opts.list {
        return print_log_list(&entries, opts.color);
    }

    let target = select_log(&entries, opts.selector.as_deref())?;
    let since_cutoff = opts.since.and_then(|d| {
        DateTime::<Utc>::from(SystemTime::now())
            .checked_sub_signed(chrono::Duration::from_std(d).unwrap_or(chrono::Duration::zero()))
    });
    let format_opts = FormatOpts {
        with_tools: opts.with_tools || opts.tools_only,
        tools_only: opts.tools_only,
        include_user: opts.include_user,
        color: opts.color,
        since: since_cutoff,
    };
    eprintln!(
        "{} {}",
        "==>".dimmed(),
        target.path.display().to_string().dimmed()
    );
    stream_log(target, &format_opts, opts.follow)
}

fn print_log_list(entries: &[LogEntry], color: bool) -> Result<()> {
    if entries.is_empty() {
        println!("No log files under .aida/headless-logs/.");
        return Ok(());
    }
    let mut stdout = std::io::stdout().lock();
    let header = format!(
        "{:<8} {:<14} {:<12} {:<26} {:>10}  {}",
        "KIND", "SPEC", "LEASE", "MTIME (local)", "SIZE", "FILE"
    );
    if color {
        writeln!(stdout, "{}", header.bold())?;
    } else {
        writeln!(stdout, "{}", header)?;
    }
    for entry in entries {
        let mtime: DateTime<chrono::Local> = entry.mtime.into();
        let size = format_size(entry.size);
        let spec = entry.spec.as_deref().unwrap_or("-");
        let lease = entry.lease.as_deref().unwrap_or("-");
        let lease_truncated = if lease.len() > 12 {
            &lease[..12]
        } else {
            lease
        };
        writeln!(
            stdout,
            "{:<8} {:<14} {:<12} {:<26} {:>10}  {}",
            entry.kind,
            spec,
            lease_truncated,
            mtime.format("%Y-%m-%d %H:%M:%S"),
            size,
            entry.filename
        )?;
    }
    Ok(())
}

fn format_size(b: u64) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
    }
}

/// Per-line formatting options derived from `TailOptions`.
#[derive(Debug, Clone)]
pub struct FormatOpts {
    pub with_tools: bool,
    pub tools_only: bool,
    pub include_user: bool,
    pub color: bool,
    pub since: Option<DateTime<Utc>>,
}

impl Default for FormatOpts {
    fn default() -> Self {
        Self {
            with_tools: false,
            tools_only: false,
            include_user: false,
            color: true,
            since: None,
        }
    }
}

/// Outcome of formatting a single JSONL line.
#[derive(Debug, Default)]
pub struct FormatResult {
    /// Zero or more output lines (already styled when `opts.color`).
    pub lines: Vec<String>,
    /// True when this line failed to parse and should bump the malformed counter.
    pub malformed: bool,
    /// True when this line was a `type=="result"` final event — caller can
    /// emit a discreet end-of-session marker once stdout drains.
    pub is_result: bool,
}

/// Format one JSONL line into zero or more printable strings.
pub fn format_line(line: &str, opts: &FormatOpts) -> FormatResult {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return FormatResult::default();
    }
    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            return FormatResult {
                malformed: true,
                ..FormatResult::default()
            };
        }
    };
    let typ = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let mut out = FormatResult::default();

    // --since filter: only enforced when the event carries a timestamp.
    if let Some(cutoff) = opts.since {
        if let Some(ts) = parsed.get("timestamp").and_then(|v| v.as_str()) {
            if let Ok(parsed_ts) = DateTime::parse_from_rfc3339(ts) {
                if parsed_ts.with_timezone(&Utc) < cutoff {
                    return out;
                }
            }
        }
    }

    match typ {
        "assistant" => {
            let content = parsed
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            let Some(content) = content else {
                return out;
            };
            for block in content {
                let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "text" if !opts.tools_only => {
                        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                            let trimmed = text.trim_end();
                            if !trimmed.is_empty() {
                                out.lines.push(if opts.color {
                                    trimmed.normal().to_string()
                                } else {
                                    trimmed.to_string()
                                });
                                out.lines.push(String::new());
                            }
                        }
                    }
                    "tool_use" if opts.with_tools => {
                        let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let preview = preview_tool_input(block.get("input"));
                        let line = format!("[{}] {}", name, preview);
                        out.lines.push(if opts.color {
                            line.cyan().to_string()
                        } else {
                            line
                        });
                    }
                    _ => {}
                }
            }
        }
        "user" if opts.include_user => {
            // Pull tool_result text out of the user-typed echo. Tool results
            // are the dominant payload in headless mode; raw user text is rare.
            let content = parsed.get("message").and_then(|m| m.get("content"));
            if let Some(blocks) = content.and_then(|c| c.as_array()) {
                for block in blocks {
                    let t = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if t == "tool_result" {
                        let text = extract_tool_result_text(block);
                        if !text.is_empty() {
                            let line = format!("← {}", text);
                            out.lines.push(if opts.color {
                                line.dimmed().to_string()
                            } else {
                                line
                            });
                        }
                    } else if t == "text" {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                            out.lines.push(if opts.color {
                                format!("> {}", t).yellow().to_string()
                            } else {
                                format!("> {}", t)
                            });
                        }
                    }
                }
            } else if let Some(text) = content.and_then(|c| c.as_str()) {
                out.lines.push(if opts.color {
                    format!("> {}", text).yellow().to_string()
                } else {
                    format!("> {}", text)
                });
            }
        }
        "result" => {
            out.is_result = true;
        }
        _ => {}
    }
    out
}

fn preview_tool_input(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let s = if let Some(s) = input.as_str() {
        s.to_string()
    } else if let Some(obj) = input.as_object() {
        // Common single-field tools (Read.file_path, Bash.command, Skill.skill,
        // Grep.pattern) read cleaner with just the value when there is one
        // dominant field; otherwise fall back to compact JSON.
        let dominant = [
            "command",
            "file_path",
            "pattern",
            "skill",
            "query",
            "prompt",
        ]
        .iter()
        .find_map(|k| obj.get(*k).and_then(|v| v.as_str()));
        match dominant {
            Some(d) => d.to_string(),
            None => serde_json::to_string(obj).unwrap_or_default(),
        }
    } else {
        serde_json::to_string(input).unwrap_or_default()
    };
    let collapsed: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if collapsed.chars().count() > TOOL_INPUT_PREVIEW_CHARS {
        let truncated: String = collapsed.chars().take(TOOL_INPUT_PREVIEW_CHARS).collect();
        format!("{}…", truncated)
    } else {
        collapsed
    }
}

fn extract_tool_result_text(block: &Value) -> String {
    if let Some(s) = block.get("content").and_then(|v| v.as_str()) {
        return first_nonempty_line(s);
    }
    if let Some(arr) = block.get("content").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                let line = first_nonempty_line(t);
                if !line.is_empty() {
                    return line;
                }
            }
        }
    }
    String::new()
}

fn first_nonempty_line(s: &str) -> String {
    for line in s.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return if trimmed.chars().count() > TOOL_INPUT_PREVIEW_CHARS {
                let cut: String = trimmed.chars().take(TOOL_INPUT_PREVIEW_CHARS).collect();
                format!("{}…", cut)
            } else {
                trimmed.to_string()
            };
        }
    }
    String::new()
}

/// Install a SIGINT handler that flips `INTERRUPTED`. Used only during
/// `stream_log` so the rest of the binary keeps the platform default.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
fn install_sigint_trap() {
    extern "C" fn handler(_: libc::c_int) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }
    unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
}

#[cfg(unix)]
fn uninstall_sigint_trap() {
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn install_sigint_trap() {}
#[cfg(not(unix))]
fn uninstall_sigint_trap() {}

/// Open `entry.path`, stream existing content, and (when `follow`) keep
/// reading new bytes appended to the file. Counts malformed JSONL lines and
/// reports a summary on stderr at end-of-stream.
fn stream_log(entry: &LogEntry, opts: &FormatOpts, follow: bool) -> Result<()> {
    INTERRUPTED.store(false, Ordering::SeqCst);
    install_sigint_trap();
    let result = stream_log_inner(entry, opts, follow);
    uninstall_sigint_trap();
    result
}

fn stream_log_inner(entry: &LogEntry, opts: &FormatOpts, follow: bool) -> Result<()> {
    let mut pos: u64 = 0;
    let mut malformed: u64 = 0;
    let mut emitted: u64 = 0;
    let mut seen_result = false;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        // Reopen each tick so a file truncated/rotated between iterations is
        // handled gracefully — fresh open, fresh seek.
        let mut file = fs::File::open(&entry.path)
            .with_context(|| format!("opening {}", entry.path.display()))?;
        let file_len = file.metadata().map(|m| m.len()).unwrap_or(pos);
        if file_len < pos {
            // Truncation — reset and re-read from the top.
            pos = 0;
        }
        file.seek(SeekFrom::Start(pos))?;
        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        loop {
            buf.clear();
            let read = reader
                .read_line(&mut buf)
                .with_context(|| format!("reading {}", entry.path.display()))?;
            if read == 0 {
                break;
            }
            if !buf.ends_with('\n') {
                // Partial line — leave `pos` where it is so the next tick
                // re-reads the partial bytes plus whatever was appended.
                break;
            }
            if INTERRUPTED.load(Ordering::SeqCst) {
                return finish(out, malformed, emitted);
            }
            pos += read as u64;
            let fmt = format_line(buf.trim_end_matches('\n'), opts);
            if fmt.malformed {
                malformed += 1;
                continue;
            }
            for line in &fmt.lines {
                if writeln!(out, "{}", line).is_err() {
                    return finish(out, malformed, emitted);
                }
                emitted += 1;
            }
            if fmt.is_result {
                seen_result = true;
            }
        }
        if INTERRUPTED.load(Ordering::SeqCst) {
            break;
        }
        if !follow {
            break;
        }
        if seen_result {
            // Reached the terminal `result` event — caller asked to follow,
            // but the session is over; no more bytes will ever be appended.
            break;
        }
        std::thread::sleep(FOLLOW_POLL);
    }
    finish(out, malformed, emitted)
}

fn finish(mut out: std::io::StdoutLock<'_>, malformed: u64, emitted: u64) -> Result<()> {
    let _ = out.flush();
    if malformed > 0 {
        eprintln!(
            "{} skipped {} malformed JSONL line{} (emitted {} formatted lines)",
            "warning:".yellow(),
            malformed,
            if malformed == 1 { "" } else { "s" },
            emitted
        );
    }
    Ok(())
}

/// Parse `--since` values like `10m`, `5s`, `2h`, `1d`, or a bare integer
/// (interpreted as seconds). Returns an explicit `Duration` rather than a
/// chrono::Duration so the caller can reuse it across stdlib APIs.
pub fn parse_since(s: &str) -> Result<Duration> {
    let s = s.trim();
    if s.is_empty() {
        bail!("--since value is empty");
    }
    let (num_part, unit_part): (&str, &str) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(idx) => s.split_at(idx),
        None => (s, "s"),
    };
    if num_part.is_empty() {
        bail!(
            "--since `{}` must start with digits (e.g. `10m`, `2h`, `1d`)",
            s
        );
    }
    let n: u64 = num_part
        .parse()
        .map_err(|_| anyhow!("--since `{}` has an unparseable numeric prefix", s))?;
    let secs = match unit_part {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => n,
        "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => n * 3600,
        "d" | "day" | "days" => n * 86400,
        other => bail!(
            "--since unit `{}` is not recognized (use s/m/h/d, e.g. `10m`)",
            other
        ),
    };
    Ok(Duration::from_secs(secs))
}

/// `aida headless tail --list` output uses these column derivations; this
/// helper is exposed for the tests so the same code path is verified.
#[allow(dead_code)]
pub fn known_kinds() -> BTreeSet<&'static str> {
    ["task", "advise", "resume", "other"]
        .iter()
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_task_branch_filename() {
        let (kind, spec, lease) =
            parse_filename("task-1-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl");
        assert_eq!(kind, "task");
        assert_eq!(spec.as_deref(), Some("TASK-1"));
        assert_eq!(
            lease.as_deref(),
            Some("019e4405-5073-7672-9395-16d4ca8be1a4")
        );
    }

    #[test]
    fn parse_task_multi_segment_branch() {
        // Node-aware spec like `TASK-1-001` produces branch `task-1-001`
        // alongside a v7 session uuid.
        let (kind, spec, lease) =
            parse_filename("task-1-001-019e4405-5073-7672-9395-16d4ca8be1a4.jsonl");
        assert_eq!(kind, "task");
        assert_eq!(spec.as_deref(), Some("TASK-1-001"));
        assert_eq!(
            lease.as_deref(),
            Some("019e4405-5073-7672-9395-16d4ca8be1a4")
        );
    }

    #[test]
    fn parse_advise_filename() {
        let (kind, spec, lease) = parse_filename("advise-TASK-1-019e43f9.jsonl");
        assert_eq!(kind, "advise");
        assert_eq!(spec.as_deref(), Some("TASK-1"));
        assert_eq!(lease.as_deref(), Some("019e43f9"));
    }

    #[test]
    fn parse_resume_filename() {
        let (kind, spec, lease) = parse_filename("resume-STORY-86-abcd1234.jsonl");
        assert_eq!(kind, "resume");
        assert_eq!(spec.as_deref(), Some("STORY-86"));
        assert_eq!(lease.as_deref(), Some("abcd1234"));
    }

    #[test]
    fn parse_short_branch_falls_back_to_last_token_as_lease() {
        // reviewer_summary.rs::tests uses this shape — three tokens, no v7 UUID.
        let (kind, spec, lease) = parse_filename("pr-65-abc.jsonl");
        assert_eq!(kind, "task");
        assert_eq!(spec.as_deref(), Some("PR-65"));
        assert_eq!(lease.as_deref(), Some("abc"));
    }

    #[test]
    fn looks_like_spec_id_discriminates() {
        assert!(looks_like_spec_id("TASK-398"));
        assert!(looks_like_spec_id("STORY-86"));
        assert!(looks_like_spec_id("TASK-1-001"));
        assert!(!looks_like_spec_id("019e4405"));
        assert!(!looks_like_spec_id("task-398"));
        assert!(!looks_like_spec_id("019e4405-5073-7672-9395-16d4ca8be1a4"));
    }

    fn entry(name: &str, mtime_secs: u64) -> LogEntry {
        let (kind, spec, lease) = parse_filename(name);
        LogEntry {
            path: PathBuf::from(name),
            filename: name.to_string(),
            mtime: UNIX_EPOCH + Duration::from_secs(mtime_secs),
            size: 0,
            kind,
            spec,
            lease,
        }
    }

    #[test]
    fn select_log_no_selector_picks_newest() {
        let entries = vec![
            entry("task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl", 200),
            entry("task-398-019e4400-1234-7000-aaaa-bbbbbbbbbbbb.jsonl", 100),
        ];
        let pick = select_log(&entries, None).unwrap();
        assert_eq!(pick.mtime, UNIX_EPOCH + Duration::from_secs(200));
    }

    #[test]
    fn select_log_spec_id_picks_newest_matching() {
        // discover_logs sorts newest-first, simulate that here.
        let entries = vec![
            entry("task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl", 300),
            entry("task-86-019e4400-1234-7000-aaaa-bbbbbbbbbbbb.jsonl", 200),
            entry("task-398-019e4300-1234-7000-aaaa-aaaaaaaaaaaa.jsonl", 100),
        ];
        let pick = select_log(&entries, Some("TASK-398")).unwrap();
        assert_eq!(pick.mtime, UNIX_EPOCH + Duration::from_secs(300));
    }

    #[test]
    fn select_log_spec_id_matches_advise_form() {
        let entries = vec![
            entry("task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl", 300),
            entry("advise-TASK-398-019e43f9.jsonl", 250),
        ];
        // SPEC selector should find the upper-case advise file too — the latest of those two wins.
        let pick = select_log(&entries, Some("TASK-398")).unwrap();
        assert_eq!(
            pick.filename,
            "task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl"
        );
    }

    #[test]
    fn select_log_lease_prefix_disambiguates() {
        let entries = vec![
            entry("task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl", 300),
            entry("task-398-019e4400-1234-7000-aaaa-bbbbbbbbbbbb.jsonl", 200),
        ];
        let pick = select_log(&entries, Some("019e4400")).unwrap();
        assert!(pick.filename.contains("019e4400"));
    }

    #[test]
    fn select_log_ambiguous_lease_errors() {
        let entries = vec![
            entry("task-398-aaaa1234-5673-7000-aaaa-cccccccccccc.jsonl", 300),
            entry("task-86-aaaa9999-1111-7000-aaaa-bbbbbbbbbbbb.jsonl", 200),
        ];
        // "aaaa" is a prefix of both lease ids → ambiguous.
        let err = select_log(&entries, Some("aaaa")).unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{}", err);
    }

    #[test]
    fn select_log_no_match_errors() {
        let entries = vec![entry(
            "task-398-019e4500-1234-7000-aaaa-cccccccccccc.jsonl",
            300,
        )];
        let err = select_log(&entries, Some("BUG-999")).unwrap_err();
        assert!(err.to_string().contains("no log files match"));
    }

    fn opts_default() -> FormatOpts {
        FormatOpts::default()
    }

    #[test]
    fn format_assistant_text_block() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello world"}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        let res = format_line(line, &opts);
        assert!(!res.malformed);
        assert_eq!(res.lines, vec!["hello world".to_string(), String::new()]);
    }

    #[test]
    fn format_assistant_thinking_block_is_silent() {
        // Thinking blocks must not leak into the clean stream.
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        let res = format_line(line, &opts);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_tool_use_with_tools_flag() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la","description":"List files"}}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        opts.with_tools = true;
        let res = format_line(line, &opts);
        assert_eq!(res.lines.len(), 1);
        assert!(res.lines[0].starts_with("[Bash] ls -la"));
    }

    #[test]
    fn format_tool_use_without_with_tools_is_silent() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        let res = format_line(line, &opts);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_tools_only_drops_text() {
        let line = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"narration"},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        opts.with_tools = true;
        opts.tools_only = true;
        let res = format_line(line, &opts);
        assert_eq!(res.lines.len(), 1);
        assert!(res.lines[0].starts_with("[Bash] ls"));
    }

    #[test]
    fn format_tool_input_truncates_to_120() {
        let long = "a".repeat(200);
        let line = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"Bash","input":{{"command":"{}"}}}}]}}}}"#,
            long
        );
        let mut opts = opts_default();
        opts.color = false;
        opts.with_tools = true;
        let res = format_line(&line, &opts);
        assert_eq!(res.lines.len(), 1);
        let body = res.lines[0].trim_start_matches("[Bash] ");
        // 120 chars + the truncation ellipsis.
        assert_eq!(body.chars().count(), 121, "{}", body);
        assert!(body.ends_with('…'));
    }

    #[test]
    fn format_malformed_line_marked() {
        let res = format_line("{not-json", &opts_default());
        assert!(res.malformed);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_empty_line_is_silent() {
        let res = format_line("", &opts_default());
        assert!(!res.malformed);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_unknown_type_is_silent() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{}}"#;
        let res = format_line(line, &opts_default());
        assert!(!res.malformed);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_result_event_marks_terminal() {
        let line = r#"{"type":"result","subtype":"success","is_error":false}"#;
        let res = format_line(line, &opts_default());
        assert!(res.is_result);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_user_tool_result_off_by_default() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"file listing"}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        let res = format_line(line, &opts);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn format_user_tool_result_when_include_user() {
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"file listing\nline2"}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        opts.include_user = true;
        let res = format_line(line, &opts);
        assert_eq!(res.lines, vec!["← file listing".to_string()]);
    }

    #[test]
    fn since_filter_drops_old_events() {
        // Cutoff: now. The event timestamp is 1970-01-01 → filtered out.
        let line = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"old"}]},"timestamp":"1970-01-01T00:00:00.000Z"}"#;
        let mut opts = opts_default();
        opts.include_user = true;
        opts.since = Some(Utc::now());
        let res = format_line(line, &opts);
        assert!(res.lines.is_empty());
    }

    #[test]
    fn since_filter_keeps_events_without_timestamp() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"keep me"}]}}"#;
        let mut opts = opts_default();
        opts.color = false;
        opts.since = Some(Utc::now());
        let res = format_line(line, &opts);
        assert!(res.lines.iter().any(|l| l == "keep me"));
    }

    #[test]
    fn parse_since_units() {
        assert_eq!(parse_since("10s").unwrap(), Duration::from_secs(10));
        assert_eq!(parse_since("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_since("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_since("1d").unwrap(), Duration::from_secs(86400));
        // Bare integer → seconds.
        assert_eq!(parse_since("30").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn parse_since_rejects_garbage() {
        assert!(parse_since("").is_err());
        assert!(parse_since("abc").is_err());
        assert!(parse_since("10y").is_err());
    }

    #[test]
    fn discover_logs_errors_on_missing_dir() {
        let tmp =
            std::env::temp_dir().join(format!("aida-headless-tail-test-{}", std::process::id()));
        let err = discover_logs(&tmp).unwrap_err();
        assert!(err.to_string().contains("no headless logs directory"));
    }

    #[test]
    fn discover_logs_sorts_newest_first() {
        let tmp = std::env::temp_dir().join(format!(
            "aida-headless-tail-test-discover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let older = tmp.join("task-1-aaaa1111-2222-7000-3333-444444444444.jsonl");
        let newer = tmp.join("task-1-bbbb1111-2222-7000-3333-555555555555.jsonl");
        fs::write(&older, "").unwrap();
        // Sleep so mtime granularity (some FSes are 1s) registers a difference.
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(&newer, "{}\n").unwrap();
        let entries = discover_logs(&tmp).unwrap();
        let _ = fs::remove_dir_all(&tmp);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].filename.contains("bbbb1111"));
    }
}
