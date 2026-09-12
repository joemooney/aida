//! STORY-993: attach a real process to each `aida session conversations` row.
//!
//! The conversation table used to paint `●` whenever a transcript had been
//! written in the last five minutes — "file touched", not "process alive" —
//! and showed no pid or tty, so an operator with a forgotten advisor tab had
//! no way to find it. This module resolves, best-effort and never invented:
//!
//!   1. **lease** — the `active_pid` of a session lease that covers the
//!      transcript (joined through the session manifest's
//!      `claude_session_id`), when that pid is alive;
//!   2. **proc-scan** — a live claude/codex process whose cwd equals the
//!      transcript's recorded cwd and whose start time fits the transcript's
//!      life: not before the transcript began (minus slack) and not after its
//!      most recent `SessionStart` (launch or resume) hook event (plus slack).
//!      Two survivors = ambiguous (`?` in the table, both pids in JSON);
//!   3. **none** — `-`; liveness then falls back to the file-touched signal.
//!
//! Everything here is pure over an injected process list so the rules are
//! unit-testable without a live `/proc`; the one `/proc` walk is shared with
//! every other liveness consumer via `aida_core::liveness`.
//!
//! trace:STORY-993 | ai:claude

use std::collections::VecDeque;
use std::io::BufRead;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

pub use aida_core::liveness::{AgentKind, LiveAgentProcess};

/// A transcript written within this window with no resolved process is
/// `◐` (file-touched, unverified) — the old `●` threshold, demoted.
pub const FILE_TOUCHED_WINDOW_SECS: u64 = 5 * 60;

/// A process may have started this long BEFORE the transcript's first event
/// (binary start-up, MCP servers, the picker) and still be its owner.
const START_BEFORE_FIRST_EVENT_SLACK_SECS: i64 = 120;

/// A process may have started this long AFTER the transcript's most recent
/// `SessionStart` hook event and still be its owner (clock skew, slow hooks).
const START_AFTER_LAST_START_SLACK_SECS: i64 = 60;

/// Liveness for a conversation row.
// trace:STORY-993 | ai:claude
// trace:STORY-998 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum Liveness {
    /// A process was resolved for this transcript and is alive.
    Alive,
    /// A process is alive, but the transcript tail is low-information repeated
    /// output long enough to meet the idlewatch spinning threshold.
    Spinning,
    /// The transcript was written in the last five minutes but no process
    /// could be resolved — the old `●`, now unverified.
    FileTouched,
    /// Neither.
    #[default]
    None,
}

impl Liveness {
    pub fn label(self) -> &'static str {
        match self {
            Liveness::Alive => "alive",
            Liveness::Spinning => "spinning",
            Liveness::FileTouched => "file-touched",
            Liveness::None => "none",
        }
    }
}

/// How the pid (if any) was found.
// trace:STORY-993 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum Resolution {
    /// A session lease covering this transcript recorded the pid.
    Lease,
    /// Matched by cwd + start-time window against the live process walk.
    /// With more than one candidate this is the *ambiguous* case.
    ProcScan,
    #[default]
    None,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Lease => "lease",
            Resolution::ProcScan => "proc-scan",
            Resolution::None => "none",
        }
    }
}

/// The resolved process facts for one conversation row.
// trace:STORY-993 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct ProcessFacts {
    /// The single resolved pid. `None` when unresolved OR ambiguous — check
    /// [`ProcessFacts::is_ambiguous`] / `candidates` to tell the two apart.
    pub pid: Option<u32>,
    /// Every pid that survived the proc-scan filter (or the one lease pid).
    /// Length > 1 with `pid == None` is the ambiguous case.
    pub candidates: Vec<u32>,
    /// Controlling tty of the resolved pid, `/dev/…` form.
    pub tty: Option<String>,
    /// Process elapsed (now − process start) for a resolved pid whose start
    /// time is known.
    pub elapsed_secs: Option<u64>,
    pub liveness: Liveness,
    /// Dominant repeated template and count when [`Liveness::Spinning`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spinning: Option<SpinningFacts>,
    pub resolution: Resolution,
    /// The covering lease's role, when a lease covers this transcript —
    /// provenance for the JSON `lease_role` field (TASK-152 parity with
    /// `aida ps`).
    pub lease_role: Option<String>,
}

