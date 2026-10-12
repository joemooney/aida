//! ps rows, seat activity probes, running-work gathering, and integrate (STORY-1488 slice 1)
// trace:STORY-1488 | ai:claude

use crate::*;

/// The shortest unambiguous prefix of a lease id, for the `aida session end
/// <id>` hint. Reuses the same disambiguation helper the `aida session leases`
/// table uses.
// trace:STORY-694 | ai:claude
pub(crate) fn short_lease_id(l: &SessionLease, all: &[SessionLease]) -> String {
    let ids: Vec<&str> = all.iter().map(|x| x.id.as_str()).collect();
    let n = unique_prefix_len(&l.id, &ids, 8);
    l.id[..n.min(l.id.len())].to_string()
}

/// One row of the `aida ps` running-work table: a classified session lease plus
/// the spec it is linked to (if any). Built off the SAME machinery the per-spec
/// `aida status <spec>` and `aida session leases` use ([`list_leases`],
/// [`lease_state_for`], the pid probe), so the global view and the per-spec view
/// can never disagree about what is live.
// trace:STORY-696 | ai:claude
pub(crate) struct PsRow {
    pub(crate) lease: SessionLease,
    pub(crate) state: LeaseState,
    /// Display role for `aida ps`: the live transcript role wins when present
    /// because it names the session's current behavior; the raw lease role is
    /// retained separately for JSON provenance.
    // trace:TASK-152 | ai:codex
    pub(crate) role: Option<String>,
    /// Raw role recorded on the lease before any live-transcript override.
    // trace:TASK-152 | ai:codex
    pub(crate) lease_role: Option<String>,
    /// PID of the live claude inside the worktree (or the review-verb creator
    /// pid). `None` when no live process backs the lease.
    pub(crate) pid: Option<u32>,
    /// BUG-763: start time of that pid, when the probe can say. Lets the
    /// renderer surface an adopted persistent lease (pid younger than the
    /// lease) instead of silently mixing lease-age and process-age.
    pub(crate) pid_started_at: Option<chrono::DateTime<chrono::Utc>>,
    /// BUG-769: the WORK-SESSION age behind the `elapsed` column — the backing
    /// process's uptime on an adopted lease (pid younger than the lease
    /// record), the lease's own age on every other row. See
    /// [`ps_elapsed_secs`]; lease-age still shows in the adopted annotation.
    // trace:BUG-769 | ai:claude
    pub(crate) elapsed_secs: u64,
    /// The display id of the spec this lease is scoped to, when the lease scope
    /// resolves to a known spec id (the AIDA-launched happy path). `None` for
    /// generic `harness-worktree` advisor fan-out leases — surfaced honestly as
    /// "scope unknown" rather than guessed.
    // trace:STORY-696
    pub(crate) spec: Option<String>,
    /// TASK-1090: the dispatch-health classification for this row (MOVING /
    /// STALLED / SALVAGEABLE) plus the exact next command to unstick it.
    /// `None` for worktree-less advisory leases (`aida review` / `aida claim`
    /// locks) or a lease whose worktree no longer exists — there is no git
    /// state to classify.
    // trace:TASK-1090 | ai:claude
    pub(crate) dispatch: Option<PsDispatch>,
    /// TASK-1143: the advisor holding the STORY-711 worktree lock on this row's
    /// worktree, if any — the `authorized_by` value on the covering session
    /// lease, read via `worktree_lock::read_authorized_by` (the same source the
    /// `aida lock` CLI writes and `locking_gate` reads). `None` when the
    /// worktree carries no lock, which is the common case since `[locking]` is
    /// opt-in — so the locked-by column stays blank until a lock exists.
    // trace:TASK-1143 | ai:claude
    pub(crate) locked_by: Option<String>,
    /// TASK-1451: the live seat's resolved mail-sender identity source —
    /// `None` when there is no live pid backing this row (nothing to probe).
    // trace:TASK-1451 | ai:claude
    pub(crate) mail_identity: Option<MailIdentityStatus>,
    /// BUG-1553: is this LIVE seat actively working, blocked on a human
    /// approval gate, or in a state the probe cannot resolve? `None` when
    /// the row has no live pid at all (Dormant/Stale rows — already
    /// unambiguous; nothing to inspect).
    // trace:BUG-1553 | ai:claude
    pub(crate) activity: Option<SeatActivity>,
    /// The derived driver of this row's spec (relay / drain / agent / queued /
    /// idle), resolved by `driver_view::overlay_running_work`. `None` for rows
    /// with no spec or before the overlay runs. Never stored on the spec.
    // trace:STORY-1653 | ai:claude
    pub(crate) driver: Option<aida_core::driver_registry::DerivedDriver>,
}

/// The TASK-1090 dispatch-health payload for one [`PsRow`].
// trace:TASK-1090 | ai:claude
pub(crate) struct PsDispatch {
    pub(crate) state: dispatch_health_ps::DispatchState,
    /// `None` only for `Moving` — nothing to unstick.
    pub(crate) hint: Option<String>,
    pub(crate) dirty: bool,
    pub(crate) ahead_of_main: u32,
    pub(crate) untracked_only: bool,
    /// BUG-1941: when `state` is `Moving` only because a fresh commit lifted
    /// a no-process reading, the reading it lifted (`Stalled` or
    /// `Salvageable`); `None` otherwise. The integration-standing overlay
    /// judges the row by THIS state, so a commit/push/exit row can still read
    /// `awaiting-integration` while salvageable work is never reframed.
    pub(crate) commit_movement_from: Option<dispatch_health_ps::DispatchState>,
}

/// TASK-1451: whether a live seat's resolved mail identity is a stable seat
/// value or the ambiguous shell-user fallback BUG-1533 flagged in the
/// envelope — surfaced here BEFORE that seat sends any mail, so the gap is
/// visible up front instead of discovered 200 messages later.
// trace:TASK-1451 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MailIdentityStatus {
    /// `AIDA_AGENT_NAME` / `AIDA_USER` / `AIDA_SESSION_ROLE` resolves a
    /// stable seat identity — mail sent from this process is attributable.
    Attributed,
    /// None of those three are set in the process environment — mail sent
    /// from this seat would collapse to the shell-user fallback.
    Unattributed,
    /// The process environment could not be read — another user's process,
    /// the process already exited, or a non-Linux host. Reported as
    /// "identity unknown" (PRIN-5) — never silently treated as fine.
    Unknown,
}

impl MailIdentityStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            MailIdentityStatus::Attributed => "attributed",
            MailIdentityStatus::Unattributed => "unattributed",
            MailIdentityStatus::Unknown => "unknown",
        }
    }
}

/// Pure classification given an already-read environment snapshot — the
/// TASK-1451 test seam. Reuses the BUG-1533 `resolve_sender` precedence
/// wholesale rather than a second, drifting copy of it. `explicit` has no
/// meaning here (`aida ps` is not sending a message), so it is always
/// absent.
// trace:TASK-1451 | ai:claude
pub(crate) fn mail_identity_status_from_env(
    env: &std::collections::HashMap<String, String>,
) -> MailIdentityStatus {
    let get = |k: &str| env.get(k).map(String::as_str);
    let (_, source) = aida_core::mailbox::resolve_sender(
        None,
        get("AIDA_AGENT_NAME"),
        get("AIDA_USER"),
        get("AIDA_SESSION_ROLE"),
        get("USER"),
    );
    if source.is_attributed() {
        MailIdentityStatus::Attributed
    } else {
        MailIdentityStatus::Unattributed
    }
}

/// Parse `/proc/<pid>/environ` (NUL-separated `KEY=VALUE` records) into a
/// map. `None` when the file can't be read — permission denied (another
/// user's process) or the process has already exited. Linux-only: that file
/// has no equivalent on other platforms.
// trace:TASK-1451 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn read_pid_environ_vars(pid: u32) -> Option<std::collections::HashMap<String, String>> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut map = std::collections::HashMap::new();
    for entry in raw.split(|&b| b == 0) {
        if entry.is_empty() {
            continue;
        }
        if let Ok(s) = std::str::from_utf8(entry) {
            if let Some((k, v)) = s.split_once('=') {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    Some(map)
}

/// The real `/proc/<pid>/environ` probe — the same "one seam per real I/O
/// source" pattern as `pid_start_time`, injected into `build_running_work`
/// so the row-building logic stays filesystem-free and unit-testable with
/// an injected environment map instead of a real `/proc` read.
// trace:TASK-1451 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn probe_mail_identity(pid: u32) -> MailIdentityStatus {
    match read_pid_environ_vars(pid) {
        Some(env) => mail_identity_status_from_env(&env),
        None => MailIdentityStatus::Unknown,
    }
}

/// Non-Linux hosts have no `/proc/<pid>/environ` to read — always "unknown",
/// never guessed as fine.
// trace:TASK-1451 | ai:claude
#[cfg(not(target_os = "linux"))]
pub(crate) fn probe_mail_identity(_pid: u32) -> MailIdentityStatus {
    MailIdentityStatus::Unknown
}

/// BUG-1553: is a LIVE seat (a lease whose pid exists) actively working, on
/// a long-running tool call, suspended (a stopped process), or in a state
/// this probe cannot resolve? `LeaseState::Live` alone only answers "does a
/// process exist" — the 2026-09-21 incident (see BUG-1553) showed that
/// answer collapses several different situations into one row. PRIN-5:
/// `Working` is returned only when the transcript positively supports it —
/// anything the probe can't read renders `Unknown`, never silently
/// `Working`. **None of these states asserts "blocked on your approval"** —
/// per the 2026-09-23 strict-review PROXY DECISION, a purely time-based or
/// process-state heuristic cannot distinguish a long-running tool call (or a
/// deliberately paused process) from a genuine unanswered permission prompt,
/// so both render neutral/warning, never red, and never claim to know a
/// human is needed. A true "blocked on approval" verdict needs a recorded
/// marker from Claude Code's Notification hook (the `permission_prompt`
/// type) and is a follow-up task.
// trace:BUG-1553 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeatActivity {
    /// Recent transcript activity, or the last turn completed cleanly (no
    /// tool call left unresolved past the stall threshold).
    Working,
    /// The transcript's last assistant turn called a tool that has had no
    /// resolving result for longer than [`LONG_TOOL_CALL_THRESHOLD_SECS`].
    /// This is NOT evidence of a blocked approval prompt — a long Bash or
    /// Task call is working, not blocked — so it renders as a neutral
    /// informational note. `tool` names the pending tool when the
    /// transcript names one; `secs` is how long it has been outstanding.
    LongToolCall { tool: Option<String>, secs: i64 },
    /// The process itself is job-control-stopped (`T`/`t` state in
    /// `/proc/<pid>/stat`) — e.g. Ctrl-Z'd or paused under a debugger.
    /// Neutral-to-warning, not "blocked on approval".
    Suspended,
    /// No session transcript could be resolved/read for this pid at all —
    /// honestly "don't know", never guessed as Working.
    Unknown,
    /// TASK-1454: a REAL `Notification(permission_prompt)` hook marker is
    /// present (and not stale) for this seat's Claude Code session — ground
    /// truth from Claude Code itself, not an inference from process/
    /// transcript state. Unlike `LongToolCall`/`Suspended`, this DOES assert
    /// "blocked on your approval", because it is sourced from the exact
    /// signal the 2026-09-23 PROXY DECISION said would justify that claim.
    /// `tool` names the pending tool when the notification message named
    /// one; `secs` is how long the marker has stood.
    // trace:TASK-1454 | ai:claude
    Blocked { tool: Option<String>, secs: i64 },
}

impl SeatActivity {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            SeatActivity::Working => "working",
            SeatActivity::LongToolCall { .. } => "long_tool_call",
            SeatActivity::Suspended => "suspended",
            SeatActivity::Unknown => "unknown",
            SeatActivity::Blocked { .. } => "blocked",
        }
    }
}

/// BUG-1553: how long a pending tool call (an assistant turn's tool_use with
/// no resolving tool_result yet, per the transcript tail) must sit unresolved
/// before this reads as a [`SeatActivity::LongToolCall`] rather than "still
/// executing". Wide enough that an ordinary slow tool call rarely trips it on
/// its own; the row frames the verdict as an informational note ("long tool
/// call"), never an alarm, because a handful of legitimately long-running
/// tools (a full test suite) routinely cross it.
// trace:BUG-1553 | ai:claude
pub(crate) const LONG_TOOL_CALL_THRESHOLD_SECS: i64 = 240;

/// BUG-1553: pure scan of a session transcript's TAIL lines (most-recent
/// last, as a JSONL tail naturally reads) for a pending tool call — the last
/// assistant-turn tool_use block(s) with no later matching tool_result.
/// Returns the pending tool's name (when the transcript names one) and the
/// age of that assistant message in seconds. `None` means the tail parsed
/// cleanly and nothing is pending (the last turn resolved, or no tool was
/// called) — distinct from "the tail could not be read/parsed at all",
/// which this function cannot express (see `classify_seat_activity`, which
/// takes `tail: Option<&[String]>` precisely to keep that distinction).
// trace:BUG-1553 | ai:claude
pub(crate) fn pending_tool_from_tail(
    tail_lines: &[String],
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(Option<String>, i64)> {
    // (tool name, tool_use id, assistant message timestamp)
    let mut pending: Option<(Option<String>, String, chrono::DateTime<chrono::Utc>)> = None;
    for line in tail_lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        let Some(content) = content else { continue };
        if msg_type == "assistant" {
            let mut turn_pending: Option<(Option<String>, String)> = None;
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string());
                    turn_pending = Some((name, id));
                }
            }
            if let Some((name, id)) = turn_pending {
                let ts = v
                    .get("timestamp")
                    .and_then(|t| t.as_str())
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&chrono::Utc));
                if let Some(ts) = ts {
                    pending = Some((name, id, ts));
                }
            }
        } else if msg_type == "user" {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    let result_id = block.get("tool_use_id").and_then(|i| i.as_str());
                    match (result_id, pending.as_ref()) {
                        // Matches the outstanding call by id — resolved.
                        (Some(rid), Some((_, pid, _))) if rid == pid => pending = None,
                        // No id on either side to compare — conservatively
                        // treat any tool_result as resolving the outstanding
                        // call, since we can't match ids.
                        // trace:BUG-1553 | ai:claude
                        (None, _) => pending = None,
                        _ => {}
                    }
                }
            }
        }
    }
    let (name, _id, ts) = pending?;
    let age = now.signed_duration_since(ts).num_seconds().max(0);
    Some((name, age))
}

