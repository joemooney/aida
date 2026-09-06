//! `aida tail <session|spec|drain>` — stream a live session's log, resolved by
//! an id the operator already has instead of a file path they shouldn't have to
//! know.
//!
//! `aida ps` answers *what is running*; `aida drain status` answers *is a drain
//! up*. Neither answers *show me what THIS one is doing right now* without the
//! operator first working out which file under `.aida/` the session writes to
//! (`.aida/burndown/<drain-id>.jsonl` for a verbose drain,
//! `.aida/headless-logs/<branch>-<session-uuid>.jsonl` for a headless
//! implementer/reviewer/advisor tier) and tailing it by hand.
//!
//! This module is the resolver in front of the existing renderer: it maps a
//! selector to exactly one log file and hands it to
//! [`crate::headless_tail::stream_path`], so the rendering, the `--since`
//! filter, the follow loop, and the SIGINT trap are shared code rather than a
//! second implementation.
//!
//! Resolution order for a selector:
//!   1. `drain` / `burndown` — the newest drain log.
//!   2. A session id from `aida ps` (exact, or an unambiguous prefix).
//!   3. A drain id (the log's filename stem, or a substring of it).
//!   4. A spec id — the newest log belonging to that spec.
//!
//! Nothing running with no log is NOT an error: an interactive session writes
//! no JSONL, so we say so and exit clean. One logless shape is special, though:
//! a fan-out worker of a LIVE drain (an Agent-tool subagent, which streams into
//! its parent's context rather than to a file). "No log" is true there but a
//! dead end — the stream the operator wants exists, it is the drain's. That case
//! redirects instead of shrugging. trace:BUG-782
//!
//! trace:TASK-1167

use anyhow::{bail, Result};
use colored::Colorize;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::headless_tail::{self, FormatOpts, LogEntry, StreamOpts};

/// Caller-supplied options, mirroring the clap flags one-for-one.
// trace:TASK-1167 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct TailOptions {
    /// Session id, spec id, drain id, `drain`, or `None` for the newest log.
    pub target: Option<String>,
    /// List the tailable logs and exit.
    pub list: bool,
    /// Pass the raw stream-json through instead of rendering phase lines.
    pub json: bool,
    /// Replay at most the last N rendered lines before going live.
    pub lines: Option<usize>,
    /// Drop events older than this.
    pub since: Option<Duration>,
    /// Print what is already there and exit instead of following.
    pub no_follow: bool,
    /// Interleave tool invocations with assistant text.
    pub with_tools: bool,
    /// Colorize (auto-disabled off a TTY / under `NO_COLOR`).
    pub color: bool,
    /// Drop the per-line `[HH:MM:SS]` event-time prefix (clean copy-paste).
    // trace:TASK-1173 | ai:claude
    pub no_timestamp: bool,
}

/// One running session, projected from the session lease to what the resolver
/// needs. Mirrors the `aida ps` row so the id the operator copies from that
/// table is the id this command accepts.
// trace:TASK-1167 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    /// The 12-char lease id — the SESSION column of `aida ps`.
    pub id: String,
    /// The lease scope: a spec id for spec-scoped work, else a scope slug.
    pub scope: String,
    /// The worktree branch, which is also the headless log's filename prefix.
    pub branch: String,
    /// The seat this session holds, when it declared one.
    pub role: Option<String>,
}

/// One drain log under `.aida/burndown/`.
// trace:TASK-1167 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainLog {
    /// The filename stem — the drain id minted at launch.
    pub id: String,
    pub path: PathBuf,
    pub mtime: SystemTime,
}

/// The currently running queue-work drain, projected from drain-state.
// trace:BUG-842 | ai:codex
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDrain {
    /// Current member, e.g. `BUG-842`.
    pub current: Option<String>,
    /// Current phase, e.g. `1 (implementer)`.
    pub phase: Option<String>,
    /// Current headless phase session id, when the live phase writes one.
    pub session_id: Option<String>,
}