/// Low-information transcript details behind a `spinning` row.
// trace:STORY-998 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SpinningFacts {
    pub template: String,
    pub count: usize,
}

impl ProcessFacts {
    /// Two or more processes matched and none could be singled out.
    pub fn is_ambiguous(&self) -> bool {
        self.pid.is_none() && self.candidates.len() > 1
    }

    /// The PID table cell: pid, `?` when ambiguous, `-` when none.
    pub fn pid_cell(&self) -> String {
        if self.is_ambiguous() {
            "?".to_string()
        } else {
            self.pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string())
        }
    }

    /// The TTY table cell: `pts/3` (the `/dev/` prefix dropped), `?` when
    /// ambiguous, `-` when none.
    pub fn tty_cell(&self) -> String {
        if self.is_ambiguous() {
            return "?".to_string();
        }
        self.tty
            .as_deref()
            .map(short_tty)
            .unwrap_or_else(|| "-".to_string())
    }
}

impl ProcessFacts {
    /// Upgrade a live row to `spinning` when the transcript tail is a repeated,
    /// low-information stream. Unresolved/fresh rows keep their weaker
    /// FileTouched/None verdicts.
    // trace:STORY-998 | ai:codex
    pub fn with_spinning(mut self, spinning: Option<SpinningFacts>) -> Self {
        if self.liveness == Liveness::Alive {
            if let Some(spinning) = spinning {
                self.liveness = Liveness::Spinning;
                self.spinning = Some(spinning);
            }
        }
        self
    }
}

/// `/dev/pts/3` → `pts/3`; anything without the prefix is returned as-is.
pub fn short_tty(tty: &str) -> String {
    tty.strip_prefix("/dev/").unwrap_or(tty).to_string()
}

/// What the resolver needs to know about one transcript.
// trace:STORY-993 | ai:claude
#[derive(Debug, Clone)]
pub struct TranscriptFacts<'a> {
    /// `claude` / `codex` / … — matched against [`AgentKind::label`].
    pub agent: &'a str,
    /// The cwd the transcript last recorded.
    pub cwd: Option<&'a Path>,
    /// Timestamp of the transcript's first event.
    pub started_at: Option<DateTime<Utc>>,
    /// Seconds since the transcript file was last written.
    pub age_seconds: u64,
}

/// The lease (if any) that covers a transcript.
// trace:STORY-993 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct LeaseFacts {
    /// The lease's recorded agent pid (`active_pid`), if any.
    pub pid: Option<u32>,
    pub role: Option<String>,
}

/// Facts about one pid that is NOT in the agent walk (a lease pid pointing at
/// a wrapper): `(start_time, tty)`.
pub type PidFacts = (Option<DateTime<Utc>>, Option<String>);