/// BUG-1553: the pure verdict — given (optionally) a transcript tail and
/// whether the process is job-control-stopped, classify seat activity.
/// `tail: None` means "could not be read at all" (no jsonl resolved, or the
/// read failed) → `Unknown`, never guessed as `Working`. Per the
/// 2026-09-23 strict-review PROXY DECISION, neither branch below asserts
/// "blocked on approval" — a job-control-stopped process reads `Suspended`
/// and a long-outstanding tool call reads `LongToolCall`, both neutral.
// trace:BUG-1553 | ai:claude
pub(crate) fn classify_seat_activity(
    tail: Option<&[String]>,
    proc_stopped: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> SeatActivity {
    if proc_stopped {
        return SeatActivity::Suspended;
    }
    let Some(lines) = tail else {
        return SeatActivity::Unknown;
    };
    match pending_tool_from_tail(lines, now) {
        Some((name, age)) if age >= LONG_TOOL_CALL_THRESHOLD_SECS => SeatActivity::LongToolCall {
            tool: name,
            secs: age,
        },
        _ => SeatActivity::Working,
    }
}

/// TASK-1454: the marker-first wrapper around [`classify_seat_activity`]. A
/// present (already-filtered-non-stale) pending-approval marker is ground
/// truth and ALWAYS wins outright over the transcript/proc-state heuristic —
/// pure and separated out specifically so that priority is unit-testable
/// without a lease/session-manifest fixture.
// trace:TASK-1454 | ai:claude
pub(crate) fn seat_activity_with_marker(
    marker: Option<&pending_approval::PendingApprovalMarker>,
    tail: Option<&[String]>,
    proc_stopped: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> SeatActivity {
    if let Some(marker) = marker {
        let secs = now.signed_duration_since(marker.since).num_seconds().max(0);
        return SeatActivity::Blocked {
            tool: marker.tool.clone(),
            secs,
        };
    }
    classify_seat_activity(tail, proc_stopped, now)
}

/// BUG-1553: read only the TAIL of a transcript file — never the whole
/// file — capped at `max_bytes` from the end. Keeps `aida ps` fast even
/// against a long-running session's multi-MB transcript, per the surface's
/// own speed constraint (STORY-707's "cache-fast, no full scan" discipline
/// applied to this probe too). Returns whole lines only (a partial first
/// line from the seek point is dropped). `None` when the file can't be
/// opened/read at all. Reads to the end of the file and decodes lossily
/// (`from_utf8_lossy`) rather than `read_to_string`, because the seek point
/// (`max_bytes` from the end) can land mid multi-byte UTF-8 character —
/// `read_to_string` would hard-error on that instead of tolerating it.
// trace:BUG-1553 | ai:claude
pub(crate) fn read_transcript_tail(path: &std::path::Path, max_bytes: u64) -> Option<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let buf = String::from_utf8_lossy(&raw);
    let mut lines: Vec<String> = buf.lines().map(|l| l.to_string()).collect();
    // Drop a partial first line when we didn't start at byte 0.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Some(lines)
}

/// BUG-1553: bytes read from the tail of a transcript — generous enough to
/// span several recent turns (a pending tool call plus the assistant text
/// leading up to it) without ever reading a whole multi-MB file.
// trace:BUG-1553 | ai:claude
pub(crate) const TRANSCRIPT_TAIL_BYTES: u64 = 64 * 1024;

/// BUG-1553: is `pid` currently job-control-stopped (`T`/`t` in
/// `/proc/<pid>/stat` field 3)? A process parked at a blocking read is
/// normally `S` (sleeping) — the SAME state as idle-between-turns — so this
/// is a narrow, unambiguous supplementary signal, not the primary one (the
/// transcript tail is); it costs one small file read. Non-Linux hosts have
/// no `/proc` to read — always `false`, folding into the transcript-only
/// verdict rather than a platform-specific guess.
// trace:BUG-1553 | ai:claude
#[cfg(target_os = "linux")]
pub(crate) fn proc_is_stopped(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // Field 3 (process state) follows the `(comm)` field, which may itself
    // contain spaces/parens — split on the LAST ')' to find it reliably.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .is_some_and(|state| state == "T" || state == "t")
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn proc_is_stopped(_pid: u32) -> bool {
    false
}

/// An In-Progress spec with NO live spec-scoped session backing it — the
/// flag-only / orphaned case. Surfaced by `aida ps` alongside the live table so
/// a crashed or never-started session can't hide behind a status flag.
// trace:STORY-696 | ai:claude
#[derive(Debug)]
pub(crate) struct PsOrphan {
    pub(crate) spec: String,
    pub(crate) title: String,
    /// `true` when a spec-scoped lease EXISTS but its holder process is dead
    /// (STALE) — distinct from no lease at all (pure flag-only). Either way the
    /// In-Progress flag is not liveness-backed.
    pub(crate) stale_lease: bool,
    /// TASK-1064: `true` when this flag-only spec (no spec-scoped lease) is most
    /// likely being built by a live advisor Agent-tool fan-out — a fan-out takes
    /// a generic non-spec-linked `harness-worktree` lease, so its specs read as
    /// having no spec lease. Surfaced informationally instead of under the
    /// alarming "Orphaned" header. Never set for a stale (crashed) spec-scoped
    /// lease — that is a genuine orphan.
    // trace:TASK-1064 | ai:claude
    pub(crate) likely_fanout: bool,
    /// BUG-1656: `true` when this spec-scoped lease is STALE (its recorded pid
    /// is dead) but a LIVE generic `harness-worktree` lease exists in this repo
    /// — the shape an Agent-tool subagent dispatched into the spec's leased
    /// worktree leaves behind. The spec may well be being worked right now;
    /// the surface says "possibly worked by a subagent" instead of a flat
    /// "abandoned".
    // trace:BUG-1656 | ai:claude
    pub(crate) possibly_subagent: bool,
    /// The derived driver of this spec. A live relay/drain/agent driver means
    /// the spec IS being worked even though no session of its own is alive —
    /// such rows are never framed as orphaned/abandoned.
    // trace:STORY-1653 | ai:claude
    pub(crate) driver: Option<aida_core::driver_registry::DerivedDriver>,
}

/// The flag-only / orphaned verdict for one In-Progress spec, given the
/// classified state of its spec-scoped lease (if any). A `Live` lease means the
/// spec is genuinely running (NOT orphaned → `None`). A dead/dormant lease →
/// orphaned with `stale_lease = true` (crashed session). No lease → orphaned
/// flag-only. Pure so the orphan matrix is unit-testable without a store or a
/// lease dir.
///
/// BUG-778: `awaiting_agent` short-circuits to "not orphaned". A spec whose
/// worktree a human just entered by hand has no live process backing it *yet*
/// — that is the launch-lag, not an orphan, and listing it under "no live
/// session backing the flag" pointed the operator at a re-dispatch that would
/// race them. The row still shows in the running-work table, framed honestly.
// trace:STORY-696 | ai:claude
// trace:BUG-778 | ai:claude
pub(crate) fn ps_orphan_verdict(
    lease_state: Option<LeaseState>,
    awaiting_agent: bool,
) -> Option<bool> {
    if awaiting_agent {
        return None;
    }
    match lease_state {
        Some(LeaseState::Live) => None,
        Some(_) => Some(true),
        None => Some(false),
    }
}

/// BUG-1656: [`ps_orphan_verdict`] plus the dirty-movement liveness signal. A
/// spec-scoped lease whose worktree's dirty files are still being written is
/// NOT orphaned — something is editing there — however dead its recorded pid
/// looks (an Agent-tool subagent runs inside the parent claude process and
/// is invisible to the pid/cwd probes). Pure so the matrix stays testable.
// trace:BUG-1656 | ai:claude
pub(crate) fn ps_orphan_verdict_with_movement(
    lease_state: Option<LeaseState>,
    awaiting_agent: bool,
    dirty_movement_fresh: bool,
) -> Option<bool> {
    if lease_state.is_some() && dirty_movement_fresh {
        return None;
    }
    ps_orphan_verdict(lease_state, awaiting_agent)
}

/// BUG-1740: is this `harness-worktree` lease pinned to the project ROOT
/// itself — the main checkout — rather than to a worktree of its own?
///
/// THE RULE, and why (AC1). A root-pinned harness lease is **never**, on its
/// own, evidence that an Agent-tool fan-out is running. The project root is
/// where the operator's own interactive session sits and where every `aida`
/// invocation runs, so a lease there is the ambient session, not a dispatched
/// subagent. Worse, `lease_state_for`'s `has_live_claude` arm matches on
/// `cwd.starts_with(worktree_path)`, so a root-pinned lease absorbs the
/// liveness of every claude process anywhere under the checkout — including
/// the one running this command. A genuine fan-out does not need this lease:
/// the harness gives each Agent-tool subagent its own worktree
/// (`.claude/worktrees/agent-*`), and the subagent takes its own
/// `harness-worktree` lease pinned there, which this filter keeps.
///
/// Decided from the lease alone — `worktree_path == parent_project_root` —
/// so the predicate stays pure and needs no ambient project-root lookup.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_lease_is_project_root_pinned(l: &SessionLease) -> bool {
    !l.worktree_path.as_os_str().is_empty()
        && l.parent_project_root.as_deref() == Some(l.worktree_path.as_path())
}

/// BUG-1740: the pid chain of the process running this command, innermost
/// first and including itself. The reporting session must never be its own
/// corroboration for a fan-out (AC2), and the session that invoked `aida` is
/// an ANCESTOR of this process, not this process — so the whole chain is
/// excluded, not just `std::process::id()`.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_caller_pid_chain() -> Vec<u32> {
    process_probe::walk_ancestor_pids(std::process::id())
}

/// BUG-1740 (rework): does the IDENTITY evidence this lease would present to
/// `lease_state_for` — the pid arm that short-circuits `Live` before the
/// presence list is consulted — name the caller or one of its ancestors?
///
/// Mirrors `lease_state_for`'s arm order exactly, so the pid examined here is
/// the pid that would have decided liveness there:
///
///  - the advisory-lock arm (`review_verb`/`claim_verb`, no worktree of its
///    own) reads `creator_pid`;
///  - otherwise an explicit `active_pid` wins.
///
/// The mirroring matters for AC4: a lease the caller MINTED (`creator_pid` in
/// the caller's chain) but whose liveness is attested by a distinct worker's
/// `active_pid` is a genuine fan-out, and an unconditional creator-pid check
/// would delete it. Only the pid `lease_state_for` would actually consult is
/// compared against the caller's chain.
///
/// This is deliberately NOT inside `lease_state_for`: the exclusion is about
/// fan-out CORROBORATION (the observer must not testify for itself), not
/// about lease liveness generally, and every other caller of
/// `lease_state_for` keeps its behavior.
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_lease_identity_is_caller(l: &SessionLease, caller_pids: &[u32]) -> bool {
    if (l.review_verb || l.claim_verb) && l.worktree_path.as_os_str().is_empty() {
        return l.creator_pid.is_some_and(|pid| caller_pids.contains(&pid));
    }
    l.active_pid.is_some_and(|pid| caller_pids.contains(&pid))
}

/// TASK-1064: is an advisor Agent-tool fan-out currently running? Detected as a
/// LIVE lease whose scope is the generic `harness-worktree` fallback — the lease
/// an Agent-tool subagent (a `general-purpose` fan-out whose branch carries no
/// SPEC-ID) takes. Such a lease is deliberately NOT spec-linked, so the specs it
/// is building read as having no spec-scoped lease. Its liveness means those
/// flag-only In-Progress specs are most likely being worked by the fan-out, not
/// genuinely orphaned. Pure over the lease set + the shared liveness machinery.
///
/// BUG-1740: two leases that used to qualify no longer do.
///
///  1. A lease pinned to the project root — see [`ps_lease_is_project_root_pinned`].
///  2. A lease whose only live backing is rooted in the caller's own pid
///     chain. THE FULL RULE (AC2): neither presence (cwd) nor identity
///     (`active_pid`/`creator_pid`) evidence rooted in the caller's own pid
///     chain corroborates a fan-out. The reporting session must never be its
///     own corroboration, whichever door the evidence comes through:
///
///     - PRESENCE: the `has_live_claude` arm of `lease_state_for` infers a
///       worker from a claude process whose cwd is inside the lease's
///       worktree — and the reporting session is such a process. So
///       `caller_pids` is subtracted from the live set before liveness is
///       classified.
///     - IDENTITY: `lease_state_for` short-circuits `Live` on a live
///       `active_pid` (and on `creator_pid` in its advisory-lock arm) BEFORE
///       the presence list is consulted. An Agent-tool subagent executes
///       INSIDE the parent claude process, so a harness lease whose
///       `active_pid` is the caller or one of its ancestors says only that
///       the long-lived session that minted it still exists — not that a
///       fan-out is running (BUG-1772's unreaped harness leases make exactly
///       this shape persistent). Such leases are excluded here via
///       [`ps_lease_identity_is_caller`], which mirrors `lease_state_for`'s
///       arm order.
///
/// WHY THIS DOES NOT DELETE TASK-1064's CASE (AC4). AC4 protects a lease
/// "backed by a live process that is NOT the caller". A fan-out observed from
/// any OTHER session (operator window, monitor seat) still corroborates: the
/// orchestrator's pid is not in that caller's ancestor chain. Only the
/// orchestrating session's own self-report loses the softening — the price
/// AC2 explicitly accepts ("`possibly_subagent` must be false when the only
/// live process backing the harness lease is the process running the command
/// (or its ancestors)").
///
/// The exclusion lives HERE, in the fan-out corroboration path, and not in
/// `lease_state_for`: liveness for every other caller (`aida ps` display,
/// drain, the orphan pass's lease display) is unchanged.
///
/// Still pure: `caller_pids` is an input, so every corner is fixture-testable.
// trace:TASK-1064 | ai:claude
// trace:BUG-1740 | ai:claude
pub(crate) fn ps_live_fanout_leases<'a>(
    leases: &'a [SessionLease],
    live: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
    caller_pids: &[u32],
) -> Vec<&'a SessionLease> {
    // The observer removed from the evidence, once for the whole pass.
    // trace:BUG-1740 | ai:claude
    let corroborating: Vec<process_probe::LiveSession> = live
        .iter()
        .filter(|s| !caller_pids.contains(&s.pid))
        .cloned()
        .collect();
    leases
        .iter()
        .filter(|l| {
            l.scope
                .eq_ignore_ascii_case(worktree_lease::HARNESS_WORKTREE_SCOPE)
                // trace:BUG-1740 | ai:claude
                && !ps_lease_is_project_root_pinned(l)
                // Identity evidence rooted in the caller's own pid chain is
                // the observer testifying for itself — see the doc comment.
                // trace:BUG-1740 | ai:claude
                && !ps_lease_identity_is_caller(l, caller_pids)
                && matches!(lease_state_for(l, &corroborating, now), LeaseState::Live)
        })
        .collect()
}

/// BUG-1681: does `text` — a branch name or a worktree path — NAME one of
/// `ids`? Case-insensitive containment with id boundaries on both sides, so
/// `claude/bug-168` never matches `BUG-1681` and `wt-bug-1681` does. Pure, so
/// the matching rule is fixture-testable.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_text_names_spec(text: &str, ids: &[&str]) -> bool {
    let hay = text.to_ascii_lowercase();
    ids.iter().any(|id| {
        let needle = id.trim().to_ascii_lowercase();
        if needle.is_empty() {
            return false;
        }
        hay.match_indices(&needle).any(|(at, _)| {
            let before = hay[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            let after = hay[at + needle.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            before && after
        })
    })
}

/// BUG-1681: is this LIVE fan-out lease plausibly working the spec `ids` name?
/// True only when the subagent's OWN branch or worktree names that spec. The
/// previous rule was "any fan-out is alive anywhere in this repo", which
/// credited a live subagent with every flag-only In-Progress spec in the store
/// — including specs no subagent had ever touched.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_fanout_names_spec(fanout: &SessionLease, ids: &[&str]) -> bool {
    ps_text_names_spec(&fanout.branch, ids)
        || ps_text_names_spec(&fanout.worktree_path.to_string_lossy(), ids)
}

/// BUG-1681: is this LIVE fan-out lease working inside `spec_lease`'s
/// worktree? That is the BUG-1656 shape — an Agent-tool subagent dispatched
/// INTO a spec's leased worktree, whose work the spec lease's own pid probe
/// structurally cannot see — and it is the only case where a dead spec lease
/// may be explained by a subagent. Matched on the worktree the subagent
/// recorded, or on the branch it is on; never on mere coexistence in the repo.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_fanout_holds_lease(fanout: &SessionLease, spec_lease: &SessionLease) -> bool {
    let same_worktree = !spec_lease.worktree_path.as_os_str().is_empty()
        && fanout.worktree_path == spec_lease.worktree_path;
    let same_branch = !spec_lease.branch.trim().is_empty()
        && fanout
            .branch
            .trim()
            .eq_ignore_ascii_case(spec_lease.branch.trim());
    same_worktree || same_branch
}

/// BUG-1681: the generic role label a fan-out row shows when the harness
/// recorded only its placeholder agent type. It says what the row IS — a
/// subagent — instead of borrowing the identity of the session hosting it.
// trace:BUG-1681 | ai:claude
pub(crate) const PS_SUBAGENT_ROLE: &str = "subagent";

/// The role an `aida ps` row displays, from the three signals available.
///
/// BUG-1521: a REAL recorded lease role is authoritative — the row shows what
/// the session recorded about itself, not what a heuristic inferred.
///
/// BUG-1681: when the recorded role is the harness's generic Agent-tool
/// placeholder, the row is an Agent-tool subagent, and its lease's pid is the
/// PARENT claude process (a subagent executes inside it — see BUG-752). The
/// transcript scan therefore resolves the HOST session's role, which is how
/// fan-out rows came to display `advisor`: the parent's identity, not their
/// own. So the placeholder falls back to the manifest join (TASK-153's stable
/// per-lease `claude_session_id` link, which is about THIS row) and then to the
/// generic subagent label — never to the ambiguous host-transcript scan.
///
/// A lease with no recorded role at all keeps the pre-existing derivation: the
/// manifest join, else the transcript scan.
// trace:BUG-1521 trace:TASK-153 trace:BUG-1681 | ai:claude
pub(crate) fn ps_display_role(
    lease_role: Option<&str>,
    manifest_role: Option<&str>,
    jsonl_role: Option<&str>,
) -> Option<String> {
    match lease_role.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) if !r.eq_ignore_ascii_case(tail_cmd::HARNESS_AGENT_TYPE) => Some(r.to_string()),
        Some(_) => Some(
            manifest_role
                .map(str::to_string)
                .unwrap_or_else(|| PS_SUBAGENT_ROLE.to_string()),
        ),
        None => manifest_role.or(jsonl_role).map(str::to_string),
    }
}

/// BUG-1941: is this lease's worktree the project's own main checkout (not an
/// isolation worktree)? Legacy `harness-worktree` leases were minted there
/// before BUG-1772 stopped that, and the cwd probe attaches whatever session
/// runs in the main checkout to them. Pure over the lease record.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_lease_is_main_checkout(l: &SessionLease) -> bool {
    !l.worktree_path.as_os_str().is_empty()
        && l.parent_project_root.as_deref() == Some(l.worktree_path.as_path())
}