/// Everything the resolver reads, gathered once so the resolution itself is a
/// pure function over in-memory data (and therefore unit-testable with no
/// filesystem, no cwd, no git).
// trace:TASK-1167 | ai:claude
#[derive(Debug, Clone, Default)]
pub struct TailIndex {
    /// Drain logs, newest first.
    pub drains: Vec<DrainLog>,
    /// Headless session logs, newest first.
    pub headless: Vec<LogEntry>,
    /// Live + recorded session leases.
    pub sessions: Vec<SessionRef>,
    /// Is a drain holding this repo's drain lock right now? The "is there a
    /// parent stream carrying this worker?" input to the fan-out redirect —
    /// gathered here so the resolution itself stays pure.
    // trace:BUG-782 | ai:claude
    pub drain_live: bool,
    /// PID-corroborated live drain-state. Unlike `.aida/burndown/`, this sees
    /// queue-work drains whose phase logs live under `.aida/headless-logs/`.
    // trace:BUG-842 | ai:codex
    pub live_drain: Option<LiveDrain>,
}

/// What a selector resolved to.
// trace:TASK-1167 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A log file to stream, with a human label for the `==>` header.
    Found {
        path: PathBuf,
        label: String,
        notice: Option<String>,
    },
    /// The selector named something real that simply has no log — an
    /// interactive session, or a drain that never ran verbose. Reported
    /// cleanly, not as an error.
    NoLog { what: String, hint: String },
    /// The selector named a fan-out worker of the drain running right now: an
    /// Agent-tool subagent whose output streams into the drain's context, not
    /// to a file of its own. Not an error and not a dead end — a pointer at the
    /// stream that does carry it. `drain` is the log `aida tail drain` would
    /// pick, or `None` when the live drain is writing no log at all.
    // trace:BUG-782 | ai:claude
    FanoutOfDrain { what: String, drain: Option<String> },
    /// The selector matched nothing.
    NotFound { message: String },
}

/// Minimum prefix length before a session id is matched by prefix, so a stray
/// one-character argument can't silently select a session.
const MIN_SESSION_PREFIX: usize = 4;

/// Resolve a selector against the gathered index. Pure.
// trace:TASK-1167 | ai:claude
pub fn resolve(index: &TailIndex, selector: Option<&str>) -> Resolution {
    let Some(sel) = selector.map(str::trim).filter(|s| !s.is_empty()) else {
        return resolve_newest(index);
    };

    if sel.eq_ignore_ascii_case("drain") || sel.eq_ignore_ascii_case("burndown") {
        return resolve_drain_keyword(index);
    }

    // A session id from `aida ps`.
    match match_sessions(index, sel) {
        SessionMatch::One(s) => return resolve_session(index, s),
        SessionMatch::Ambiguous(ids) => {
            return Resolution::NotFound {
                message: format!(
                    "`{}` matches {} sessions ({}) — pass more of the id.",
                    sel,
                    ids.len(),
                    ids.join(", ")
                ),
            }
        }
        SessionMatch::None => {}
    }

    // A drain id (its log's filename stem, or enough of it to be unique). The
    // substring form needs a few characters so a stub argument can't sweep in
    // the newest drain by accident.
    let drain_hits: Vec<&DrainLog> = index
        .drains
        .iter()
        .filter(|d| {
            d.id.eq_ignore_ascii_case(sel)
                || (sel.len() >= MIN_SESSION_PREFIX && d.id.contains(sel))
        })
        .collect();
    if let Some(d) = drain_hits.first() {
        return Resolution::Found {
            path: d.path.clone(),
            label: format!("drain {}", d.id),
            notice: None,
        };
    }

    // A spec id — the newest log that belongs to it.
    let sel_upper = sel.to_uppercase();
    if let Some(entry) = index.headless.iter().find(|e| entry_matches_spec(e, sel)) {
        return Resolution::Found {
            path: entry.path.clone(),
            label: log_label(entry),
            notice: None,
        };
    }
    if let Some(s) = index
        .sessions
        .iter()
        .find(|s| s.scope.to_uppercase() == sel_upper)
    {
        return no_log_for_session(index, s);
    }

    Resolution::NotFound {
        message: format!(
            "nothing to tail for `{}` — `aida ps` lists the running sessions, and `aida tail --list` lists every log this project has.",
            sel
        ),
    }
}

