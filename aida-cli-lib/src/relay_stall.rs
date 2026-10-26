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

// STORY-1700 (F1 Option A): the relay.status parsing and liveness helpers
// live in `fleet_lanes`; this scan maps its classified lanes to findings.
// trace:STORY-1700 | ai:claude
#[cfg(test)]
use std::collections::BTreeMap;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::gate::fleet_lanes::acknowledgement_marker;
#[cfg(test)]
use crate::gate::fleet_lanes::parse_status;
use crate::gate::fleet_lanes::updated_at;
use crate::DoctorFinding;
use chrono::{DateTime, Duration, Utc};
#[cfg(test)]
use fs2::FileExt;

pub(crate) const CATEGORY: &str = "relay-stall";

// trace:TASK-1792 | ai:codex
// Explicit exits include deliberate Ctrl-C stops; do not page for them forever.
const EXIT_REPORT_HOURS: i64 = 24;

/// Scan `<project_root>/.aida/relay/*/relay.status` for stalled runs.
pub(crate) fn scan(project_root: &Path) -> Vec<DoctorFinding> {
    scan_at(project_root, Utc::now())
}

fn scan_at(project_root: &Path, now: DateTime<Utc>) -> Vec<DoctorFinding> {
    let mut findings = Vec::new();
    for lane in crate::gate::fleet_lanes::classify_lanes(project_root, now) {
        let run_dir = lane.run_dir;
        let status = lane.evidence.status;
        let state = status.get("state").map(String::as_str).unwrap_or("");
        let dead_owner =
            matches!(state, "running" | "queued") && lane.evidence.owner_alive == Some(false);
        let recent_exit = state == "exited"
            && updated_at(&status)
                .map(|updated| now - updated <= Duration::hours(EXIT_REPORT_HOURS))
                .unwrap_or(false);
        if !(state == "blocked" || recent_exit || dead_owner) || lane.evidence.acknowledged {
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
