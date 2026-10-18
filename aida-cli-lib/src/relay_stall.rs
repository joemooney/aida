//! Relay stall guard: the `relay-stall` doctor category.
//!
//! STORY-1699 (quick-win slice): on 2026-10-15 a relay run went
//! `state=blocked` (exit 3, waiting on advisor signoff) and nobody noticed
//! for 4.5 hours. Every relay run writes `.aida/relay/<run>/relay.status` as
//! `key=value` lines; this scan reports each run whose state is `blocked` or
//! recent `exited`, or whose running/queued owner died. Only a seat handoff
//! acknowledgement counts; the run-directory sentinel drives live rotation.
//!
//! Zero-token, store-free and silent when no run is stuck, so the scheduled
//! `relay-stall-guard` job (`--fail-on-findings`) only routes to the advisor
//! when there is something to act on.
// trace:STORY-1699 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::DoctorFinding;
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;

pub(crate) const CATEGORY: &str = "relay-stall";

// trace:TASK-1792 | ai:codex
// Explicit exits include deliberate Ctrl-C stops; do not page for them forever.
const EXIT_REPORT_HOURS: i64 = 24;

fn updated_at(status: &BTreeMap<String, String>) -> Option<DateTime<Utc>> {
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
fn owner_alive(run_dir: &Path, status: &BTreeMap<String, String>) -> Option<bool> {
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
    let identity = crate::process_probe::process_start_identity(pid)?;
    let started = DateTime::parse_from_rfc3339(&identity).ok()?;
    // Current relay.status has no start-time field. Its write timestamp bounds
    // the original process start, so a later recycled PID cannot mask death.
    Some(started.with_timezone(&Utc) <= updated_at(status)?)
}

/// Parse a `relay.status` body (`key=value` per line; blank and malformed
/// lines are ignored).
fn parse_status(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Resolve the seat handoff's acknowledgement. The run's own sentinel drives
/// rotation, so it must never be presented or accepted as an acknowledgement.
// trace:TASK-1792 | ai:codex
fn acknowledgement_marker(run_dir: &Path, status: &BTreeMap<String, String>) -> Option<PathBuf> {
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

fn is_acknowledged(run_dir: &Path, status: &BTreeMap<String, String>) -> bool {
    acknowledgement_marker(run_dir, status).is_some_and(|marker| marker.exists())
}

/// Scan `<project_root>/.aida/relay/*/relay.status` for stalled runs.
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    scan_at(project_root, Utc::now())
}

fn scan_at(project_root: &Path, now: DateTime<Utc>) -> Vec<DoctorFinding> {
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

    let mut findings = Vec::new();
    for run_dir in run_dirs {
        let Ok(text) = std::fs::read_to_string(run_dir.join("relay.status")) else {
            continue;
        };
        let status = parse_status(&text);
        let state = status.get("state").map(String::as_str).unwrap_or("");
        let dead_owner =
            matches!(state, "running" | "queued") && owner_alive(&run_dir, &status) == Some(false);
        let recent_exit = state == "exited"
            && updated_at(&status)
                .map(|updated| now - updated <= Duration::hours(EXIT_REPORT_HOURS))
                .unwrap_or(false);
        if !(state == "blocked" || recent_exit || dead_owner) || is_acknowledged(&run_dir, &status)
        {
            continue;
        }
        let state = if dead_owner {
            "exited (owner dead)"
        } else {
            state
        };
        let run = status
            .get("run")
            .filter(|r| !r.is_empty())
            .cloned()
            .unwrap_or_else(|| {
                run_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        let field = |k: &str| status.get(k).map(String::as_str).unwrap_or("?");
        let handoff = status
            .get("handoff")
            .filter(|h| !h.is_empty())
            .map(String::as_str)
            .unwrap_or("(none recorded)");
        let acknowledgement = match acknowledgement_marker(&run_dir, &status) {
            Some(marker) => format!(
                "acknowledge its seat handoff with `touch {}`",
                marker.display()
            ),
            None => "retire the run directory after handling it".to_string(),
        };
        findings.push(DoctorFinding {
            category: CATEGORY.to_string(),
            id: format!("{CATEGORY}:{run}"),
            summary: format!(
                "relay run {run} is {state} (exit {}, session {}, since {}); handoff: {handoff}",
                field("exit"),
                field("session"),
                field("updated"),
            ),
            action: format!(
                "read the handoff, then unblock the run (answer its signoff or question and \
                 resume the relay) or retire it; once handled, {acknowledgement} \
                 so the guard stops reporting it"
            ),
            safe_heal: false,
        });
    }
    findings
}

#[cfg(test)]
#[path = "tests/story_1699_relay_stall_tests.rs"]
mod tests;