// trace:BUG-842 | ai:codex
fn resolve_drain_keyword(index: &TailIndex) -> Resolution {
    if let Some(live) = &index.live_drain {
        let Some(spec) = live.current.as_deref().filter(|s| !s.is_empty()) else {
            return Resolution::NoLog {
                what: "the live drain".to_string(),
                hint: "the drain is live but has not started a member log yet.".to_string(),
            };
        };
        if let Some(entry) = live
            .session_id
            .as_deref()
            .and_then(|session_id| headless_for_session(index, session_id))
        {
            return Resolution::Found {
                path: entry.path.clone(),
                label: live_drain_label(live, entry),
                notice: None,
            };
        }
        return Resolution::NoLog {
            what: format!("the live drain ({spec})"),
            hint: "no headless log exists for the drain's current member yet.".to_string(),
        };
    }

    match index.drains.first() {
        Some(d) => Resolution::Found {
            path: d.path.clone(),
            label: format!("drain {}", d.id),
            notice: Some(format!(
                "no live drain — showing {} (finished {})",
                d.id,
                fmt_date(d.mtime)
            )),
        },
        None => Resolution::NoLog {
            what: "the drain".to_string(),
            hint: "no drain has written a log in this project yet — a drain only streams to one when it runs with verbose output.".to_string(),
        },
    }
}

/// No selector: the most recently written log of any kind.
fn resolve_newest(index: &TailIndex) -> Resolution {
    let newest_drain = index.drains.first();
    let newest_headless = index.headless.first();
    let drain_time = newest_drain.map(|d| d.mtime).unwrap_or(UNIX_EPOCH);
    let headless_time = newest_headless.map(|e| e.mtime).unwrap_or(UNIX_EPOCH);
    match (newest_drain, newest_headless) {
        (Some(d), Some(e)) => {
            if drain_time >= headless_time {
                Resolution::Found {
                    path: d.path.clone(),
                    label: format!("drain {}", d.id),
                    notice: None,
                }
            } else {
                Resolution::Found {
                    path: e.path.clone(),
                    label: log_label(e),
                    notice: None,
                }
            }
        }
        (Some(d), None) => Resolution::Found {
            path: d.path.clone(),
            label: format!("drain {}", d.id),
            notice: None,
        },
        (None, Some(e)) => Resolution::Found {
            path: e.path.clone(),
            label: log_label(e),
            notice: None,
        },
        (None, None) => Resolution::NoLog {
            what: "this project".to_string(),
            hint: "no session has written a log here yet — headless work streams to one, an interactive session does not.".to_string(),
        },
    }
}

enum SessionMatch<'a> {
    None,
    One(&'a SessionRef),
    Ambiguous(Vec<String>),
}

fn match_sessions<'a>(index: &'a TailIndex, sel: &str) -> SessionMatch<'a> {
    if let Some(s) = index
        .sessions
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(sel))
    {
        return SessionMatch::One(s);
    }
    if sel.len() < MIN_SESSION_PREFIX {
        return SessionMatch::None;
    }
    let lower = sel.to_lowercase();
    let hits: Vec<&SessionRef> = index
        .sessions
        .iter()
        .filter(|s| s.id.to_lowercase().starts_with(&lower))
        .collect();
    match hits.len() {
        0 => SessionMatch::None,
        1 => SessionMatch::One(hits[0]),
        _ => SessionMatch::Ambiguous(hits.iter().map(|s| s.id.clone()).collect()),
    }
}

/// The log a session writes to: its branch names the file, and the scope is the
/// fallback when the branch doesn't (an adopted or renamed worktree).
fn resolve_session(index: &TailIndex, session: &SessionRef) -> Resolution {
    if let Some(entry) = session_log(index, session) {
        return Resolution::Found {
            path: entry.path.clone(),
            label: format!("{} · {}", session.id, log_label(entry)),
            notice: None,
        };
    }
    no_log_for_session(index, session)
}

