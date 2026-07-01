//! Per-spec work-liveness for the cockpit Targets list — the ambient "is
//! anything live working this row?" signal (TASK-978).
//!
//! ## The shared-logic seam
//!
//! The authoritative liveness machinery lives in `aida-cli`: the `/proc` process
//! probe (`process_probe::pid_is_alive` / `probe_live_claude_sessions`), the
//! session-lease parse, and the `SpecLiveness` / `LeaseState` classifiers that
//! back `aida ps` and `aida status <spec>`. `aida-tui` MUST NOT depend on
//! `aida-cli`, so we do NOT reimplement the probe. Instead the cockpit shells
//! out to `aida ps --json` (the SAME binary, resolved via `current_exe()`) and
//! maps each row's spec id through [`RowLiveness`]. The probe runs once in the
//! CLI; the TUI only consumes its already-computed verdict.
//!
//! FOLLOW-UP (BUG-677, not this change): the durable fix is to eliminate the
//! `aida ps` shell-out entirely by lifting the `/proc` liveness probe + lease
//! classifiers out of `aida-cli` into `aida-core` (which both `aida-cli` and
//! `aida-tui` already depend on) and calling them in-process. The exact seam:
//! `aida-cli/src/process_probe.rs` (`pid_is_alive` / `probe_live_claude_sessions`)
//! plus the session-lease read + `classify_spec_liveness` matrix behind
//! `handle_ps` move to a new `aida-core::liveness` module; `handle_ps` and this
//! module then both call it (no subprocess). That lift is bigger than this perf
//! hotfix, so it is filed separately.
//!
//! ## Keeping it cheap (BUG-676)
//!
//! `aida ps` runs a real process probe (~1.3s standalone), so firing it on the
//! TUI's render loop makes the whole machine sluggish. Three guards keep it
//! cheap, all funnelled through [`should_refresh`] (a pure, unit-testable gate):
//!
//!   1. **Long TTL.** A probe result stays fresh for [`DEFAULT_PROBE_TTL`]
//!      (20s) — liveness does not change second-to-second, so a slightly stale
//!      glyph is fine. Override with `AIDA_TUI_LIVENESS_TTL_SECS`.
//!   2. **Lazy when visible.** [`LivenessProbe::refresh_if_due`] takes a
//!      `visible` flag; it probes ONLY when the current scope actually surfaces
//!      the liveness glyph (the running-work scopes — see
//!      [`super::state::Scope::shows_liveness`]). On Backlog (approved + planned,
//!      never live-worked) it never probes.
//!   3. **Single-flight + non-blocking.** The shell-out runs on a background
//!      thread (never freezing the render loop) and a new probe is NEVER spawned
//!      while a previous one is still in flight, so the subprocesses cannot pile
//!      up. The render path only ever calls [`LivenessProbe::for_id`], a
//!      `HashMap` lookup.
//!
//! trace:TASK-978 trace:BUG-676 | ai:claude

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use super::list_row::RowLiveness;

/// Default freshness window for a probe result before `refresh_if_due` will
/// shell out again. Long enough that the `aida ps` subprocess runs rarely (a
/// session starting/dying shows up within ~20s), short enough to stay useful.
/// Was 3s (BUG-676: at 3s the ~1.3s probe consumed ~45% of the machine).
// trace:BUG-676 | ai:claude
pub const DEFAULT_PROBE_TTL: Duration = Duration::from_secs(20);

/// Env override for [`DEFAULT_PROBE_TTL`], in whole seconds. A power user can
/// tune the liveness cadence without a rebuild; an absent / non-numeric / zero
/// value falls back to the default. Documented in `docs/environment-variables.md`.
// trace:BUG-676 | ai:claude
pub const PROBE_TTL_ENV: &str = "AIDA_TUI_LIVENESS_TTL_SECS";

/// The effective probe TTL: `AIDA_TUI_LIVENESS_TTL_SECS` if set to a positive
/// integer, else [`DEFAULT_PROBE_TTL`].
// trace:BUG-676 | ai:claude
pub fn probe_ttl() -> Duration {
    parse_ttl(std::env::var(PROBE_TTL_ENV).ok().as_deref())
}