/// BUG-1941: the role a row shows, given whether its lease sits on the main
/// checkout. A placeholder-role lease there is NOT an Agent-tool subagent: the
/// process the cwd probe attaches is a top-level session (the operator's
/// advisor, an autonomous relay, …), so the generic `subagent` label would be
/// wrong. Such a row shows the manifest-joined role when one exists, else no
/// role. The ambiguous transcript scan is not used: every session in the main
/// checkout shares one transcript directory, so it can name another session's
/// role. Everywhere else this is [`ps_display_role`] unchanged.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_row_role(
    lease_role: Option<&str>,
    manifest_role: Option<&str>,
    jsonl_role: Option<&str>,
    main_checkout: bool,
) -> Option<String> {
    let placeholder = lease_role
        .map(str::trim)
        .is_some_and(|r| r.eq_ignore_ascii_case(tail_cmd::HARNESS_AGENT_TYPE));
    if main_checkout && placeholder {
        return manifest_role.map(str::to_string);
    }
    ps_display_role(lease_role, manifest_role, jsonl_role)
}

/// BUG-1941: the live session a lease's worktree hosts, chosen
/// deterministically. Several sessions can share one cwd (the main checkout
/// runs the operator's advisor beside autonomous relays) and the `/proc` walk
/// returns them in no stable order, so "first match" made the row's pid change
/// from run to run. An exact cwd match beats a subdirectory, then the lowest
/// pid wins.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_live_in_worktree<'a>(
    live: &'a [process_probe::LiveSession],
    worktree: &std::path::Path,
) -> Option<&'a process_probe::LiveSession> {
    live.iter()
        .filter(|s| !s.stale_cwd && (s.cwd == worktree || s.cwd.starts_with(worktree)))
        .min_by_key(|s| (s.cwd != worktree, s.pid))
}

/// BUG-1941: the note a row carries when its dispatch reads `moving` while no
/// running process backs the row (no pid shown) — fresh files or a fresh
/// commit in the worktree. Without it the human view showed only "dormant"
/// while the agent view said `moving` for the same row. `None` otherwise.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_unattributed_movement_note(
    pid: Option<u32>,
    dispatch: Option<dispatch_health_ps::DispatchState>,
) -> Option<&'static str> {
    // Gated on the row having no pid, not on its lease state: a Stale lease
    // backed by an occupant process (BUG-1941 occupant rule) shows that pid,
    // and must not also claim no process is attributed.
    (pid.is_none() && dispatch == Some(dispatch_health_ps::DispatchState::Moving)).then_some(
        "recent changes in this worktree, but no running process is attributed to this session",
    )
}

/// BUG-1941: the reading the integration-standing overlay judges a row by —
/// the pre-lift state for a commit-only `Moving` row, the row's own state
/// otherwise. Pure.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_integration_base_state(
    dispatch: &PsDispatch,
) -> dispatch_health_ps::DispatchState {
    dispatch.commit_movement_from.unwrap_or(dispatch.state)
}

/// BUG-1681 / BUG-1941: is this row a candidate for the integration-standing
/// overlay? Only a stalled/stopped reading — including one a fresh commit
/// lifted to `Moving` — is; a `Salvageable` reading (lifted or not) keeps its
/// urgency and is never looked up. Pure.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_integration_overlay_candidate(dispatch: &PsDispatch) -> bool {
    matches!(
        ps_integration_base_state(dispatch),
        dispatch_health_ps::DispatchState::Stalled | dispatch_health_ps::DispatchState::Stopped
    )
}

/// BUG-1681: re-read one row's dispatch verdict in the light of its
/// integration standing, replacing the stalled/stopped reading (and its
/// resume hint) with the actionless `awaiting-integration` note. Returns
/// whether the row changed. Pure over the row + the standing, so the
/// reframing is fixture-testable without a store or a forge.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_apply_integration_standing(
    row: &mut PsRow,
    standing: Option<&dispatch_health_ps::IntegrationStanding>,
) -> bool {
    let Some(dispatch) = row.dispatch.as_mut() else {
        return false;
    };
    // BUG-1941: commit-only movement is the no-process reading lifted by a
    // fresh commit; a finished/approved spec's "commit, push, exit" end state
    // is awaiting integration, not moving. trace:BUG-1941 | ai:claude
    let base = ps_integration_base_state(dispatch);
    let next = dispatch_health_ps::apply_integration(base, standing);
    if next == base {
        return false;
    }
    dispatch.state = next;
    dispatch.commit_movement_from = None;
    dispatch.hint = Some(dispatch_health_ps::integration_hint(standing));
    true
}

/// BUG-1681: ask the substrate whether a stalled-looking row's spec has in
/// fact FINISHED before `aida ps` advertises a resume for it. The observed
/// failure was ~20 rows whose specs were already Completed and landed in
/// integration batches, each advertised as `stalled — resume/rebrief`; an
/// agent that follows that hint redoes shipped work.
///
/// Cost-gated: nothing is read unless some row actually classified
/// stalled/stopped, or was stalled before a fresh commit lifted it to
/// `moving` (BUG-1941; see [`ps_integration_overlay_candidate`]) — the quiet
/// case pays zero — and then only the cached
/// requirement lookup for those rows' specs plus their local review-verdict
/// files. No forge call.
// trace:BUG-1681 | ai:claude
pub(crate) fn ps_overlay_integration_standing(project_root: &std::path::Path, rows: &mut [PsRow]) {
    let candidates: Vec<String> = rows
        .iter()
        .filter(|r| {
            r.dispatch
                .as_ref()
                .is_some_and(ps_integration_overlay_candidate)
        })
        .filter_map(|r| r.spec.clone())
        .collect();
    if candidates.is_empty() {
        return;
    }
    let finished = session_reap::finished_scopes(project_root, &candidates);
    for row in rows.iter_mut() {
        let Some(spec) = row.spec.clone() else {
            continue;
        };
        if !candidates.iter().any(|c| c.eq_ignore_ascii_case(&spec)) {
            continue;
        }
        let branch = row.lease.branch.clone();
        let verdict = review_verdict::read_recorded_verdict_any(project_root, &[spec.as_str()]);
        let standing = dispatch_health_ps::integration_standing(
            finished.contains(&spec.to_ascii_uppercase()),
            verdict.as_ref().is_some_and(|v| v.kind.approves()),
            verdict.as_ref().and_then(|v| v.reviewed_branch.as_deref()),
            &branch,
            verdict.as_ref().and_then(|v| v.comment_url.as_deref()),
        );
        ps_apply_integration_standing(row, standing.as_ref());
    }
}

/// TASK-1064: should a flag-only In-Progress spec (no spec-scoped lease) be
/// surfaced as "likely worked by a fan-out" rather than "orphaned"? True only
/// when there is NO spec-scoped lease at all (`stale_lease == false` — a crashed
/// spec-scoped lease is a genuine orphan regardless) AND a live fan-out harness
/// lease exists (a plausible live worker). Pure so the three-way framing is
/// unit-testable without a store or a lease dir.
// trace:TASK-1064 | ai:claude
pub(crate) fn ps_orphan_likely_fanout(stale_lease: bool, fanout_names_spec: bool) -> bool {
    !stale_lease && fanout_names_spec
}

/// BUG-752: does this lease carry NO process-liveness signal at all while
/// being one of the harness (Agent-tool) isolation worktrees? Such a worker
/// runs inside the parent claude process (cwd = parent project root), so the
/// cwd-based worktree probe structurally cannot see it — "no live claude in
/// the worktree" is NOT evidence of death there. True only for the legacy /
/// degraded case: a harness-worktree lease minted without an `active_pid`
/// stamp (pre-fix binary, or no claude ancestor found at register time).
/// Pure so the predicate is unit-testable on lease fixtures.
// trace:BUG-752 | ai:claude
pub(crate) fn harness_lease_without_pid_signal(l: &SessionLease) -> bool {
    l.active_pid.is_none()
        && l.creator_pid.is_none()
        && is_agent_managed_worktree(&l.worktree_path, Some(&l.branch))
}

/// BUG-752: the tri-state pid-liveness input to the dispatch-health
/// classifier. `Some(true)` for a Live lease (a real process demonstrably
/// backs it), `None` — liveness undeterminable — for a harness lease with no
/// pid signal (see [`harness_lease_without_pid_signal`]), `Some(false)`
/// otherwise (the probe looked and found nothing alive). Keeping this pure
/// keeps the "salvage hint only on genuinely determined death" rule
/// unit-testable without a lease dir or `/proc`.
// trace:BUG-752 | ai:claude
pub(crate) fn ps_pid_liveness(state: LeaseState, harness_without_pid_signal: bool) -> Option<bool> {
    match state {
        LeaseState::Live => Some(true),
        _ if harness_without_pid_signal => None,
        _ => Some(false),
    }
}

/// BUG-778: seconds since `aida worktree enter|add` handed this worktree to a
/// human, or `None` when it never did (every orchestrator-spawned lease). The
/// "was an agent ever expected here?" input to the dispatch-health classifier.
/// A clock-skewed future stamp clamps to 0 (freshly handed over) rather than
/// underflowing. Pure so the provenance read is unit-testable on fixtures.
// trace:BUG-778 | ai:claude
pub(crate) fn ps_manual_enter_secs(
    l: &SessionLease,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<u64> {
    l.manual_enter_at
        .map(|at| now.signed_duration_since(at).num_seconds().max(0) as u64)
}

/// BUG-778: is this lease's row the hand-entered, launch-pending shape? Read
/// straight off the row's already-computed dispatch state so the orphan
/// section and the running-work table can never disagree about the same lease
/// (the reported bug listed ONE hand-entered spec in both, under two
/// contradictory framings). Pure over the built rows.
// trace:BUG-778 | ai:claude
pub(crate) fn ps_row_awaiting_agent(rows: &[PsRow], lease_id: &str) -> bool {
    rows.iter().any(|r| {
        r.lease.id == lease_id
            && r.dispatch
                .as_ref()
                .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::AwaitingAgent)
    })
}

