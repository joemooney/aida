//! Durable "morning-after" drain outcome — `.aida/last-drain.json` (STORY-730).
//!
//! # The gap
//!
//! The single highest-trust moment with an autonomous drain is "I walked away —
//! what happened?". But `aida status` (the natural first command the next
//! morning) said nothing about an overnight drain: the outcome tally only ever
//! reached the EPHEMERAL [`crate::drain_summary::DrainSummary::render`] eprint to
//! stderr, which scrolls away the instant the terminal closes. The live
//! [`crate::drain_state`] file is REMOVED on a clean exit (its presence means a
//! drain that is live or crashed — never a finished one), so it cannot carry the
//! finished-drain summary either.
//!
//! # The last-drain file
//!
//! When a drain ends, [`finalize_drain_summary`](crate) persists the compact
//! outcome tally — shipped / shelved / skipped + findings-to-triage + a finish
//! timestamp — to `.aida/last-drain.json` (a SIBLING of the live drain-state
//! file, so the "presence ⇒ live-or-crashed" invariant of `drain-state.json`
//! stays intact). The bare `aida status` then LEADS with a "since you were away"
//! banner when that persisted outcome is recent and un-acknowledged. Running the
//! pointed-to `aida human` aggregator [`acknowledge`]s it, so the banner stops
//! nagging once the operator has actually looked.
//!
//! Everything that decides whether/how to render is PURE ([`LastDrainOutcome::
//! should_show`] / [`LastDrainOutcome::banner`] / [`format_age`]); the I/O
//! ([`LastDrainOutcome::read`] / [`write`](LastDrainOutcome::write) /
//! [`acknowledge`]) is thin, so the banner is unit-testable from a fixture with
//! no drain.
//!
//! trace:STORY-730 | ai:claude

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name under `.aida/` holding the LAST FINISHED drain's outcome. Gitignored
/// by the deny-by-default `.aida/*` rule — pure per-clone runtime state.
const LAST_DRAIN_FILE: &str = "last-drain.json";

/// How recent a finished drain must be for the morning-after banner to surface.
/// Past this the banner is suppressed (an old outcome is no longer "since you
/// were away"). 24h covers the overnight-drain → next-morning case with margin.
pub(crate) const RECENT_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Allow-listed process provenance captured while the drain process still
/// exists. Deliberately contains no argv, cwd, or environment data.
// trace:BUG-1548 | ai:codex
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DrainInvocation {
    pub(crate) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) process_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_started_at: Option<String>,
    pub(crate) source: String,
}

impl DrainInvocation {
    pub(crate) fn capture() -> Self {
        let pid = std::process::id();
        Self::capture_with(
            pid,
            crate::process_probe::walk_ancestor_pids,
            crate::process_probe::process_start_identity,
        )
    }

    fn capture_with(
        pid: u32,
        ancestors: impl FnOnce(u32) -> Vec<u32>,
        mut start_identity: impl FnMut(u32) -> Option<String>,
    ) -> Self {
        // Snapshot the ancestry once, at drain invocation. A short-lived
        // wrapper may exit or be reparented long before finalization.
        let parent_pid = ancestors(pid).get(1).copied();
        Self {
            pid,
            process_started_at: start_identity(pid),
            parent_pid,
            parent_started_at: parent_pid.and_then(&mut start_identity),
            source: "foreground-process".to_string(),
        }
    }
}

/// Path of the last-drain file under `project_root`.
pub(crate) fn last_drain_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join(LAST_DRAIN_FILE)
}