/// The newest headless log belonging to `session`, if any.
// trace:TASK-1167 | ai:claude
pub fn session_log<'a>(index: &'a TailIndex, session: &SessionRef) -> Option<&'a LogEntry> {
    if !session.branch.is_empty() {
        let prefix = format!("{}-", session.branch.to_lowercase());
        if let Some(e) = index
            .headless
            .iter()
            .find(|e| e.filename.to_lowercase().starts_with(&prefix))
        {
            return Some(e);
        }
    }
    if !session.scope.is_empty() {
        if let Some(e) = index
            .headless
            .iter()
            .find(|e| entry_matches_spec(e, &session.scope))
        {
            return Some(e);
        }
    }
    None
}

// trace:BUG-872 | ai:codex
fn headless_for_session<'a>(index: &'a TailIndex, session_id: &str) -> Option<&'a LogEntry> {
    index
        .headless
        .iter()
        .find(|e| e.lease.as_deref() == Some(session_id))
}

fn no_log_for_session(index: &TailIndex, session: &SessionRef) -> Resolution {
    let seat = session
        .role
        .as_deref()
        .filter(|r| !r.is_empty())
        .map(|r| format!(", {r}"))
        .unwrap_or_default();
    let what = format!("{} ({}{})", session.id, session.scope, seat);
    // A logless harness lease while a drain holds the lock is not the
    // interactive case — it is one of that drain's fan-out workers, and the
    // drain's own stream carries it. Say so instead of dead-ending.
    // trace:BUG-782 | ai:claude
    if index.drain_live && is_fanout_worker(session) {
        return Resolution::FanoutOfDrain {
            what,
            drain: index.drains.first().map(|d| d.id.clone()),
        };
    }
    Resolution::NoLog {
        what,
        hint: "that session is running without a log — an interactive session streams to its terminal, not to a file. `aida ps` shows its worktree and pid.".to_string(),
    }
}

/// The harness's fallback `agent_type` for an Agent-tool subagent, which the
/// SubagentStart lease writer records in the lease's role slot.
// trace:BUG-782 | ai:claude
const HARNESS_AGENT_TYPE: &str = "general-purpose";

/// Branch-name prefix the Agent-tool harness gives its isolation worktrees.
// trace:BUG-782 | ai:claude
const HARNESS_BRANCH_PREFIX: &str = "worktree-agent-";

/// Does this lease have the shape the Agent-tool harness mints for a fan-out
/// subagent? Any one of three independent markers is conclusive: the generic
/// `harness-worktree` scope (the worktree's branch carried no SPEC-ID), the
/// harness's fallback agent type in the role slot, or a harness-named branch. A
/// spec-scoped fan-out shows only the latter two, so all three are checked.
///
/// Only ever consulted for a session with NO log of its own, so a headless
/// implementer (which writes one) can never be swept in here.
// trace:BUG-782 | ai:claude
fn is_fanout_worker(session: &SessionRef) -> bool {
    session
        .scope
        .eq_ignore_ascii_case(crate::worktree_lease::HARNESS_WORKTREE_SCOPE)
        || session
            .role
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case(HARNESS_AGENT_TYPE))
        || session
            .branch
            .to_lowercase()
            .starts_with(HARNESS_BRANCH_PREFIX)
}

/// Does this log belong to `spec`? Matches the spec parsed out of the filename,
/// or — only for something actually shaped like a spec id — the id appearing
/// anywhere in the filename (which covers both the `task-1167-<uuid>` branch
/// form and the `advise-TASK-1167-<id>` form). The shape guard matters: a bare
/// substring test would let a one-character argument match every log.
fn entry_matches_spec(entry: &LogEntry, spec: &str) -> bool {
    if entry
        .spec
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case(spec))
    {
        return true;
    }
    looks_like_spec_id(spec) && entry.filename.to_uppercase().contains(&spec.to_uppercase())
}