/// Resolve the process behind one transcript. Pure over the injected inputs:
///
/// - `procs` — the live agent-process walk;
/// - `pid_alive` — kernel liveness for a lease pid;
/// - `pid_facts` — start time + tty for a lease pid absent from `procs`;
/// - `last_start_event` — LAZY lookup of the transcript's most recent
///   `SessionStart` hook timestamp; only called when a cwd-matched candidate
///   exists, so the transcript is never re-read for rows with no live process.
// trace:STORY-993 | ai:claude
pub fn resolve(
    t: &TranscriptFacts<'_>,
    lease: Option<&LeaseFacts>,
    procs: &[LiveAgentProcess],
    pid_alive: &dyn Fn(u32) -> bool,
    pid_facts: &dyn Fn(u32) -> PidFacts,
    last_start_event: &dyn Fn() -> Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> ProcessFacts {
    let lease_role = lease.and_then(|l| l.role.clone());
    let file_liveness = if t.age_seconds < FILE_TOUCHED_WINDOW_SECS {
        Liveness::FileTouched
    } else {
        Liveness::None
    };

    // (a) lease-recorded pid, when alive.
    if let Some(pid) = lease.and_then(|l| l.pid).filter(|p| pid_alive(*p)) {
        let (start_time, tty) = match procs.iter().find(|p| p.pid == pid) {
            Some(p) => (p.start_time, p.tty.clone()),
            None => pid_facts(pid),
        };
        return ProcessFacts {
            pid: Some(pid),
            candidates: vec![pid],
            tty,
            elapsed_secs: start_time.map(|s| elapsed_since(s, now)),
            liveness: Liveness::Alive,
            resolution: Resolution::Lease,
            lease_role,
            ..ProcessFacts::default()
        };
    }

    // (b) proc-scan: same agent, same cwd, start time inside the transcript's
    // life.
    let Some(cwd) = t.cwd else {
        return ProcessFacts {
            liveness: file_liveness,
            lease_role,
            ..ProcessFacts::default()
        };
    };
    let lower = t
        .started_at
        .map(|s| s - chrono::Duration::seconds(START_BEFORE_FIRST_EVENT_SLACK_SECS));
    let mut candidates: Vec<&LiveAgentProcess> = procs
        .iter()
        .filter(|p| p.agent.label() == t.agent && !p.stale_cwd && same_dir(&p.cwd, cwd))
        .filter(|p| match (p.start_time, lower) {
            (Some(start), Some(lower)) => start >= lower,
            _ => true,
        })
        .collect();
    if !candidates.is_empty() {
        // Upper bound: the last launch/resume hook event (+ slack). Evaluated
        // lazily — only rows with a cwd-matched live process pay for it. No
        // recorded start event = no upper bound (honest: a later process in
        // the same cwd then reads as a candidate, never silently dropped).
        if let Some(last_start) = last_start_event() {
            let upper = last_start + chrono::Duration::seconds(START_AFTER_LAST_START_SLACK_SECS);
            candidates.retain(|p| match p.start_time {
                Some(start) => start <= upper,
                None => true,
            });
        }
    }
    match candidates.as_slice() {
        [] => ProcessFacts {
            liveness: file_liveness,
            lease_role,
            ..ProcessFacts::default()
        },
        [p] => ProcessFacts {
            pid: Some(p.pid),
            candidates: vec![p.pid],
            tty: p.tty.clone(),
            elapsed_secs: p.start_time.map(|s| elapsed_since(s, now)),
            liveness: Liveness::Alive,
            resolution: Resolution::ProcScan,
            lease_role,
            ..ProcessFacts::default()
        },
        many => ProcessFacts {
            pid: None,
            candidates: many.iter().map(|p| p.pid).collect(),
            tty: None,
            elapsed_secs: None,
            // A process is present but not singled out — do not claim `●`.
            liveness: file_liveness,
            resolution: Resolution::ProcScan,
            lease_role,
            ..ProcessFacts::default()
        },
    }
}

fn elapsed_since(start: DateTime<Utc>, now: DateTime<Utc>) -> u64 {
    now.signed_duration_since(start).num_seconds().max(0) as u64
}

/// Directory equality tolerant of a trailing slash; no canonicalization (the
/// walk reports resolved paths, the transcript records what Claude saw — a
/// symlinked project root is a known best-effort miss, not a guess).
fn same_dir(a: &Path, b: &Path) -> bool {
    let trim = |p: &Path| p.to_string_lossy().trim_end_matches('/').to_string();
    trim(a) == trim(b)
}

/// Timestamp of the transcript's most recent process start — the last
/// `SessionStart:startup` / `SessionStart:resume` hook attachment Claude Code
/// logged (a `SessionStart:compact` is the same process and does not count).
/// Scanned BACKWARDS in 1 MiB chunks so a multi-MB transcript that resumed
/// recently costs one chunk; capped so a pathological file cannot stall the
/// table. Each chunk is searched ONCE — only the newly read bytes plus a
/// marker-length overlap — so the cost is linear in the bytes read, not
/// quadratic in the buffer (a marker 5 MiB from the end of an 18 MB advisor
/// transcript is ~10 ms, not ~70). `None` when no hook event was logged (no
/// SessionStart hook installed) or the file is unreadable.
// trace:STORY-993 | ai:claude
pub fn last_process_start_event(path: &Path) -> Option<DateTime<Utc>> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 1024 * 1024;
    const SCAN_CAP: u64 = 32 * 1024 * 1024;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut buf: Vec<u8> = Vec::new();
    let mut lo = len; // file offset of buf[0]
                      // How many bytes at the FRONT of `buf` still need searching after the
                      // next chunk is prepended: a marker straddling the chunk edge, or a hit
                      // whose line head was cut off and must be re-read with more context.
    let mut carry: usize = 0;
    loop {
        if lo == 0 || len - lo >= SCAN_CAP {
            return match find_last_start_event(&buf, carry.min(buf.len()), lo == 0) {
                Found::Timestamp(ts) => Some(ts),
                _ => None,
            };
        }
        let step = CHUNK.min(lo);
        lo -= step;
        let mut chunk = vec![0u8; step as usize];
        file.seek(SeekFrom::Start(lo)).ok()?;
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
        let search_end = (step as usize + carry).min(buf.len());
        match find_last_start_event(&buf, search_end, lo == 0) {
            Found::Timestamp(ts) => return Some(ts),
            // The marker is at `hit` but its line head lies before buf[0]:
            // keep it in the search window of the next (larger) buffer.
            Found::CutOff(hit) => carry = hit + MAX_MARKER_LEN,
            Found::Nothing => carry = MAX_MARKER_LEN,
        }
    }
}