/// The compact, persisted outcome of the LAST finished drain — the data the
/// morning-after banner needs, lifted out of the ephemeral
/// [`crate::drain_summary::DrainSummary`] before it dies on stderr.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LastDrainOutcome {
    /// Specs that shipped (merged) during the drain.
    pub(crate) shipped: usize,
    /// Specs the drain shelved (`NeedsAttention`) on a phase failure.
    pub(crate) shelved: usize,
    /// Dependents skipped because their blocker shelved.
    pub(crate) skipped: usize,
    /// Specs that parked needing a human — the `aida findings list` count.
    pub(crate) findings_to_triage: usize,
    /// RFC-3339 timestamp the drain finished.
    pub(crate) finished_at: String,
    /// Set once the operator has looked (ran `aida human`). Suppresses the
    /// banner so it does not nag forever. Defaults to `false` for a file written
    /// by an older binary that did not know the field.
    #[serde(default)]
    pub(crate) acknowledged: bool,
    /// Consecutive completed drains which shipped/shelved/skipped nothing.
    #[serde(default)]
    pub(crate) consecutive_idle_runs: usize,
    /// Process identity captured at invocation. Optional for legacy files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) invocation: Option<DrainInvocation>,
}

impl LastDrainOutcome {
    /// Lift the compact outcome out of a finished-drain
    /// [`crate::drain_summary::DrainSummary`], stamped with `finished_at`.
    pub(crate) fn from_summary_with_previous(
        summary: &crate::drain_summary::DrainSummary,
        finished_at: &str,
        previous: Option<&Self>,
        invocation: Option<DrainInvocation>,
    ) -> Self {
        let idle = summary.tallies.shipped == 0
            && summary.tallies.shelved == 0
            && summary.tallies.skipped == 0
            && summary.tallies.punted == 0
            && summary.tallies.escalated == 0;
        let consecutive_idle_runs = if idle {
            previous
                .map(|p| p.consecutive_idle_runs)
                .unwrap_or(0)
                .saturating_add(1)
        } else {
            0
        };
        Self {
            shipped: summary.tallies.shipped,
            shelved: summary.tallies.shelved,
            skipped: summary.tallies.skipped,
            findings_to_triage: summary.tallies.findings_to_triage(),
            finished_at: finished_at.to_string(),
            acknowledged: false,
            consecutive_idle_runs,
            invocation,
        }
    }

