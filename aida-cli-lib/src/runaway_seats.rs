//! Runaway-seat watchdog: the `runaway-seats` doctor category.
//!
//! BUG-1589: two Claude seats burned ~9.8B tokens answering mail ticks that
//! fired every ~7-10s, and nothing noticed for ~24h. Every signal needed was
//! already on disk. This module reads it and judges it, without a model call.
//!
//! Evidence, in the order it is trusted:
//! - Claude Code transcripts under `~/.claude/projects/<slug>/**/*.jsonl` for
//!   the project root AND every registered worktree (a seat in a worktree is
//!   still this project's spend). These carry timestamps, per-request usage,
//!   the injected prompts (`isMeta` / `promptSource`), scheduled-task fires
//!   (with their declared cron), and compaction boundaries.
//! - Headless session logs under `.aida/headless-logs/` and `.aida/burndown/`
//!   (stream-json). These usually carry NO per-line timestamps, so they count
//!   toward the project's daily budget by file mtime, and their hourly rates
//!   are reported as unknown, never as ok.
//! - The agent registry (`.aida/agents/*.toml`), only to name a seat and pid
//!   for a session id.
//!
//! Deliberately NOT read: `~/.aida/usage.jsonl` (machine-global, no session
//! id, dominated by statusline calls, so it cannot attribute a rate to a
//! seat) and Codex sessions (`~/.codex/sessions`, not scoped to a project
//! cheaply). The coverage report says so rather than implying they were
//! checked.
//!
//! Cost is bounded: a per-file byte watermark (runtime state under
//! `.aida/watchdog/`) means a steady-state tick only re-reads the trailing
//! day of each transcript, files untouched for a day are skipped by mtime,
//! and hard byte and wall-clock budgets cap a run. When a budget cuts a run
//! short, the result is reported as partial evidence, never as clean. The
//! module calls no model and makes no network request; its only subprocess
//! is the one `git worktree list` (in `aida_core::git_ops`) that finds the
//! worktree slugs.
// trace:STORY-1462 | ai:claude

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "runaway-seats";

/// Lines longer than this are skipped unparsed (a giant tool result carries
/// no usage worth reading and would dominate the run's cost).
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const DAY_HOURS: i64 = 24;

/// Thresholds, read from `[watchdog]` in `.aida/config.toml`. Every default is
/// chosen to be silent on an ordinary busy seat and loud on the BUG-1589
/// storm (a tick every 7-10s is 360-500 wakes an hour).
// trace:STORY-1462 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatchdogPolicy {
    /// Trailing window the per-session rate rules judge.
    pub(crate) window_minutes: i64,
    /// New turns (prompts delivered to the model) per session per window.
    pub(crate) max_wakes_per_hour: u64,
    /// Tokens (uncached input + cache read + cache write + output) per
    /// session per window.
    pub(crate) max_tokens_per_hour: u64,
    /// Project-wide token budget per trailing 24h.
    pub(crate) daily_token_budget: u64,
    /// Alert once this share of the daily budget is spent, so the alert
    /// lands before the budget is gone.
    pub(crate) budget_alert_pct: u64,
    /// The same injected (non-human) prompt per session per window.
    pub(crate) max_repeated_prompts_per_hour: u64,
    /// Share of end-of-turn replies that are no-ops, over the window.
    pub(crate) max_idle_ratio_pct: u64,
    /// Minimum replies in the window before the idle ratio is judged.
    pub(crate) idle_min_replies: u64,
    /// A reply of at most this many characters counts as a no-op.
    pub(crate) noop_reply_max_chars: usize,
    /// Per-call context above which a restart is recommended.
    pub(crate) restart_context_tokens: u64,
    /// Auto-compactions per session per trailing 24h.
    pub(crate) max_compactions_per_day: u64,
    /// Bytes read from one file on its first (watermark-less) scan.
    pub(crate) max_bytes_per_file: u64,
    /// Bytes read across all files in one run.
    pub(crate) max_total_bytes: u64,
    /// Wall-clock budget for one run.
    pub(crate) time_budget_ms: u64,
}

impl Default for WatchdogPolicy {
    fn default() -> Self {
        Self {
            window_minutes: 60,
            max_wakes_per_hour: 120,
            max_tokens_per_hour: 100_000_000,
            daily_token_budget: 2_000_000_000,
            budget_alert_pct: 50,
            max_repeated_prompts_per_hour: 30,
            max_idle_ratio_pct: 80,
            idle_min_replies: 20,
            noop_reply_max_chars: 60,
            restart_context_tokens: 300_000,
            max_compactions_per_day: 3,
            max_bytes_per_file: 256 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024,
            time_budget_ms: 10_000,
        }
    }
}