const START_MARKERS: [&str; 2] = [
    "\"hookName\":\"SessionStart:startup\"",
    "\"hookName\":\"SessionStart:resume\"",
];

/// Longest marker — the overlap kept between backward chunks so a marker
/// straddling a chunk edge is still found.
const MAX_MARKER_LEN: usize = 40;

enum Found {
    Timestamp(DateTime<Utc>),
    /// A marker at this offset whose line start lies before `buf[0]`.
    CutOff(usize),
    Nothing,
}

/// Find the LAST start marker that BEGINS inside `buf[..search_end]` and
/// parse the `"timestamp"` on its line (the line itself may run past
/// `search_end`). When `complete_head` is false and the marker's line start
/// lies before `buf[0]`, reports `CutOff` so the caller extends the buffer.
fn find_last_start_event(buf: &[u8], search_end: usize, complete_head: bool) -> Found {
    let window = &buf[..search_end.min(buf.len())];
    let Some(hit) = START_MARKERS
        .iter()
        .filter_map(|m| rfind(window, m.as_bytes()))
        .max()
    else {
        return Found::Nothing;
    };
    let line_start = match buf[..hit].iter().rposition(|b| *b == b'\n') {
        Some(nl) => nl + 1,
        None if complete_head => 0,
        None => return Found::CutOff(hit),
    };
    let line_end = buf[hit..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|i| hit + i)
        .unwrap_or(buf.len());
    let line = String::from_utf8_lossy(&buf[line_start..line_end]);
    match timestamp_on_line(&line) {
        Some(ts) => Found::Timestamp(ts),
        None => Found::Nothing,
    }
}

