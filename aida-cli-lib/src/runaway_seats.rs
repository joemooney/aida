//! Runaway-seat watchdog: the `runaway-seats` doctor category.
//!
//! BUG-1589: two Claude seats answered a mail tick every ~7-10s — ~460 wakes
//! and ~700M tokens an hour each — and nothing noticed for ~24h. Every
//! signal needed was already on disk. This module reads it and judges it,
//! without a model call.
//!
//! Evidence:
//! - Claude Code transcripts under `~/.claude/projects/<slug>/**/*.jsonl` for
//!   the project root AND every registered worktree. These carry timestamps,
//!   per-request usage, the injected prompts (`isMeta` / `promptSource`),
//!   scheduled-task fires (with their declared cron) and compaction
//!   boundaries.
//! - Headless session logs under `.aida/headless-logs/` and `.aida/burndown/`
//!   (stream-json). These usually carry NO per-line timestamps, so they count
//!   toward the daily budget at the file's mtime, and their hourly rates are
//!   reported as unknown, never as ok.
//! - The agent registry (`.aida/agents/*.toml`), only to name a seat and pid
//!   for a session id.
//!
//! Deliberately NOT read: `~/.aida/usage.jsonl` (machine-global, no session
//! id) and Codex sessions (`~/.codex/sessions`, not project-scoped). The
//! coverage report says so rather than implying they were checked.
//!
//! Calibration (backtest over 2026-09-17..23 of this repo's transcripts): an
//! ordinary lead seat peaked at 229M tokens/h, 34 wakes/h, 19 repeats of one
//! injected prompt per hour, 44% no-op replies, 3 compactions a day, and
//! routinely reached the ~967k auto-compaction context; the ordinary day spent
//! 2.25B. The storm sessions ran ~460 wakes/h, ~460 repeats/h, 99% no-op
//! replies, ~700M tokens/h and 6-7 compactions/day. The defaults sit between
//! with a wide margin on the ordinary side, and lean on the storm's SHAPE
//! (wakes, repeats, no-ops) rather than raw volume.
//!
//! State lives in `.aida/watchdog/state.json` (runtime, gitignored): a byte
//! offset per file and per-session per-minute aggregates for the trailing
//! day, so a tick reads only bytes appended since the last one; and the set
//! of rules currently tripped, so each rule reports once per crossing and not
//! again until it clears. A run cut short by its byte/time budget, or one
//! that could read nothing, is reported as degraded/unknown evidence in the
//! coverage block — never as ok, and never as a finding (a finding makes
//! `--fail-on-findings` page a seat, and a slow disk is not a runaway seat).
//! The module calls no model and makes no network request; its only
//! subprocess is the one `git worktree list` (in `aida_core::git_ops`) that
//! finds the worktree slugs.
// trace:STORY-1462 | ai:claude

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "runaway-seats";

/// Lines longer than this are skipped unparsed (a giant tool result carries
/// no usage worth reading and would dominate the run's cost).
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const DAY_MINUTES: i64 = 24 * 60;
const STATE_VERSION: u32 = 1;

/// Thresholds, read from `[watchdog]` in `.aida/config.toml`. See the module
/// doc for the backtest each default is placed against.
// trace:STORY-1462 | ai:claude
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatchdogPolicy {
    /// Trailing window the per-session rate rules judge.
    pub(crate) window_minutes: i64,
    /// New turns (prompts delivered to the model) per session per window.
    /// Ordinary peak 34, storm ~460.
    pub(crate) max_wakes_per_hour: u64,
    /// Tokens (uncached input + cache read + cache write + output) per
    /// session per window. Ordinary peak 229M, storm ~700M.
    pub(crate) max_tokens_per_hour: u64,
    /// Project-wide token budget per trailing 24h. Ordinary day 2.25B.
    pub(crate) daily_token_budget: u64,
    /// Alert once this share of the daily budget is spent: near the budget,
    /// before it is gone.
    pub(crate) budget_alert_pct: u64,
    /// The same injected (non-human) prompt per session per window.
    /// Ordinary peak 19, storm ~460.
    pub(crate) max_repeated_prompts_per_hour: u64,
    /// Share of end-of-turn replies that are no-ops, over the window.
    /// Ordinary peak 44%, storm 99%.
    pub(crate) max_idle_ratio_pct: u64,
    /// Minimum replies in the window before the idle ratio is judged.
    pub(crate) idle_min_replies: u64,
    /// A reply of at most this many characters counts as a no-op.
    pub(crate) noop_reply_max_chars: usize,
    /// Per-call context above which a restart is recommended. `0` = off, the
    /// default: every long ordinary session reaches the ~967k compaction
    /// point, so no ceiling is silent on an ordinary day. Opt in to trade
    /// one alert per session crossing for cheaper calls.
    pub(crate) restart_context_tokens: u64,
    /// Auto-compactions per session per trailing 24h. Ordinary peak 3,
    /// storm 6-7.
    pub(crate) max_compactions_per_day: u64,
    /// Bytes read from one file on first sight (older bytes are skipped).
    pub(crate) max_bytes_per_file: u64,
    /// Bytes read across all files in one run; the rest resumes next run.
    pub(crate) max_total_bytes: u64,
    /// Wall-clock budget for one run; the rest resumes next run.
    pub(crate) time_budget_ms: u64,
}