/// Read `[watchdog]`, keeping the default for any key that is absent,
/// non-integer or non-positive.
// trace:STORY-1462 | ai:claude
pub(crate) fn policy(cfg: Option<&toml::Value>) -> WatchdogPolicy {
    let mut p = WatchdogPolicy::default();
    let Some(section) = cfg.and_then(|c| c.get("watchdog")) else {
        return p;
    };
    let get = |key: &str| {
        section
            .get(key)
            .and_then(|v| v.as_integer())
            .filter(|v| *v > 0)
            .map(|v| v as u64)
    };
    if let Some(v) = get("window_minutes") {
        p.window_minutes = v as i64;
    }
    macro_rules! set {
        ($($field:ident),*) => {$(
            if let Some(v) = get(stringify!($field)) {
                p.$field = v;
            }
        )*};
    }
    set!(
        max_wakes_per_hour,
        max_tokens_per_hour,
        daily_token_budget,
        max_repeated_prompts_per_hour,
        idle_min_replies,
        restart_context_tokens,
        max_compactions_per_day,
        max_bytes_per_file,
        max_total_bytes,
        time_budget_ms
    );
    if let Some(v) = get("budget_alert_pct").filter(|v| *v <= 100) {
        p.budget_alert_pct = v;
    }
    if let Some(v) = get("max_idle_ratio_pct").filter(|v| *v <= 100) {
        p.max_idle_ratio_pct = v;
    }
    if let Some(v) = get("noop_reply_max_chars") {
        p.noop_reply_max_chars = v as usize;
    }
    p
}

/// Where the evidence lives. Injectable so tests never touch `$HOME`.
#[derive(Debug, Clone, Default)]
pub(crate) struct Sources {
    /// Claude Code project transcript directories (one per worktree slug).
    pub(crate) transcript_dirs: Vec<PathBuf>,
    /// Headless / burndown stream-json log directories.
    pub(crate) headless_dirs: Vec<PathBuf>,
    /// Watermark file; `None` disables incremental state (full re-read).
    pub(crate) watermark_path: Option<PathBuf>,
}

/// The real sources for `project_root`: its own Claude slug plus every
/// registered worktree's, and the two project-local headless log dirs.
// trace:STORY-1462 | ai:claude
pub(crate) fn default_sources(project_root: &Path) -> Sources {
    let mut roots = vec![project_root.to_path_buf()];
    for wt in aida_core::git_ops::list_worktree_paths(project_root) {
        if !roots.contains(&wt) {
            roots.push(wt);
        }
    }
    let transcript_dirs = match dirs::home_dir() {
        Some(home) => roots
            .iter()
            .map(|r| {
                home.join(".claude")
                    .join("projects")
                    .join(aida_core::liveness::encode_cwd_for_projects(r))
            })
            .collect(),
        None => Vec::new(),
    };
    Sources {
        transcript_dirs,
        headless_dirs: vec![
            project_root.join(".aida").join("headless-logs"),
            project_root.join(".aida").join("burndown"),
        ],
        watermark_path: Some(
            project_root
                .join(".aida")
                .join("watchdog")
                .join("watermarks.json"),
        ),
    }
}

/// Seat identity for a session id, from the agent registry.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SeatInfo {
    pub(crate) seat: String,
    pub(crate) pid: Option<u32>,
}

/// Map Claude / native session ids to the registry's seat label and pid.
// trace:STORY-1462 | ai:claude
pub(crate) fn registry_attribution(project_root: &Path) -> HashMap<String, SeatInfo> {
    let mut out = HashMap::new();
    for (_, e) in crate::agent_registry::load_entries(project_root) {
        let seat = e
            .role
            .clone()
            .or_else(|| e.name.clone())
            .unwrap_or_else(|| e.id.clone());
        for sid in [&e.claude_session_id, &e.native_session_id]
            .into_iter()
            .flatten()
        {
            out.insert(
                sid.clone(),
                SeatInfo {
                    seat: seat.clone(),
                    pid: Some(e.pid),
                },
            );
        }
    }
    out
}

/// One evidence source's state. `unavailable` and `partial` are never read as
/// "nothing wrong".
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct SourceStatus {
    pub(crate) source: String,
    pub(crate) path: String,
    /// `available` | `unavailable` | `not-read`
    pub(crate) state: String,
    pub(crate) detail: String,
}

/// What the run could and could not see.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Coverage {
    pub(crate) sources: Vec<SourceStatus>,
    pub(crate) files_scanned: usize,
    pub(crate) bytes_read: u64,
    pub(crate) sessions: usize,
    /// Sessions whose logs carry no timestamps: hourly rules are unknown.
    pub(crate) sessions_timing_unknown: usize,
    /// A byte or time budget stopped the run before all evidence was read.
    pub(crate) partial: bool,
    pub(crate) elapsed_ms: u64,
    /// `ok` (evidence read, nothing tripped), `tripped`, or `unknown`.
    pub(crate) verdict: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Watermarks {
    #[serde(default)]
    files: BTreeMap<String, u64>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Call {
    ts: Option<DateTime<Utc>>,
    tokens: u64,
    context: u64,
}

#[derive(Debug, Default)]
struct Session {
    calls: HashMap<String, Call>,
    wakes: Vec<DateTime<Utc>>,
    /// Injected-prompt key -> (preview, fire times).
    injected: BTreeMap<String, (String, Vec<DateTime<Utc>>)>,
    /// Injected-prompt key -> declared cron interval in minutes.
    declared_minutes: HashMap<String, u64>,
    replies: Vec<(DateTime<Utc>, bool)>,
    compactions: Vec<DateTime<Utc>>,
    poll_setups: Vec<(DateTime<Utc>, String)>,
    timed: bool,
    /// Latest file mtime among this session's untimed files.
    untimed_mtime: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// One assistant content block. A tool call's `input` is deliberately not a
/// field, so serde skips it unparsed: it can be a whole file, and only the
/// tool's name matters here.
#[derive(Deserialize, Default)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    name: Option<String>,
}

/// Message content: a plain string (a prompt) or a list of blocks.
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize, Default)]
struct Message {
    id: Option<String>,
    stop_reason: Option<String>,
    usage: Option<Usage>,
    content: Option<Content>,
}

