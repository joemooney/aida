//! Relay stall guard: the `relay-stall` doctor category.
//!
//! STORY-1699 (quick-win slice): on 2026-10-15 a relay run went
//! `state=blocked` (exit 3, waiting on advisor signoff) and nobody noticed
//! for 4.5 hours. Every relay run writes `.aida/relay/<run>/relay.status` as
//! `key=value` lines; this scan reports each run whose state is `blocked` or
//! `exited` and that has not been acknowledged with a `.done` marker. The
//! marker is either `<handoff>.done` beside the handoff file the status
//! names, or `handoff.md.done` in the run directory.
//!
//! Zero-token, store-free and silent when no run is stuck, so the scheduled
//! `relay-stall-guard` job (`--fail-on-findings`) only routes to the advisor
//! when there is something to act on.
// trace:STORY-1699 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::DoctorFinding;

pub(crate) const CATEGORY: &str = "relay-stall";

/// States that mean the relay stopped and is waiting on someone.
const STALLED_STATES: &[&str] = &["blocked", "exited"];

/// Parse a `relay.status` body (`key=value` per line; blank and malformed
/// lines are ignored).
fn parse_status(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

/// Has this stalled run been acknowledged with a `.done` marker?
fn is_acknowledged(run_dir: &Path, status: &BTreeMap<String, String>) -> bool {
    if run_dir.join("handoff.md.done").exists() {
        return true;
    }
    status
        .get("handoff")
        .filter(|h| !h.is_empty())
        .map(|h| {
            let handoff = PathBuf::from(h);
            let handoff = if handoff.is_absolute() {
                handoff
            } else {
                run_dir.join(handoff)
            };
            let mut marker = handoff.into_os_string();
            marker.push(".done");
            PathBuf::from(marker).exists()
        })
        .unwrap_or(false)
}

/// Scan `<project_root>/.aida/relay/*/relay.status` for stalled runs.
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
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
        if !STALLED_STATES.contains(&state) || is_acknowledged(&run_dir, &status) {
            continue;
        }
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
                 resume the relay) or retire it; once handled, acknowledge it with \
                 `touch {handoff}.done` so the guard stops reporting it"
            ),
            safe_heal: false,
        });
    }
    findings
}

#[cfg(test)]
#[path = "tests/story_1699_relay_stall_tests.rs"]
mod tests;