impl Default for WatchdogPolicy {
    fn default() -> Self {
        Self {
            window_minutes: 60,
            max_wakes_per_hour: 120,
            max_tokens_per_hour: 500_000_000,
            daily_token_budget: 6_000_000_000,
            budget_alert_pct: 90,
            max_repeated_prompts_per_hour: 60,
            max_idle_ratio_pct: 80,
            idle_min_replies: 20,
            noop_reply_max_chars: 60,
            restart_context_tokens: 0,
            max_compactions_per_day: 4,
            max_bytes_per_file: 256 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024,
            time_budget_ms: 10_000,
        }
    }
}

/// Read `[watchdog]`, keeping the default for any key that is absent,
/// non-integer or out of range. `restart_context_tokens = 0` is honoured
/// (it is the "off" value).
// trace:STORY-1462 | ai:claude
pub(crate) fn policy(cfg: Option<&toml::Value>) -> WatchdogPolicy {
    let mut p = WatchdogPolicy::default();
    let Some(section) = cfg.and_then(|c| c.get("watchdog")) else {
        return p;
    };
    let raw = |key: &str| section.get(key).and_then(|v| v.as_integer());
    let get = |key: &str| raw(key).filter(|v| *v > 0).map(|v| v as u64);
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
        max_compactions_per_day,
        max_bytes_per_file,
        max_total_bytes,
        time_budget_ms
    );
    if let Some(v) = raw("restart_context_tokens").filter(|v| *v >= 0) {
        p.restart_context_tokens = v as u64;
    }
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
    /// Claude Code project transcript directories; the first is the project
    /// root's own slug, the rest are worktree slugs.
    pub(crate) transcript_dirs: Vec<PathBuf>,
    /// Headless / burndown stream-json log directories.
    pub(crate) headless_dirs: Vec<PathBuf>,
    /// State file; `None` = stateless (full re-read, no dedupe).
    pub(crate) state_path: Option<PathBuf>,
}

/// The real sources for `project_root`.
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
        state_path: Some(
            project_root
                .join(".aida")
                .join("watchdog")
                .join("state.json"),
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

/// One evidence source's state. `unavailable` and `not-read` are never read
/// as "nothing wrong".
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct SourceStatus {
    pub(crate) source: String,
    pub(crate) path: String,
    /// `available` | `unavailable` | `not-read`
    pub(crate) state: String,
    pub(crate) detail: String,
}

/// What the run could and could not see, and what it is still holding.
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Coverage {
    pub(crate) sources: Vec<SourceStatus>,
    pub(crate) files_scanned: usize,
    /// New bytes read this run (a steady-state tick reads only appends).
    pub(crate) bytes_read: u64,
    pub(crate) sessions: usize,
    /// Sessions whose logs carry no timestamps: hourly rules are unknown.
    pub(crate) sessions_timing_unknown: usize,
    /// A byte or time budget stopped the run; the rest resumes next run.
    pub(crate) partial: bool,
    pub(crate) elapsed_ms: u64,
    /// Rules still tripped from an earlier run: already reported once, not
    /// reported again until they clear.
    pub(crate) still_active: Vec<String>,
    /// `ok` (evidence read, nothing tripped), `tripped`, `degraded` (the run
    /// was budget-cut; judged on what was read) or `unknown` (no evidence
    /// source was readable). Neither `degraded` nor `unknown` is a finding.
    pub(crate) verdict: String,
}

/// One minute of one session.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
struct Bucket {
    #[serde(default, skip_serializing_if = "is_zero32")]
    wakes: u32,
    #[serde(default, skip_serializing_if = "is_zero32")]
    calls: u32,
    #[serde(default, skip_serializing_if = "is_zero64")]
    tokens: u64,
    #[serde(default, skip_serializing_if = "is_zero32")]
    replies: u32,
    #[serde(default, skip_serializing_if = "is_zero32")]
    noop: u32,
    #[serde(default, skip_serializing_if = "is_zero32")]
    compactions: u32,
    #[serde(default, skip_serializing_if = "is_zero32")]
    polls: u32,
    /// Context of the latest call in this minute.
    #[serde(default, skip_serializing_if = "is_zero64")]
    last_ctx: u64,
    #[serde(default, skip_serializing_if = "is_zero64")]
    last_ctx_ts: u64,
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}
fn is_zero64(n: &u64) -> bool {
    *n == 0
}

/// A session's trailing-day aggregate. Prompt text is never stored: an
/// injected prompt is keyed by a short hash.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct SessionAgg {
    #[serde(default)]
    timed: bool,
    /// Last usage-bearing message id: Claude writes one line per content
    /// block, all carrying the same usage, consecutively.
    #[serde(default)]
    last_msg: Option<String>,
    #[serde(default)]
    minutes: BTreeMap<i64, Bucket>,
    /// Prompt hash -> minute -> count.
    #[serde(default)]
    prompts: BTreeMap<String, BTreeMap<i64, u32>>,
    /// Prompt hash -> declared cron interval in minutes.
    #[serde(default)]
    declared: BTreeMap<String, u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    version: u32,
    /// File path -> byte offset already folded.
    #[serde(default)]
    files: BTreeMap<String, u64>,
    #[serde(default)]
    sessions: BTreeMap<String, SessionAgg>,
    /// Finding ids tripped as of the last run.
    #[serde(default)]
    active: BTreeSet<String>,
}