/// Pure parse of the TTL override so it is unit-testable without touching the
/// process environment: a positive integer number of seconds wins; anything
/// else (absent, blank, non-numeric, zero) falls back to [`DEFAULT_PROBE_TTL`].
// trace:BUG-676 | ai:claude
fn parse_ttl(raw: Option<&str>) -> Duration {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&secs| secs > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PROBE_TTL)
}

/// Has the TTL elapsed since the last probe? Pure so the cache gate is
/// unit-testable without a clock or a subprocess: `None` (never probed) always
/// fires; otherwise only once the TTL has elapsed.
// trace:TASK-978 | ai:claude
pub fn should_probe(last: Option<Instant>, now: Instant, ttl: Duration) -> bool {
    match last {
        None => true,
        Some(prev) => now.duration_since(prev) >= ttl,
    }
}

/// The full spawn decision for [`LivenessProbe::refresh_if_due`], factored out
/// pure so the three BUG-676 guards are unit-testable without a subprocess:
///
///   - `visible` — the liveness glyph is actually on screen for the current
///     scope. When `false` we never probe (lazy-when-visible).
///   - `inflight` — a previous probe is still running. When `true` we never
///     spawn a second (single-flight; stops the pile-up).
///   - the usual [`should_probe`] TTL check.
///
/// A probe fires only when all three agree.
// trace:BUG-676 | ai:claude
pub fn should_refresh(
    visible: bool,
    inflight: bool,
    last: Option<Instant>,
    now: Instant,
    ttl: Duration,
) -> bool {
    visible && !inflight && should_probe(last, now, ttl)
}

/// Parse `aida ps --json` output into a spec-id → [`RowLiveness`] map.
///
/// The JSON shape (see `handle_ps` in `aida-cli/src/main.rs`):
/// `{ "sessions": [{ "spec": "TASK-1", "live": true, ... }],
///    "orphaned": [{ "spec": "TASK-2", "liveness": "stale" | "flag-only" }] }`.
///
/// Mapping (faithful to the `aida ps` semantics):
///   - a session with `live == true`  → [`RowLiveness::Live`]
///   - a session with `live == false` (dormant/dead lease) → [`RowLiveness::Stale`]
///   - any orphaned entry (stale lease OR flag-only In-Progress) → [`RowLiveness::Stale`]
///   - a spec absent from both → [`RowLiveness::Idle`] (the lookup default)
///
/// `Live` wins if a spec somehow appears as both live and orphaned. Keys are
/// upper-cased so the cockpit's display ids match regardless of case. A null /
/// missing `spec` (a generic non-spec-linked harness lease) is skipped — it has
/// no row to mark. Malformed JSON yields an empty map (everything reads Idle).
// trace:TASK-978 | ai:claude
pub fn parse_ps_json(json: &str) -> HashMap<String, RowLiveness> {
    let mut map: HashMap<String, RowLiveness> = HashMap::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return map;
    };

    let upsert = |map: &mut HashMap<String, RowLiveness>, spec: &str, state: RowLiveness| {
        let key = spec.trim().to_ascii_uppercase();
        if key.is_empty() {
            return;
        }
        // Live is the strongest verdict — never let a later Stale overwrite it.
        match map.get(&key) {
            Some(RowLiveness::Live) => {}
            _ => {
                map.insert(key, state);
            }
        }
    };

    if let Some(sessions) = value.get("sessions").and_then(|s| s.as_array()) {
        for s in sessions {
            let Some(spec) = s.get("spec").and_then(|v| v.as_str()) else {
                continue;
            };
            let live = s.get("live").and_then(|v| v.as_bool()).unwrap_or(false);
            upsert(
                &mut map,
                spec,
                if live {
                    RowLiveness::Live
                } else {
                    RowLiveness::Stale
                },
            );
        }
    }

    if let Some(orphaned) = value.get("orphaned").and_then(|o| o.as_array()) {
        for o in orphaned {
            let Some(spec) = o.get("spec").and_then(|v| v.as_str()) else {
                continue;
            };
            // Every orphaned entry is, by definition, not liveness-backed.
            upsert(&mut map, spec, RowLiveness::Stale);
        }
    }

    map
}

/// Run `aida ps --json` and parse it. Returns `None` on any failure (binary
/// missing, non-zero exit, non-UTF-8) so the caller can keep the previous map
/// intact rather than blanking every glyph to Idle. Runs on a background thread.
// trace:BUG-676 | ai:claude
fn run_probe(aida_exe: &Path, project_root: &Path) -> Option<HashMap<String, RowLiveness>> {
    let output = std::process::Command::new(aida_exe)
        .arg("ps")
        .arg("--json")
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(parse_ps_json(&text))
}