/// Parse the `"timestamp":"…"` value on one transcript line.
pub fn timestamp_on_line(line: &str) -> Option<DateTime<Utc>> {
    let key = "\"timestamp\":\"";
    let start = line.find(key)? + key.len();
    let end = line[start..].find('"')? + start;
    DateTime::parse_from_rfc3339(&line[start..end])
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Classify the recent transcript tail for STORY-998 session liveness.
///
/// This reads only the tail of the JSONL-ish transcript, extracts the payload a
/// human would recognize as repeated work/status text, and feeds it through the
/// shared idle detector. Unique JSON envelope fields such as timestamps and IDs
/// are deliberately excluded so they cannot mask a real spinner.
// trace:STORY-998 | ai:codex
pub fn transcript_spinning(path: &Path, now: DateTime<Utc>) -> Option<SpinningFacts> {
    transcript_spinning_with_config(path, now, aida_core::idle::IdleConfig::default())
}

// trace:STORY-998 | ai:codex
pub fn transcript_spinning_with_config(
    path: &Path,
    now: DateTime<Utc>,
    cfg: aida_core::idle::IdleConfig,
) -> Option<SpinningFacts> {
    let lines = read_tail_lines(path, cfg.max_window_lines.max(32)).ok()?;
    let mut detector = aida_core::idle::IdleDetector::new(cfg);
    let instant_now = Instant::now();
    for line in lines {
        let payload = transcript_payload(&line);
        if payload.trim().is_empty() {
            continue;
        }
        let at = timestamp_on_line(&line)
            .map(|ts| {
                let age = now
                    .signed_duration_since(ts)
                    .to_std()
                    .unwrap_or(Duration::ZERO);
                instant_now.checked_sub(age).unwrap_or(instant_now)
            })
            .unwrap_or(instant_now);
        detector.feed_line(&payload, at);
    }
    match detector.verdict(instant_now) {
        aida_core::idle::IdleVerdict::Spinning { template, count } => {
            Some(SpinningFacts { template, count })
        }
        _ => None,
    }
}

fn read_tail_lines(path: &Path, keep: usize) -> std::io::Result<Vec<String>> {
    let file = std::fs::File::open(path)?;
    let mut out = VecDeque::new();
    for line in std::io::BufReader::new(file).lines() {
        out.push_back(line?);
        while out.len() > keep {
            out.pop_front();
        }
    }
    Ok(out.into_iter().collect())
}

fn transcript_payload(line: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return line.to_string();
    };
    let mut parts = Vec::new();
    if let Some(hook) = v.get("hookName").and_then(|v| v.as_str()) {
        parts.push(hook.to_string());
    }
    if let Some(typ) = v.get("type").and_then(|v| v.as_str()) {
        parts.push(typ.to_string());
    }
    let content = v
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| v.get("content"));
    if let Some(blocks) = content.and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|v| v.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                Some("tool_use") => {
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                    parts.push(format!("tool:{name}"));
                }
                Some(other) => parts.push(other.to_string()),
                None => {}
            }
        }
    } else if let Some(text) = content.and_then(|c| c.as_str()) {
        parts.push(text.to_string());
    }
    if parts.is_empty() {
        line.to_string()
    } else {
        parts.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn proc(
        pid: u32,
        agent: AgentKind,
        cwd: &str,
        start: &str,
        tty: Option<&str>,
    ) -> LiveAgentProcess {
        LiveAgentProcess {
            pid,
            agent,
            cwd: PathBuf::from(cwd),
            stale_cwd: false,
            start_time: Some(start.parse().unwrap()),
            tty: tty.map(str::to_string),
        }
    }

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn no_facts(_: u32) -> PidFacts {
        (None, None)
    }

    fn transcript<'a>(cwd: &'a Path, started: &str, age: u64) -> TranscriptFacts<'a> {
        TranscriptFacts {
            agent: "claude",
            cwd: Some(cwd),
            started_at: Some(ts(started)),
            age_seconds: age,
        }
    }

    /// Acceptance 7a: a covering lease's `active_pid` wins over a proc-scan
    /// that would otherwise match a different process in the same cwd.
    #[test]
    fn resolver_prefers_lease_active_pid_over_proc_scan() {
        let cwd = PathBuf::from("/w/aida");
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 10);
        let procs = vec![
            proc(
                4242,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T11:59:30Z",
                Some("/dev/pts/9"),
            ),
            proc(
                5151,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T11:59:40Z",
                Some("/dev/pts/1"),
            ),
        ];
        let lease = LeaseFacts {
            pid: Some(5151),
            role: Some("general-purpose".into()),
        };
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(
            &t,
            Some(&lease),
            &procs,
            &|_| true,
            &no_facts,
            &|| None,
            now,
        );
        assert_eq!(facts.pid, Some(5151));
        assert_eq!(facts.resolution, Resolution::Lease);
        assert_eq!(facts.liveness, Liveness::Alive);
        assert_eq!(facts.tty.as_deref(), Some("/dev/pts/1"));
        assert_eq!(facts.tty_cell(), "pts/1");
        assert_eq!(facts.elapsed_secs, Some(620));
        assert_eq!(facts.lease_role.as_deref(), Some("general-purpose"));
    }

    /// A lease whose pid is DEAD is not "resolved" — fall through to the scan.
    #[test]
    fn dead_lease_pid_falls_through_to_proc_scan() {
        let cwd = PathBuf::from("/w/aida");
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 10);
        let procs = vec![proc(
            4242,
            AgentKind::Claude,
            "/w/aida",
            "2026-09-08T11:59:30Z",
            None,
        )];
        let lease = LeaseFacts {
            pid: Some(999_999),
            role: Some("implementer".into()),
        };
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(
            &t,
            Some(&lease),
            &procs,
            &|_| false,
            &no_facts,
            &|| None,
            now,
        );
        assert_eq!(facts.pid, Some(4242));
        assert_eq!(facts.resolution, Resolution::ProcScan);
        assert_eq!(facts.lease_role.as_deref(), Some("implementer"));
    }

    /// Acceptance 7b: two candidates in one cwd inside the window → `?`,
    /// both pids listed, and NOT `●`.
    #[test]
    fn ambiguous_proc_scan_yields_question_mark_with_both_pids() {
        let cwd = PathBuf::from("/w/aida");
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 30);
        let procs = vec![
            proc(
                111,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T11:59:00Z",
                Some("/dev/pts/1"),
            ),
            proc(
                222,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T11:59:50Z",
                Some("/dev/pts/2"),
            ),
        ];
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(&t, None, &procs, &|_| true, &no_facts, &|| None, now);
        assert!(facts.is_ambiguous());
        assert_eq!(facts.pid, None);
        assert_eq!(facts.candidates, vec![111, 222]);
        assert_eq!(facts.pid_cell(), "?");
        assert_eq!(facts.tty_cell(), "?");
        assert_eq!(facts.resolution, Resolution::ProcScan);
        assert_eq!(
            facts.liveness,
            Liveness::FileTouched,
            "ambiguous must not claim ●"
        );
    }

    /// Acceptance 7c: a fresh transcript with no matching process is
    /// file-touched (`◐`), never alive (`●`).
    #[test]
    fn fresh_mtime_without_process_is_file_touched_not_alive() {
        let cwd = PathBuf::from("/w/aida");
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 30);
        let other_cwd = vec![proc(
            777,
            AgentKind::Claude,
            "/w/other",
            "2026-09-08T11:59:00Z",
            None,
        )];
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(&t, None, &other_cwd, &|_| true, &no_facts, &|| None, now);
        assert_eq!(facts.liveness, Liveness::FileTouched);
        assert_eq!(facts.resolution, Resolution::None);
        assert_eq!(facts.pid_cell(), "-");

        let stale = TranscriptFacts {
            age_seconds: FILE_TOUCHED_WINDOW_SECS,
            ..t.clone()
        };
        let facts = resolve(
            &stale,
            None,
            &other_cwd,
            &|_| true,
            &no_facts,
            &|| None,
            now,
        );
        assert_eq!(facts.liveness, Liveness::None);
    }

    // trace:STORY-998 | ai:codex
    #[test]
    fn live_process_can_upgrade_to_spinning_liveness() {
        let facts = ProcessFacts {
            liveness: Liveness::Alive,
            ..ProcessFacts::default()
        }
        .with_spinning(Some(SpinningFacts {
            template: "poll: no work §N jobs".to_string(),
            count: 41,
        }));
        assert_eq!(facts.liveness, Liveness::Spinning);
        assert_eq!(facts.spinning.as_ref().map(|s| s.count), Some(41));

        let unresolved = ProcessFacts {
            liveness: Liveness::FileTouched,
            ..ProcessFacts::default()
        }
        .with_spinning(Some(SpinningFacts {
            template: "poll".to_string(),
            count: 41,
        }));
        assert_eq!(unresolved.liveness, Liveness::FileTouched);
        assert!(unresolved.spinning.is_none());
    }

    // trace:STORY-998 | ai:codex
    #[test]
    fn transcript_tail_detects_spinning_without_json_envelope_churn() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let now = ts("2026-09-08T12:02:00Z");
        for i in 0..100 {
            let ts = (now - chrono::Duration::seconds(100 - i)).to_rfc3339();
            writeln!(
                tmp.as_file(),
                r#"{{"timestamp":"{}","type":"assistant","message":{{"content":[{{"type":"text","text":"poll: no work 0 jobs"}}]}}}}"#,
                ts
            )
            .unwrap();
        }
        let mut cfg = aida_core::idle::IdleConfig::default();
        cfg.spinning_after = Duration::from_secs(60);
        let spin = transcript_spinning_with_config(tmp.path(), now, cfg)
            .expect("repeated transcript tail should be spinning");
        assert!(spin.template.contains("poll: no work"), "{spin:?}");
        assert!(spin.count >= 60, "{spin:?}");
    }

    /// The start-time window disambiguates the common two-tabs-same-dir case:
    /// an OLD transcript must not adopt a process that started after its last
    /// launch/resume event, and a NEW transcript must not adopt a process that
    /// started before it began.
    #[test]
    fn start_window_disambiguates_two_sessions_in_one_cwd() {
        let cwd = PathBuf::from("/w/aida");
        let procs = vec![
            proc(
                100,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T09:00:00Z",
                Some("/dev/pts/0"),
            ),
            proc(
                200,
                AgentKind::Claude,
                "/w/aida",
                "2026-09-08T12:00:00Z",
                Some("/dev/pts/3"),
            ),
        ];
        let now = ts("2026-09-08T12:10:00Z");

        // Old transcript: began 09:00:10, last SessionStart hook at 09:00:12.
        let old = transcript(&cwd, "2026-09-08T09:00:10Z", 5);
        let facts = resolve(
            &old,
            None,
            &procs,
            &|_| true,
            &no_facts,
            &|| Some(ts("2026-09-08T09:00:12Z")),
            now,
        );
        assert_eq!(facts.pid, Some(100));
        assert_eq!(facts.liveness, Liveness::Alive);
        assert_eq!(facts.elapsed_secs, Some(3 * 3600 + 600));

        // New transcript: began 12:00:05 — pid 100 started hours earlier.
        let new = transcript(&cwd, "2026-09-08T12:00:05Z", 5);
        let facts = resolve(
            &new,
            None,
            &procs,
            &|_| true,
            &no_facts,
            &|| Some(ts("2026-09-08T12:00:07Z")),
            now,
        );
        assert_eq!(facts.pid, Some(200));
        assert_eq!(facts.tty_cell(), "pts/3");
    }

    /// A resumed transcript: its original process is gone, the resuming
    /// process started well after the first event but before the last
    /// `SessionStart:resume` — that process is the owner.
    #[test]
    fn resumed_transcript_adopts_the_resuming_process() {
        let cwd = PathBuf::from("/w/aida");
        let procs = vec![proc(
            300,
            AgentKind::Claude,
            "/w/aida",
            "2026-09-08T11:30:00Z",
            Some("/dev/pts/5"),
        )];
        let t = transcript(&cwd, "2026-09-05T08:00:00Z", 20);
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(
            &t,
            None,
            &procs,
            &|_| true,
            &no_facts,
            &|| Some(ts("2026-09-08T11:30:04Z")),
            now,
        );
        assert_eq!(facts.pid, Some(300));
        assert_eq!(facts.resolution, Resolution::ProcScan);
    }

    /// Agent kind is part of the match: a codex process never backs a claude
    /// transcript, and vice versa.
    #[test]
    fn agent_kind_must_match() {
        let cwd = PathBuf::from("/w/aida");
        let procs = vec![proc(
            400,
            AgentKind::Codex,
            "/w/aida",
            "2026-09-08T11:59:00Z",
            None,
        )];
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 20);
        let now = ts("2026-09-08T12:10:00Z");
        let facts = resolve(&t, None, &procs, &|_| true, &no_facts, &|| None, now);
        assert_eq!(facts.pid, None);
        let codex = TranscriptFacts {
            agent: "codex",
            ..t.clone()
        };
        let facts = resolve(&codex, None, &procs, &|_| true, &no_facts, &|| None, now);
        assert_eq!(facts.pid, Some(400));
    }

    /// The lazy start-event lookup is NOT invoked for a row with no
    /// cwd-matched candidate (cost guard: no transcript re-read per row).
    #[test]
    fn start_event_lookup_is_lazy() {
        let cwd = PathBuf::from("/w/aida");
        let t = transcript(&cwd, "2026-09-08T12:00:00Z", 20);
        let now = ts("2026-09-08T12:10:00Z");
        let called = std::cell::Cell::new(false);
        let _ = resolve(
            &t,
            None,
            &[],
            &|_| true,
            &no_facts,
            &|| {
                called.set(true);
                None
            },
            now,
        );
        assert!(!called.get());
    }

    /// Backward chunk scan picks the LAST launch/resume hook event and ignores
    /// a later `SessionStart:compact` (same process).
    #[test]
    fn last_process_start_event_reads_last_startup_or_resume_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let line = |hook: &str, ts: &str| {
            format!(
                "{{\"parentUuid\":null,\"attachment\":{{\"type\":\"hook_success\",\"hookName\":\"{hook}\",\"hookEvent\":\"SessionStart\"}},\"type\":\"attachment\",\"uuid\":\"u\",\"timestamp\":\"{ts}\",\"cwd\":\"/w\"}}\n"
            )
        };
        let mut body = String::new();
        body.push_str(&line("SessionStart:startup", "2026-09-05T08:00:00.000Z"));
        for i in 0..3000 {
            body.push_str(&format!(
                "{{\"type\":\"assistant\",\"timestamp\":\"2026-09-05T09:{:02}:00.000Z\",\"message\":\"{}\"}}\n",
                i % 60,
                "x".repeat(400)
            ));
        }
        body.push_str(&line("SessionStart:resume", "2026-09-08T11:30:04.000Z"));
        body.push_str(&line("SessionStart:compact", "2026-09-08T11:45:00.000Z"));
        for _ in 0..10 {
            body.push_str("{\"type\":\"user\",\"timestamp\":\"2026-09-08T11:50:00.000Z\"}\n");
        }
        std::fs::write(&path, body).unwrap();
        assert!(
            path.metadata().unwrap().len() > 1024 * 1024,
            "spans >1 chunk"
        );
        assert_eq!(
            last_process_start_event(&path),
            Some(ts("2026-09-08T11:30:04Z"))
        );
        // No marker at all → None (caller falls back to "no upper bound").
        let bare = tmp.path().join("bare.jsonl");
        std::fs::write(
            &bare,
            "{\"type\":\"user\",\"timestamp\":\"2026-09-08T11:50:00.000Z\"}\n",
        )
        .unwrap();
        assert_eq!(last_process_start_event(&bare), None);
    }

    /// The backward scan is linear in the bytes read: a marker ~6 chunks from
    /// the end of a large transcript is found without re-searching the whole
    /// accumulated buffer per chunk, and a marker that straddles a 1 MiB
    /// chunk edge is still found.
    #[test]
    fn last_process_start_event_is_linear_and_handles_chunk_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.jsonl");
        let filler = format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-09-05T09:00:00.000Z\",\"message\":\"{}\"}}\n",
            "y".repeat(1000)
        );
        let marker = "{\"attachment\":{\"type\":\"hook_success\",\"hookName\":\"SessionStart:resume\"},\"type\":\"attachment\",\"timestamp\":\"2026-09-08T11:30:04.000Z\"}\n";
        // Place the marker so that it straddles a chunk edge: the tail after
        // it must be N MiB minus a few bytes into the marker.
        let mut body = String::new();
        while body.len() < 2 * 1024 * 1024 {
            body.push_str(&filler);
        }
        body.push_str(marker);
        // Pad the tail so a chunk boundary (measured from EOF) lands ~20 bytes
        // into the marker.
        let target_tail = 6 * 1024 * 1024 + (marker.len() - 20);
        let mut tail = String::new();
        while tail.len() + filler.len() <= target_tail {
            tail.push_str(&filler);
        }
        let pad = target_tail - tail.len();
        tail.push_str(&format!(
            "{{\"type\":\"user\",\"timestamp\":\"2026-09-08T11:50:00.000Z\",\"p\":\"{}\"}}\n",
            "z".repeat(pad.saturating_sub(60))
        ));
        body.push_str(&tail);
        std::fs::write(&path, body).unwrap();
        let t0 = std::time::Instant::now();
        assert_eq!(
            last_process_start_event(&path),
            Some(ts("2026-09-08T11:30:04Z"))
        );
        // Generous bound: quadratic re-search of a growing 7 MiB buffer took
        // ~70 ms in release on the real 18 MB transcript; linear is ~10 ms.
        // Debug builds are slower, so only guard against the pathological
        // shape, not the exact figure.
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(5),
            "backward scan too slow: {:?}",
            t0.elapsed()
        );
    }

    #[test]
    fn short_tty_strips_dev_prefix() {
        assert_eq!(short_tty("/dev/pts/3"), "pts/3");
        assert_eq!(short_tty("pts/3"), "pts/3");
    }
}