fn load_state(path: Option<&Path>) -> State {
    path.and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<State>(&s).ok())
        .filter(|s| s.version == STATE_VERSION)
        .unwrap_or_default()
}

fn save_state(path: Option<&Path>, state: &State) {
    let Some(path) = path else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string(state) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, body).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
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

/// One content block. A tool call's `input` is deliberately not a field, so
/// serde skips it unparsed: it can be a whole file.
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

/// Short, content-free key for an injected prompt: a hash of its collapsed
/// leading text, so the same tick with a different trailing timestamp keys
/// together and the text itself is never stored or reported.
fn prompt_hash(text: &str) -> String {
    let collapsed: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(80)
        .collect();
    let digest = Sha256::digest(collapsed.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
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
    // One SIMD-accelerated pass (regex's literal prefilter) rather than a
    // byte-window compare per needle: tool-result lines run to megabytes.
    static TYPE_TAG: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let re = TYPE_TAG.get_or_init(|| {
        regex::bytes::Regex::new(r#""type":"(assistant|system|user|tool_result)""#)
            .expect("static regex")
    });
    let (mut user, mut tool_result) = (false, false);
    for cap in re.captures_iter(line) {
        match &cap[1] {
            b"assistant" | b"system" => return true,
            b"user" => user = true,
            _ => tool_result = true,
        }
    }
    user && !tool_result
}

fn minute_of(t: DateTime<Utc>) -> i64 {
    t.timestamp().div_euclid(60)
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

/// Fold one parsed line into the aggregates. `untimed_minute` is where a line
/// without a timestamp lands (the file's mtime minute).
fn fold_line(
    line: &Line,
    raw: &[u8],
    fallback_session: &str,
    policy: &WatchdogPolicy,
    day_start: DateTime<Utc>,
    untimed_minute: i64,
    sessions: &mut BTreeMap<String, SessionAgg>,
) {
    let ts = line
        .timestamp
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc));
    if ts.is_some_and(|t| t < day_start) {
        return;
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
    let minute = ts.map(minute_of).unwrap_or(untimed_minute);
    match line.kind.as_deref() {
        Some("assistant") => {
            let Some(msg) = &line.message else { return };
            if let Some(u) = &msg.usage {
                let fresh = msg.id.is_none() || msg.id != s.last_msg;
                if fresh {
                    s.last_msg = msg.id.clone();
                    let context =
                        u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
                    let b = s.minutes.entry(minute).or_default();
                    b.calls += 1;
                    b.tokens += context + u.output_tokens;
                    let at = ts.map(|t| t.timestamp().max(0) as u64).unwrap_or(0);
                    if at >= b.last_ctx_ts {
                        b.last_ctx_ts = at;
                        b.last_ctx = context;
                    }
                }
            }
            let (Some(_), Some(Content::Blocks(items))) = (ts, &msg.content) else {
                return;
            };
            for item in items {
                match item.kind.as_deref() {
                    Some("text") if msg.stop_reason.as_deref() == Some("end_turn") => {
                        let text = item.text.as_deref().unwrap_or("");
                        let b = s.minutes.entry(minute).or_default();
                        b.replies += 1;
                        if text.trim().chars().count() <= policy.noop_reply_max_chars {
                            b.noop += 1;
                        }
                    }
                    Some("tool_use") => {
                        let name = item.name.as_deref().unwrap_or("");
                        // The input was skipped unparsed; the raw line holds
                        // it, and a poll prompt names the mailbox.
                        if matches!(name, "CronCreate" | "ScheduleWakeup")
                            && mentions_mailbox(&String::from_utf8_lossy(raw))
                        {
                            s.minutes.entry(minute).or_default().polls += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        Some("user") => {
            let (Some(_), Some(msg)) = (ts, &line.message) else {
                return;
            };
            if line.is_sidechain == Some(true) {
                return;
            }
            let Some(text) = msg.content.as_ref().and_then(text_of) else {
                return;
            };
            s.minutes.entry(minute).or_default().wakes += 1;
            let injected = line.is_meta == Some(true)
                || matches!(line.prompt_source.as_deref(), Some(src) if src != "typed" && src != "queued");
            if injected {
                *s.prompts
                    .entry(prompt_hash(&text))
                    .or_default()
                    .entry(minute)
                    .or_default() += 1;
            } else if text.trim_start().starts_with("/loop") && mentions_mailbox(&text) {
                s.minutes.entry(minute).or_default().polls += 1;
            }
        }
        Some("system") => {
            if ts.is_none() {
                return;
            }
            match line.subtype.as_deref() {
                Some("compact_boundary") => s.minutes.entry(minute).or_default().compactions += 1,
                Some("scheduled_task_fire") => {
                    let prompt = line.prompt.as_deref().unwrap_or("");
                    if let Some(mins) = line.cron.as_deref().and_then(cron_interval_minutes) {
                        s.declared.insert(prompt_hash(prompt), mins);
                    }
                    if mentions_mailbox(prompt) {
                        s.minutes.entry(minute).or_default().polls += 1;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Fold `path` from `start` (or a bounded tail on first sight) up to the
/// last complete line, or until the run's budget is spent. Returns the byte
/// offset reached — the next run resumes exactly there.
fn scan_file(
    path: &Path,
    start: Option<u64>,
    policy: &WatchdogPolicy,
    day_start: DateTime<Utc>,
    sessions: &mut BTreeMap<String, SessionAgg>,
    budget: &mut Budget,
) -> std::io::Result<u64> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let (mut offset, skip_partial) = match start {
        Some(w) if w <= len => (w, false),
        // Shrunk below the recorded offset: rewritten, start over.
        Some(_) => (0, false),
        None if len > policy.max_bytes_per_file => (len - policy.max_bytes_per_file, true),
        None => (0, false),
    };
    if offset == len {
        return Ok(offset);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    if skip_partial {
        offset += reader.read_until(b'\n', &mut buf)? as u64;
    }
    let fallback = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let untimed_minute = file_mtime(path).map(minute_of).unwrap_or(0);
    loop {
        if budget.exhausted(policy) {
            break;
        }
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        // EOF, or a line still being written: stop before it.
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        budget.bytes += n as u64;
        offset += n as u64;
        if n > MAX_LINE_BYTES || !worth_parsing(&buf) {
            continue;
        }
        if let Ok(line) = serde_json::from_slice::<Line>(&buf) {
            fold_line(
                &line,
                &buf,
                &fallback,
                policy,
                day_start,
                untimed_minute,
                sessions,
            );
        }
    }
    Ok(offset)
}

fn fmt_tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.2}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.0}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// Drop everything older than the trailing day.
fn prune(sessions: &mut BTreeMap<String, SessionAgg>, day_start_minute: i64) {
    for s in sessions.values_mut() {
        s.minutes = s.minutes.split_off(&day_start_minute);
        for per_minute in s.prompts.values_mut() {
            *per_minute = per_minute.split_off(&day_start_minute);
        }
        s.prompts.retain(|_, m| !m.is_empty());
    }
    sessions.retain(|_, s| !s.minutes.is_empty() || !s.prompts.is_empty());
}

/// Every rule currently tripped, as (id, summary, action).
fn judge(
    project_label: &str,
    sessions: &BTreeMap<String, SessionAgg>,
    policy: &WatchdogPolicy,
    now: DateTime<Utc>,
    seats: &HashMap<String, SeatInfo>,
) -> Vec<DoctorFinding> {
    let now_minute = minute_of(now);
    let window_start = now_minute - policy.window_minutes + 1;
    let day_start = now_minute - DAY_MINUTES + 1;
    let window_label = format!(
        "{}m window ending {}",
        policy.window_minutes,
        now.format("%Y-%m-%dT%H:%MZ")
    );
    let mut findings = Vec::new();
    let mut day_tokens: u64 = 0;
    for (sid, s) in sessions {
        day_tokens += s
            .minutes
            .range(day_start..=now_minute)
            .map(|(_, b)| b.tokens)
            .sum::<u64>();
        if !s.timed {
            continue;
        }
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
        let window: Vec<&Bucket> = s
            .minutes
            .range(window_start..=now_minute)
            .map(|(_, b)| b)
            .collect();
        let sum = |f: fn(&Bucket) -> u64| window.iter().map(|b| f(b)).sum::<u64>();

        let wakes = sum(|b| b.wakes as u64);
        if wakes > policy.max_wakes_per_hour {
            push(
                "wake-rate",
                format!(
                    "{wakes} wakes in the {window_label} (threshold {})",
                    policy.max_wakes_per_hour
                ),
                "find what is waking this seat (a cron, loop or tick) and stop it",
            );
        }
        let tokens = sum(|b| b.tokens);
        if tokens > policy.max_tokens_per_hour {
            push(
                "token-rate",
                format!(
                    "{} tokens over {} calls in the {window_label} (threshold {})",
                    fmt_tokens(tokens),
                    sum(|b| b.calls as u64),
                    fmt_tokens(policy.max_tokens_per_hour)
                ),
                "stop or restart the seat; it is spending far faster than an ordinary working seat",
            );
        }
        for (hash, per_minute) in &s.prompts {
            let count: u64 = per_minute
                .range(window_start..=now_minute)
                .map(|(_, n)| *n as u64)
                .sum();
            let declared = s.declared.get(hash).copied();
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
                        "injected prompt #{hash} fired {count} times in the {window_label} (threshold {}{declared_note})",
                        policy.max_repeated_prompts_per_hour
                    ),
                    "delete the cron/loop that injects this prompt; mail reaches a seat through hooks, not model-side polling",
                );
            }
        }
        let replies = sum(|b| b.replies as u64);
        let noop = sum(|b| b.noop as u64);
        if replies >= policy.idle_min_replies && noop * 100 > policy.max_idle_ratio_pct * replies {
            push(
                "idle-ratio",
                format!(
                    "{noop} of {replies} replies ({}%) were no-ops in the {window_label} (threshold {}%)",
                    noop * 100 / replies.max(1),
                    policy.max_idle_ratio_pct
                ),
                "the seat is being woken with nothing to do; stop the tick that wakes it",
            );
        }
        if policy.restart_context_tokens > 0 {
            if let Some(latest) = window
                .iter()
                .filter(|b| b.calls > 0)
                .max_by_key(|b| b.last_ctx_ts)
                .filter(|b| b.last_ctx > policy.restart_context_tokens)
            {
                push(
                    "restart-recommended",
                    format!(
                        "per-call context is {} (ceiling {}, `[watchdog] restart_context_tokens`)",
                        fmt_tokens(latest.last_ctx),
                        fmt_tokens(policy.restart_context_tokens)
                    ),
                    "run /aida-handoff in that seat, then start a fresh session",
                );
            }
        }
        let compactions: u64 = s
            .minutes
            .range(day_start..=now_minute)
            .map(|(_, b)| b.compactions as u64)
            .sum();
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
        if let Some((minute, _)) = s
            .minutes
            .range(day_start..=now_minute)
            .rev()
            .find(|(_, b)| b.polls > 0)
        {
            let at = Utc
                .timestamp_opt(minute * 60, 0)
                .single()
                .map(|t| t.format("%Y-%m-%dT%H:%MZ").to_string())
                .unwrap_or_default();
            push(
                "mail-poll-setup",
                format!("a model-side mailbox poll was active at {at} (forbidden since BUG-1589)"),
                "delete the cron/loop in that seat; unread mail reaches a seat through the prompt hook",
            );
        }
    }

    let alert_at = policy.daily_token_budget / 100 * policy.budget_alert_pct;
    if day_tokens > 0 && day_tokens >= alert_at {
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
            action: "find the session carrying the spend before the budget is gone".to_string(),
            safe_heal: false,
        });
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    findings
}

/// Run the watchdog once: fold new bytes into the persisted aggregates,
/// judge the trailing window, and report only rules that newly crossed.
// trace:STORY-1462 | ai:claude
pub(crate) fn scan(
    project_label: &str,
    sources: &Sources,
    policy: &WatchdogPolicy,
    now: DateTime<Utc>,
    seats: &HashMap<String, SeatInfo>,
) -> (Vec<DoctorFinding>, Coverage) {
    let day_start = now - Duration::minutes(DAY_MINUTES);
    let mut coverage = Coverage::default();
    let mut budget = Budget {
        started: Instant::now(),
        bytes: 0,
        partial: false,
    };
    let mut state = load_state(sources.state_path.as_deref());
    // Offsets of files this run does not visit (budget-cut, or quiet for a
    // day) carry forward; only files that no longer exist are dropped.
    state.files.retain(|p, _| Path::new(p).exists());

    let groups = [
        ("claude-transcripts", &sources.transcript_dirs),
        ("headless-logs", &sources.headless_dirs),
    ];
    let mut absent_worktree_slugs = 0usize;
    for (label, dirs) in groups {
        for (i, dir) in dirs.iter().enumerate() {
            if !dir.is_dir() {
                // Most worktrees never host a session of their own; only the
                // project root's slug being absent is worth its own line.
                if label == "claude-transcripts" && i > 0 {
                    absent_worktree_slugs += 1;
                } else {
                    coverage.sources.push(SourceStatus {
                        source: label.to_string(),
                        path: dir.display().to_string(),
                        state: "unavailable".into(),
                        detail: "directory not present".into(),
                    });
                }
                continue;
            }
            let mut files = Vec::new();
            walk_jsonl(dir, &mut files);
            files.sort();
            let mut read = 0usize;
            for file in files {
                if file_mtime(&file).is_none_or(|m| m < day_start) {
                    continue;
                }
                if budget.exhausted(policy) {
                    break;
                }
                let key = file.display().to_string();
                let start = state.files.get(&key).copied();
                if let Ok(offset) = scan_file(
                    &file,
                    start,
                    policy,
                    day_start,
                    &mut state.sessions,
                    &mut budget,
                ) {
                    if start != Some(offset) {
                        read += 1;
                    }
                    state.files.insert(key, offset);
                }
            }
            coverage.files_scanned += read;
            coverage.sources.push(SourceStatus {
                source: label.to_string(),
                path: dir.display().to_string(),
                state: "available".into(),
                detail: format!("{read} file(s) with new bytes"),
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

    prune(&mut state.sessions, minute_of(day_start));
    let tripped = judge(project_label, &state.sessions, policy, now, seats);
    let tripped_ids: BTreeSet<String> = tripped.iter().map(|f| f.id.clone()).collect();
    coverage.still_active = tripped_ids.intersection(&state.active).cloned().collect();
    let findings: Vec<DoctorFinding> = tripped
        .into_iter()
        .filter(|f| !state.active.contains(&f.id))
        .collect();
    // A budget-cut run judged incomplete aggregates: never let it clear a
    // rule (that would re-alert on the next complete run).
    state.active = if budget.partial {
        state.active.union(&tripped_ids).cloned().collect()
    } else {
        tripped_ids.clone()
    };
    state.version = STATE_VERSION;
    save_state(sources.state_path.as_deref(), &state);

    coverage.bytes_read = budget.bytes;
    coverage.partial = budget.partial;
    coverage.sessions = state.sessions.len();
    coverage.sessions_timing_unknown = state.sessions.values().filter(|s| !s.timed).count();
    coverage.elapsed_ms = budget.started.elapsed().as_millis() as u64;
    let any_available = coverage.sources.iter().any(|s| s.state == "available");
    coverage.verdict = if !any_available {
        "unknown"
    } else if !tripped_ids.is_empty() {
        "tripped"
    } else if budget.partial {
        "degraded"
    } else {
        "ok"
    }
    .into();
    (findings, coverage)
}

/// Text rendering of the coverage block, printed under the doctor report.
pub(crate) fn render_coverage(c: &Coverage) -> String {
    let mut out = format!(
        "runaway-seats evidence: verdict {} — {} file(s) with new bytes, {} bytes read, {} session(s), {} with unknown timing ({} ms)\n",
        c.verdict, c.files_scanned, c.bytes_read, c.sessions, c.sessions_timing_unknown, c.elapsed_ms
    );
    if c.verdict == "unknown" {
        out.push_str("  UNKNOWN: no transcript or headless log directory was readable, so no seat was judged — this is not a clean result\n");
    }
    if c.partial {
        out.push_str("  DEGRADED: the run stopped at its byte/time budget; unread bytes resume on the next run (raise `[watchdog] max_total_bytes` / `time_budget_ms` to finish sooner)\n");
    }
    if !c.still_active.is_empty() {
        out.push_str(&format!(
            "  still tripped (already reported once): {}\n",
            c.still_active.join(", ")
        ));
    }
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

    /// The BUG-1589 storm shape as measured: a mail tick every 8s (~450 an
    /// hour), each answered "mail tick: quiet" on a ~580k context, ~700M
    /// tokens an hour at 3 calls per tick, plus the declared-every-10m
    /// scheduled fire and the CronCreate that set it up.
    fn storm_transcript(session: &str, start_secs: i64, ticks: i64) -> String {
        let mut out = String::new();
        let line = |v: serde_json::Value| format!("{v}\n");
        out.push_str(&line(serde_json::json!({"type":"assistant","timestamp":ts(start_secs),"sessionId":session,
            "message":{"id":format!("{session}-cron-{start_secs}"),"stop_reason":"tool_use",
            "usage":{"input_tokens":5,"cache_read_input_tokens":500000,"cache_creation_input_tokens":1000,"output_tokens":50},
            "content":[{"type":"tool_use","name":"CronCreate","input":{"cron":"7,17,27,37,47,57 * * * *","prompt":TICK}}]}})));
        for i in 0..ticks {
            let at = start_secs + 8 * i + 1;
            out.push_str(&line(serde_json::json!({"type":"system","subtype":"scheduled_task_fire","timestamp":ts(at),"sessionId":session,
                "cron":"7,17,27,37,47,57 * * * *","prompt":TICK})));
            out.push_str(&line(serde_json::json!({"type":"user","timestamp":ts(at),"sessionId":session,"isMeta":true,"promptSource":"system",
                "message":{"role":"user","content":TICK}})));
            for call in 0..3 {
                let last = call == 2;
                out.push_str(&line(serde_json::json!({"type":"assistant","timestamp":ts(at + 1 + call),"sessionId":session,
                    "message":{"id":format!("{session}-{start_secs}-{i}-{call}"),"stop_reason":if last {"end_turn"} else {"tool_use"},
                    "usage":{"input_tokens":3,"cache_read_input_tokens":518000,"cache_creation_input_tokens":2000,"output_tokens":20},
                    "content":[if last {serde_json::json!({"type":"text","text":"mail tick: quiet"})} else {serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"aida mailbox inbox --unread"}})}]}})));
            }
        }
        out
    }

    /// An ordinary lead seat at the measured 2026-09-23 peak, sustained for
    /// `hours`: 30 wakes an hour (19 of them the same injected stop-hook
    /// prompt), ~229M tokens an hour on a ~900k context, 44% short replies,
    /// and 3 compactions over the day.
    fn ordinary_transcript(session: &str, hours: i64) -> String {
        let mut out = String::new();
        let line = |v: serde_json::Value| format!("{v}\n");
        for h in 0..hours {
            let base = h * 3600;
            if h % 3 == 0 && h > 0 {
                out.push_str(&line(serde_json::json!({"type":"system","subtype":"compact_boundary","timestamp":ts(base),"sessionId":session})));
            }
            for w in 0..30 {
                let at = base + w * 115;
                let injected = w < 19;
                let prompt = if injected {
                    "Stop hook feedback: [all open specs terminal] is not yet met".to_string()
                } else {
                    format!("please take the next step {h}-{w}")
                };
                out.push_str(&line(
                    serde_json::json!({"type":"user","timestamp":ts(at),"sessionId":session,
                    "isMeta": injected, "promptSource": if injected {"system"} else {"typed"},
                    "message":{"role":"user","content":prompt}}),
                ));
                // 254 calls/h * ~902k = ~229M tokens/h.
                let calls = if w < 14 { 9 } else { 8 };
                for c in 0..calls {
                    let last = c == calls - 1;
                    let text = if w % 9 < 4 {
                        "Done."
                    } else {
                        "Implemented the change, added tests, and verified the suite passes end to end."
                    };
                    out.push_str(&line(serde_json::json!({"type":"assistant","timestamp":ts(at + 1 + c),"sessionId":session,
                        "message":{"id":format!("{session}-{h}-{w}-{c}"),"stop_reason":if last {"end_turn"} else {"tool_use"},
                        "usage":{"input_tokens":3,"cache_read_input_tokens":898000,"cache_creation_input_tokens":3000,"output_tokens":400},
                        "content":[if last {serde_json::json!({"type":"text","text":text})} else {serde_json::json!({"type":"tool_use","name":"Bash","input":{"command":"cargo test"}})}]}})));
                }
            }
        }
        out
    }

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        p
    }

    fn append(path: &Path, body: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    fn sources(root: &Path) -> Sources {
        fs::create_dir_all(root.join("headless")).unwrap();
        Sources {
            transcript_dirs: vec![root.join("claude")],
            headless_dirs: vec![root.join("headless")],
            state_path: Some(root.join("state").join("state.json")),
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
    /// within the first simulated hour, under the calibrated defaults.
    // trace:STORY-1462 | ai:claude
    #[test]
    fn storm_replay_trips_the_storm_rules_on_both_sessions_within_the_first_hour() {
        let tmp = tempfile::tempdir().unwrap();
        let a = "01a0bfde-0000-7000-8000-000000000001";
        let b = "01a0adee-16e1-7982-ac9f-e15abb207b6c";
        write(
            &tmp.path().join("claude"),
            &format!("{a}.jsonl"),
            &storm_transcript(a, 0, 450),
        );
        write(
            &tmp.path().join("claude"),
            &format!("{b}.jsonl"),
            &storm_transcript(b, 0, 450),
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
                "token-rate",
                "wake-rate",
            ] {
                assert!(
                    got.contains(&rule.to_string()),
                    "{sid}: {rule} missing from {got:?}"
                );
            }
            assert!(
                !got.contains(&"restart-recommended".to_string()),
                "restart is opt-in"
            );
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
        assert!(wake.summary.contains("60m window ending"));
        let repeated = findings
            .iter()
            .find(|f| f.id == format!("repeated-prompt:{a}"))
            .unwrap();
        assert!(repeated.summary.contains("declared every 10m"));
    }

    /// Privacy: a repeated prompt is reported as a short hash and a count,
    /// and neither findings nor persisted state carry the prompt text.
    #[test]
    fn repeated_prompts_are_reported_by_hash_never_by_text() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("claude"),
            "s.jsonl",
            &storm_transcript("s", 0, 450),
        );
        let src = sources(tmp.path());
        let (findings, _) = scan(
            "aida",
            &src,
            &WatchdogPolicy::default(),
            t0() + Duration::minutes(60),
            &HashMap::new(),
        );
        let repeated = findings
            .iter()
            .find(|f| f.id == "repeated-prompt:s")
            .unwrap();
        assert!(repeated
            .summary
            .contains(&format!("#{}", prompt_hash(TICK))));
        for f in &findings {
            assert!(!f.summary.contains("mail tick"), "{}", f.summary);
            assert!(!f.summary.contains("mailbox inbox"), "{}", f.summary);
        }
        let state = fs::read_to_string(src.state_path.unwrap()).unwrap();
        assert!(!state.contains("mailbox inbox"));
    }

    /// Restart recommendation is opt-in and, when on, names the context.
    #[test]
    fn restart_recommendation_fires_when_a_ceiling_is_configured() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("claude"),
            "s.jsonl",
            &storm_transcript("s", 0, 10),
        );
        let policy = WatchdogPolicy {
            restart_context_tokens: 300_000,
            ..WatchdogPolicy::default()
        };
        let (findings, _) = scan(
            "aida",
            &sources(tmp.path()),
            &policy,
            t0() + Duration::minutes(5),
            &HashMap::new(),
        );
        let f = findings
            .iter()
            .find(|f| f.id == "restart-recommended:s")
            .unwrap();
        assert!(f.summary.contains("520k"), "{}", f.summary);
        assert!(f.action.contains("/aida-handoff"));
    }

    /// Calibration: a seat at the measured ordinary peak, held for ten
    /// hours (2.29B tokens, above the 2.25B ordinary day), is silent.
    #[test]
    fn a_busy_ordinary_seat_is_silent_and_reported_ok() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            &tmp.path().join("claude"),
            "lead.jsonl",
            &ordinary_transcript("lead", 10),
        );
        let src = sources(tmp.path());
        for hour in 1..=10 {
            let now = t0() + Duration::minutes(60 * hour);
            let (findings, coverage) = scan(
                "aida",
                &src,
                &WatchdogPolicy::default(),
                now,
                &HashMap::new(),
            );
            assert!(findings.is_empty(), "hour {hour}: {findings:?}");
            assert_eq!(coverage.verdict, "ok");
        }
    }

    /// Acceptance 3 and 8: each rule reports once per crossing, stays quiet
    /// while it holds, and reports again only after it clears and recrosses.
    // trace:STORY-1462 | ai:claude
    #[test]
    fn each_rule_reports_once_per_crossing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            &tmp.path().join("claude"),
            "s.jsonl",
            &storm_transcript("s", 0, 450),
        );
        let src = sources(tmp.path());
        let policy = WatchdogPolicy::default();
        let now = t0() + Duration::minutes(60);
        let (first, _) = scan("aida", &src, &policy, now, &HashMap::new());
        assert!(first.iter().any(|f| f.id == "wake-rate:s"));
        let (second, cov) = scan("aida", &src, &policy, now, &HashMap::new());
        assert!(second.is_empty(), "{second:?}");
        assert!(cov.still_active.contains(&"wake-rate:s".to_string()));
        assert_eq!(cov.verdict, "tripped");
        // Two quiet hours later the hourly rules have cleared.
        let quiet = t0() + Duration::minutes(180);
        let (third, cov) = scan("aida", &src, &policy, quiet, &HashMap::new());
        assert!(third.is_empty());
        assert!(!cov
            .still_active
            .iter()
            .any(|id| id.starts_with("wake-rate")));
        // A fresh storm is a new crossing.
        append(&path, &storm_transcript("s", 3 * 3600, 450));
        let (fourth, _) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(240),
            &HashMap::new(),
        );
        assert!(fourth.iter().any(|f| f.id == "wake-rate:s"), "{fourth:?}");
    }

    /// Incremental: an unchanged file costs nothing, the aggregate carries
    /// the judgment across runs, and appended bytes are the only bytes read.
    #[test]
    fn a_tick_reads_only_new_bytes_and_keeps_its_aggregates() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write(
            &tmp.path().join("claude"),
            "s.jsonl",
            &storm_transcript("s", 0, 200),
        );
        let src = sources(tmp.path());
        let policy = WatchdogPolicy::default();
        let (_, first) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(30),
            &HashMap::new(),
        );
        assert_eq!(first.bytes_read, fs::metadata(&path).unwrap().len());
        let (_, second) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(30),
            &HashMap::new(),
        );
        assert_eq!(second.bytes_read, 0);
        assert!(second.still_active.contains(&"wake-rate:s".to_string()));
        let extra = storm_transcript("s", 1800, 5);
        append(&path, &extra);
        let (_, third) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(40),
            &HashMap::new(),
        );
        assert_eq!(third.bytes_read, extra.len() as u64);
        // A half-written trailing line is left for the next run.
        append(&path, "{\"type\":\"user\"");
        let (_, fourth) = scan(
            "aida",
            &src,
            &policy,
            t0() + Duration::minutes(40),
            &HashMap::new(),
        );
        assert_eq!(fourth.bytes_read, 0);
    }

    /// A budget-cut run is degraded evidence, not a finding (so it cannot
    /// page a seat under --fail-on-findings); unvisited files keep their
    /// offsets and the read resumes where it stopped.
    #[test]
    fn a_budget_cut_run_is_degraded_and_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let one = write(
            &tmp.path().join("claude"),
            "a.jsonl",
            &ordinary_transcript("a", 1),
        );
        let two = write(
            &tmp.path().join("claude"),
            "b.jsonl",
            &ordinary_transcript("b", 1),
        );
        let total = fs::metadata(&one).unwrap().len() + fs::metadata(&two).unwrap().len();
        let src = sources(tmp.path());
        let tight = WatchdogPolicy {
            max_total_bytes: 50_000,
            ..WatchdogPolicy::default()
        };
        let now = t0() + Duration::minutes(60);
        let mut read = 0;
        for _ in 0..200 {
            let (findings, cov) = scan("aida", &src, &tight, now, &HashMap::new());
            assert!(
                findings.is_empty(),
                "degraded evidence is never a finding: {findings:?}"
            );
            read += cov.bytes_read;
            if !cov.partial {
                assert_eq!(cov.verdict, "ok");
                break;
            }
            assert_eq!(cov.verdict, "degraded");
            assert!(render_coverage(&cov).contains("DEGRADED"));
        }
        assert_eq!(
            read, total,
            "every byte read exactly once across the resumed runs"
        );
    }

    /// Unavailable evidence is reported as unknown, never as ok — and is not
    /// a finding.
    #[test]
    fn missing_evidence_is_unknown_not_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let src = Sources {
            transcript_dirs: vec![tmp.path().join("claude")],
            headless_dirs: vec![tmp.path().join("headless")],
            state_path: None,
        };
        let (findings, coverage) = scan(
            "aida",
            &src,
            &WatchdogPolicy::default(),
            t0(),
            &HashMap::new(),
        );
        assert_eq!(coverage.verdict, "unknown");
        assert!(findings.is_empty());
        assert!(render_coverage(&coverage).contains("UNKNOWN"));
    }

    /// Untimed headless stream-json counts toward the daily budget but its
    /// hourly rates are not judged.
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

    /// Acceptance 6: bounded runtime and zero model calls — no process
    /// spawn, HTTP client or vendor SDK in the watchdog itself.
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
        let one = storm_transcript("big", 0, 400);
        for i in 0..5 {
            write(&dir, &format!("big-{i}.jsonl"), &one);
        }
        let src = sources(tmp.path());
        let now = t0() + Duration::minutes(60);
        let started = Instant::now();
        let (_, coverage) = scan(
            "aida",
            &src,
            &WatchdogPolicy::default(),
            now,
            &HashMap::new(),
        );
        assert!(!coverage.partial);
        assert!(
            started.elapsed().as_secs() < 3,
            "took {:?}",
            started.elapsed()
        );
        let (_, again) = scan(
            "aida",
            &src,
            &WatchdogPolicy::default(),
            now,
            &HashMap::new(),
        );
        assert_eq!(again.bytes_read, 0);
    }

    #[test]
    fn policy_reads_the_watchdog_section_and_keeps_defaults_for_bad_values() {
        assert_eq!(policy(None), WatchdogPolicy::default());
        let cfg: toml::Value = toml::from_str(
            "[watchdog]\nmax_wakes_per_hour = 60\nrestart_context_tokens = 300000\nbudget_alert_pct = 150\nmax_idle_ratio_pct = -5\n",
        )
        .unwrap();
        let p = policy(Some(&cfg));
        assert_eq!(p.max_wakes_per_hour, 60);
        assert_eq!(p.restart_context_tokens, 300_000);
        assert_eq!(p.budget_alert_pct, 90);
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
