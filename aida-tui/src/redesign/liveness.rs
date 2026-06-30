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
//! ## Keeping it cheap
//!
//! `aida ps` runs a real process probe, so it is too expensive to fire on the
//! TUI's hot render loop. [`LivenessProbe::refresh_if_due`] gates the shell-out
//! behind [`should_probe`] — a pure time check against a TTL — so the subprocess
//! runs at most once per [`PROBE_TTL`] and every frame in between reads the
//! cached map. The render path only ever calls [`LivenessProbe::for_id`], a
//! `HashMap` lookup.
//!
//! trace:TASK-978 | ai:claude

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use super::list_row::RowLiveness;

/// How long a probe result stays fresh before `refresh_if_due` will shell out
/// again. Short enough that a session starting/dying shows up within a couple of
/// seconds; long enough that the subprocess never runs on a per-frame cadence
/// (the idle render loop ticks ~1/s).
// trace:TASK-978 | ai:claude
pub const PROBE_TTL: Duration = Duration::from_secs(3);

/// Should a probe fire, given when the last one ran? Pure so the cache gate is
/// unit-testable without a clock or a subprocess: `None` (never probed) always
/// fires; otherwise only once the TTL has elapsed.
// trace:TASK-978 | ai:claude
pub fn should_probe(last: Option<Instant>, now: Instant, ttl: Duration) -> bool {
    match last {
        None => true,
        Some(prev) => now.duration_since(prev) >= ttl,
    }
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

/// The cockpit's cached per-spec liveness, refreshed on a poll cadence.
///
/// Holds the last `aida ps --json` verdict map plus the timestamp of the last
/// probe so [`Self::refresh_if_due`] can gate the subprocess behind [`PROBE_TTL`].
// trace:TASK-978 | ai:claude
#[derive(Debug, Default, Clone)]
pub struct LivenessProbe {
    map: HashMap<String, RowLiveness>,
    last_probe: Option<Instant>,
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

    /// Shell out to `aida ps --json` and refresh the map IF the TTL has elapsed
    /// since the last probe (or it has never run). Cheap no-op within a frame /
    /// within [`PROBE_TTL`]. `aida_exe` is the running binary (`current_exe()`),
    /// `project_root` the cockpit's project dir. Failures (binary missing, parse
    /// error) leave the previous map intact and still stamp the probe time, so a
    /// broken probe doesn't busy-loop the subprocess.
    // trace:TASK-978 | ai:claude
    pub fn refresh_if_due(&mut self, aida_exe: &Path, project_root: &Path) {
        if !should_probe(self.last_probe, Instant::now(), PROBE_TTL) {
            return;
        }
        self.last_probe = Some(Instant::now());
        let output = std::process::Command::new(aida_exe)
            .arg("ps")
            .arg("--json")
            .current_dir(project_root)
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                if let Ok(text) = String::from_utf8(out.stdout) {
                    self.map = parse_ps_json(&text);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_probe_fires_when_never_probed() {
        assert!(should_probe(None, Instant::now(), PROBE_TTL));
    }

    #[test]
    fn should_probe_is_false_within_ttl() {
        // A probe that just ran must NOT re-fire within the same frame / TTL —
        // this is the "don't re-probe per render" guard. trace:TASK-978
        let now = Instant::now();
        assert!(!should_probe(Some(now), now, PROBE_TTL));
        // A tick well inside the TTL is still suppressed.
        assert!(!should_probe(
            Some(now),
            now + Duration::from_millis(500),
            PROBE_TTL
        ));
    }

    #[test]
    fn should_probe_fires_after_ttl_elapses() {
        let start = Instant::now();
        assert!(should_probe(
            Some(start),
            start + PROBE_TTL + Duration::from_millis(1),
            PROBE_TTL
        ));
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