    /// Serialize to JSON.
    fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Write to `.aida/last-drain.json` atomically (TASK-331), creating `.aida/`
    /// if absent. Best-effort at the call site — a write failure must not fail
    /// the drain, it just leaves the morning-after banner unavailable.
    pub(crate) fn write(&self, project_root: &Path) -> std::io::Result<()> {
        let path = last_drain_path(project_root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        aida_core::write_atomic(&path, self.to_json())
    }

    /// Read + parse the last-drain file, or `None` when it is absent or
    /// unparseable (a torn write fails safe to "no banner").
    pub(crate) fn read(project_root: &Path) -> Option<Self> {
        let body = std::fs::read_to_string(last_drain_path(project_root)).ok()?;
        serde_json::from_str(&body).ok()
    }

    /// A drain that touched nothing — no shipped / shelved / skipped specs. An
    /// empty outcome never surfaces a banner (there was nothing to report).
    pub(crate) fn is_empty(&self) -> bool {
        self.shipped == 0 && self.shelved == 0 && self.skipped == 0
    }

    /// Age in whole seconds at `now`, or `None` when `finished_at` does not parse
    /// (a corrupt timestamp fails safe to "not recent" → suppressed).
    pub(crate) fn age_secs(&self, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
        let finished = chrono::DateTime::parse_from_rfc3339(&self.finished_at).ok()?;
        Some((now - finished.with_timezone(&chrono::Utc)).num_seconds())
    }

    /// Whether the morning-after banner should surface: a NON-EMPTY, RECENT,
    /// UN-ACKNOWLEDGED outcome. A future-dated `finished_at` (clock skew) still
    /// counts as recent — `age >= 0` is not required, only `age < window`.
    pub(crate) fn should_show(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        if self.acknowledged || self.is_empty() {
            return false;
        }
        match self.age_secs(now) {
            Some(secs) => secs < RECENT_WINDOW_SECS,
            None => false,
        }
    }

    /// Drain-status visibility intentionally includes idle outcomes. Unlike the
    /// morning-after status banner, an idle run is the diagnostic signal.
    pub(crate) fn should_show_in_drain_status(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        !self.acknowledged
            && self
                .age_secs(now)
                .map(|secs| secs < RECENT_WINDOW_SECS)
                .unwrap_or(false)
    }

    pub(crate) fn status_json(&self, now: chrono::DateTime<chrono::Utc>) -> serde_json::Value {
        let invocation = self.invocation.as_ref().map(|invocation| {
            serde_json::json!({
                "pid": invocation.pid,
                "process_started_at": invocation.process_started_at,
                "process_alive": crate::process_probe::process_identity_is_alive(
                    invocation.pid,
                    invocation.process_started_at.as_deref(),
                ),
                "parent_pid": invocation.parent_pid,
                "parent_started_at": invocation.parent_started_at,
                "parent_alive": invocation.parent_pid.map(|pid| {
                    crate::process_probe::process_identity_is_alive(
                        pid,
                        invocation.parent_started_at.as_deref(),
                    )
                }),
                "source": invocation.source,
            })
        });
        serde_json::json!({
            "finished_at": self.finished_at,
            "age_secs": self.age_secs(now),
            "acknowledged": self.acknowledged,
            "shipped": self.shipped,
            "shelved": self.shelved,
            "skipped": self.skipped,
            "findings_to_triage": self.findings_to_triage,
            "consecutive_idle_runs": self.consecutive_idle_runs,
            "invocation": invocation,
        })
    }

    /// The "since you were away" banner line, e.g.
    /// `<pause> Last drain (3h ago): 5 shipped · 3 shelved · 1 skipped — 3 need you → aida human`
    /// (the leading `<pause>` is the resolved Pause glyph the caller passes in).
    /// `pause` is the (already profile-resolved) marker glyph so this stays pure
    /// and testable. Returns `None` when [`should_show`](Self::should_show) is
    /// false, so the caller prints nothing rather than an empty line.
    pub(crate) fn banner(&self, now: chrono::DateTime<chrono::Utc>, pause: &str) -> Option<String> {
        if !self.should_show(now) {
            return None;
        }
        let age = self
            .age_secs(now)
            .map(format_age)
            .unwrap_or_else(|| "recently".to_string());
        let mut line = format!(
            "{pause} Last drain ({age}): {} shipped · {} shelved · {} skipped",
            self.shipped, self.shelved, self.skipped
        );
        let n = self.findings_to_triage;
        if n > 0 {
            let verb = if n == 1 { "needs" } else { "need" };
            line.push_str(&format!(" — {n} {verb} you → aida human"));
        } else {
            line.push_str(" — all clear");
        }
        Some(line)
    }

    /// The COMPACT agent/TOON form of the same outcome — no glyph, no pointer,
    /// just the figures: `5 shipped · 3 shelved · 1 skipped · 3 need-you · 3h ago`.
    /// Returns `None` under the same suppression rules as [`banner`](Self::banner).
    pub(crate) fn compact(&self, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
        if !self.should_show(now) {
            return None;
        }
        let age = self
            .age_secs(now)
            .map(format_age)
            .unwrap_or_else(|| "recently".to_string());
        Some(format!(
            "{} shipped · {} shelved · {} skipped · {} need-you · {}",
            self.shipped, self.shelved, self.skipped, self.findings_to_triage, age
        ))
    }
}

/// Mark the last-drain outcome acknowledged so the banner stops nagging. Called
/// when the operator runs `aida human` (the aggregator the banner points at).
/// Best-effort — a missing/unreadable file is a silent no-op.
// trace:STORY-730 | ai:claude
pub(crate) fn acknowledge(project_root: &Path) {
    let Some(mut outcome) = LastDrainOutcome::read(project_root) else {
        return;
    };
    if outcome.acknowledged {
        return;
    }
    outcome.acknowledged = true;
    let _ = outcome.write(project_root);
}

/// Render a whole-seconds age as a compact human string: `just now`, `45m ago`,
/// `3h ago`, `2d ago`. Pure, ASCII-only. Negative ages (clock skew / future
/// stamp) read `just now`.
pub(crate) fn format_age(secs: i64) -> String {
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-730 | ai:claude
    /// A non-glyph stand-in for the resolved Pause marker the caller passes to
    /// [`LastDrainOutcome::banner`]. Kept ASCII so this test module carries no
    /// raw registry-glyph literal — the real Pause glyph is resolved via the
    /// glyph registry at the call site, never hard-coded here.
    const PAUSE: &str = "P|";

    /// A fixture finished `age_secs` ago, with the given tallies.
    fn outcome(
        shipped: usize,
        shelved: usize,
        skipped: usize,
        findings: usize,
        age_secs: i64,
    ) -> (LastDrainOutcome, chrono::DateTime<chrono::Utc>) {
        let now = chrono::Utc::now();
        let finished = now - chrono::Duration::seconds(age_secs);
        let o = LastDrainOutcome {
            shipped,
            shelved,
            skipped,
            findings_to_triage: findings,
            finished_at: finished.to_rfc3339(),
            acknowledged: false,
            consecutive_idle_runs: 0,
            invocation: None,
        };
        (o, now)
    }

    // The pure banner renders the STORY-730 example shape from a fixture.
    #[test]
    fn banner_renders_counts_age_and_human_pointer() {
        let (o, now) = outcome(5, 3, 1, 3, 3 * 60 * 60);
        let line = o
            .banner(now, PAUSE)
            .expect("a recent non-empty outcome shows");
        assert!(
            line.starts_with(&format!("{PAUSE} Last drain (3h ago):")),
            "got: {line}"
        );
        assert!(
            line.contains("5 shipped · 3 shelved · 1 skipped"),
            "got: {line}"
        );
        assert!(line.contains("3 need you → aida human"), "got: {line}");
    }

    // Singular findings → "needs you" (1), plural → "need you".
    #[test]
    fn banner_pluralizes_needs_you() {
        let (o, now) = outcome(1, 1, 0, 1, 60);
        let line = o.banner(now, PAUSE).unwrap();
        assert!(line.contains("1 needs you → aida human"), "got: {line}");
    }

    // Findings == 0 → an "all clear" tail, not a dangling pointer.
    #[test]
    fn banner_all_clear_when_no_findings() {
        let (o, now) = outcome(4, 0, 0, 0, 90 * 60);
        let line = o.banner(now, PAUSE).unwrap();
        assert!(line.contains("4 shipped"), "got: {line}");
        assert!(line.ends_with("— all clear"), "got: {line}");
        assert!(!line.contains("aida human"), "got: {line}");
    }

    // Suppressed when the outcome touched nothing (empty).
    #[test]
    fn suppressed_when_empty() {
        let (o, now) = outcome(0, 0, 0, 0, 60);
        assert!(!o.should_show(now));
        assert!(o.banner(now, PAUSE).is_none());
    }

    // Suppressed when older than the recent window (an old outcome is no longer
    // "since you were away").
    #[test]
    fn suppressed_when_old() {
        let (o, now) = outcome(5, 1, 0, 1, RECENT_WINDOW_SECS + 60);
        assert!(!o.should_show(now));
        assert!(o.banner(now, PAUSE).is_none());
        // Right at the boundary (just under the window) it still shows.
        let (fresh, now2) = outcome(5, 1, 0, 1, RECENT_WINDOW_SECS - 60);
        assert!(fresh.should_show(now2));
    }

    // Suppressed once acknowledged, even when recent + non-empty.
    #[test]
    fn suppressed_when_acknowledged() {
        let (mut o, now) = outcome(5, 3, 1, 3, 60);
        assert!(o.should_show(now));
        o.acknowledged = true;
        assert!(!o.should_show(now));
        assert!(o.banner(now, PAUSE).is_none());
    }

    // A corrupt timestamp fails safe to suppressed.
    #[test]
    fn suppressed_when_timestamp_unparseable() {
        let now = chrono::Utc::now();
        let o = LastDrainOutcome {
            shipped: 5,
            shelved: 0,
            skipped: 0,
            findings_to_triage: 0,
            finished_at: "not-a-timestamp".to_string(),
            acknowledged: false,
            consecutive_idle_runs: 0,
            invocation: None,
        };
        assert!(!o.should_show(now));
    }

    // The compact agent form mirrors the human banner's figures, no glyph.
    #[test]
    fn compact_mirrors_figures_without_glyph() {
        let (o, now) = outcome(5, 3, 1, 3, 3 * 60 * 60);
        let c = o.compact(now).unwrap();
        assert_eq!(c, "5 shipped · 3 shelved · 1 skipped · 3 need-you · 3h ago");
        // Same suppression as the banner.
        let (empty, now2) = outcome(0, 0, 0, 0, 60);
        assert!(empty.compact(now2).is_none());
    }

    #[test]
    fn format_age_buckets() {
        assert_eq!(format_age(0), "just now");
        assert_eq!(format_age(59), "just now");
        assert_eq!(format_age(60), "1m ago");
        assert_eq!(format_age(45 * 60), "45m ago");
        assert_eq!(format_age(3 * 60 * 60), "3h ago");
        assert_eq!(format_age(2 * 24 * 60 * 60), "2d ago");
        // Negative (clock skew) reads "just now".
        assert_eq!(format_age(-10), "just now");
    }

    // trace:BUG-1548 | ai:codex
    #[test]
    fn drain_status_surfaces_recent_idle_legacy_record() {
        let now = chrono::Utc::now();
        let body = format!(
            r#"{{"shipped":0,"shelved":0,"skipped":0,"findings_to_triage":0,"finished_at":"{}","acknowledged":false}}"#,
            (now - chrono::Duration::minutes(3)).to_rfc3339()
        );
        let legacy: LastDrainOutcome = serde_json::from_str(&body).unwrap();
        assert!(legacy.should_show_in_drain_status(now));
        assert_eq!(legacy.consecutive_idle_runs, 0);
        assert!(legacy.invocation.is_none());
        assert!(
            !legacy.should_show(now),
            "the general status banner stays quiet"
        );
    }

    // trace:BUG-1548 | ai:codex
    #[test]
    fn idle_streak_increments_and_material_run_resets_it() {
        fn summary(
            shipped: usize,
            punted: usize,
            escalated: usize,
        ) -> crate::drain_summary::DrainSummary {
            crate::drain_summary::DrainSummary {
                kind: "batch".into(),
                label: "test".into(),
                outcome: "drained".into(),
                tallies: crate::drain_summary::DrainTallies {
                    shipped,
                    punted,
                    escalated,
                    ..Default::default()
                },
                cumulative_tokens: Some(0),
                diff: Default::default(),
                elapsed_secs: 1,
                events: Default::default(),
            }
        }
        let first = LastDrainOutcome::from_summary_with_previous(
            &summary(0, 0, 0),
            "2026-09-22T00:00:00Z",
            None,
            None,
        );
        let second = LastDrainOutcome::from_summary_with_previous(
            &summary(0, 0, 0),
            "2026-09-22T00:04:00Z",
            Some(&first),
            None,
        );
        let productive = LastDrainOutcome::from_summary_with_previous(
            &summary(1, 0, 0),
            "2026-09-22T00:08:00Z",
            Some(&second),
            None,
        );
        assert_eq!(first.consecutive_idle_runs, 1);
        assert_eq!(second.consecutive_idle_runs, 2);
        assert_eq!(productive.consecutive_idle_runs, 0);

        for material in [summary(0, 1, 0), summary(0, 0, 1)] {
            let outcome = LastDrainOutcome::from_summary_with_previous(
                &material,
                "2026-09-22T00:12:00Z",
                Some(&second),
                None,
            );
            assert_eq!(
                outcome.consecutive_idle_runs, 0,
                "punt/escalation is material work and must reset the idle streak"
            );
        }
    }

    // The launcher can disappear or be reparented while a drain runs. Capture
    // is one start-time snapshot; finalization persists that exact value and
    // never asks the process table again. trace:BUG-1548 | ai:codex
    #[test]
    fn invocation_snapshot_survives_short_lived_reparented_launcher_without_recapture() {
        use std::cell::Cell;

        let ancestry_reads = Cell::new(0);
        let identity_reads = Cell::new(0);
        let snapshot = DrainInvocation::capture_with(
            300,
            |pid| {
                ancestry_reads.set(ancestry_reads.get() + 1);
                assert_eq!(pid, 300);
                vec![300, 200, 1]
            },
            |pid| {
                identity_reads.set(identity_reads.get() + 1);
                Some(format!("start-{pid}"))
            },
        );
        assert_eq!(ancestry_reads.get(), 1);
        assert_eq!(identity_reads.get(), 2);

        // Simulate the completion-time world: pid 300 exited and its former
        // parent 200 was reparented/exited. There is intentionally no callback
        // for finalization to invoke; it can only persist `snapshot`.
        let previous_reads = (ancestry_reads.get(), identity_reads.get());
        let completed = LastDrainOutcome::from_summary_with_previous(
            &crate::drain_summary::DrainSummary {
                kind: "batch".into(),
                label: "test".into(),
                outcome: "drained".into(),
                tallies: Default::default(),
                cumulative_tokens: Some(0),
                diff: Default::default(),
                elapsed_secs: 240,
                events: Default::default(),
            },
            "2026-09-22T00:04:00Z",
            None,
            Some(snapshot.clone()),
        );
        assert_eq!(
            (ancestry_reads.get(), identity_reads.get()),
            previous_reads,
            "completion must not recapture process ancestry or identities"
        );
        assert_eq!(completed.invocation, Some(snapshot));
        let saved = completed.invocation.unwrap();
        assert_eq!(saved.pid, 300);
        assert_eq!(saved.parent_pid, Some(200));
        assert_eq!(saved.process_started_at.as_deref(), Some("start-300"));
        assert_eq!(saved.parent_started_at.as_deref(), Some("start-200"));
    }

    // trace:BUG-1548 | ai:codex
    #[test]
    fn drain_status_recency_boundary_is_strict_and_acknowledged_is_hidden() {
        let (mut recent, now) = outcome(0, 0, 0, 0, RECENT_WINDOW_SECS - 1);
        assert!(recent.should_show_in_drain_status(now));
        let (expired, now) = outcome(0, 0, 0, 0, RECENT_WINDOW_SECS);
        assert!(!expired.should_show_in_drain_status(now));
        recent.acknowledged = true;
        assert!(!recent.should_show_in_drain_status(now));
    }

    // write → read round-trips; acknowledge flips the flag durably.
    #[test]
    fn write_read_round_trips_and_acknowledge_persists() {
        let dir = tempfile::tempdir().unwrap();
        let (o, _now) = outcome(5, 3, 1, 3, 60);
        o.write(dir.path()).unwrap();
        let read = LastDrainOutcome::read(dir.path()).unwrap();
        assert_eq!(read, o);
        assert!(!read.acknowledged);
        acknowledge(dir.path());
        let after = LastDrainOutcome::read(dir.path()).unwrap();
        assert!(after.acknowledged);
        // acknowledge on an absent file is a silent no-op.
        let empty = tempfile::tempdir().unwrap();
        acknowledge(empty.path());
        assert!(LastDrainOutcome::read(empty.path()).is_none());
    }

    // from_summary lifts the tallies + findings count off a DrainSummary.
    #[test]
    fn from_summary_lifts_tallies_and_findings() {
        let summary = crate::drain_summary::DrainSummary {
            kind: "batch".into(),
            label: "batch:foo".into(),
            outcome: "drained-with-shelved".into(),
            tallies: crate::drain_summary::DrainTallies {
                shipped: 4,
                shelved: 2,
                skipped: 1,
                punted: 1,
                escalated: 0,
            },
            cumulative_tokens: Some(0),
            diff: crate::drain_summary::DrainDiffStats::default(),
            elapsed_secs: 0,
            events: crate::events::EventTally::default(),
        };
        let o = LastDrainOutcome::from_summary_with_previous(
            &summary,
            "2026-06-30T00:00:00+00:00",
            None,
            None,
        );
        assert_eq!(o.shipped, 4);
        assert_eq!(o.shelved, 2);
        assert_eq!(o.skipped, 1);
        // findings_to_triage = shelved + punted + escalated = 3.
        assert_eq!(o.findings_to_triage, 3);
        assert_eq!(o.finished_at, "2026-06-30T00:00:00+00:00");
        assert!(!o.acknowledged);
    }
}