/// The cockpit's cached per-spec liveness, refreshed on a poll cadence.
///
/// Holds the last `aida ps --json` verdict map, the timestamp of the last probe
/// (for the [`should_probe`] TTL gate), and the receiver of an in-flight
/// background probe (for the single-flight guard — never spawn a second probe
/// while one is running).
// trace:TASK-978 trace:BUG-676 | ai:claude
#[derive(Debug, Default)]
pub struct LivenessProbe {
    map: HashMap<String, RowLiveness>,
    last_probe: Option<Instant>,
    /// The channel end of the currently-running background probe, if any. `Some`
    /// means a probe is in flight (the single-flight guard).
    // trace:BUG-676 | ai:claude
    inflight: Option<Receiver<Option<HashMap<String, RowLiveness>>>>,
}

impl Clone for LivenessProbe {
    // An in-flight receiver can't be shared, so a clone starts with no probe in
    // flight and re-probes on its next due tick. trace:BUG-676 | ai:claude
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            last_probe: self.last_probe,
            inflight: None,
        }
    }
}

impl LivenessProbe {
    /// The liveness verdict for one display id — [`RowLiveness::Idle`] when no
    /// session/lease backs it. Case-insensitive. This is the only method the
    /// render path calls (a `HashMap` lookup, no IO).
    // trace:TASK-978 | ai:claude
    pub fn for_id(&self, id: &str) -> RowLiveness {
        self.map
            .get(&id.trim().to_ascii_uppercase())
            .copied()
            .unwrap_or(RowLiveness::Idle)
    }

    /// Replace the verdict map. Test-only seam for asserting `for_id` without
    /// shelling out to `aida ps`.
    #[cfg(test)]
    pub fn set_map(&mut self, map: HashMap<String, RowLiveness>) {
        self.map = map;
    }

    /// Force the next `refresh_if_due` (when visible) to probe immediately,
    /// regardless of the TTL. Wired to the cockpit's `r` live-refresh key so the
    /// operator can pull a fresh liveness read on demand.
    // trace:BUG-676 | ai:claude
    pub fn mark_stale(&mut self) {
        self.last_probe = None;
    }

    /// Reap a finished background probe: swap in its verdict map (keeping the
    /// previous one if the probe failed / sent `None`) and clear the in-flight
    /// slot. A no-op while the probe is still running.
    // trace:BUG-676 | ai:claude
    fn reap_inflight(&mut self) {
        if let Some(rx) = self.inflight.as_ref() {
            match rx.try_recv() {
                Ok(Some(map)) => {
                    self.map = map;
                    self.inflight = None;
                }
                // Probe ran but failed — keep the last good map, clear in-flight.
                Ok(None) => self.inflight = None,
                Err(TryRecvError::Empty) => {}
                // Worker thread died without sending — clear in-flight.
                Err(TryRecvError::Disconnected) => self.inflight = None,
            }
        }
    }