/// TASK-1143: the `locked-by` cell for one `aida ps` row — the advisor holding
/// the STORY-711 worktree lock, or an empty string when the worktree carries no
/// lock. Blank (not `-`) for the unlocked common case so the column stays quiet
/// until a lock actually exists — `[locking]` is opt-in, so most rows have no
/// lock. An empty `authorized_by` (defensive: should never be written) is
/// treated the same as absent, mirroring `verify_worktree_lock`. Pure so the
/// locked/blank split is unit-testable without a lease dir.
// trace:TASK-1143 | ai:claude
pub(crate) fn ps_locked_by_cell(locked_by: Option<&str>) -> String {
    locked_by
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Floor / ceiling for the auto-sized `spec` column of the `aida ps` table. The
/// floor is the historical fixed width (so a table of ordinary SPEC-IDs looks
/// exactly as before); the ceiling bounds a pathological scope value so one long
/// row can't blow the table past a normal terminal width.
// trace:TASK-1168 | ai:claude
pub(crate) const PS_SPEC_MIN_WIDTH: usize = 14;

pub(crate) const PS_SPEC_MAX_WIDTH: usize = 20;

/// Floor / ceiling for the auto-sized `role` column. The ceiling is sized so the
/// whole closed set of role names — `implementer`, `advisor`, `reviewer`,
/// `general-purpose`, `harness-worktree` (16 chars, the longest) — renders whole.
// trace:TASK-1168 | ai:claude
pub(crate) const PS_ROLE_MIN_WIDTH: usize = 10;

pub(crate) const PS_ROLE_MAX_WIDTH: usize = 16;

/// Auto-size one `aida ps` column to its widest cell, clamped to `[min, max]`.
/// The previous fixed widths pre-truncated the `role` and `spec` cells to ~13
/// visible chars, so bounded closed-set identifiers (`harness-worktree`,
/// `general-purpose`) rendered ellipsized even though nothing else needed the
/// space. Sizing to content within a bound renders them whole while still
/// capping a pathological scope. Pure so the width math is unit-testable.
// trace:TASK-1168 | ai:claude
pub(crate) fn ps_column_width<S: AsRef<str>>(cells: &[S], min: usize, max: usize) -> usize {
    let widest = cells
        .iter()
        .map(|c| c.as_ref().chars().count())
        .max()
        .unwrap_or(0);
    widest.clamp(min, max.max(min))
}

/// Split an over-wide `aida ps` cell into `(cell, continuation)` — a WRAP, not an
/// ellipsis: every character survives, the remainder moves to an indented
/// continuation line alongside the existing per-row worktree / adopted-lease
/// lines. Prefers breaking after a separator (`-`, `_`, `/`, `.`, space) inside
/// the column so a hyphenated identifier wraps at a segment boundary rather than
/// mid-word; falls back to a hard split when the first `width` chars hold no
/// separator. Pure so the wrap points are unit-testable.
// trace:TASK-1168 | ai:claude
pub(crate) fn ps_wrap_cell(value: &str, width: usize) -> (String, Option<String>) {
    let chars: Vec<char> = value.chars().collect();
    if width == 0 || chars.len() <= width {
        return (value.to_string(), None);
    }
    // Break AFTER the last separator that still fits in the column, so the
    // delimiter stays with the head and the tail starts on a segment.
    let split = chars[..width]
        .iter()
        .rposition(|c| matches!(c, '-' | '_' | '/' | '.' | ' '))
        .map(|i| i + 1)
        .filter(|i| *i > 0 && *i < chars.len())
        .unwrap_or(width);
    let head: String = chars[..split].iter().collect();
    let tail: String = chars[split..].iter().collect();
    (head, Some(tail))
}

/// The `started` cell for one `aida ps` row — time-of-day only when the lease
/// started today (local time), date-qualified ("Jun-26 11:55") for anything
/// older, so a weeks-old persistent lease can't read as if it started this
/// morning next to a 500h elapsed. Pure over already-localized values so the
/// today/not-today split is unit-testable without freezing the clock or the
/// host timezone.
// trace:BUG-763 | ai:claude
pub(crate) fn ps_started_cell(
    started_local: chrono::NaiveDateTime,
    today_local: chrono::NaiveDate,
) -> String {
    if started_local.date() == today_local {
        started_local.format("%H:%M").to_string()
    } else {
        started_local.format("%b-%d %H:%M").to_string()
    }
}

/// Minimum a live pid must postdate its lease before the row is annotated as
/// adopted. Small enough to catch any real re-adoption, large enough to absorb
/// ordinary start-up ordering (a lease is written moments around the process
/// it records, in either order).
pub(crate) const PS_ADOPTED_SLACK_SECS: i64 = 300;

/// Is this row's lease ADOPTED — i.e. did the live pid backing it start
/// meaningfully (more than [`PS_ADOPTED_SLACK_SECS`]) after the lease record
/// itself was written? The single definition of adoption, shared by the
/// annotation ([`ps_adopted_note`]) and the elapsed column
/// ([`ps_elapsed_secs`]) so the two can never disagree about which rows are
/// adopted.
// trace:BUG-769 | ai:claude
pub(crate) fn ps_lease_is_adopted(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: chrono::DateTime<chrono::Utc>,
) -> bool {
    pid_started
        .signed_duration_since(lease_started)
        .num_seconds()
        >= PS_ADOPTED_SLACK_SECS
}

/// The `elapsed` cell for one `aida ps` row, in seconds — the age of the
/// WORK SESSION, not unconditionally the age of the lease record.
///
/// BUG-769: for an adopted lease (see [`ps_lease_is_adopted`]) this is the
/// backing process's uptime. A persistent harness lease is born once and then
/// re-adopted by every later session, so `now - lease.started_at` printed
/// "583h" next to a process that had been alive three minutes — the headline
/// number contradicted the BUG-763 annotation directly under it. Lease-age
/// keeps its place in that annotation, which still names BOTH ages; the column
/// now carries the actionable one ("how long has this actual worker been
/// going"). Every other row — no live pid, no resolvable start time, or a pid
/// born with its lease — is unchanged: `now - lease.started_at`.
///
/// Pure so both branches are unit-testable without a lease dir or `/proc`.
// trace:BUG-769 | ai:claude
pub(crate) fn ps_elapsed_secs(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> u64 {
    let anchor = ps_session_started_at(lease_started, pid_started);
    now.signed_duration_since(anchor).num_seconds().max(0) as u64
}

/// BUG-1941: when the row's work session started — the ONE anchor both the
/// `started` and the `elapsed` columns read, so they always describe the same
/// thing. On an adopted lease that is the live process's start; otherwise the
/// lease's. The lease's own age stays in the adopted note only.
// trace:BUG-1941 | ai:claude
pub(crate) fn ps_session_started_at(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: Option<chrono::DateTime<chrono::Utc>>,
) -> chrono::DateTime<chrono::Utc> {
    match pid_started {
        Some(p) if ps_lease_is_adopted(lease_started, p) => p,
        _ => lease_started,
    }
}

/// The adopted-lease annotation for one `aida ps` row — `Some` (naming BOTH
/// ages) when the live pid backing the row started meaningfully later than the
/// lease itself. That is the persistent harness-worktree pattern: the lease is
/// born once, then adopted and pid-restamped by every subsequent session, so
/// the row otherwise mixes provenance silently — pid/liveness from today's
/// process, started/elapsed from the lease record. Pure so the threshold and
/// wording are unit-testable.
// trace:BUG-763 | ai:claude
pub(crate) fn ps_adopted_note(
    lease_started: chrono::DateTime<chrono::Utc>,
    pid_started: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<String> {
    let pid_started = pid_started?;
    if !ps_lease_is_adopted(lease_started, pid_started) {
        return None;
    }
    let lease_age = now
        .signed_duration_since(lease_started)
        .num_seconds()
        .max(0) as u64;
    let pid_age = now.signed_duration_since(pid_started).num_seconds().max(0) as u64;
    Some(format!(
        "adopted — lease born {} ago, current pid up {}",
        humanize_duration_secs(lease_age),
        humanize_duration_secs(pid_age)
    ))
}

/// Best-effort start time of a single live pid, via a pid-scoped sysinfo
/// refresh (the BUG-613 single-pid discipline — never a full process-table
/// walk). `None` when the process is gone or the platform can't say.
// trace:BUG-763 | ai:claude
pub(crate) fn pid_start_time(pid: u32) -> Option<chrono::DateTime<chrono::Utc>> {
    use sysinfo::{Pid, ProcessRefreshKind, System};
    let target = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_pids_specifics(&[target], ProcessRefreshKind::new());
    let start = sys.process(target)?.start_time();
    if start == 0 {
        return None;
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp(start as i64, 0)
}

// Rollup / stateless requirement types never carry a session of their own, so
// flagging them in the orphan pass is pure noise. Since BUG-626 made an epic's
// status a child-rollup, In-Progress epics are common — and would otherwise
// *all* read as "orphaned" (an epic never holds a spec-scoped lease). Folder
// and Meta are stateless containers/prompts that also never get worked
// directly. Skip these in the orphan walk so only real work-item In-Progress
// specs without a live session surface.
// trace:TASK-940 | ai:claude
pub(crate) fn ps_orphan_excluded_type(req_type: &aida_core::RequirementType) -> bool {
    matches!(
        req_type,
        aida_core::RequirementType::Epic
            | aida_core::RequirementType::Folder
            | aida_core::RequirementType::Meta
    )
}

/// `aida ps` — the GLOBAL running-work table. One row per active session/agent
/// across the project (the project-wide companion to `aida status <spec>`), plus
/// a pass over In-Progress specs with no live spec-scoped lease so orphaned
/// flags surface. Reuses the per-spec liveness machinery wholesale.
// trace:STORY-696 trace:STORY-694 | ai:claude
// trace:STORY-1024 | ai:codex
pub(crate) struct IntegrateCommandOpts {
    pub(crate) json: bool,
    pub(crate) run: bool,
    pub(crate) dry_run: bool,
    pub(crate) watch: bool,
    pub(crate) interval: u64,
    pub(crate) max: usize,
    pub(crate) wait_ci: bool,
    pub(crate) rebase: bool,
    pub(crate) strategy: Option<integrate::IntegrateStrategy>,
    pub(crate) focus: Option<String>,
    pub(crate) idle_minutes: Option<u64>,
    pub(crate) force: bool,
    pub(crate) user: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct MergeQueueRow {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) state: String,
    pub(crate) position: Option<usize>,
}

// trace:STORY-1024 | ai:codex
pub(crate) fn merge_queue_state_label(
    position: Option<usize>,
    ready: bool,
    has_open_pr: bool,
) -> String {
    match (position, ready, has_open_pr) {
        (Some(n), _, _) => format!("In merge queue (position {n})"),
        (None, true, true) => "In merge queue".to_string(),
        (None, false, true) => "Not ready for merge queue".to_string(),
        (None, _, false) => "Done without open PR".to_string(),
    }
}

/// `aida integrate` (bare) — the read-only integrator throughput view
/// (TASK-1034). One screen, scoped to the active focus: the focus-scoped queue,
/// the Done-with-PR merge queue, live throughput, and active fan-out. `--run`
/// hands off to the serialized merge engine.
// trace:TASK-1034 trace:STORY-718 trace:STORY-1024 | ai:claude codex
pub(crate) fn handle_integrate(opts: IntegrateCommandOpts) -> Result<()> {
    if opts.run || opts.watch || opts.dry_run {
        enforce_team_gate(permissions::GatedOp::Integrate, opts.force)?;
        let store_path = detect_distributed_store().ok_or_else(|| {
            anyhow::anyhow!("could not locate AIDA store for `aida integrate --run`")
        })?;
        let storage = Storage::new(store_path);
        let user_id = current_user_id(opts.user.as_deref());
        return queue_cmd::handle_queue_integrate(
            &storage,
            &user_id,
            opts.dry_run,
            opts.watch,
            opts.interval,
            opts.max,
            opts.wait_ci,
            opts.rebase,
            opts.strategy,
            opts.focus,
            opts.idle_minutes,
        );
    }

    use std::collections::HashSet;

    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    let now = chrono::Utc::now();

    // --- Focus scope ---------------------------------------------------------
    let focus_label = focus::resolve_focus(&project_root);

    // --- Focus-scoped queue (reuse the queue read + cache summaries) ---------
    // The store is resolved the same way every store-bearing command resolves
    // it; a fresh / un-attached clone degrades to an empty queue rather than an
    // error (the throughput + running-work sections still render).
    let store_path = detect_distributed_store();
    let user_id = current_user_id(None);
    let mut queue_rows: Vec<(String, String, String, String)> = Vec::new(); // (id, title, status, role)
    let mut merge_rows: Vec<MergeQueueRow> = Vec::new();
    if let Some(sp) = store_path.as_deref() {
        let storage = Storage::new(sp);
        if let Ok(backend) = advance_backend(sp) {
            let summaries = backend
                .list_summaries(&aida_core::ListFilter::default())
                .unwrap_or_default();
            let by_id: std::collections::HashMap<Uuid, &aida_core::RequirementSummary> =
                summaries.iter().map(|s| (s.id, s)).collect();

            // Resolve the focus label to its subtree (cache-fast descendant
            // closure, the same `backend.descendant_ids` the focused `aida list`
            // / `aida queue list` use). None => no focus, show the whole queue.
            let subtree: Option<HashSet<Uuid>> = focus_label.as_deref().and_then(|label| {
                summaries
                    .iter()
                    .find(|s| {
                        [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                            .into_iter()
                            .flatten()
                            .any(|id| id.eq_ignore_ascii_case(label))
                    })
                    .and_then(|s| backend.descendant_ids(&s.id).ok())
            });

            let raw = storage.queue_list(&user_id, false).unwrap_or_default();
            for e in &raw {
                if let Some(set) = subtree.as_ref() {
                    if !set.contains(&e.requirement_id) {
                        continue;
                    }
                }
                let Some(s) = by_id.get(&e.requirement_id) else {
                    continue;
                };
                // Mirror the default queue/list view: hide archived + terminal
                // (Completed / Rejected) so the queue reads as live work.
                if s.archived {
                    continue;
                }
                let st = s.status.to_ascii_lowercase();
                if st == "completed" || st == "rejected" {
                    continue;
                }
                let id = s
                    .agreed_id
                    .as_deref()
                    .or(s.spec_id.as_deref())
                    .unwrap_or("")
                    .to_string();
                queue_rows.push((
                    id,
                    s.title.clone(),
                    s.status.clone(),
                    e.for_role.clone().unwrap_or_default(),
                ));
            }

            let store = storage.load().ok();
            let mut ready_position = 0usize;
            for req in store
                .as_ref()
                .map(|s| s.requirements.as_slice())
                .unwrap_or(&[])
            {
                if req.status != aida_core::RequirementStatus::Done || req.archived {
                    continue;
                }
                let id = req.display_id();
                if let Some(set) = subtree.as_ref() {
                    if !set.contains(&req.id) {
                        continue;
                    }
                }
                let lookup = detect_open_pr_for_spec_via_forge(&project_root, &id);
                let inconclusive = matches!(
                    lookup,
                    PrLookup::GhMissing | PrLookup::GhFailed(_) | PrLookup::GhUnreachable(_)
                );
                let (facts, _branch, pr) = probe_resume_facts(&project_root, &storage, &id, None);
                let candidate = integrate::IntegrationCandidate {
                    id: id.clone(),
                    is_done: true,
                    has_open_pr: pr.is_some(),
                    pr_merged: facts.pr_merged,
                    pr_lookup_inconclusive: inconclusive,
                    held_for_human: req.tags.iter().any(|t| {
                        let t = t.trim();
                        t.eq_ignore_ascii_case("supervised")
                            || t.eq_ignore_ascii_case("review:draft-only")
                    }),
                };
                let ready = integrate::classify_candidate(&candidate)
                    == integrate::CandidateVerdict::Integrate;
                let position = if ready {
                    ready_position += 1;
                    Some(ready_position)
                } else {
                    None
                };
                merge_rows.push(MergeQueueRow {
                    id,
                    title: req.title.clone(),
                    state: merge_queue_state_label(position, ready, candidate.has_open_pr),
                    position,
                });
            }
        }
    }

    // --- Throughput (git log origin/main, falling back to the event stream) --
    let git_merges =
        integrate_view::read_git_merge_times(&project_root, integrate_view::GIT_LOG_SCAN_LIMIT);
    let events = integrate_view::read_events(&project_root);
    let merge_times = if git_merges.is_empty() {
        integrate_view::merge_times_from_events(&events)
    } else {
        git_merges
    };
    let throughput = integrate_view::summarize_throughput(&merge_times, now);
    let idle = integrate_view::main_idle_verdict(
        throughput.last_merge,
        now,
        integrate_view::DEFAULT_IDLE_THRESHOLD_MINS,
    );
    let drain_times = integrate_view::drain_times_from_events(&events);
    let drains_last_day = drain_times
        .iter()
        .filter(|t| **t >= now - chrono::Duration::hours(24) && **t <= now)
        .count();

    // --- Active fan-out work (reuse the `aida ps` running-work table) --------
    let (mut run_rows, _orphans) = gather_running_work(&project_root);
    // When a focus is active, keep rows whose spec is in the focus subtree; a
    // generic harness-worktree lease (spec unknown) stays visible since it
    // cannot be excluded honestly. With no focus, every row shows.
    if let (Some(_label), Some(sp)) = (focus_label.as_deref(), store_path.as_deref()) {
        if let Ok(backend) = advance_backend(sp) {
            let summaries = backend
                .list_summaries(&aida_core::ListFilter::default())
                .unwrap_or_default();
            if let Some(froot) = summaries.iter().find(|s| {
                [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|id| id.eq_ignore_ascii_case(focus_label.as_deref().unwrap_or("")))
            }) {
                if let Ok(subtree) = backend.descendant_ids(&froot.id) {
                    let in_scope: HashSet<String> = summaries
                        .iter()
                        .filter(|s| subtree.contains(&s.id))
                        .flat_map(|s| {
                            [s.agreed_id.clone(), s.spec_id.clone()]
                                .into_iter()
                                .flatten()
                        })
                        .map(|id| id.to_ascii_lowercase())
                        .collect();
                    run_rows.retain(|r| match &r.spec {
                        Some(sp) => in_scope.contains(&sp.to_ascii_lowercase()),
                        None => true,
                    });
                }
            }
        }
    }

    // ---------------------------------------------------------------- JSON ----
    if opts.json {
        let queue_json: Vec<serde_json::Value> = queue_rows
            .iter()
            .map(|(id, title, status, role)| {
                serde_json::json!({ "id": id, "title": title, "status": status, "role": role })
            })
            .collect();
        let merge_queue_json: Vec<serde_json::Value> = merge_rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "id": row.id,
                    "title": row.title,
                    "state": row.state,
                    "position": row.position,
                })
            })
            .collect();
        let running_json: Vec<serde_json::Value> = run_rows
            .iter()
            .map(|row| {
                serde_json::json!({
                    "spec": row.spec,
                    "role": row.role,
                    "lease_role": row.lease_role,
                    "pid": row.pid,
                    "elapsed_secs": row.elapsed_secs,
                    "live": matches!(row.state, LeaseState::Live),
                    "liveness": row.state.label(),
                })
            })
            .collect();
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "focus": focus_label,
                "queue_depth": queue_rows.len(),
                "queue": queue_json,
                "merge_queue_depth": merge_rows.iter().filter(|r| r.position.is_some()).count(),
                "merge_queue": merge_queue_json,
                "throughput": {
                    "last_merge": throughput.last_merge.map(|t| t.to_rfc3339()),
                    "merges_last_hour": throughput.merges_last_hour,
                    "merges_last_day": throughput.merges_last_day,
                    "drains_last_day": drains_last_day,
                    "main_idle": idle.idle,
                    "minutes_since_last_merge": idle.minutes_since_last_merge,
                },
                "running": running_json,
            }))?
        );
        return Ok(());
    }

    // Hide STALE running rows (dead leases: worktree gone OR no live process
    // and >24h old) behind a footer count in the two consumption-facing renders
    // below, mirroring `aida ps`, which hides stale rows unless `--all`. The
    // JSON block above deliberately keeps every row so structured consumers can
    // filter on `live`/`liveness` themselves. Without this, a long-dead lease
    // (e.g. a Completed spec whose worktree was removed) clutters the
    // integrator's throughput view. trace:BUG-693 | ai:claude
    let stale_hidden = run_rows
        .iter()
        .filter(|r| matches!(r.state, LeaseState::Stale))
        .count();
    run_rows.retain(|r| !matches!(r.state, LeaseState::Stale));

    // --------------------------------------------------- AGENT (TOON) mode ----
    // Token-efficient rendering for non-TTY / AIDA_AGENT_OUTPUT consumers, the
    // TASK-964 AXI pattern: flat scalars + uniform TOON tables, no glyphs/color.
    if agent_output_mode() {
        println!("view: integrate");
        println!("focus: {}", focus_label.as_deref().unwrap_or("none"));
        println!("main_idle: {}", idle.idle);
        println!(
            "minutes_since_last_merge: {}",
            idle.minutes_since_last_merge
                .map(|m| m.to_string())
                .unwrap_or_else(|| "none".to_string())
        );
        println!("merges_last_hour: {}", throughput.merges_last_hour);
        println!("merges_last_day: {}", throughput.merges_last_day);
        println!("drains_last_day: {drains_last_day}");
        println!("queue_depth: {}", queue_rows.len());
        println!(
            "merge_queue_depth: {}",
            merge_rows.iter().filter(|r| r.position.is_some()).count()
        );
        println!("running: {}", run_rows.len());
        println!("stale_hidden: {stale_hidden}");
        let q: Vec<Vec<String>> = queue_rows
            .iter()
            .map(|(id, title, status, role)| {
                vec![
                    id.clone(),
                    title.clone(),
                    toon_status_token(status),
                    role.clone(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("queue", &["id", "title", "status", "role"], &q)
        );
        let mq: Vec<Vec<String>> = merge_rows
            .iter()
            .map(|row| vec![row.id.clone(), row.title.clone(), row.state.clone()])
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("merge_queue", &["id", "title", "state"], &mq)
        );
        let r: Vec<Vec<String>> = run_rows
            .iter()
            .map(|row| {
                vec![
                    row.spec.clone().unwrap_or_else(|| "-".to_string()),
                    row.role.clone().unwrap_or_else(|| "-".to_string()),
                    humanize_duration_secs(row.elapsed_secs),
                    row.state.label().to_string(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw("running", &["spec", "role", "elapsed", "live"], &r)
        );
        return Ok(());
    }

    // --------------------------------------------------------- HUMAN mode ----
    let arrow = crate::glyph(crate::glyphs::Glyph::Arrow);
    let queued_g = crate::glyph(crate::glyphs::Glyph::Queued);
    let robot_g = crate::glyph(crate::glyphs::Glyph::Robot);
    let hourglass = crate::glyph(crate::glyphs::Glyph::Hourglass);
    let check_g = crate::glyph(crate::glyphs::Glyph::Check);
    let warn_g = crate::glyph(crate::glyphs::Glyph::Warning);

    println!("{}", "Integrator".bold());
    match focus_label.as_deref() {
        Some(f) => println!("{} {}", arrow.cyan(), format!("focus: {f}").cyan()),
        None => println!("{}", "scope: all (no focus set)".dimmed()),
    }
    println!();

    // Queue ------------------------------------------------------------------
    println!(
        "{}  {}",
        "Queue".bold(),
        format!("{} routed", queue_rows.len()).dimmed()
    );
    if queue_rows.is_empty() {
        println!("  {}", "(queue empty)".dimmed());
    } else {
        for (id, title, _status, role) in queue_rows.iter().take(15) {
            let role_col = if role.is_empty() {
                "unrouted".dimmed().to_string()
            } else {
                role.cyan().to_string()
            };
            println!(
                "  {} {:<14} {}  {}",
                queued_g.cyan(),
                id.yellow(),
                truncate(title, 48),
                role_col,
            );
        }
        if queue_rows.len() > 15 {
            println!(
                "  {}",
                format!("(+{} more)", queue_rows.len() - 15).dimmed()
            );
        }
    }
    println!();

    // Merge queue ------------------------------------------------------------
    let merge_queue_depth = merge_rows.iter().filter(|r| r.position.is_some()).count();
    println!(
        "{}  {}",
        "Merge queue".bold(),
        format!("{} ready", merge_queue_depth).dimmed()
    );
    if merge_rows.is_empty() {
        println!("  {}", "(no Done specs awaiting integration)".dimmed());
    } else {
        for row in merge_rows.iter().take(15) {
            let marker = if row.position.is_some() {
                arrow.green()
            } else {
                "·".dimmed()
            };
            println!(
                "  {} {:<14} {}  {}",
                marker,
                row.id.yellow(),
                truncate(&row.title, 48),
                row.state.cyan(),
            );
        }
        if merge_rows.len() > 15 {
            println!(
                "  {}",
                format!("(+{} more)", merge_rows.len() - 15).dimmed()
            );
        }
    }
    println!();

    // Throughput -------------------------------------------------------------
    println!("{}", "Throughput".bold());
    let last_merge_str = match throughput.last_merge {
        Some(ts) => {
            let secs = (now - ts).num_seconds().max(0) as u64;
            format!("{} ago", humanize_duration_secs(secs))
        }
        None => "no merges on record".to_string(),
    };
    let idle_badge = if idle.idle {
        format!("{} idle", warn_g).yellow().to_string()
    } else {
        format!("{} moving", check_g).green().to_string()
    };
    println!("  Last merge to main:  {last_merge_str}   {idle_badge}");
    if idle.idle {
        if let Some(mins) = idle.minutes_since_last_merge {
            println!(
                "  {}",
                format!(
                    "(no merge in {mins}m — threshold {}m)",
                    integrate_view::DEFAULT_IDLE_THRESHOLD_MINS
                )
                .dimmed()
            );
        }
    }
    println!(
        "  Merges to main:      {} in last 1h {} {} in last 24h",
        throughput.merges_last_hour,
        "·".dimmed(),
        throughput.merges_last_day,
    );
    if !drain_times.is_empty() {
        println!("  Drains completed:    {drains_last_day} in last 24h");
    }
    println!();

    // Active fan-out ---------------------------------------------------------
    println!(
        "{}  {}",
        "Running work".bold(),
        format!("{} active", run_rows.len()).dimmed()
    );
    if run_rows.is_empty() {
        println!("  {}", "(no active sessions)".dimmed());
    } else {
        for row in &run_rows {
            let spec_col = row
                .spec
                .clone()
                .unwrap_or_else(|| truncate(&row.lease.scope, 14));
            let role_col = row.role.as_deref().unwrap_or("-");
            let live_label = format!("{} {}", row.state.glyph(), row.state.label());
            let live_col = match row.state {
                LeaseState::Live => live_label.green(),
                LeaseState::Dormant => live_label.cyan(),
                LeaseState::Stale => live_label.yellow(),
            };
            println!(
                "  {} {:<14} {:<10} {:<8} {}",
                robot_g.cyan(),
                truncate(&spec_col, 14),
                truncate(role_col, 10),
                humanize_duration_secs(row.elapsed_secs),
                live_col,
            );
        }
    }
    if stale_hidden > 0 {
        println!(
            "  {}",
            format!(
                "({stale_hidden} stale session{} hidden — `aida ps --all` for detail)",
                if stale_hidden == 1 { "" } else { "s" }
            )
            .dimmed()
        );
    }
    println!();
    println!(
        "{}",
        format!("{hourglass} read-only view — `aida integrate --run` to drain the merge queue, `aida ps` for full session detail").dimmed()
    );

    Ok(())
}

/// TASK-1072: the minimal per-spec projection `gather_running_work` needs — a
/// lease scope's display id (row spec resolution) plus the In-Progress /
/// excluded-type flags for the orphan pass. Deliberately tiny so it can be built
/// from the cache-fast SQLite summaries (the STORY-707 pattern) instead of a
/// full `backend.load()` (which parses every YAML — ~1.0s of `aida ps`'s ~1.3s).
// trace:TASK-1072 | ai:claude
pub(crate) struct RunningWorkSpec {
    /// agreed_id || spec_id || uuid — what a resolved row / orphan displays.
    pub(crate) disp: String,
    pub(crate) agreed_id: Option<String>,
    pub(crate) spec_id: Option<String>,
    pub(crate) title: String,
    /// The spec is currently In Progress (orphan-pass candidate).
    pub(crate) in_progress: bool,
    /// Rollup / stateless type (epic / folder / meta) — excluded from the
    /// orphan pass because it never holds a spec-scoped lease.
    // trace:TASK-940 | ai:claude
    pub(crate) orphan_excluded_type: bool,
}

/// TASK-1072: string form of [`ps_orphan_excluded_type`], for the cache-fast
/// path where the type arrives as the cache's Debug-form string ("Epic",
/// "Folder", "Meta") rather than a parsed [`aida_core::RequirementType`]. Kept
/// in lockstep with the enum variant so the two paths agree.
// trace:TASK-1072 trace:TASK-940 | ai:claude
pub(crate) fn ps_orphan_excluded_type_str(req_type: &str) -> bool {
    req_type.eq_ignore_ascii_case("Epic")
        || req_type.eq_ignore_ascii_case("Folder")
        || req_type.eq_ignore_ascii_case("Meta")
}

/// TASK-1072: pure mapping from a cache summary's raw fields to a
/// [`RunningWorkSpec`] — the disp-id precedence (agreed_id → spec_id → uuid) and
/// the two derived flags (In-Progress candidacy, orphan-excluded type). Extracted
/// so the cache-fast projection is unit-testable without a live backend, and so
/// the string-form status/type decoding stays in one audited place.
// trace:TASK-1072 | ai:claude
pub(crate) fn running_work_spec_from_summary(s: aida_core::RequirementSummary) -> RunningWorkSpec {
    let disp = s
        .agreed_id
        .clone()
        .or_else(|| s.spec_id.clone())
        .unwrap_or_else(|| s.id.to_string());
    RunningWorkSpec {
        disp,
        in_progress: aida_core::RequirementStatus::from_filter_str(&s.status)
            == Some(aida_core::RequirementStatus::InProgress),
        orphan_excluded_type: ps_orphan_excluded_type_str(&s.req_type),
        agreed_id: s.agreed_id,
        spec_id: s.spec_id,
        title: s.title,
    }
}

/// TASK-1072: build the [`RunningWorkSpec`] index cache-fast. Reads the SQLite
/// summary projection (`list_summaries`, which triggers a stale-check +
/// auto-rebuild first — the same freshness contract bare `aida status` /
/// `aida list` ride) instead of a full `backend.load()`. The summaries carry
/// exactly the fields the running-work picture needs (ids, title, status,
/// req_type), so no YAML parse is required. Uses `ArchiveFilter::Both` +
/// `DeferFilter::Both` to match the prior full-store scan, which iterated every
/// requirement regardless of the archive/defer view flags. Falls back to the
/// full store load only when the cached backend is unreachable (legacy /
/// un-attached clones), preserving `load_store_for_lookup`'s degrade path.
/// Read-only; returns an empty index when no store is reachable.
// trace:TASK-1072 | ai:claude
pub(crate) fn running_work_spec_index(project_root: &std::path::Path) -> Vec<RunningWorkSpec> {
    if let Some(store_path) = detect_distributed_store_from(project_root) {
        if let Ok(backend) = advance_backend(&store_path) {
            let filter = aida_core::ListFilter {
                archive: aida_core::ArchiveFilter::Both,
                defer: aida_core::DeferFilter::Both,
                ..Default::default()
            };
            if let Ok(summaries) = backend.list_summaries(&filter) {
                return summaries
                    .into_iter()
                    .map(running_work_spec_from_summary)
                    .collect();
            }
        }
    }
    // Legacy / un-attached fallback: the full store load (same degrade path as
    // `load_store_for_lookup`). trace:TASK-1072 | ai:claude
    load_store_for_lookup(project_root)
        .map(|store| {
            store
                .requirements
                .iter()
                .map(|r| {
                    let disp = r
                        .agreed_id
                        .clone()
                        .or_else(|| r.spec_id.clone())
                        .unwrap_or_else(|| r.id.to_string());
                    RunningWorkSpec {
                        disp,
                        agreed_id: r.agreed_id.clone(),
                        spec_id: r.spec_id.clone(),
                        title: r.title.clone(),
                        in_progress: matches!(r.status, aida_core::RequirementStatus::InProgress),
                        orphan_excluded_type: ps_orphan_excluded_type(&r.req_type),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build the GLOBAL running-work picture: one [`PsRow`] per active session
/// lease (with liveness/pid/elapsed/spec resolved) plus the orphaned
/// In-Progress specs with no live spec-scoped lease. Extracted from `handle_ps`
/// so the `aida ps` table and the `aida integrate` throughput view share ONE
/// definition of "what's running right now" and can never disagree.
///
/// One pid-probe + one lease read feed both passes, the same single-probe
/// discipline `aida session leases` uses. Read-only; degrades gracefully when
/// the store is unreachable (rows still resolve, specs read as scope-unknown).
// trace:TASK-1034 trace:STORY-696 | ai:claude
pub(crate) fn gather_running_work(project_root: &std::path::Path) -> (Vec<PsRow>, Vec<PsOrphan>) {
    let now = chrono::Utc::now();
    // ONE /proc pass, reused for every liveness check below (memoized by
    // `probe_live_claude_sessions`'s OnceLock — BUG-613). The row + orphan
    // passes look each session up in this in-memory slice; they never re-probe
    // per lease. trace:TASK-1072 | ai:claude
    let live = process_probe::probe_live_claude_sessions();
    let leases = list_leases(project_root);
    let manifests = session_manifest::list_all(project_root);
    let manifest_roles: std::collections::HashMap<String, String> = manifests
        .iter()
        .filter_map(|m| {
            let role = m
                .claude_session_id
                .as_deref()
                .and_then(session::role_from_claude_session_id)?;
            Some((m.session_id.clone(), role))
        })
        .collect();
    // BUG-1553: lease id -> the `claude` conversation's own session id
    // (STORY-153's join, reused here) — lets the activity probe resolve
    // this session's transcript path even long after `probe_live_claude_sessions`'s
    // own recent-jsonl window (60s) has lapsed, which a 20-minute permission
    // block always exceeds. trace:BUG-1553 | ai:claude
    let manifest_claude_session_ids: std::collections::HashMap<String, String> = manifests
        .into_iter()
        .filter_map(|m| m.claude_session_id.map(|csid| (m.session_id, csid)))
        .collect();
    // TASK-1454: active (non-stale) pending-approval markers, keyed by the
    // Claude Code session id they were recorded for. `list_active` is cheap
    // when nothing is blocked (the overwhelming common case — a single
    // `read_dir` on a directory that's usually empty), so building this map
    // up front costs nothing extra on a quiet `aida ps`.
    // trace:TASK-1454 | ai:claude
    let pending_approvals: std::collections::HashMap<
        String,
        pending_approval::PendingApprovalMarker,
    > = pending_approval::list_active(project_root, now)
        .into_iter()
        .map(|m| (m.session_id.clone(), m))
        .collect();
    // BUG-1553 / TASK-1454: the real (marker-first, then transcript-tail +
    // /proc-reading) seat-activity probe — injected into `build_running_work`
    // the same way every other real I/O source here is, so the row-building
    // logic stays testable on fixtures. A recorded pending-approval marker is
    // ground truth and wins outright; only when none exists does this fall
    // back to reading the TAIL of the resolved transcript
    // (`read_transcript_tail`, capped at `TRANSCRIPT_TAIL_BYTES`, never the
    // whole file). trace:BUG-1553 trace:TASK-1454 | ai:claude
    let seat_activity_probe = |l: &SessionLease, pid: u32| -> SeatActivity {
        let marker = manifest_claude_session_ids
            .get(&l.id)
            .and_then(|csid| pending_approvals.get(csid));
        let tail = manifest_claude_session_ids.get(&l.id).and_then(|csid| {
            aida_core::liveness::claude_projects_dir_for_cwd(&l.worktree_path)
                .map(|dir| dir.join(format!("{csid}.jsonl")))
        });
        let tail_lines = tail.and_then(|path| read_transcript_tail(&path, TRANSCRIPT_TAIL_BYTES));
        seat_activity_with_marker(marker, tail_lines.as_deref(), proc_is_stopped(pid), now)
    };

    // The store gives us (a) the set of known spec ids (so a lease scope can be
    // resolved to a spec vs. a generic harness scope) and (b) the In-Progress
    // specs for the orphan pass. TASK-1072: this is the cache-fast summary
    // projection (the STORY-707 pattern), NOT a full `backend.load()` — profiling
    // showed the full YAML parse was ~1.0s of `aida ps`'s ~1.3s. Read-only;
    // degrades gracefully (empty index) when the store is unreachable.
    // trace:TASK-1072 | ai:claude
    let specs = running_work_spec_index(project_root);

    // trace:TASK-1090 | ai:claude — the real (git-probing) dispatch seam.
    // trace:TASK-1143 | ai:claude — the real (lease-reading) lock-owner seam:
    // `worktree_lock::read_authorized_by` reads the `authorized_by` value off the
    // session lease covering each worktree — the same STORY-711 slice-1 source
    // `aida lock` writes and the commit gate reads. Injected (not called inline)
    // so `build_running_work` stays filesystem-free and unit-testable.
    // trace:BUG-763 | ai:claude — the real (single-pid /proc-reading) start-time
    // seam, injected the same way as the git + lock probes so
    // `build_running_work` stays filesystem-free and unit-testable.
    let (mut rows, mut orphans) = build_running_work(
        &specs,
        &leases,
        &live,
        now,
        dispatch_health_ps::probe_worktree,
        |wt| worktree_lock::read_authorized_by(project_root, wt),
        pid_start_time,
        |jsonl| session::role_from_jsonl(jsonl, "claude").ok().flatten(),
        |lease_id| manifest_roles.get(lease_id).cloned(),
        probe_mail_identity,
        seat_activity_probe,
    );
    // TASK-163: a dead phase child does not make its lease stale while the
    // drain orchestrator owns that spec. Overlay the authoritative drain PID
    // and suppress the contradictory orphan row.
    for row in &mut rows {
        let Some(spec) = row.spec.as_deref() else {
            continue;
        };
        if let Some(drain) = drain_state::live_drain_spec(project_root, spec) {
            row.state = LeaseState::Live;
            row.pid = Some(drain.pid);
            row.pid_started_at = pid_start_time(drain.pid);
            row.role = Some(format!("drain {}", drain.phase));
            row.dispatch = None;
            // TASK-1451: re-probe under the authoritative drain pid, not the
            // (possibly stale/absent) pid this row resolved before the
            // overlay.
            row.mail_identity = Some(probe_mail_identity(drain.pid));
            // BUG-1553: a drain phase is orchestrator-driven, not a human
            // sitting at a permission prompt, and the overlay pid isn't the
            // one the lease's own transcript join resolves — leave activity
            // unclassified rather than risk a misattributed tail read.
            row.activity = None;
        }
    }
    // BUG-1681: a row whose spec has already FINISHED (done + approved, or
    // riding an integration PR) is waiting on the integrator — reframe it
    // before anything prints a resume hint for work that already landed.
    // trace:BUG-1681 | ai:claude
    ps_overlay_integration_standing(project_root, &mut rows);
    orphans.retain(|orphan| drain_state::live_drain_spec(project_root, &orphan.spec).is_none());
    // The derived driver: a relay / drain / registered agent working a spec
    // whose own lease pid is gone keeps the row live (no stalled framing).
    // trace:STORY-1653 | ai:claude
    driver_view::overlay_running_work(project_root, &leases, &mut rows, &mut orphans, now);
    (rows, orphans)
}

/// BUG-1523 (AC3): the pure mapping from `aida ps`'s orphan-detection output
/// ([`PsOrphan`], produced by [`build_running_work`] / [`gather_running_work`])
/// to the `aida awaiting` surface item ([`awaiting_you::OrphanedInProgressItem`]).
/// Extracted from `collect_awaiting_report_inner` so the "does a genuinely
/// orphaned In-Progress spec reach the report" question is testable end to end
/// from detection through emission, not just against a hand-built item (what
/// the pre-existing rendering test covered). A fan-out-worked flag-only spec
/// (TASK-1064) is filtered out here, same as `aida ps`'s own framing — it is
/// informational, not a genuine anomaly. `since_label_for` is injected so this
/// stays free of the cache/summary lookup and `Utc::now()` the real caller
/// wires in.
// trace:BUG-1523 | ai:claude
pub(crate) fn orphaned_in_progress_items(
    orphans: Vec<PsOrphan>,
    since_label_for: impl Fn(&str) -> String,
) -> Vec<awaiting_you::OrphanedInProgressItem> {
    orphans
        .into_iter()
        .filter(|o| !o.likely_fanout)
        // A spec a live relay/drain covers is being worked. trace:STORY-1653 | ai:claude
        .filter(|o| !driver_view::orphan_is_driven(o))
        .map(|o| awaiting_you::OrphanedInProgressItem {
            since_label: since_label_for(&o.spec),
            spec_id: o.spec,
            title: o.title,
            abandoned: o.stale_lease,
            // trace:BUG-1656 | ai:claude
            possibly_subagent: o.possibly_subagent,
        })
        .collect()
}

/// TASK-1454: `aida awaiting`'s view of every LIVE seat `aida ps` classifies
/// `SeatActivity::Blocked` — reuses `gather_running_work` verbatim (the same
/// marker-first classifier `seat_activity_probe` applies), so there is
/// exactly ONE place that decides "is this seat blocked," not a second
/// implementation drifting from the first.
///
/// Cheap on EVERY caller, including the per-turn notice-fast path: it starts
/// with [`pending_approval::list_active`], a single local directory read
/// that is empty on the overwhelming majority of turns (nothing is ever
/// blocked most of the time) — that empty case returns immediately with no
/// lease scan, no `/proc` probe, no store read at all. Only when a marker
/// genuinely exists does this pay `gather_running_work`'s heavier
/// lease-resolution cost, and that is exactly the rare, actionable moment
/// where paying it is worth it.
// trace:TASK-1454 | ai:claude
pub(crate) fn collect_blocked_seat_items(
    project_root: &std::path::Path,
) -> Vec<awaiting_you::BlockedSeatItem> {
    if pending_approval::list_active(project_root, chrono::Utc::now()).is_empty() {
        return Vec::new();
    }
    let (rows, _orphans) = gather_running_work(project_root);
    rows.into_iter()
        .filter_map(|row| match row.activity {
            Some(SeatActivity::Blocked { tool, secs }) => Some(awaiting_you::BlockedSeatItem {
                session_id: row.lease.id.clone(),
                spec: row.spec.clone().or(Some(row.lease.scope.clone())),
                tool,
                since_label: humanize_duration_secs(secs.max(0) as u64),
            }),
            _ => None,
        })
        .collect()
}

/// TASK-1072: the pure core of [`gather_running_work`] — given the resolved spec
/// index, the session leases, and the ONE already-computed live-session slice,
/// build the row + orphan picture. Extracted from the store/proc/lease I/O so
/// the running-work logic (scope→spec resolution, the orphan verdict, the
/// fan-out framing, the sort) is unit-testable on fixtures without touching the
/// filesystem or `/proc`. Takes `live` by reference — the single-probe
/// discipline (`aida ps` probes `/proc` once, not once per lease) is a caller
/// invariant this signature enforces.
///
/// TASK-1090: `dispatch_probe` is the ONE seam that touches git — injected
/// (rather than called directly) so this function stays exercisable on pure
/// fixtures in tests (a no-op stub) while the real caller
/// ([`gather_running_work`]) wires in [`dispatch_health_ps::probe_worktree`].
///
/// TASK-1143: `lock_probe` is a second injected seam — given a worktree path it
/// returns the advisor holding the STORY-711 worktree lock (the lease's
/// `authorized_by`), or `None`. Injected the same way as `dispatch_probe` so the
/// lock-owner resolution stays exercisable on pure fixtures; the real caller
/// wires in `worktree_lock::read_authorized_by`.
///
/// BUG-763: `pid_start_probe` is a third injected seam — given a live pid it
/// returns that process's start time, or `None`. Feeds the adopted-lease
/// annotation (a persistent lease whose pid is younger than the lease record);
/// the real caller wires in [`pid_start_time`].
///
/// TASK-153: `manifest_role_probe` resolves the transcript role through the
/// lease-manifest `claude_session_id` join, so long-lived resumed sessions do
/// not fall back to the stale harness lease role when the live `/proc` JSONL
/// association is absent.
// trace:TASK-1072 trace:STORY-696 trace:TASK-1090 trace:TASK-1143 trace:BUG-763 trace:TASK-153 | ai:claude
pub(crate) fn build_running_work(
    specs: &[RunningWorkSpec],
    leases: &[SessionLease],
    live: &[process_probe::LiveSession],
    now: chrono::DateTime<chrono::Utc>,
    dispatch_probe: impl Fn(&std::path::Path) -> dispatch_health_ps::WorktreeGitProbe,
    lock_probe: impl Fn(&std::path::Path) -> Option<String>,
    pid_start_probe: impl Fn(u32) -> Option<chrono::DateTime<chrono::Utc>>,
    role_probe: impl Fn(&std::path::Path) -> Option<String>,
    manifest_role_probe: impl Fn(&str) -> Option<String>,
    mail_identity_probe: impl Fn(u32) -> MailIdentityStatus,
    // BUG-1553: given the lease and its live pid, classify Working /
    // LongToolCall / Suspended / Unknown. Only called for a row with a
    // resolved live pid — a Dormant/Stale row has no process to inspect and
    // stays `None`.
    seat_activity_probe: impl Fn(&SessionLease, u32) -> SeatActivity,
) -> (Vec<PsRow>, Vec<PsOrphan>) {
    let rows: Vec<PsRow> = leases
        .iter()
        .map(|l| {
            let state = lease_state_for(l, live, now);
            // trace:BUG-1941 | ai:claude — deterministic pick among sessions
            // sharing the worktree.
            let live_in_worktree = ps_live_in_worktree(live, &l.worktree_path);
            let pid = l
                .active_pid
                .filter(|p| process_probe::pid_is_alive(*p))
                .or_else(|| live_in_worktree.map(|s| s.pid))
                .or(if l.review_verb || l.claim_verb {
                    l.creator_pid
                } else {
                    None
                });
            let live_by_pid = pid.and_then(|p| live.iter().find(|s| s.pid == p));
            let jsonl_role = live_by_pid
                .or(live_in_worktree)
                .and_then(|s| s.jsonl.as_deref())
                .and_then(&role_probe);
            let manifest_role = manifest_role_probe(&l.id);
            let lease_role = l.role.clone();
            // BUG-1521: the role shown must be READ from the session's own
            // record (the lease's stored `role`) whenever that record is a
            // REAL role — not the harness's generic Agent-tool placeholder
            // (`tail_cmd::HARNESS_AGENT_TYPE`, "general-purpose"). The
            // jsonl/manifest heuristics match on ambiguous signals (a shared
            // cwd, a text marker anywhere in the log) that can resolve to a
            // DIFFERENT live session's transcript — and can resolve
            // differently between two invocations of the same command
            // seconds apart, since each re-scans independently — so a real
            // recorded lease role is authoritative and wins first. But the
            // placeholder itself carries no information (every harness
            // fan-out subagent gets it, regardless of actual role), so a
            // lease stuck with it falls through to the derived signals
            // instead of masking them: manifest_role (TASK-153's stable
            // claude_session_id join) next, then jsonl_role (the ambiguous
            // transcript scan), and only the placeholder itself as the last
            // resort when nothing else resolved. trace:BUG-1521 | ai:claude
            // trace:BUG-1941 | ai:claude — no `subagent` label on the main
            // checkout's placeholder leases.
            let role = ps_row_role(
                lease_role.as_deref(),
                manifest_role.as_deref(),
                jsonl_role.as_deref(),
                ps_lease_is_main_checkout(l),
            );
            // BUG-763: resolve the backing pid's own start time so an adopted
            // persistent lease (pid younger than the lease record) can name
            // both ages instead of mixing provenance silently.
            let pid_started_at = pid.and_then(&pid_start_probe);
            // BUG-769: the displayed `elapsed` is the WORK-SESSION age — the
            // process's uptime on an adopted lease, the lease's age otherwise.
            // trace:BUG-769 | ai:claude
            let elapsed_secs = ps_elapsed_secs(l.started_at, pid_started_at, now);
            // The lease record's own age stays the input to the dispatch-health
            // stalled threshold (TASK-1090 semantics, deliberately unchanged by
            // BUG-769 — that classifier is not the elapsed column).
            let lease_elapsed_secs =
                now.signed_duration_since(l.started_at).num_seconds().max(0) as u64;
            let spec = specs
                .iter()
                .find(|s| {
                    [s.agreed_id.as_deref(), s.spec_id.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|id| l.scope.eq_ignore_ascii_case(id))
                })
                .map(|s| s.disp.clone());
            // TASK-1090: worktree-less advisory leases (review/claim locks)
            // have no git state to classify — dispatch stays None for them.
            // A dead-worktree lease (removed dir) also has nothing to probe;
            // `dispatch_health_ps::probe_worktree` degrades to the zero
            // default in that case rather than erroring.
            let dispatch = if l.review_verb || l.claim_verb {
                None
            } else {
                let probe = dispatch_probe(&l.worktree_path);
                // BUG-752: tri-state — a harness (Agent-tool) lease with no
                // recorded pid has UNDETERMINABLE liveness (the worker runs
                // inside the parent claude process, invisible to the
                // cwd-based worktree probe), so it must not be treated as a
                // dead process and offered the salvage-commit hint.
                let pid_alive = ps_pid_liveness(state, harness_lease_without_pid_signal(l));
                // BUG-1941: a live process with its cwd in this worktree means
                // the worktree is in use — never Salvageable, whatever this
                // lease's own recorded pid says. trace:BUG-1941 | ai:claude
                let pid_alive = dispatch_health_ps::pid_liveness_with_occupant(
                    pid_alive,
                    live_in_worktree.is_some(),
                );
                // BUG-778: how long ago `aida worktree enter|add` handed this
                // worktree to a human, when it did. `None` on every
                // orchestrator-spawned lease — those DO expect an agent, so
                // their crash detection is untouched. trace:BUG-778 | ai:claude
                let manual_enter_secs = ps_manual_enter_secs(l, now);
                // BUG-1656: a dirty tree still being written is liveness in
                // its own right — never offer the salvage-commit while the
                // files are changing under someone. trace:BUG-1656 | ai:claude
                let dirty_movement_fresh = dispatch_health_ps::dirty_movement_is_fresh(
                    probe.dirty_newest_mtime_age_secs,
                    dispatch_health_ps::DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
                );
                let ds = dispatch_health_ps::dispatch_state_with_movement(
                    pid_alive,
                    probe.dirty,
                    probe.ahead_of_main,
                    lease_elapsed_secs,
                    dispatch_health_ps::DEFAULT_STALLED_THRESHOLD_SECS,
                    manual_enter_secs,
                    dispatch_health_ps::DEFAULT_AWAITING_AGENT_GRACE_SECS,
                    dirty_movement_fresh,
                );
                // TASK-1518: a lease the drain's SIGTERM handler stamped on
                // its way out is a stopped wave, not a crashed agent — the
                // dead-process/clean-tree arm reads `stopped`.
                // trace:TASK-1518 | ai:claude
                // BUG-1941: a fresh commit on a branch ahead of main is
                // movement; untracked-only scratch on a protected branch (the
                // main checkout) is not salvageable work. trace:BUG-1941 | ai:claude
                // An interrupted drain wave stays `stopped` (TASK-1518) even
                // with a fresh commit; the commit-only lift is remembered so
                // the integration overlay can still read the row.
                let before_commit_movement = ds;
                let ds = if l.interrupted_at.is_some() {
                    ds
                } else {
                    dispatch_health_ps::apply_commit_movement(
                        ds,
                        pid_alive,
                        probe.ahead_of_main,
                        probe.head_commit_age_secs,
                        dispatch_health_ps::DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
                    )
                };
                let commit_movement_from =
                    (ds != before_commit_movement).then_some(before_commit_movement);
                let ds = if ds == dispatch_health_ps::DispatchState::Salvageable
                    && probe.untracked_records_only
                {
                    dispatch_health_ps::apply_main_checkout_untracked(
                        ds,
                        dispatch_health_ps::is_protected_branch_at(&l.worktree_path, &l.branch),
                        probe.untracked_records_only,
                    )
                } else {
                    ds
                };
                let ds = dispatch_health_ps::apply_interruption(
                    ds,
                    pid_alive,
                    l.interrupted_at.is_some(),
                );
                let hint = dispatch_health_ps::next_command_hint_with_untracked(
                    ds,
                    &l.worktree_path,
                    &l.branch,
                    probe.last_commit_subject.as_deref(),
                    spec.as_deref(),
                    manual_enter_secs.is_some(),
                    probe.untracked_only,
                );
                Some(PsDispatch {
                    state: ds,
                    hint,
                    dirty: probe.dirty,
                    ahead_of_main: probe.ahead_of_main,
                    untracked_only: probe.untracked_only,
                    commit_movement_from,
                })
            };
            // TASK-1143: the worktree lock owner (if any) for this row, read
            // from the same lease source the lock CLI writes. A worktree-less
            // advisory lease (review/claim) has an empty path that matches no
            // lease → `None`.
            let locked_by = lock_probe(&l.worktree_path).filter(|s| !s.is_empty());
            // TASK-1451: only probe a pid that actually backs this row — no
            // live process, nothing to read an environment from.
            let mail_identity = pid.map(&mail_identity_probe);
            // BUG-1553: same gate as mail_identity — only a row with a live
            // pid has a process worth classifying at all.
            let activity = pid.map(|p| seat_activity_probe(l, p));
            PsRow {
                lease: l.clone(),
                state,
                role,
                lease_role,
                pid,
                pid_started_at,
                elapsed_secs,
                spec,
                dispatch,
                locked_by,
                mail_identity,
                activity,
                driver: None, // trace:STORY-1653 | ai:claude
            }
        })
        .collect();

    // Orphan pass: every In-Progress spec with no LIVE spec-scoped lease. A
    // dead-pid (STALE) lease still counts as orphaned — the flag is not
    // liveness-backed — but we mark it so the operator sees "crashed session"
    // vs "never started". trace:STORY-696
    let mut orphans: Vec<PsOrphan> = Vec::new();
    // TASK-1064: is a live advisor Agent-tool fan-out running? If so, a flag-only
    // In-Progress spec (no spec-scoped lease) is most likely being built by it
    // (the fan-out takes a generic non-spec-linked harness lease) — surface it
    // informationally instead of as a genuine orphan. trace:TASK-1064 | ai:claude
    // BUG-1681: the live fan-out leases THEMSELVES, so the framing below can
    // be matched per spec instead of applied to every flag-only row in the
    // store the moment any subagent is alive. trace:BUG-1681 | ai:claude
    // BUG-1740: the caller is excluded from fan-out corroboration. Resolved
    // ONCE here (one /proc walk per `aida ps`, not one per lease) and passed
    // into the pure predicate. trace:BUG-1740 | ai:claude
    let caller_pids = ps_caller_pid_chain();
    let fanouts = ps_live_fanout_leases(leases, live, now, &caller_pids);
    for s in specs {
        if !s.in_progress {
            continue;
        }
        // Rollup / stateless types (epic, folder, meta) never hold a
        // spec-scoped lease, so they'd all read as orphaned — skip them so
        // only real work-item In-Progress specs surface. trace:TASK-940
        if s.orphan_excluded_type {
            continue;
        }
        let mut id_owned: Vec<String> = Vec::new();
        if let Some(a) = s.agreed_id.as_deref() {
            id_owned.push(a.to_string());
        }
        if let Some(sp) = s.spec_id.as_deref() {
            id_owned.push(sp.to_string());
        }
        let id_refs: Vec<&str> = id_owned.iter().map(|s| s.as_str()).collect();
        let lease = spec_scoped_lease(leases, &id_refs);
        let lease_state = lease.map(|l| lease_state_for(l, live, now));
        // trace:BUG-778 | ai:claude
        let awaiting_agent = lease.is_some_and(|l| ps_row_awaiting_agent(&rows, &l.id));
        // BUG-1656: a non-live spec lease whose worktree is still being
        // written (fresh dirty mtimes) is being worked — by an Agent-tool
        // subagent the pid probe cannot see — not orphaned. Only probed for
        // the few non-live spec leases, never for every row.
        // trace:BUG-1656 | ai:claude
        let dirty_movement_fresh = lease
            .filter(|_| !matches!(lease_state, Some(LeaseState::Live)))
            .filter(|l| !l.review_verb && !l.claim_verb)
            .map(|l| dispatch_probe(&l.worktree_path))
            .is_some_and(|p| {
                p.dirty
                    && dispatch_health_ps::dirty_movement_is_fresh(
                        p.dirty_newest_mtime_age_secs,
                        dispatch_health_ps::DEFAULT_DIRTY_MOVEMENT_FRESH_SECS,
                    )
            });
        // BUG-1681: attribution must MATCH. A live subagent counts for this
        // spec only when its own branch/worktree names the spec (the flag-only
        // framing), or when it is working inside this spec's own leased
        // worktree (the stale-lease framing). trace:BUG-1681 | ai:claude
        let fanout_names_this_spec = fanouts.iter().any(|f| ps_fanout_names_spec(f, &id_refs));
        let subagent_in_this_worktree =
            lease.is_some_and(|l| fanouts.iter().any(|f| ps_fanout_holds_lease(f, l)));
        if let Some(stale_lease) =
            ps_orphan_verdict_with_movement(lease_state, awaiting_agent, dirty_movement_fresh)
        {
            orphans.push(PsOrphan {
                spec: s.disp.clone(),
                title: s.title.clone(),
                stale_lease,
                // trace:TASK-1064 | ai:claude
                // trace:BUG-1681 | ai:claude
                likely_fanout: ps_orphan_likely_fanout(stale_lease, fanout_names_this_spec),
                // trace:BUG-1656 | ai:claude
                // trace:BUG-1681 | ai:claude
                possibly_subagent: stale_lease && subagent_in_this_worktree,
                driver: None, // trace:STORY-1653 | ai:claude
            });
        }
    }
    orphans.sort_by(|a, b| a.spec.cmp(&b.spec));

    (rows, orphans)
}

/// BUG-1680: Group salvageable rows by worktree path to collapse duplicates.
/// Rows for the same worktree path are shown once with a count, preserving all spec IDs.
// trace:BUG-1680 | ai:antigravity
pub(crate) struct CollapsedSalvageRow<'a> {
    pub(crate) row: &'a PsRow,
    pub(crate) count: usize,
    pub(crate) spec_ids: Vec<String>,
    pub(crate) untracked_only: bool,
}

impl<'a> CollapsedSalvageRow<'a> {
    pub(crate) fn display_spec(&self) -> String {
        if !self.spec_ids.is_empty() {
            self.spec_ids.join(", ")
        } else {
            self.row
                .spec
                .clone()
                .unwrap_or_else(|| self.row.lease.scope.clone())
        }
    }

    pub(crate) fn display_spec_with_count(&self) -> String {
        let spec = self.display_spec();
        if self.count > 1 {
            format!("{spec} ({} sessions)", self.count)
        } else {
            spec
        }
    }
}

// trace:BUG-1680 | ai:antigravity
pub(crate) fn collapse_salvageable_by_worktree<'a>(
    rows: &[&'a PsRow],
) -> Vec<CollapsedSalvageRow<'a>> {
    let mut collapsed: Vec<CollapsedSalvageRow<'a>> = Vec::new();
    for row in rows {
        let wt = &row.lease.worktree_path;
        if wt.as_os_str().is_empty() {
            let mut spec_ids = Vec::new();
            if let Some(ref s) = row.spec {
                spec_ids.push(s.clone());
            }
            let untracked_only = row
                .dispatch
                .as_ref()
                .map(|d| d.untracked_only)
                .unwrap_or(false);
            collapsed.push(CollapsedSalvageRow {
                row,
                count: 1,
                spec_ids,
                untracked_only,
            });
            continue;
        }

        if let Some(pos) = collapsed
            .iter()
            .position(|c| &c.row.lease.worktree_path == wt)
        {
            collapsed[pos].count += 1;
            if matches!(row.state, LeaseState::Live)
                && !matches!(collapsed[pos].row.state, LeaseState::Live)
            {
                collapsed[pos].row = row;
            } else if collapsed[pos].row.dispatch.is_none() && row.dispatch.is_some() {
                collapsed[pos].row = row;
            }
            if let Some(ref s) = row.spec {
                if !collapsed[pos].spec_ids.contains(s) {
                    collapsed[pos].spec_ids.push(s.clone());
                }
            }
            if let Some(d) = &row.dispatch {
                if collapsed[pos].row.dispatch.is_none() {
                    collapsed[pos].untracked_only = d.untracked_only;
                } else if !d.untracked_only {
                    collapsed[pos].untracked_only = false;
                }
            }
        } else {
            let mut spec_ids = Vec::new();
            if let Some(ref s) = row.spec {
                spec_ids.push(s.clone());
            }
            let untracked_only = row
                .dispatch
                .as_ref()
                .map(|d| d.untracked_only)
                .unwrap_or(false);
            collapsed.push(CollapsedSalvageRow {
                row,
                count: 1,
                spec_ids,
                untracked_only,
            });
        }
    }
    collapsed
}

pub(crate) fn handle_ps(json: bool, all: bool) -> Result<()> {
    let project_root =
        find_project_root().unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());

    let (rows, orphans) = gather_running_work(&project_root);
    // trace:TASK-1285 | ai:codex
    let live_wave = drain_lock::read_pid_live_lock(&project_root);

    // STORY-769: the last-human-input presence oracle — "operator last seen Nm
    // ago" + an active/idle/stale verdict, from the per-turn `aida awaiting
    // --notice` stamp. A fan-out overseer reading `aida ps` sees at a glance
    // whether the operator is around to answer an interactive ask.
    // trace:STORY-769 | ai:claude
    let now = chrono::Utc::now();
    let thresholds = presence::read_presence_thresholds(&config_path_for_project(&project_root));
    let operator_last_seen = presence::latest_human_input();

    if json {
        let operator = match operator_last_seen {
            Some(last) => serde_json::json!({
                "last_seen": last.to_rfc3339(),
                "ago": presence::since_label(last, now),
                "verdict": presence::human_presence(now, last, thresholds).word(),
            }),
            None => serde_json::Value::Null,
        };
        let row_refs: Vec<&PsRow> = rows.iter().collect();
        let collapsed_rows = collapse_salvageable_by_worktree(&row_refs);
        let sessions: Vec<serde_json::Value> = collapsed_rows
            .iter()
            .map(|item| {
                let row = item.row;
                let spec_val = if item.spec_ids.is_empty() {
                    row.spec.clone()
                } else {
                    Some(item.spec_ids.join(", "))
                };
                serde_json::json!({
                    "session_id": row.lease.id,
                    "scope": row.lease.scope,
                    "spec": spec_val,
                    "specs": item.spec_ids,
                    "count": item.count,
                    "untracked_only": item.untracked_only,
                    "role": row.role,
                    "lease_role": row.lease_role,
                    "worktree": row.lease.worktree_path.display().to_string(),
                    "branch": row.lease.branch,
                    "pid": row.pid,
                    "started_at": row.lease.started_at.to_rfc3339(),
                    // BUG-763: the backing pid's OWN start time (null when
                    // unresolvable) — an adopted persistent lease has a pid
                    // younger than the lease record, and both ages matter.
                    "pid_started_at": row.pid_started_at.map(|t| t.to_rfc3339()),
                    "elapsed_secs": row.elapsed_secs,
                    "liveness": row.state.label(),
                    "live": matches!(row.state, LeaseState::Live),
                    // TASK-1090: dispatch-health — null for worktree-less
                    // advisory leases (no git state to classify).
                    "dispatch_state": row.dispatch.as_ref().map(|d| d.state.label()),
                    "dispatch_hint": row.dispatch.as_ref().and_then(|d| d.hint.clone()),
                    "worktree_dirty": row.dispatch.as_ref().map(|d| d.dirty),
                    "branch_ahead_of_main": row.dispatch.as_ref().map(|d| d.ahead_of_main),
                    // TASK-1143: the advisor holding the STORY-711 worktree lock
                    // on this row's worktree — null when unlocked (the common
                    // case; `[locking]` is opt-in).
                    "locked_by": row.locked_by,
                    // TASK-1451: null when no live pid backs the row (nothing
                    // to probe); otherwise "attributed" / "unattributed" /
                    // "unknown" — never collapsed to a boolean "fine".
                    "mail_identity": row.mail_identity.map(MailIdentityStatus::as_str),
                    // BUG-1553 / TASK-1454: "working" / "long_tool_call" /
                    // "suspended" / "unknown" / "blocked", null when no live
                    // pid backs the row (nothing to classify). "blocked" is
                    // the only value sourced from ground truth (the
                    // Notification hook marker) rather than a heuristic.
                    "activity": row.activity.as_ref().map(SeatActivity::label),
                    "activity_pending_tool": match &row.activity {
                        Some(SeatActivity::LongToolCall { tool, .. })
                        | Some(SeatActivity::Blocked { tool, .. }) => tool.clone(),
                        _ => None,
                    },
                    "activity_secs": match &row.activity {
                        Some(SeatActivity::LongToolCall { secs, .. })
                        | Some(SeatActivity::Blocked { secs, .. }) => Some(*secs),
                        _ => None,
                    },
                })
            })
            .collect();
        // The derived driver (`driver` / `driver_kind`). trace:STORY-1653 | ai:claude
        let mut sessions = sessions;
        for (v, item) in sessions.iter_mut().zip(collapsed_rows.iter()) {
            driver_view::insert_json_fields(v, item.row.driver.as_ref());
        }
        // A spec with no live session of its own but a live relay/drain
        // driver is being worked: report it under `driven`, never `orphaned`.
        // trace:STORY-1653 | ai:claude
        let driven: Vec<serde_json::Value> = orphans
            .iter()
            .filter(|o| driver_view::orphan_is_driven(o))
            .map(|o| {
                let mut v = serde_json::json!({ "spec": o.spec, "title": o.title });
                driver_view::insert_json_fields(&mut v, o.driver.as_ref());
                v
            })
            .collect();
        let orphaned: Vec<serde_json::Value> = orphans
            .iter()
            .filter(|o| !driver_view::orphan_is_driven(o))
            .map(|o| {
                serde_json::json!({
                    "spec": o.spec,
                    "title": o.title,
                    "liveness": if o.stale_lease { "stale" } else { "flag-only" },
                    // TASK-1064: a flag-only spec while a fan-out is live is most
                    // likely being built by it, not genuinely orphaned.
                    "likely_fanout": o.likely_fanout,
                    // trace:BUG-1656 | ai:claude
                    "possibly_subagent": o.possibly_subagent,
                    "live": false,
                    // BUG-1553: an orphan has no lease/worktree to probe a
                    // process against at all, so whether it's blocked or
                    // truly exited is never determinable here — say so
                    // explicitly rather than let "flag-only" imply "just
                    // not started". A stale (crashed) lease IS unambiguous
                    // (the process is confirmed gone), so it keeps its own
                    // "exited" reading instead of "unknown".
                    "activity": if o.stale_lease { "exited" } else { "unknown" },
                })
            })
            .collect();
        // Orphans carry their queued/idle reading. trace:STORY-1653 | ai:claude
        let mut orphaned = orphaned;
        for (v, o) in orphaned
            .iter_mut()
            .zip(orphans.iter().filter(|o| !driver_view::orphan_is_driven(o)))
        {
            driver_view::insert_json_fields(v, o.driver.as_ref());
        }
        println!(
            "{}",
            crate::cache_output::json_pretty(&serde_json::json!({
                "sessions": sessions,
                "orphaned": orphaned,
                // trace:STORY-1653 | ai:claude
                "driven": driven,
                // STORY-769: last-human-input oracle, null when never stamped.
                "operator": operator,
                "wave": live_wave.as_ref().map(|l| serde_json::json!({
                    "id": if l.wave_id.is_empty() { format!("pid-{}", l.pid) } else { l.wave_id.clone() },
                    "pid": l.pid,
                    "binary_sha": l.binary_sha,
                    "binary_mtime_secs": l.binary_mtime_secs,
                    "binary_path": l.binary_path,
                })),
            }))?
        );
        return Ok(());
    }

    // Agent mode: token-efficient TOON, mirroring `aida integrate` / `awaiting`
    // so agents orienting via `aida ps` get flat scalars + uniform tables, not
    // the human column table. `--json` above still wins for structured
    // consumers; stale rows are hidden behind a count as in the human view.
    // trace:STORY-753 | ai:claude
    if agent_output_mode() {
        let (shown, hidden_stale): (Vec<&PsRow>, Vec<&PsRow>) = if all {
            (rows.iter().collect(), Vec::new())
        } else {
            rows.iter()
                .partition(|r| !matches!(r.state, LeaseState::Stale))
        };
        // TASK-1090: dead-and-dirty rows are the single highest-value signal
        // this report adds — never let them hide behind the stale-hidden
        // footer just because `--all` wasn't passed. Computed from the FULL
        // row set (not `shown`) so a machine consumer sees them regardless.
        let salvageable_hidden: Vec<&PsRow> = hidden_stale
            .iter()
            .filter(|r| {
                r.dispatch
                    .as_ref()
                    .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::Salvageable)
            })
            .copied()
            .collect();

        println!("view: ps");
        // BUG-1521 (proxy decision): `running:` stays a bare integer — the
        // shape every other agent-mode view (`aida integrate`, `aida
        // awaiting`) uses — so a machine consumer can parse it without
        // branching on whether a parenthetical got appended. The hidden
        // count lives in its own `stale_hidden:` scalar right below, exactly
        // like `aida integrate`'s `running:`/`stale_hidden:` pair.
        println!("running: {}", shown.len());
        println!("stale_hidden: {}", hidden_stale.len());
        // trace:STORY-1653 | ai:claude — driven orphans have their own count.
        let driven_orphans = orphans
            .iter()
            .filter(|o| driver_view::orphan_is_driven(o))
            .count();
        println!("orphaned: {}", orphans.len() - driven_orphans);
        println!("driven: {driven_orphans}");
        if let Some(lock) = &live_wave {
            println!(
                "wave_id: {}",
                if lock.wave_id.is_empty() {
                    format!("pid-{}", lock.pid)
                } else {
                    lock.wave_id.clone()
                }
            );
            println!("wave_binary_sha: {}", lock.binary_sha);
            println!(
                "wave_binary_mtime_secs: {}",
                lock.binary_mtime_secs
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into())
            );
        }
        // STORY-769: last-human-input oracle as flat scalars for agent consumers.
        match operator_last_seen {
            Some(last) => {
                println!("operator_last_seen: {}", presence::since_label(last, now));
                println!(
                    "operator_presence: {}",
                    presence::human_presence(now, last, thresholds).word()
                );
            }
            None => {
                println!("operator_last_seen: unknown");
                println!("operator_presence: unknown");
            }
        }
        let collapsed_shown = collapse_salvageable_by_worktree(&shown);
        let run: Vec<Vec<String>> = collapsed_shown
            .iter()
            .map(|item| {
                let r = item.row;
                vec![
                    r.lease.id.clone(),
                    item.display_spec_with_count(),
                    r.role.clone().unwrap_or_else(|| "-".to_string()),
                    r.pid
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    humanize_duration_secs(r.elapsed_secs),
                    r.state.label().to_string(),
                    // TASK-1143: the worktree lock owner, blank when unlocked.
                    ps_locked_by_cell(r.locked_by.as_deref()),
                    // TASK-1090: dispatch-health state + the exact next
                    // command, blank for Moving / worktree-less rows.
                    r.dispatch
                        .as_ref()
                        .map(|d| d.state.label().to_string())
                        .unwrap_or_default(),
                    r.dispatch
                        .as_ref()
                        .and_then(|d| d.hint.clone())
                        .unwrap_or_default(),
                    // TASK-1451: blank when no live pid backs the row;
                    // otherwise "attributed" / "unattributed" / "unknown".
                    r.mail_identity
                        .map(MailIdentityStatus::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    // BUG-1553: blank when no live pid backs the row;
                    // otherwise "working" / "long_tool_call" / "suspended" /
                    // "unknown".
                    r.activity
                        .as_ref()
                        .map(SeatActivity::label)
                        .unwrap_or_default()
                        .to_string(),
                ]
                .into_iter()
                // trace:STORY-1653 | ai:claude
                .chain(driver_view::toon_cells(r.driver.as_ref()))
                .collect()
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "running",
                &[
                    "session",
                    "spec",
                    "role",
                    "pid",
                    "elapsed",
                    "live",
                    "locked_by",
                    "dispatch_state",
                    "dispatch_hint",
                    "mail_identity",
                    "activity",
                    "driver",
                    "driver_kind"
                ],
                &run
            )
        );
        // trace:STORY-1653 | ai:claude — driven specs are not orphans.
        let orph: Vec<Vec<String>> = orphans
            .iter()
            .filter(|o| !driver_view::orphan_is_driven(o))
            .map(|o| {
                let [driver, driver_kind] = driver_view::toon_cells(o.driver.as_ref());
                vec![
                    o.spec.clone(),
                    o.title.clone(),
                    if o.stale_lease {
                        "stale".to_string()
                    } else {
                        "flag-only".to_string()
                    },
                    o.likely_fanout.to_string(),
                    driver,
                    driver_kind,
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "orphaned",
                &[
                    "spec",
                    "title",
                    "liveness",
                    "likely_fanout",
                    "driver",
                    "driver_kind"
                ],
                &orph
            )
        );
        let driven: Vec<Vec<String>> = orphans
            .iter()
            .filter(|o| driver_view::orphan_is_driven(o))
            .map(|o| {
                let [driver, driver_kind] = driver_view::toon_cells(o.driver.as_ref());
                vec![o.spec.clone(), o.title.clone(), driver, driver_kind]
            })
            .collect();
        if !driven.is_empty() {
            println!(
                "{}",
                crate::toon::table_raw(
                    "driven",
                    &["spec", "title", "driver", "driver_kind"],
                    &driven
                )
            );
        }
        // TASK-1090: always-shown (not gated by --all) — dead process +
        // uncommitted work hidden behind the stale-session footer.
        // BUG-1680: collapse duplicate rows for the same worktree path with a count.
        // trace:BUG-1680 | ai:antigravity
        let collapsed = collapse_salvageable_by_worktree(&salvageable_hidden);
        let salv: Vec<Vec<String>> = collapsed
            .iter()
            .map(|item| {
                let r = item.row;
                vec![
                    r.lease.id.clone(),
                    item.display_spec_with_count(),
                    r.lease.worktree_path.display().to_string(),
                    r.dispatch
                        .as_ref()
                        .and_then(|d| d.hint.clone())
                        .unwrap_or_default(),
                ]
            })
            .collect();
        println!(
            "{}",
            crate::toon::table_raw(
                "salvageable",
                &["session", "spec", "worktree", "hint"],
                &salv
            )
        );
        return Ok(());
    }

    // Hide STALE session rows behind a footer count unless --all, mirroring
    // `aida session leases`. trace:STORY-696
    let (shown, hidden_stale): (Vec<&PsRow>, Vec<&PsRow>) = if all {
        (rows.iter().collect(), Vec::new())
    } else {
        rows.iter()
            .partition(|r| !matches!(r.state, LeaseState::Stale))
    };

    // TASK-965: stranded-primary alarm — loud banner ABOVE the running-work table
    // when the primary checkout is parked on a feature branch with in-flight
    // leases. trace:TASK-965 | ai:claude
    if let Some(stranded) = detect_stranded_primary(&project_root) {
        print_stranded_primary_banner(&stranded);
    }

    println!("{}", "Running work".bold());
    if let Some(lock) = &live_wave {
        let wave = if lock.wave_id.is_empty() {
            format!("pid-{}", lock.pid)
        } else {
            lock.wave_id.clone()
        };
        println!(
            "live wave {wave} · binary {} · mtime {}",
            truncate(&lock.binary_sha, 8),
            lock.binary_mtime_secs
                .and_then(|secs| {
                    std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
                })
                .map(|time| {
                    let local: chrono::DateTime<chrono::Local> = time.into();
                    local.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|| "?".into())
        );
    }
    // STORY-769: last-human-input oracle line — "operator last seen Nm ago —
    // active/idle/stale". Quiet ("unknown") until the per-turn notice stamps.
    match presence::last_seen_line(now, thresholds) {
        Some(line) => println!("{}", line.dimmed()),
        None => println!("{}", "operator last seen — unknown".dimmed()),
    }
    println!();

    if shown.is_empty() && hidden_stale.is_empty() {
        println!("{}", "(no active sessions)".dimmed());
    } else {
        let all_ids: Vec<&str> = rows.iter().map(|r| r.lease.id.as_str()).collect();
        // TASK-1168: size the spec + role columns to their widest cell (bounded)
        // instead of pre-truncating every cell to a fixed ~13 visible chars —
        // `harness-worktree` / `general-purpose` are short, bounded identifiers
        // and must render whole. trace:TASK-1168 | ai:claude
        let collapsed_shown = collapse_salvageable_by_worktree(&shown);
        let spec_cells: Vec<String> = collapsed_shown
            .iter()
            .map(|item| item.display_spec_with_count())
            .collect();
        let role_cells: Vec<String> = collapsed_shown
            .iter()
            .map(|item| item.row.role.clone().unwrap_or_else(|| "-".to_string()))
            .collect();
        let spec_w = ps_column_width(&spec_cells, PS_SPEC_MIN_WIDTH, PS_SPEC_MAX_WIDTH);
        let role_w = ps_column_width(&role_cells, PS_ROLE_MIN_WIDTH, PS_ROLE_MAX_WIDTH);
        // Column start offsets, so an overflow wrap continues under its column.
        let spec_offset = 11;
        let role_offset = spec_offset + spec_w + 1;
        // BUG-763: the started column is wide enough for a date-qualified
        // "Jun-26 11:55" — a non-today start always shows its date.
        let header = format!(
            "{:<10} {:<specw$} {:<rolew$} {:<8} {:<12} {:<11} {:<12} {}",
            "session",
            "spec",
            "role",
            "pid",
            "started",
            "elapsed",
            "locked-by",
            "live",
            specw = spec_w,
            rolew = role_w,
        );
        println!("{}", header.dimmed());
        for item in &collapsed_shown {
            let row = item.row;
            let l = &row.lease;
            let prefix_len = unique_prefix_len(&l.id, &all_ids, 8);
            let spec_col = item.display_spec_with_count();
            let pid_col = row.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
            // BUG-763: time-of-day for today's leases, "Jun-26 11:55" for
            // anything older — a June birth must never read as this morning
            // next to a 500h elapsed.
            // BUG-1941: the same anchor as `elapsed` — the live process on an
            // adopted lease — so the two columns never describe different
            // things. trace:BUG-1941 | ai:claude
            let started = ps_started_cell(
                ps_session_started_at(l.started_at, row.pid_started_at)
                    .with_timezone(&chrono::Local)
                    .naive_local(),
                now.with_timezone(&chrono::Local).date_naive(),
            );
            // trace:STORY-1653 | ai:claude — `live · relay:<run>`.
            let live_label = driver_view::ps_live_label(row);
            // BUG-1553 (2026-09-23 PROXY DECISION): neither `LongToolCall`
            // nor `Suspended` overrides this cell's color/text — a purely
            // time-based or process-state heuristic cannot assert "blocked
            // on your approval", so the `live` cell keeps its ordinary
            // Live/Dormant/Stale coloring for those; the neutral/warning
            // activity note prints as an extra line below instead.
            // `SeatActivity::Blocked` is different: it is a REAL
            // Notification-hook marker, not a heuristic, so THIS is the
            // ground-truth case the proxy decision deferred — it DOES
            // override the cell, loudly, naming the state "Blocked"
            // instead of "Live" so it reads as visually distinct, not a
            // footnote (BUG-1553 AC1). trace:BUG-1553 trace:TASK-1454 | ai:claude
            let live_col = match &row.activity {
                Some(SeatActivity::Blocked { .. }) => {
                    format!("{} Blocked", crate::glyph(crate::glyphs::Glyph::Blocked)).red()
                }
                _ => match row.state {
                    LeaseState::Live => live_label.green(),
                    LeaseState::Dormant => live_label.cyan(),
                    LeaseState::Stale => live_label.yellow(),
                },
            };
            // TASK-1143: the worktree lock owner, blank when unlocked. Plain
            // text (paddable), so it slots into the fixed-width table before the
            // colored `live` column without breaking alignment.
            let locked_col = ps_locked_by_cell(row.locked_by.as_deref());
            // TASK-1168: an over-wide cell WRAPS onto an indented continuation
            // line (nothing is lost) instead of being ellipsized mid-word.
            let (spec_head, spec_rest) = ps_wrap_cell(&spec_col, spec_w);
            let (role_head, role_rest) = ps_wrap_cell(row.role.as_deref().unwrap_or("-"), role_w);
            println!(
                "{:<10} {:<specw$} {:<rolew$} {:<8} {:<12} {:<11} {:<12} {}",
                (&l.id[..prefix_len]).yellow(),
                spec_head,
                role_head,
                pid_col,
                started,
                humanize_duration_secs(row.elapsed_secs),
                truncate(&locked_col, 12),
                live_col,
                specw = spec_w,
                rolew = role_w,
            );
            if let Some(rest) = spec_rest {
                println!("{}{}", " ".repeat(spec_offset), rest.dimmed());
            }
            if let Some(rest) = role_rest {
                println!("{}{}", " ".repeat(role_offset), rest.dimmed());
            }
            // A second dimmed line carries the worktree so the wide path
            // doesn't blow out the table's column alignment.
            println!("{}{}", " ".repeat(11), l.worktree_path.display());
            // BUG-763: an adopted persistent lease — the backing pid started
            // later than the lease record — names both ages explicitly, so
            // lease-age vs process-age never mix silently in one row.
            if let Some(note) = ps_adopted_note(l.started_at, row.pid_started_at, now) {
                println!("{}{}", " ".repeat(11), note.dimmed());
            }
            // BUG-1941: a no-process row whose dispatch reads `moving` says so,
            // so this view and the agent view agree. trace:BUG-1941 | ai:claude
            if let Some(note) =
                ps_unattributed_movement_note(row.pid, row.dispatch.as_ref().map(|d| d.state))
            {
                println!(
                    "{}{} {}: {}",
                    " ".repeat(11),
                    crate::glyph(crate::glyphs::Glyph::Info),
                    "moving".cyan(),
                    note.dimmed()
                );
            }
            // TASK-1090: a third line names the dispatch-health hint —
            // nothing printed for Moving (it's fine, per the acceptance).
            if let Some(d) = &row.dispatch {
                if let Some(hint) = &d.hint {
                    let (glyph, colored_label) = match d.state {
                        dispatch_health_ps::DispatchState::Salvageable => (
                            crate::glyph(crate::glyphs::Glyph::Warning),
                            d.state.label().red().bold(),
                        ),
                        dispatch_health_ps::DispatchState::Stalled => (
                            crate::glyph(crate::glyphs::Glyph::Warning),
                            d.state.label().yellow().bold(),
                        ),
                        // BUG-752: undeterminable liveness — informational,
                        // not an alarm (the agent may well be working).
                        // trace:BUG-752 | ai:claude
                        dispatch_health_ps::DispatchState::Unknown => (
                            crate::glyph(crate::glyphs::Glyph::Neutral),
                            d.state.label().dimmed(),
                        ),
                        // BUG-778: the operator entered this worktree by hand
                        // and hasn't launched an agent in it yet — an
                        // informational nudge (start one), never an alarm.
                        // trace:BUG-778 | ai:claude
                        dispatch_health_ps::DispatchState::AwaitingAgent => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        // TASK-1518: the wave was stopped and the lease marked
                        // on the way out — informational (resume as normal),
                        // not the dead-agent alarm.
                        // trace:TASK-1518 | ai:claude
                        dispatch_health_ps::DispatchState::Stopped => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        // BUG-1681: finished work waiting on the integrator —
                        // informational, and deliberately actionless.
                        // trace:BUG-1681 | ai:claude
                        dispatch_health_ps::DispatchState::AwaitingIntegration => (
                            crate::glyph(crate::glyphs::Glyph::Info),
                            d.state.label().cyan(),
                        ),
                        dispatch_health_ps::DispatchState::Moving => unreachable!(
                            "hint is None for Moving — see dispatch_health_ps::next_command_hint"
                        ),
                    };
                    println!(
                        "{}{} {}: {}",
                        " ".repeat(11),
                        glyph,
                        colored_label,
                        hint.dimmed()
                    );
                }
            }
            // BUG-1553 (2026-09-23 PROXY DECISION): a neutral informational
            // note for a long-outstanding tool call — a long Bash or Task
            // call is working, not blocked, so this is dim/neutral, never
            // red and never framed as "waiting on your approval". A
            // job-control-stopped process gets its own neutral-to-warning
            // "suspended" note. `Unknown` gets its own honest, quieter note
            // (say so rather than guess); silent for `Working` (nothing to
            // flag) and `None` (no live pid to probe at all).
            match &row.activity {
                Some(SeatActivity::LongToolCall { tool, secs }) => {
                    let what = tool.as_deref().unwrap_or("a tool call");
                    println!(
                        "{}{} {}: {what} ({})",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "long tool call".dimmed(),
                        humanize_duration_secs(*secs as u64),
                    );
                }
                Some(SeatActivity::Suspended) => {
                    println!(
                        "{}{} {}",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Warning),
                        "suspended (stopped process)".yellow(),
                    );
                }
                Some(SeatActivity::Unknown) => {
                    println!(
                        "{}{} {}: could not read this session's transcript — activity unknown",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "activity".dimmed()
                    );
                }
                // TASK-1454: ground truth from the Notification hook — loud
                // and named, since a human is the only one who can unblock
                // this seat right now (BUG-1553's whole motivation).
                Some(SeatActivity::Blocked { tool, secs }) => {
                    let what = tool.as_deref().unwrap_or("a tool call");
                    println!(
                        "{}{} {}: {what} — waiting {}",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Blocked),
                        "blocked on your approval".red().bold(),
                        humanize_duration_secs(*secs as u64),
                    );
                }
                Some(SeatActivity::Working) | None => {}
            }
            // TASK-1451: flag a live seat whose mail identity would fall
            // back to the shell user — visible BEFORE it sends unattributable
            // mail, not discovered after the fact in the envelope. Silent
            // for `Attributed` (nothing to flag) and for `None` (no live pid
            // to probe).
            match row.mail_identity {
                Some(MailIdentityStatus::Unattributed) => {
                    println!(
                        "{}{} {}: no AIDA_AGENT_NAME / AIDA_USER / AIDA_SESSION_ROLE in this process's environment — mail would go out unattributed",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Warning),
                        "mail identity".yellow().bold()
                    );
                }
                Some(MailIdentityStatus::Unknown) => {
                    println!(
                        "{}{} {}: could not read this process's environment — identity unknown",
                        " ".repeat(11),
                        crate::glyph(crate::glyphs::Glyph::Neutral),
                        "mail identity".dimmed()
                    );
                }
                Some(MailIdentityStatus::Attributed) | None => {}
            }
        }
        if !hidden_stale.is_empty() {
            println!();
            println!(
                "{}",
                format!(
                    "({} stale session{} hidden — pass --all to show)",
                    hidden_stale.len(),
                    if hidden_stale.len() == 1 { "" } else { "s" }
                )
                .dimmed()
            );
        }
    }

    // TASK-1090: dead process + uncommitted work is the single highest-value
    // signal this report adds — never let it hide silently behind the
    // stale-session footer just because `--all` wasn't passed. Always shown
    // (naturally empty once `--all` folds `hidden_stale` into the main
    // table, where its hint line already printed above).
    let salvageable_hidden: Vec<&PsRow> = hidden_stale
        .iter()
        .filter(|r| {
            r.dispatch
                .as_ref()
                .is_some_and(|d| d.state == dispatch_health_ps::DispatchState::Salvageable)
        })
        .copied()
        .collect();
    if !salvageable_hidden.is_empty() {
        let warn = crate::glyph(crate::glyphs::Glyph::Warning);
        println!();
        // BUG-1680: collapse duplicate rows for the same worktree path with a count.
        // trace:BUG-1680 | ai:antigravity
        let collapsed = collapse_salvageable_by_worktree(&salvageable_hidden);
        let work_desc = if collapsed.iter().all(|c| c.untracked_only) {
            "untracked files only"
        } else {
            "uncommitted work"
        };
        println!(
            "{}",
            format!(
                "Salvageable (dead process, {work_desc} — hidden behind the stale-session count above)"
            )
            .bold()
            .red()
        );
        for item in &collapsed {
            let row = item.row;
            let header = item.display_spec_with_count();
            println!("  {} {}", warn.red(), header.red().bold());
            println!(
                "      {}",
                row.lease.worktree_path.display().to_string().dimmed()
            );
            if let Some(hint) = row.dispatch.as_ref().and_then(|d| d.hint.as_deref()) {
                println!("      {}", hint.dimmed());
            }
        }
    }

    // TASK-1064: split the no-live-spec-lease specs into two honestly-framed
    // groups. A flag-only spec while a fan-out is live is most likely being
    // BUILT by that fan-out (which takes a generic non-spec-linked harness
    // lease) — informational, not alarming. Only the rest — crashed (stale)
    // spec-scoped leases, or flag-only with NO live worker — are genuine
    // orphans. trace:TASK-1064 | ai:claude
    // trace:STORY-1653 | ai:claude — a spec a live relay/drain drives is
    // being worked: it prints under its own "Driven" header, never as orphaned.
    driver_view::print_driven_orphans(&orphans);
    let (fanout_worked, genuine): (Vec<&PsOrphan>, Vec<&PsOrphan>) = orphans
        .iter()
        .filter(|o| !driver_view::orphan_is_driven(o))
        .partition(|o| o.likely_fanout);

    // Likely worked by a fan-out — informational (a live advisor Agent-tool
    // subagent is plausibly building these; its lease just isn't spec-linked).
    if !fanout_worked.is_empty() {
        let info = crate::glyph(crate::glyphs::Glyph::Info);
        println!();
        println!(
            "{}",
            "Likely worked by a fan-out (live advisor Agent-tool subagent — generic harness lease, spec not linked)"
                .bold()
                .cyan()
        );
        for o in &fanout_worked {
            println!(
                "  {} {}  {}",
                info.cyan(),
                o.spec.cyan().bold(),
                truncate(&o.title, 48).dimmed(),
            );
        }
    }

    // Orphaned In-Progress specs — the genuine flag-only / crashed-session case.
    if !genuine.is_empty() {
        let warn = crate::glyph(crate::glyphs::Glyph::Warning);
        println!();
        println!(
            "{}",
            "Orphaned In-Progress specs (no live session backing the flag)"
                .bold()
                .yellow()
        );
        for o in &genuine {
            // BUG-1553 (acceptance #3): a stale lease is unambiguous (the
            // process is confirmed gone). A pure flag-only spec has no
            // lease/worktree to probe a process against at all — whether
            // the (possibly still-working, possibly gone) seat behind it is
            // blocked or exited is genuinely undeterminable from here, so
            // say that plainly instead of a bare "flag-only" that reads as
            // "nothing has started". trace:BUG-1553 | ai:claude
            let why = if o.possibly_subagent {
                // trace:BUG-1656 | ai:claude
                "stale lease — recorded pid dead, but a live harness lease is in this repo: \
                 possibly worked by a subagent; verify before any cleanup"
            } else if o.stale_lease {
                "stale lease — process dead"
            } else {
                "flag-only — cannot determine whether this seat is blocked or exited"
            };
            println!(
                "  {} {}  {}  {}",
                warn.yellow(),
                o.spec.yellow().bold(),
                truncate(&o.title, 48).dimmed(),
                // trace:STORY-1653 | ai:claude — `· queued (batch:X)` / `· idle 26h`
                format!("({why}{})", driver_view::orphan_driver_suffix(o)).dimmed(),
            );
        }
        println!(
            "{}",
            "  (advisor Agent-tool fan-outs take generic harness-worktree leases — \
             not spec-linked — so their specs read flag-only)"
                .dimmed()
        );
    }

    Ok(())
}