/// `TASK-1167`, `BUG-89`, `FR-1-042` — a letter-prefixed, hyphenated id with a
/// digit after the first hyphen.
fn looks_like_spec_id(s: &str) -> bool {
    let Some((head, tail)) = s.split_once('-') else {
        return false;
    };
    !head.is_empty()
        && head.chars().all(|c| c.is_ascii_alphabetic())
        && tail.chars().next().is_some_and(|c| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn log_label(entry: &LogEntry) -> String {
    match (entry.spec.as_deref(), entry.kind.as_str()) {
        (Some(spec), "task") => spec.to_string(),
        (Some(spec), kind) => format!("{spec} ({kind})"),
        (None, _) => entry.filename.clone(),
    }
}

// trace:BUG-842 | ai:codex
fn live_drain_label(live: &LiveDrain, entry: &LogEntry) -> String {
    match (&live.current, &live.phase) {
        (Some(spec), Some(phase)) => format!("live drain {spec} phase {phase}"),
        (Some(spec), None) => format!("live drain {spec}"),
        (None, _) => format!("live drain {}", log_label(entry)),
    }
}

fn fmt_date(t: SystemTime) -> String {
    let when: chrono::DateTime<chrono::Local> = t.into();
    when.format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Gathering
// ---------------------------------------------------------------------------

/// Read the drain logs, the headless logs, the session leases, and whether a
/// drain currently holds the lock.
// trace:TASK-1167 | ai:claude
pub fn build_index(project_root: &Path, sessions: Vec<SessionRef>) -> TailIndex {
    let live_drain = match crate::drain_state::probe(project_root) {
        crate::drain_state::DrainStatus::Active(state) => Some(LiveDrain {
            current: state.current,
            phase: state.current_phase,
            session_id: state.current_session_id,
        }),
        crate::drain_state::DrainStatus::None | crate::drain_state::DrainStatus::Stale(_) => None,
    };
    TailIndex {
        drains: discover_drain_logs(&project_root.join(".aida").join("burndown")),
        headless: headless_tail::discover_logs(&project_root.join(".aida").join("headless-logs"))
            .unwrap_or_default(),
        sessions,
        // The same pid-corroborated read `aida drain status` uses, so a crashed
        // drain's leftover lock never fakes a live parent stream.
        // trace:BUG-782 | ai:claude
        drain_live: matches!(
            crate::drain_lock::probe_lock(project_root),
            crate::drain_lock::LockStatus::Running(_)
        ),
        live_drain,
    }
}

/// Every `*.jsonl` under `.aida/burndown/`, newest first.
// trace:TASK-1167 | ai:claude
pub fn discover_drain_logs(dir: &Path) -> Vec<DrainLog> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<DrainLog> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push(DrainLog {
            id,
            path,
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
        });
    }
    out.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    out
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// Top-level entry point.
// trace:TASK-1167 | ai:claude
pub fn handle_tail(
    project_root: &Path,
    sessions: Vec<SessionRef>,
    opts: &TailOptions,
) -> Result<()> {
    let index = build_index(project_root, sessions);

    if opts.list {
        return print_list(&index, opts.color);
    }

    match resolve(&index, opts.target.as_deref()) {
        Resolution::Found {
            path,
            label,
            notice,
        } => {
            let since_cutoff = opts.since.and_then(|d| {
                chrono::DateTime::<chrono::Utc>::from(SystemTime::now()).checked_sub_signed(
                    chrono::Duration::from_std(d).unwrap_or_else(|_| chrono::Duration::zero()),
                )
            });
            let format_opts = FormatOpts {
                with_tools: opts.with_tools,
                tools_only: false,
                include_user: false,
                color: opts.color,
                since: since_cutoff,
                // On by default here: without a clock the stream reads as one
                // undifferentiated flow, so "is it moving or stalled?" is
                // unanswerable. `--json` is a raw passthrough and never
                // rendered, so the flag is moot on that path.
                // trace:TASK-1173 | ai:claude
                timestamps: !opts.no_timestamp,
            };
            let stream_opts = StreamOpts {
                follow: !opts.no_follow,
                backlog_lines: opts.lines,
                raw: opts.json,
            };
            if let Some(notice) = notice {
                eprintln!("{notice}");
            }
            eprintln!(
                "{} {} {}",
                "==>".dimmed(),
                label,
                path.display().to_string().dimmed()
            );
            if is_drain_selector(opts.target.as_deref())
                && stream_opts.follow
                && index.live_drain.is_some()
            {
                return stream_live_drain(project_root, &format_opts, &stream_opts);
            }
            headless_tail::stream_path(&path, &format_opts, &stream_opts)
        }
        Resolution::NoLog { what, hint } => {
            println!("No live log for {what}.");
            println!("{hint}");
            Ok(())
        }
        // A redirect, not a failure — the operator asked a reasonable question
        // and gets the stream that answers it, so this exits 0 like `NoLog`.
        // trace:BUG-782 | ai:claude
        Resolution::FanoutOfDrain { what, drain } => {
            let info = crate::glyph(crate::glyphs::Glyph::Info);
            let arrow = crate::glyph(crate::glyphs::Glyph::SubArrow);
            println!("{info} {what} is a fan-out worker of the drain running here.");
            println!("Its output streams into the drain, not into a log of its own.");
            match drain {
                Some(id) => println!("{arrow} tail the stream that carries it: `aida tail drain` (currently {id})"),
                None => println!(
                    "{arrow} that drain is writing no log — a drain only streams to one when it runs with verbose output, so its live output is in the terminal it was launched from."
                ),
            }
            Ok(())
        }
        Resolution::NotFound { message } => bail!(message),
    }
}

fn is_drain_selector(selector: Option<&str>) -> bool {
    selector
        .map(str::trim)
        .is_some_and(|s| s.eq_ignore_ascii_case("drain") || s.eq_ignore_ascii_case("burndown"))
}

// Follow the current member log named by drain-state, switching as the drain
// crosses spec or phase-session boundaries. trace:BUG-842 | ai:codex
fn stream_live_drain(project_root: &Path, opts: &FormatOpts, stream: &StreamOpts) -> Result<()> {
    let mut current_path: Option<PathBuf> = None;
    let mut current_spec: Option<String> = None;
    let mut current_phase: Option<String> = None;
    let mut pos: u64 = 0;
    let mut malformed: u64 = 0;
    let mut emitted: u64 = 0;
    let mut initial_backlog = stream.backlog_lines;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        let state = match crate::drain_state::probe(project_root) {
            crate::drain_state::DrainStatus::Active(state) => state,
            crate::drain_state::DrainStatus::None | crate::drain_state::DrainStatus::Stale(_) => {
                break;
            }
        };
        let Some(spec) = state.current.as_deref() else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        let logs = headless_tail::discover_logs(&project_root.join(".aida").join("headless-logs"))
            .unwrap_or_default();
        let idx = TailIndex {
            drains: Vec::new(),
            headless: logs,
            sessions: Vec::new(),
            drain_live: true,
            live_drain: Some(LiveDrain {
                current: Some(spec.to_string()),
                phase: state.current_phase.clone(),
                session_id: state.current_session_id.clone(),
            }),
        };
        let Some(entry) = idx
            .live_drain
            .as_ref()
            .and_then(|live| live.session_id.as_deref())
            .and_then(|session_id| headless_for_session(&idx, session_id))
        else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };

        if current_path.as_deref() != Some(entry.path.as_path()) {
            if current_path.is_some() {
                writeln!(
                    out,
                    "-- phase/spec boundary: now following {} phase {} --",
                    spec,
                    state.current_phase.as_deref().unwrap_or("?")
                )?;
            }
            current_path = Some(entry.path.clone());
            current_spec = Some(spec.to_string());
            current_phase = state.current_phase.clone();
            pos = 0;
        } else if current_spec.as_deref() != Some(spec)
            || current_phase.as_deref() != state.current_phase.as_deref()
        {
            current_spec = Some(spec.to_string());
            current_phase = state.current_phase.clone();
        }

        let _saw_result = stream_path_once(
            &entry.path,
            &mut pos,
            opts,
            stream.raw,
            initial_backlog.take(),
            &mut malformed,
            &mut emitted,
            &mut out,
        )?;
        std::thread::sleep(Duration::from_millis(250));
    }

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

fn stream_path_once(
    path: &Path,
    pos: &mut u64,
    opts: &FormatOpts,
    raw: bool,
    backlog_lines: Option<usize>,
    malformed: &mut u64,
    emitted: &mut u64,
    out: &mut std::io::StdoutLock<'_>,
) -> Result<bool> {
    let mut file = fs::File::open(path)?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(*pos);
    if file_len < *pos {
        *pos = 0;
    }
    file.seek(SeekFrom::Start(*pos))?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    let mut rendered = Vec::new();
    let mut saw_result = false;

    loop {
        buf.clear();
        let read = reader.read_line(&mut buf)?;
        if read == 0 {
            break;
        }
        if !buf.ends_with('\n') {
            break;
        }
        *pos += read as u64;
        let raw_line = buf.trim_end_matches('\n');
        if raw {
            rendered.push(raw_line.to_string());
            continue;
        }
        let fmt = headless_tail::format_line(raw_line, opts);
        if fmt.malformed {
            *malformed += 1;
            continue;
        }
        rendered.extend(fmt.lines);
        if fmt.is_result {
            saw_result = true;
        }
    }

    let start = backlog_lines
        .map(|keep| rendered.len().saturating_sub(keep))
        .unwrap_or(0);
    for line in &rendered[start..] {
        writeln!(out, "{line}")?;
        *emitted += 1;
    }
    Ok(saw_result)
}

fn print_list(index: &TailIndex, color: bool) -> Result<()> {
    if index.live_drain.is_none()
        && index.drains.is_empty()
        && index.headless.is_empty()
        && index.sessions.is_empty()
    {
        println!("Nothing to tail — no drain logs, no session logs, no running sessions.");
        return Ok(());
    }

    let head = |s: &str| {
        if color {
            println!("{}", s.bold());
        } else {
            println!("{s}");
        }
    };

    if index.live_drain.is_some() || !index.drains.is_empty() {
        head("Drains");
        if let Some(live) = &index.live_drain {
            let current = live.current.as_deref().unwrap_or("-");
            let phase = live.phase.as_deref().unwrap_or("-");
            println!("  live  {current}  {phase}");
        }
        for d in index.drains.iter().take(5) {
            let when: chrono::DateTime<chrono::Local> = d.mtime.into();
            println!("  {}  {}", d.id, when.format("%Y-%m-%d %H:%M:%S"));
        }
        println!("  (tail the newest with `aida tail drain`)");
        println!();
    }

    if !index.sessions.is_empty() {
        // Only sessions that actually resolve to a log earn a row; the rest
        // (interactive seats, harness worktrees, ended sessions) collapse to a
        // one-line count so the table stays scannable. Newest log first.
        let mut with_log: Vec<(&SessionRef, &LogEntry)> = index
            .sessions
            .iter()
            .filter_map(|s| session_log(index, s).map(|e| (s, e)))
            .collect();
        with_log.sort_by(|a, b| b.1.mtime.cmp(&a.1.mtime));
        let without = index.sessions.len() - with_log.len();

        head("Sessions");
        let id_width = with_log.iter().map(|(s, _)| s.id.len()).max().unwrap_or(4);
        for (s, entry) in &with_log {
            println!(
                "  {:<id_width$}  {:<16}  {}",
                s.id,
                if s.scope.is_empty() { "-" } else { &s.scope },
                entry.filename
            );
        }
        if without > 0 {
            println!(
                "  ({without} more session{} writing no log — interactive seats stream to their terminal)",
                if without == 1 { "" } else { "s" }
            );
        }
        println!();
    }

    if !index.headless.is_empty() {
        head("Recent session logs");
        for e in index.headless.iter().take(10) {
            let when: chrono::DateTime<chrono::Local> = e.mtime.into();
            println!(
                "  {:<16} {:<8} {}",
                e.spec.as_deref().unwrap_or("-"),
                e.kind,
                when.format("%Y-%m-%d %H:%M:%S")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/tail_cmd_tests.rs"]
mod tail_cmd_tests;