#[derive(Deserialize, Default)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    subtype: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_camel: Option<String>,
    session_id: Option<String>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    #[serde(rename = "promptSource")]
    prompt_source: Option<String>,
    #[serde(rename = "isSidechain")]
    is_sidechain: Option<bool>,
    message: Option<Message>,
    cron: Option<String>,
    prompt: Option<String>,
}

/// Collapse whitespace and keep a stable prefix, so the same tick prompt with
/// a different trailing timestamp still keys together.
fn prompt_key(text: &str) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(80).collect()
}

/// Rough interval of a 5-field cron expression's minute field, when the hour
/// field is `*`. `None` when the cadence cannot be read cheaply.
fn cron_interval_minutes(cron: &str) -> Option<u64> {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() < 5 || fields[1] != "*" {
        return None;
    }
    let minute = fields[0];
    if minute == "*" {
        return Some(1);
    }
    if let Some(step) = minute.strip_prefix("*/") {
        return step.parse().ok().filter(|s| *s > 0);
    }
    let n = minute.split(',').count() as u64;
    (n > 0).then(|| 60 / n)
}

fn mentions_mailbox(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("mailbox") || t.contains("inbox") || t.contains("mail tick")
}

fn text_of(content: &Content) -> Option<String> {
    match content {
        Content::Text(s) => Some(s.clone()),
        Content::Blocks(items) => {
            if items
                .iter()
                .any(|i| i.kind.as_deref() == Some("tool_result"))
            {
                return None;
            }
            let parts: Vec<&str> = items
                .iter()
                .filter(|i| i.kind.as_deref() == Some("text"))
                .filter_map(|i| i.text.as_deref())
                .collect();
            (!parts.is_empty()).then(|| parts.join("\n"))
        }
    }
}