    /// Refresh the per-row liveness map, subject to the three BUG-676 guards.
    ///
    /// `visible` says whether the current scope actually shows the liveness
    /// glyph (see [`super::state::Scope::shows_liveness`]) — when `false` this is
    /// a cheap no-op. Otherwise, at most once per [`probe_ttl`] and never while a
    /// previous probe is still running, it spawns a BACKGROUND thread that runs
    /// `aida ps --json` and posts the parsed map back; the render loop is never
    /// blocked. `aida_exe` is the running binary (`current_exe()`),
    /// `project_root` the cockpit's project dir.
    // trace:TASK-978 trace:BUG-676 | ai:claude
    pub fn refresh_if_due(&mut self, aida_exe: &Path, project_root: &Path, visible: bool) {
        // Always harvest a finished probe first so a completed result shows even
        // on a frame where we don't (re)probe.
        self.reap_inflight();

        if !should_refresh(
            visible,
            self.inflight.is_some(),
            self.last_probe,
            Instant::now(),
            probe_ttl(),
        ) {
            return;
        }

        // Stamp now so the TTL runs from spawn time (a broken/slow probe never
        // busy-loops the subprocess).
        self.last_probe = Some(Instant::now());
        let (tx, rx) = std::sync::mpsc::channel();
        let exe = aida_exe.to_path_buf();
        let root = project_root.to_path_buf();
        let spawned = thread::Builder::new()
            .name("aida-tui-liveness".to_string())
            .spawn(move || {
                // If the receiver was dropped, the send just fails harmlessly.
                let _ = tx.send(run_probe(&exe, &root));
            });
        // Only mark in-flight if the thread actually started; a spawn failure
        // leaves us free to retry on the next due tick.
        self.inflight = spawned.ok().map(|_| rx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_probe_fires_when_never_probed() {
        assert!(should_probe(None, Instant::now(), DEFAULT_PROBE_TTL));
    }

    #[test]
    fn should_probe_is_false_within_ttl() {
        // A probe that just ran must NOT re-fire within the same frame / TTL —
        // this is the "don't re-probe per render" guard. trace:TASK-978
        let now = Instant::now();
        assert!(!should_probe(Some(now), now, DEFAULT_PROBE_TTL));
        // A tick well inside the TTL is still suppressed.
        assert!(!should_probe(
            Some(now),
            now + Duration::from_millis(500),
            DEFAULT_PROBE_TTL
        ));
    }

    #[test]
    fn should_probe_fires_after_ttl_elapses() {
        let start = Instant::now();
        assert!(should_probe(
            Some(start),
            start + DEFAULT_PROBE_TTL + Duration::from_millis(1),
            DEFAULT_PROBE_TTL
        ));
    }

    #[test]
    fn default_ttl_is_much_longer_than_the_old_3s() {
        // BUG-676: the regression cadence was 3s; the fix must be substantially
        // higher so the ~1.3s probe stops dominating the machine.
        assert!(DEFAULT_PROBE_TTL >= Duration::from_secs(20));
    }

    #[test]
    fn parse_ttl_honours_positive_override_and_falls_back_otherwise() {
        assert_eq!(parse_ttl(Some("30")), Duration::from_secs(30));
        assert_eq!(parse_ttl(Some(" 25 ")), Duration::from_secs(25));
        // Absent / blank / non-numeric / zero → default.
        assert_eq!(parse_ttl(None), DEFAULT_PROBE_TTL);
        assert_eq!(parse_ttl(Some("")), DEFAULT_PROBE_TTL);
        assert_eq!(parse_ttl(Some("abc")), DEFAULT_PROBE_TTL);
        assert_eq!(parse_ttl(Some("0")), DEFAULT_PROBE_TTL);
    }

    #[test]
    fn should_refresh_skips_when_panel_not_visible() {
        // Lazy-when-visible: on a scope that doesn't show the glyph, we never
        // shell out even though the TTL has long elapsed. trace:BUG-676
        let now = Instant::now();
        assert!(!should_refresh(false, false, None, now, DEFAULT_PROBE_TTL));
    }

    #[test]
    fn should_refresh_fires_when_visible_due_and_idle() {
        // Visible + TTL elapsed + no probe in flight → the one case that probes.
        assert!(should_refresh(
            true,
            false,
            None,
            Instant::now(),
            DEFAULT_PROBE_TTL
        ));
    }

    #[test]
    fn should_refresh_single_flight_blocks_a_second_spawn() {
        // The single-flight guard: even visible + due, an in-flight probe stops
        // a second `aida ps` from spawning (the pile-up cause). trace:BUG-676
        assert!(!should_refresh(
            true,
            true, // a probe is already running
            None,
            Instant::now(),
            DEFAULT_PROBE_TTL
        ));
    }

    #[test]
    fn should_refresh_respects_ttl_when_visible() {
        let now = Instant::now();
        // Just probed, still visible, nothing in flight → suppressed by the TTL.
        assert!(!should_refresh(
            true,
            false,
            Some(now),
            now,
            DEFAULT_PROBE_TTL
        ));
    }

    #[test]
    fn refresh_when_not_visible_never_stamps_or_spawns() {
        // A real call on an invisible panel must not touch the probe state at
        // all — no timer stamp, no in-flight thread. trace:BUG-676
        let mut probe = LivenessProbe::default();
        probe.refresh_if_due(Path::new("/nonexistent/aida"), Path::new("."), false);
        assert!(probe.last_probe.is_none());
        assert!(probe.inflight.is_none());
    }

    #[test]
    fn mark_stale_clears_the_last_probe_timer() {
        let mut probe = LivenessProbe::default();
        probe.last_probe = Some(Instant::now());
        probe.mark_stale();
        assert!(probe.last_probe.is_none());
    }

    #[test]
    fn reap_inflight_keeps_map_while_probe_runs_and_swaps_on_completion() {
        // Drive the single-flight bookkeeping without spawning `aida ps`: install
        // a hand-made in-flight channel and step it. trace:BUG-676
        let mut probe = LivenessProbe::default();
        let (tx, rx) = std::sync::mpsc::channel();
        probe.inflight = Some(rx);

        // Nothing sent yet → in-flight stays, map unchanged (still empty).
        probe.reap_inflight();
        assert!(probe.inflight.is_some());
        assert_eq!(probe.for_id("TASK-1"), RowLiveness::Idle);

        // Probe posts a verdict → reap swaps it in and clears the in-flight slot.
        let mut map = HashMap::new();
        map.insert("TASK-1".to_string(), RowLiveness::Live);
        tx.send(Some(map)).unwrap();
        probe.reap_inflight();
        assert!(probe.inflight.is_none());
        assert_eq!(probe.for_id("TASK-1"), RowLiveness::Live);
    }

    #[test]
    fn reap_inflight_keeps_last_good_map_on_probe_failure() {
        // A failed probe sends `None`; reap must keep the previous map, not blank
        // every glyph to Idle. trace:BUG-676
        let mut probe = LivenessProbe::default();
        let mut good = HashMap::new();
        good.insert("TASK-2".to_string(), RowLiveness::Live);
        probe.set_map(good);
        let (tx, rx) = std::sync::mpsc::channel();
        probe.inflight = Some(rx);
        tx.send(None).unwrap();
        probe.reap_inflight();
        assert!(probe.inflight.is_none());
        assert_eq!(probe.for_id("TASK-2"), RowLiveness::Live);
    }

    #[test]
    fn parse_maps_live_stale_and_idle() {
        let json = r#"{
            "sessions": [
                { "spec": "TASK-1", "live": true },
                { "spec": "TASK-2", "live": false }
            ],
            "orphaned": [
                { "spec": "TASK-3", "liveness": "stale" },
                { "spec": "TASK-4", "liveness": "flag-only" }
            ]
        }"#;
        let map = parse_ps_json(json);
        assert_eq!(map.get("TASK-1"), Some(&RowLiveness::Live));
        assert_eq!(map.get("TASK-2"), Some(&RowLiveness::Stale));
        assert_eq!(map.get("TASK-3"), Some(&RowLiveness::Stale));
        assert_eq!(map.get("TASK-4"), Some(&RowLiveness::Stale));
        // Absent spec is not in the map (looked up as Idle by `for_id`).
        assert!(map.get("TASK-9").is_none());
    }

    #[test]
    fn parse_skips_null_spec_and_tolerates_garbage() {
        // A generic harness lease has a null spec — nothing to mark.
        let json = r#"{ "sessions": [{ "spec": null, "live": true }], "orphaned": [] }"#;
        assert!(parse_ps_json(json).is_empty());
        // Malformed JSON → empty map (everything reads Idle), never a panic.
        assert!(parse_ps_json("not json").is_empty());
        assert!(parse_ps_json("").is_empty());
    }

    #[test]
    fn parse_keys_are_case_insensitive_and_live_wins() {
        // Lower-case spec id from the CLI still matches an upper-case display id.
        let json = r#"{
            "sessions": [{ "spec": "task-7", "live": true }],
            "orphaned": [{ "spec": "TASK-7", "liveness": "flag-only" }]
        }"#;
        let map = parse_ps_json(json);
        // Live must win over the orphaned Stale for the same spec.
        assert_eq!(map.get("TASK-7"), Some(&RowLiveness::Live));
    }

    #[test]
    fn probe_for_id_defaults_to_idle_and_folds_case() {
        let mut probe = LivenessProbe::default();
        let mut map = HashMap::new();
        map.insert("TASK-5".to_string(), RowLiveness::Live);
        probe.set_map(map);
        assert_eq!(probe.for_id("TASK-5"), RowLiveness::Live);
        assert_eq!(probe.for_id("task-5"), RowLiveness::Live);
        assert_eq!(probe.for_id("TASK-404"), RowLiveness::Idle);
    }
}
