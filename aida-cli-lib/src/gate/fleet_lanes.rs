//! Fleet lanes: one classified lane per relay run.
//!
//! STORY-1700 (F1 Option A): the drive-mandate tick and the STORY-1699
//! `relay-stall` doctor scan read the same `.aida/relay/<run>/relay.status`
//! evidence. The parsing and liveness helpers moved here from
//! `relay_stall.rs` unchanged, and [`classify_lanes`] turns each run into a
//! [`Lane`] with a [`LaneClass`] the tick routes on. `relay_stall::scan_at`
//! maps the same lanes to doctor findings, so the doctor behaviour did not
//! move.
// trace:STORY-1700 | ai:claude
// trace:STORY-1699 | ai:claude
// mandate-gate

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;

pub(crate) fn updated_at(status: &BTreeMap<String, String>) -> Option<DateTime<Utc>> {
    let updated = status.get("updated")?;
    // relay.sh uses strftime %z (-0700), while other producers may use RFC3339.
    DateTime::parse_from_rfc3339(updated)
        .or_else(|_| DateTime::parse_from_str(updated, "%Y-%m-%dT%H:%M:%S%z"))
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

/// Probe the run lock without waiting, then validate its recorded process.
/// A relay can run without flock(1), so even an unlocked file needs a PID/start
/// check. Unknown I/O/identity evidence is not proof of death.
// trace:TASK-1792 | ai:codex
pub(crate) fn owner_alive(run_dir: &Path, status: &BTreeMap<String, String>) -> Option<bool> {
    let lock = match std::fs::File::open(run_dir.join("relay.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    match FileExt::try_lock_shared(&lock) {
        Ok(()) => {
            FileExt::unlock(&lock).ok()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(_) => return None,
    }
    let pid = status
        .get("pid")?
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)?;
    if !crate::process_probe::pid_is_alive(pid) {
        return Some(false);
    }
    // trace:BUG-2130 | ai:agy
    let started = aida_core::liveness::process_start_time(pid)?;
    // Current relay.status has no start-time field. Its write timestamp bounds
    // the original process start, so a later recycled PID cannot mask death.
    Some(started.with_timezone(&Utc) <= updated_at(status)?)
}

/// Is the run's `relay.lock` held right now? `None` when it cannot be probed.
/// This is the `relay.sh --status` liveness test: the relay holds the lock for
/// its whole life and the kernel drops it when the relay dies.
// trace:STORY-1700 | ai:claude
pub(crate) fn lock_held(run_dir: &Path) -> Option<bool> {
    let lock = match std::fs::File::open(run_dir.join("relay.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    match FileExt::try_lock_shared(&lock) {
        Ok(()) => {
            FileExt::unlock(&lock).ok()?;
            Some(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Some(true),
        Err(_) => None,
    }
}

/// Parse a `relay.status` body (`key=value` per line; blank and malformed
/// lines are ignored).
pub(crate) fn parse_status(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Resolve the seat handoff's acknowledgement. The run's own sentinel drives
/// rotation, so it must never be presented or accepted as an acknowledgement.
// trace:TASK-1792 | ai:codex
pub(crate) fn acknowledgement_marker(
    run_dir: &Path,
    status: &BTreeMap<String, String>,
) -> Option<PathBuf> {
    let handoff = PathBuf::from(status.get("handoff").filter(|h| !h.is_empty())?);
    let handoff = if handoff.is_absolute() {
        handoff
    } else {
        run_dir.join(handoff)
    };
    let mut marker = handoff.into_os_string();
    marker.push(".done");
    let marker = PathBuf::from(marker);
    let live_marker = run_dir.join("handoff.md.done");
    // Resolve the parent too, so an absent sentinel reached via a symlink or
    // `..` alias is not advertised as an acknowledgement that will be refused.
    let identity = |path: &Path| {
        path.canonicalize()
            .ok()
            .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))
    };
    if marker == live_marker
        || identity(&marker).is_some_and(|path| Some(path) == identity(&live_marker))
    {
        return None;
    }
    Some(marker)
}

pub(crate) fn is_acknowledged(run_dir: &Path, status: &BTreeMap<String, String>) -> bool {
    acknowledgement_marker(run_dir, status).is_some_and(|marker| marker.exists())
}

/// Which seat a relay run serves. relay.status records no role, so the seat
/// comes from the handoff path.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeatKind {
    Implementer,
    /// A handoff under `.aida/handoff/advisor*`, or no readable handoff path
    /// at all (fail closed: an unknown run routes to the operator).
    Advisor,
}

/// How a relay run stands. See the classification table in
/// `docs/plans/2026-10-15-story-1700-drive-mandate.md` (R7).
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaneClass {
    /// The run lock is held, or a running/queued owner is not proven dead.
    Running,
    /// state=running/queued with a free lock and a dead owner: the EXIT trap
    /// never ran. Also any exit code the relay does not define.
    Crashed,
    /// Exit 130 / 143: a deliberate stop.
    SignalStopped,
    /// Exited with rc 0 but not complete: the session cap was reached.
    SessionCap,
    Complete,
    /// Exit 1: a setup or usage error before any session.
    SetupError,
    /// Exit 3 / state=blocked: waiting on a human or advisor answer.
    Blocked,
    /// Exit 4 / 5: no usable usage binary.
    UsageBin,
    /// Exit 6: a duplicate launch of a live run; the live run continues.
    RunLocked,
    /// Exit 7: a terminal API error (credits).
    TerminalApi,
    /// Exit 8: no relay slot was free.
    NoSlot,
    /// The seat handoff's `.done` marker is present.
    Acknowledged,
}

impl LaneClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Crashed => "crashed",
            Self::SignalStopped => "signal-stopped",
            Self::SessionCap => "session-cap",
            Self::Complete => "complete",
            Self::SetupError => "setup-error",
            Self::Blocked => "blocked",
            Self::UsageBin => "usage-bin",
            Self::RunLocked => "run-locked",
            Self::TerminalApi => "terminal-api",
            Self::NoSlot => "no-slot",
            Self::Acknowledged => "acknowledged",
        }
    }
}