/// Cheap byte-level gate before any JSON parse: only assistant, user and
/// system lines carry evidence, and a user line carrying a tool result (the
/// bulk of a transcript's bytes) is never a wake.
fn worth_parsing(line: &[u8]) -> bool {
    let has = |needle: &[u8]| line.windows(needle.len()).any(|w| w == needle);
    if has(b"\"type\":\"assistant\"") || has(b"\"type\":\"system\"") {
        return true;
    }
    has(b"\"type\":\"user\"") && !has(b"\"type\":\"tool_result\"")
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

fn walk_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            walk_jsonl(&path, out);
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

struct Budget {
    started: Instant,
    bytes: u64,
    partial: bool,
}

impl Budget {
    fn exhausted(&mut self, policy: &WatchdogPolicy) -> bool {
        if self.bytes >= policy.max_total_bytes
            || self.started.elapsed().as_millis() as u64 >= policy.time_budget_ms
        {
            self.partial = true;
        }
        self.partial
    }
}

/// Fold one line into its session. `fallback_session` names sessions whose
/// lines carry no id (the file stem).
fn fold_line(
    line: &Line,
    raw: &[u8],
    fallback_session: &str,
    policy: &WatchdogPolicy,
    day_start: DateTime<Utc>,
    now: DateTime<Utc>,
    sessions: &mut HashMap<String, Session>,
) -> Option<DateTime<Utc>> {
    let ts = line
        .timestamp
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc));
    if let Some(t) = ts {
        if t < day_start || t > now {
            return ts;
        }
    }
    let sid = line
        .session_camel
        .clone()
        .or_else(|| line.session_id.clone())
        .unwrap_or_else(|| fallback_session.to_string());
    let s = sessions.entry(sid).or_default();
    if ts.is_some() {
        s.timed = true;
    }
    match line.kind.as_deref() {
        Some("assistant") => {
            let Some(msg) = &line.message else { return ts };
            if let Some(u) = &msg.usage {
                let context =
                    u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
                let key = msg
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("#{}", s.calls.len()));
                s.calls.entry(key).or_insert(Call {
                    ts,
                    tokens: context + u.output_tokens,
                    context,
                });
            }
            if let (Some(t), Some(Content::Blocks(items))) = (ts, &msg.content) {
                for item in items {
                    match item.kind.as_deref() {
                        Some("text") if msg.stop_reason.as_deref() == Some("end_turn") => {
                            let text = item.text.as_deref().unwrap_or("");
                            let noop = text.trim().chars().count() <= policy.noop_reply_max_chars;
                            s.replies.push((t, noop));
                        }
                        Some("tool_use") => {
                            let name = item.name.as_deref().unwrap_or("");
                            // The input was skipped unparsed; the raw line
                            // holds it, and a poll prompt names the mailbox.
                            if matches!(name, "CronCreate" | "ScheduleWakeup")
                                && mentions_mailbox(&String::from_utf8_lossy(raw))
                            {
                                s.poll_setups.push((t, name.to_string()));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Some("user") => {
            let (Some(t), Some(msg)) = (ts, &line.message) else {
                return ts;
            };
            if line.is_sidechain == Some(true) {
                return ts;
            }
            let Some(text) = msg.content.as_ref().and_then(text_of) else {
                return ts;
            };
            s.wakes.push(t);
            let injected = line.is_meta == Some(true)
                || matches!(line.prompt_source.as_deref(), Some(src) if src != "typed" && src != "queued");
            if injected {
                let key = prompt_key(&text);
                s.injected
                    .entry(key.clone())
                    .or_insert_with(|| (key.clone(), Vec::new()))
                    .1
                    .push(t);
            } else if text.trim_start().starts_with("/loop") && mentions_mailbox(&text) {
                s.poll_setups.push((t, "/loop".to_string()));
            }
        }
        Some("system") => {
            let Some(t) = ts else { return ts };
            match line.subtype.as_deref() {
                Some("compact_boundary") => s.compactions.push(t),
                Some("scheduled_task_fire") => {
                    let prompt = line.prompt.as_deref().unwrap_or("");
                    let key = prompt_key(prompt);
                    if let Some(mins) = line.cron.as_deref().and_then(cron_interval_minutes) {
                        s.declared_minutes.insert(key, mins);
                    }
                    if mentions_mailbox(prompt) {
                        s.poll_setups.push((t, "scheduled task".to_string()));
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    ts
}

/// Read `path` from its watermark (or a bounded tail on first sight) to EOF,
/// folding every in-window line. Returns the new watermark: the offset of the
/// first line still inside the day window, so the next tick starts there.
#[allow(clippy::too_many_arguments)]
fn scan_file(
    path: &Path,
    start: Option<u64>,
    policy: &WatchdogPolicy,
    day_start: DateTime<Utc>,
    now: DateTime<Utc>,
    sessions: &mut HashMap<String, Session>,
    budget: &mut Budget,
) -> std::io::Result<u64> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let (mut offset, skip_partial) = match start {
        Some(w) if w <= len => (w, false),
        _ if len > policy.max_bytes_per_file => (len - policy.max_bytes_per_file, true),
        _ => (0, false),
    };
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    if skip_partial {
        offset += reader.read_until(b'\n', &mut buf)? as u64;
        buf.clear();
    }
    let fallback = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut watermark: Option<u64> = None;
    let mut saw_timestamp = false;
    loop {
        if budget.exhausted(policy) {
            break;
        }
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        budget.bytes += n as u64;
        let line_start = offset;
        offset += n as u64;
        if n > MAX_LINE_BYTES || !worth_parsing(&buf) {
            continue;
        }
        let Ok(line) = serde_json::from_slice::<Line>(&buf) else {
            continue;
        };
        if let Some(ts) = fold_line(&line, &buf, &fallback, policy, day_start, now, sessions) {
            saw_timestamp = true;
            if watermark.is_none() && ts >= day_start {
                watermark = Some(line_start);
            }
        }
    }
    if !saw_timestamp {
        // Nothing new was timestamped. A file resumed from a watermark keeps
        // its place; an untimed stream-json file has nothing to window on, so
        // it is re-read next time (one-shot sessions, small next to
        // transcripts).
        return Ok(if start.is_some_and(|w| w > 0) {
            offset
        } else {
            0
        });
    }
    Ok(watermark.unwrap_or(offset))
}

fn load_watermarks(path: Option<&Path>) -> Watermarks {
    path.and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_watermarks(path: Option<&Path>, marks: &Watermarks) {
    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(marks) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, body).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.2}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Run the watchdog once. Pure over its inputs except for reading the
/// sources and writing the watermark file.
// trace:STORY-1462 | ai:claude
pub(crate) fn scan(
    project_label: &str,
    sources: &Sources,
    policy: &WatchdogPolicy,
    now: DateTime<Utc>,
    seats: &HashMap<String, SeatInfo>,
) -> (Vec<DoctorFinding>, Coverage) {
    let window_start = now - Duration::minutes(policy.window_minutes);
    let day_start = now - Duration::hours(DAY_HOURS);
    let mut coverage = Coverage::default();
    let mut budget = Budget {
        started: Instant::now(),
        bytes: 0,
        partial: false,
    };
    let mut marks = load_watermarks(sources.watermark_path.as_deref());
    let mut next_marks = Watermarks::default();
    let mut sessions: HashMap<String, Session> = HashMap::new();

    let groups = [
        ("claude-transcripts", &sources.transcript_dirs),
        ("headless-logs", &sources.headless_dirs),
    ];
    let mut absent_worktree_slugs = 0usize;
    for (label, dirs) in groups {
        for (i, dir) in dirs.iter().enumerate() {
            // Most worktrees never host a session of their own; only the
            // project root's slug being absent is worth a line of its own.
            if label == "claude-transcripts" && i > 0 && !dir.is_dir() {
                absent_worktree_slugs += 1;
                continue;
            }
            if !dir.is_dir() {
                coverage.sources.push(SourceStatus {
                    source: label.to_string(),
                    path: dir.display().to_string(),
                    state: "unavailable".into(),
                    detail: "directory not present".into(),
                });
                continue;
            }
            let mut files = Vec::new();
            walk_jsonl(dir, &mut files);
            files.sort();
            let mut read = 0usize;
            for file in files {
                let Some(mtime) = file_mtime(&file) else {
                    continue;
                };
                if mtime < day_start {
                    continue;
                }
                if budget.exhausted(policy) {
                    break;
                }
                let key = file.display().to_string();
                match scan_file(
                    &file,
                    marks.files.remove(&key),
                    policy,
                    day_start,
                    now,
                    &mut sessions,
                    &mut budget,
                ) {
                    Ok(mark) => {
                        read += 1;
                        next_marks.files.insert(key, mark);
                    }
                    Err(_) => continue,
                }
                for s in sessions.values_mut().filter(|s| !s.timed) {
                    if s.untimed_mtime.is_none() {
                        s.untimed_mtime = Some(mtime);
                    }
                }
            }
            coverage.files_scanned += read;
            coverage.sources.push(SourceStatus {
                source: label.to_string(),
                path: dir.display().to_string(),
                state: "available".into(),
                detail: format!("{read} file(s) touched in the last 24h"),
            });
        }
    }
    if absent_worktree_slugs > 0 {
        coverage.sources.push(SourceStatus {
            source: "claude-transcripts".into(),
            path: "(worktree slugs)".into(),
            state: "unavailable".into(),
            detail: format!("{absent_worktree_slugs} worktree(s) have no transcript directory"),
        });
    }
    coverage.sources.push(SourceStatus {
        source: "usage-jsonl".into(),
        path: "~/.aida/usage.jsonl".into(),
        state: "not-read".into(),
        detail: "machine-global and carries no session id, so it cannot attribute a rate to a seat"
            .into(),
    });
    coverage.sources.push(SourceStatus {
        source: "codex-sessions".into(),
        path: "~/.codex/sessions".into(),
        state: "not-read".into(),
        detail: "not project-scoped; Codex seats are not covered by this check".into(),
    });
    save_watermarks(sources.watermark_path.as_deref(), &next_marks);

    coverage.bytes_read = budget.bytes;
    coverage.partial = budget.partial;
    coverage.sessions = sessions.len();
    coverage.sessions_timing_unknown = sessions.values().filter(|s| !s.timed).count();
    coverage.elapsed_ms = budget.started.elapsed().as_millis() as u64;

    let mut findings = Vec::new();
    let window_label = format!(
        "{}m window ending {}",
        policy.window_minutes,
        now.format("%Y-%m-%dT%H:%MZ")
    );
    let mut day_tokens: u64 = 0;
    let mut ids: Vec<&String> = sessions.keys().collect();
    ids.sort();
    for sid in ids {
        let s = &sessions[sid];
        let who = match seats.get(sid) {
            Some(info) => format!(
                "session {sid} (seat {}{}) in {project_label}",
                info.seat,
                info.pid.map(|p| format!(", pid {p}")).unwrap_or_default()
            ),
            None => format!("session {sid} (seat unresolved) in {project_label}"),
        };
        let mut push = |rule: &str, summary: String, action: &str| {
            findings.push(DoctorFinding {
                category: CATEGORY.to_string(),
                id: format!("{rule}:{sid}"),
                summary: format!("{rule}: {who}: {summary}"),
                action: action.to_string(),
                safe_heal: false,
            });
        };
        if !s.timed {
            if s.untimed_mtime.is_some_and(|m| m >= day_start) {
                day_tokens += s.calls.values().map(|c| c.tokens).sum::<u64>();
            }
            continue;
        }
        let in_window = |t: &DateTime<Utc>| *t >= window_start && *t <= now;
        day_tokens += s
            .calls
            .values()
            .filter(|c| c.ts.is_some())
            .map(|c| c.tokens)
            .sum::<u64>();

        let wakes = s.wakes.iter().filter(|t| in_window(t)).count() as u64;
        if wakes > policy.max_wakes_per_hour {
            push(
                "wake-rate",
                format!(
                    "{wakes} wakes in the {window_label} (threshold {})",
                    policy.max_wakes_per_hour
                ),
                "find what is waking this seat (a cron/loop/tick) and stop it; see `aida doctor check runaway-seats` for the repeated prompt",
            );
        }
        let window_calls: Vec<&Call> = s
            .calls
            .values()
            .filter(|c| c.ts.as_ref().is_some_and(in_window))
            .collect();
        let tokens: u64 = window_calls.iter().map(|c| c.tokens).sum();
        if tokens > policy.max_tokens_per_hour {
            push(
                "token-rate",
                format!(
                    "{} tokens over {} calls in the {window_label} (threshold {})",
                    fmt_tokens(tokens),
                    window_calls.len(),
                    fmt_tokens(policy.max_tokens_per_hour)
                ),
                "stop or restart the seat; it is spending faster than any interactive session should",
            );
        }
        for (key, (preview, times)) in &s.injected {
            let count = times.iter().filter(|t| in_window(t)).count() as u64;
            let declared = s.declared_minutes.get(key).copied();
            let expected = declared.map(|m| (policy.window_minutes as u64 / m.max(1)).max(1));
            let over_declared = expected.is_some_and(|e| count > e * 2 + 1);
            if count > policy.max_repeated_prompts_per_hour || over_declared {
                let declared_note = match (declared, expected) {
                    (Some(m), Some(e)) => format!(", declared every {m}m so ~{e} expected"),
                    _ => String::new(),
                };
                push(
                    "repeated-prompt",
                    format!(
                        "injected prompt \"{preview}\" fired {count} times in the {window_label} (threshold {}{declared_note})",
                        policy.max_repeated_prompts_per_hour
                    ),
                    "delete the cron/loop that injects this prompt; mail is delivered by hooks, not model-side polling",
                );
            }
        }
        let replies: Vec<bool> = s
            .replies
            .iter()
            .filter(|(t, _)| in_window(t))
            .map(|(_, noop)| *noop)
            .collect();
        let total = replies.len() as u64;
        let noop = replies.iter().filter(|n| **n).count() as u64;
        if total >= policy.idle_min_replies && noop * 100 > policy.max_idle_ratio_pct * total {
            push(
                "idle-ratio",
                format!(
                    "{noop} of {total} replies ({}%) were no-ops in the {window_label} (threshold {}%)",
                    noop * 100 / total.max(1),
                    policy.max_idle_ratio_pct
                ),
                "the seat is being woken with nothing to do; stop the tick that wakes it",
            );
        }
        if let Some(latest) = window_calls
            .iter()
            .max_by_key(|c| c.ts)
            .filter(|c| c.context > policy.restart_context_tokens)
        {
            push(
                "restart-recommended",
                format!(
                    "per-call context is {} (ceiling {}, `[watchdog] restart_context_tokens`)",
                    fmt_tokens(latest.context),
                    fmt_tokens(policy.restart_context_tokens)
                ),
                "run /aida-handoff in that seat, then start a fresh session",
            );
        }
        let compactions = s.compactions.len() as u64;
        if compactions > policy.max_compactions_per_day {
            push(
                "compactions",
                format!(
                    "{compactions} auto-compactions in the last 24h (threshold {})",
                    policy.max_compactions_per_day
                ),
                "run /aida-handoff in that seat, then start a fresh session",
            );
        }
        if let Some((t, how)) = s.poll_setups.iter().max_by_key(|(t, _)| *t) {
            push(
                "mail-poll-setup",
                format!(
                    "a model-side mailbox poll ({how}) was active at {} (forbidden since BUG-1589)",
                    t.format("%Y-%m-%dT%H:%MZ")
                ),
                "delete the cron/loop in that seat; unread mail reaches a seat through the prompt hook",
            );
        }
    }

    let alert_at = policy.daily_token_budget / 100 * policy.budget_alert_pct;
    if day_tokens >= alert_at && day_tokens > 0 {
        findings.push(DoctorFinding {
            category: CATEGORY.to_string(),
            id: "daily-budget".to_string(),
            summary: format!(
                "daily-budget: {project_label} spent {} tokens in the last 24h, {}% of the {} budget (alert at {}%)",
                fmt_tokens(day_tokens),
                day_tokens.saturating_mul(100) / policy.daily_token_budget.max(1),
                fmt_tokens(policy.daily_token_budget),
                policy.budget_alert_pct
            ),
            action: "find the session carrying the spend (`aida tokens` / the findings above) before the budget is gone".to_string(),
            safe_heal: false,
        });
    }

    let any_available = coverage.sources.iter().any(|s| s.state == "available");
    if !any_available || coverage.partial {
        let summary = if !any_available {
            "evidence unknown: no session transcript or headless log directory was readable, so no seat could be judged — this is not a clean result".to_string()
        } else {
            format!(
                "evidence partial: the run stopped at its byte/time budget after {} file(s) and {} — later sessions were not judged",
                coverage.files_scanned,
                fmt_tokens(coverage.bytes_read)
            )
        };
        findings.push(DoctorFinding {
            category: CATEGORY.to_string(),
            id: if any_available {
                "evidence-partial"
            } else {
                "evidence-unknown"
            }
            .to_string(),
            summary,
            action: "check the evidence paths in the coverage report, or raise `[watchdog] max_total_bytes` / `time_budget_ms` deliberately".to_string(),
            safe_heal: false,
        });
        coverage.verdict = "unknown".into();
    } else if findings.is_empty() {
        coverage.verdict = "ok".into();
    } else {
        coverage.verdict = "tripped".into();
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    (findings, coverage)
}

/// Text rendering of the coverage block, printed under the doctor report.
pub(crate) fn render_coverage(c: &Coverage) -> String {
    let mut out = format!(
        "runaway-seats evidence: verdict {} — {} file(s), {} bytes, {} session(s), {} with unknown timing{} ({} ms)\n",
        c.verdict,
        c.files_scanned,
        c.bytes_read,
        c.sessions,
        c.sessions_timing_unknown,
        if c.partial { ", PARTIAL" } else { "" },
        c.elapsed_ms
    );
    for s in &c.sources {
        out.push_str(&format!(
            "  {:<11} {:<18} {} — {}\n",
            s.state, s.source, s.path, s.detail
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn t0() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-21T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn ts(offset_secs: i64) -> String {
        (t0() + Duration::seconds(offset_secs)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    const TICK: &str = "Advisor mail tick: run `aida mailbox inbox --unread` and `aida awaiting --notice` in the repo. If nothing is unread, reply \"mail tick: quiet\".";

    /// A transcript shaped like the BUG-1589 storm sessions: a mail tick
    /// every 8 seconds, each answered with "mail tick: quiet" on a ~580k
    /// context, plus the declared-every-10m scheduled-task fire and the
    /// CronCreate that set it up.
    fn storm_transcript(session: &str, ticks: i64) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}\n",
            serde_json::json!({"type":"assistant","timestamp":ts(0),"sessionId":session,
                "message":{"id":format!("{session}-cron"),"stop_reason":"tool_use",
                "usage":{"input_tokens":5,"cache_read_input_tokens":500000,"cache_creation_input_tokens":1000,"output_tokens":50},
                "content":[{"type":"tool_use","name":"CronCreate","input":{"cron":"7,17,27,37,47,57 * * * *","prompt":TICK}}]}})
        ));
        for i in 0..ticks {
            let at = 8 * i + 1;
            out.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"system","subtype":"scheduled_task_fire","timestamp":ts(at),"sessionId":session,
                    "cron":"7,17,27,37,47,57 * * * *","prompt":TICK})
            ));
            out.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"user","timestamp":ts(at),"sessionId":session,"isMeta":true,"promptSource":"system",
                    "message":{"role":"user","content":TICK}})
            ));
            out.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"assistant","timestamp":ts(at + 3),"sessionId":session,
                    "message":{"id":format!("{session}-{i}"),"stop_reason":"end_turn",
                    "usage":{"input_tokens":3,"cache_read_input_tokens":580000,"cache_creation_input_tokens":2000,"output_tokens":20},
                    "content":[{"type":"text","text":"mail tick: quiet"}]}})
            ));
        }
        out
    }

    /// An ordinary working seat: a typed prompt every ~6 minutes, tool-use
    /// heavy, modest context.
    fn calm_transcript(session: &str) -> String {
        let mut out = String::new();
        for i in 0..10 {
            let at = 360 * i;
            out.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"user","timestamp":ts(at),"sessionId":session,"promptSource":"typed",
                    "message":{"role":"user","content":format!("please implement part {i}")}})
            ));
            for j in 0..8 {
                out.push_str(&format!(
                    "{}\n",
                    serde_json::json!({"type":"assistant","timestamp":ts(at + 10 + j),"sessionId":session,
                        "message":{"id":format!("{session}-{i}-{j}"),"stop_reason":if j == 7 {"end_turn"} else {"tool_use"},
                        "usage":{"input_tokens":3,"cache_read_input_tokens":90000,"cache_creation_input_tokens":2000,"output_tokens":400},
                        "content":[if j == 7 {serde_json::json!({"type":"text","text":"Implemented the change, added tests, and verified that the suite passes."})} else {serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"cargo test"}})}]}})
                ));
            }
        }
        out
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    fn sources(root: &Path) -> Sources {
        Sources {
            transcript_dirs: vec![root.join("claude")],
            headless_dirs: vec![root.join("headless")],
            watermark_path: Some(root.join("state").join("watermarks.json")),
        }
    }

    fn rules(findings: &[DoctorFinding], sid: &str) -> Vec<String> {
        let mut r: Vec<String> = findings
            .iter()
            .filter(|f| f.id.ends_with(sid))
            .filter_map(|f| f.id.split_once(':').map(|(rule, _)| rule.to_string()))
            .collect();
        r.sort();
        r
    }

    /// Acceptance 2: replaying the storm shape for both BUG-1589 sessions
    /// trips wake-rate, token-rate, repeated-prompt and idle-ratio on each
    /// within the first simulated hour.
    // trace:STORY-1462 | ai:claude
    #[test]
    fn storm_replay_trips_every_rate_rule_on_both_sessions_within_the_first_hour() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "01a0bfde-0000-7000-8000-000000000001";
        let b = "01a0adee-16e1-7982-ac9f-e15abb207b6c";
        write(
            &tmp.path().join("claude"),
            &format!("{a}.jsonl"),
            &storm_transcript(a, 450),
        );
        write(
            &tmp.path().join("claude"),
            &format!("{b}.jsonl"),
            &storm_transcript(b, 450),
        );
        let mut seats = HashMap::new();
        seats.insert(
            b.to_string(),
            SeatInfo {
                seat: "advisor".into(),
                pid: Some(4242),
            },
        );
        let now = t0() + Duration::minutes(60);
        let (findings, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &WatchdogPolicy::default(),
            now,
            &seats,
        );
        for sid in [a, b] {
            let got = rules(&findings, sid);
            for rule in [
                "idle-ratio",
                "mail-poll-setup",
                "repeated-prompt",
                "restart-recommended",
                "token-rate",
                "wake-rate",
            ] {
                assert!(
                    got.contains(&rule.to_string()),
                    "{sid}: {rule} missing from {got:?}"
                );
            }
        }
        assert_eq!(coverage.verdict, "tripped");
        let wake = findings
            .iter()
            .find(|f| f.id == format!("wake-rate:{b}"))
            .unwrap();
        assert!(
            wake.summary.contains("seat advisor, pid 4242"),
            "{}",
            wake.summary
        );
        assert!(wake.summary.contains("in aida"));
        assert!(wake.summary.contains("threshold 120"));
        assert!(findings
            .iter()
            .find(|f| f.id == format!("repeated-prompt:{a}"))
            .unwrap()
            .summary
            .contains("declared every 10m"));
    }

    #[test]
    fn a_busy_ordinary_seat_is_silent_and_reported_ok() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("claude"),
            "calm.jsonl",
            &calm_transcript("calm"),
        );
        fs::create_dir_all(tmp.path().join("headless")).unwrap();
        let now = t0() + Duration::minutes(60);
        let (findings, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &WatchdogPolicy::default(),
            now,
            &HashMap::new(),
        );
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(coverage.verdict, "ok");
        assert_eq!(coverage.sessions, 1);
    }

    /// Unavailable evidence is reported as unknown, never as ok.
    #[test]
    fn missing_evidence_is_unknown_not_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let (findings, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &WatchdogPolicy::default(),
            t0(),
            &HashMap::new(),
        );
        assert_eq!(coverage.verdict, "unknown");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, "evidence-unknown");
        assert!(coverage.sources.iter().all(|s| s.state != "available"));
    }

    /// A budget-cut run is partial evidence, and says so.
    #[test]
    fn a_budget_cut_run_is_partial_not_clean() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("claude"),
            "calm.jsonl",
            &calm_transcript("calm"),
        );
        let policy = WatchdogPolicy {
            max_total_bytes: 64,
            ..WatchdogPolicy::default()
        };
        let (findings, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &policy,
            t0() + Duration::minutes(60),
            &HashMap::new(),
        );
        assert!(coverage.partial);
        assert_eq!(coverage.verdict, "unknown");
        assert!(findings.iter().any(|f| f.id == "evidence-partial"));
    }

    /// Untimed headless stream-json counts toward the daily budget but its
    /// hourly rates are not judged (reported as unknown timing).
    #[test]
    fn untimed_headless_logs_feed_the_budget_and_report_unknown_timing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("claude")).unwrap();
        let mut body = String::new();
        for i in 0..20 {
            body.push_str(&format!(
                "{}\n",
                serde_json::json!({"type":"assistant","session_id":"headless-1",
                    "message":{"id":format!("m{i}"),"usage":{"input_tokens":1,"cache_read_input_tokens":60_000_000,"output_tokens":1}}})
            ));
        }
        write(&tmp.path().join("headless"), "task-1-work.jsonl", &body);
        let policy = WatchdogPolicy {
            daily_token_budget: 1_000_000_000,
            ..WatchdogPolicy::default()
        };
        let (findings, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &policy,
            Utc::now(),
            &HashMap::new(),
        );
        assert_eq!(coverage.sessions_timing_unknown, 1);
        assert!(
            findings.iter().any(|f| f.id == "daily-budget"),
            "{findings:?}"
        );
        assert!(!findings.iter().any(|f| f.id.starts_with("token-rate")));
    }

    /// Acceptance 6: incremental reads. After a first run the watermark sits
    /// at the first in-window line, so a second run over an unchanged file
    /// with an advanced clock reads strictly less, and appended lines are
    /// still seen.
    // trace:STORY-1462 | ai:claude
    #[test]
    fn watermark_makes_the_next_tick_incremental() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("claude");
        let path = write(&dir, "s.jsonl", &storm_transcript("s", 300));
        let src = sources(tmp.path());
        let policy = WatchdogPolicy::default();
        let (_, first) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(50),
            &HashMap::new(),
        );
        // The clock moves past the whole file's day window: nothing is left
        // in-window, so the watermark lands at EOF.
        let later = t0() + Duration::hours(30);
        let (_, second) = scan("aida", &src, &policy, later, &HashMap::new());
        let len = fs::metadata(&path).unwrap().len();
        assert_eq!(first.bytes_read, len);
        assert_eq!(
            second.bytes_read, len,
            "first sight of a stale-window file reads it once"
        );
        let (_, third) = scan("aida", &src, &policy, later, &HashMap::new());
        assert_eq!(
            third.bytes_read, 0,
            "an unchanged file past its window costs nothing"
        );
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        let extra = format!(
            "{}\n",
            serde_json::json!({"type":"user","timestamp":(later - Duration::minutes(1)).to_rfc3339(),"sessionId":"s","message":{"content":"hi"}})
        );
        f.write_all(extra.as_bytes()).unwrap();
        let (_, fourth) = scan("aida", &src, &policy, later, &HashMap::new());
        assert_eq!(fourth.bytes_read, extra.len() as u64);
    }

    /// Acceptance 6: bounded runtime on a large transcript set, and zero
    /// model calls — no process spawn, HTTP client or vendor SDK in the
    /// watchdog itself.
    // trace:STORY-1462 | ai:claude
    #[test]
    fn the_watchdog_is_bounded_and_spawns_nothing() {
        let src = include_str!("runaway_seats.rs");
        let body = src.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["Command::new", "std::process", "reqwest", "anthropic"] {
            assert!(
                !body.contains(forbidden),
                "watchdog must stay zero-token: found {forbidden}"
            );
        }
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("claude");
        let one = storm_transcript("big", 2_000);
        for i in 0..10 {
            write(&dir, &format!("big-{i}.jsonl"), &one);
        }
        let started = Instant::now();
        let (_, coverage) = scan(
            "aida",
            &sources(tmp.path()),
            &WatchdogPolicy::default(),
            t0() + Duration::minutes(60),
            &HashMap::new(),
        );
        assert!(!coverage.partial);
        assert!(
            started.elapsed().as_secs() < 20,
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn policy_reads_the_watchdog_section_and_keeps_defaults_for_bad_values() {
        assert_eq!(policy(None), WatchdogPolicy::default());
        let cfg: toml::Value = toml::from_str(
            "[watchdog]\nmax_wakes_per_hour = 60\nrestart_context_tokens = 200000\nbudget_alert_pct = 150\nmax_idle_ratio_pct = -5\n",
        )
        .unwrap();
        let p = policy(Some(&cfg));
        assert_eq!(p.max_wakes_per_hour, 60);
        assert_eq!(p.restart_context_tokens, 200_000);
        assert_eq!(p.budget_alert_pct, 50);
        assert_eq!(p.max_idle_ratio_pct, 80);
    }

    #[test]
    fn cron_interval_reads_common_minute_fields() {
        assert_eq!(cron_interval_minutes("7,17,27,37,47,57 * * * *"), Some(10));
        assert_eq!(cron_interval_minutes("*/5 * * * *"), Some(5));
        assert_eq!(cron_interval_minutes("* * * * *"), Some(1));
        assert_eq!(cron_interval_minutes("0 9 * * *"), None);
    }
}