/// What the classifier saw for one run.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct LaneEvidence {
    pub status: BTreeMap<String, String>,
    pub lock_held: Option<bool>,
    pub owner_alive: Option<bool>,
    pub acknowledged: bool,
}

impl LaneEvidence {
    pub(crate) fn field(&self, key: &str) -> Option<&str> {
        self.status
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// STORY-1706: the crash ending, and only it. The status file still
    /// says running or queued, the run lock is provably free and the owner
    /// provably dead, and no exit reason the trap writes is present. An
    /// unknown exit code is `Crashed` too, but the trap ran, so it is not
    /// this.
    // trace:STORY-1706 | ai:claude
    pub(crate) fn trap_never_ran(&self) -> bool {
        matches!(self.field("state"), Some("running") | Some("queued"))
            && self.lock_held == Some(false)
            && self.owner_alive == Some(false)
            && !self.field("reason").is_some_and(is_trap_reason)
    }
}

/// An exit reason relay.sh's EXIT trap writes (STORY-1706).
// trace:STORY-1706 | ai:claude
pub(crate) fn is_trap_reason(reason: &str) -> bool {
    reason.starts_with("signal:") || reason == "session-cap" || reason == "setup-error"
}

/// One relay run, classified.
// trace:STORY-1700 | ai:claude
#[derive(Debug, Clone)]
pub(crate) struct Lane {
    pub run_dir: PathBuf,
    pub run_id: String,
    pub seat_kind: SeatKind,
    pub class: LaneClass,
    pub evidence: LaneEvidence,
}

/// The seat a run serves, from its recorded handoff path.
// trace:STORY-1700 | ai:claude
pub(crate) fn seat_kind_of(status: &BTreeMap<String, String>) -> SeatKind {
    let Some(handoff) = status.get("handoff").filter(|h| !h.trim().is_empty()) else {
        return SeatKind::Advisor;
    };
    let parts: Vec<String> = Path::new(handoff)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let advisor = parts.windows(3).any(|w| {
        w[0] == ".aida" && w[1] == "handoff" && w[2].to_ascii_lowercase().starts_with("advisor")
    });
    if advisor {
        SeatKind::Advisor
    } else {
        SeatKind::Implementer
    }
}

/// Classify one run from its evidence. Pure.
// trace:STORY-1700 | ai:claude
pub(crate) fn classify(evidence: &LaneEvidence) -> LaneClass {
    if evidence.lock_held == Some(true) {
        return LaneClass::Running;
    }
    if evidence.acknowledged {
        return LaneClass::Acknowledged;
    }
    let exit = evidence.field("exit").and_then(|e| e.parse::<i64>().ok());
    match evidence.field("state").unwrap_or("") {
        "running" | "queued" => {
            if evidence.owner_alive == Some(false) {
                LaneClass::Crashed
            } else {
                LaneClass::Running
            }
        }
        "complete" => LaneClass::Complete,
        // relay.sh writes state=blocked for exit 3 and for exit 7.
        "blocked" if exit == Some(7) => LaneClass::TerminalApi,
        "blocked" => LaneClass::Blocked,
        _ => match exit {
            Some(130) | Some(143) => LaneClass::SignalStopped,
            Some(0) => LaneClass::SessionCap,
            Some(1) => LaneClass::SetupError,
            Some(3) => LaneClass::Blocked,
            Some(4) | Some(5) => LaneClass::UsageBin,
            Some(6) => LaneClass::RunLocked,
            Some(7) => LaneClass::TerminalApi,
            Some(8) => LaneClass::NoSlot,
            _ => LaneClass::Crashed,
        },
    }
}

/// Every relay run under `<project_root>/.aida/relay/` that has a
/// `relay.status`, sorted by run directory, each classified. `now` is the
/// instant the evidence is judged at (kept for callers that age lanes).
// trace:STORY-1700 | ai:claude
pub(crate) fn classify_lanes(project_root: &Path, _now: DateTime<Utc>) -> Vec<Lane> {
    let relay_root = project_root.join(".aida").join("relay");
    let Ok(entries) = std::fs::read_dir(&relay_root) else {
        return Vec::new();
    };
    let mut run_dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    run_dirs.sort();

    run_dirs.into_iter().filter_map(probe_lane).collect()
}

/// One run directory, read and classified afresh; `None` without a
/// readable `relay.status`. The drive tick re-probes a lane with this
/// immediately before restarting it (STORY-1706).
// trace:STORY-1706 | ai:claude
pub(crate) fn probe_lane(run_dir: PathBuf) -> Option<Lane> {
    let text = std::fs::read_to_string(run_dir.join("relay.status")).ok()?;
    let status = parse_status(&text);
    let evidence = LaneEvidence {
        lock_held: lock_held(&run_dir),
        owner_alive: owner_alive(&run_dir, &status),
        acknowledged: is_acknowledged(&run_dir, &status),
        status,
    };
    let run_id = evidence
        .field("run")
        .map(str::to_string)
        .unwrap_or_else(|| {
            run_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        });
    Some(Lane {
        seat_kind: seat_kind_of(&evidence.status),
        class: classify(&evidence),
        run_id,
        run_dir,
        evidence,
    })
}

#[cfg(test)]
#[path = "tests/story_1700_lane_classify_tests.rs"]
mod story_1700_lane_classify_tests;
