//! Seat policy (STORY-620): which "needs a person" buckets land on the
//! operator's worklist (`aida human`) vs the advisor's (`aida advisor`).
//!
//! STORY-618 split the two worklists with a hardcoded partition. This module
//! makes the split a *resolved policy* with built-in defaults. Only the four
//! buckets that BOTH worklists already know how to render are configurable, so
//! reassigning one can never create a rendering hole:
//!
//!   `to-groom` · `decompose` · `ready-to-close` · `triage`
//!
//! Each defaults to the **advisor**. The intrinsically seat-bound buckets stay
//! fixed and are not configurable here: `reviews-awaiting` /
//! `decisions-awaiting` / `awaiting-merge` / `held-for-review` /
//! `build-supervised` are the operator's (a human reviews / decides / merges /
//! drives keystone builds); `distill` / `bless` are the advisor's (authoring
//! questions / queue sign-off).
//!
//! Resolution order, highest precedence first:
//!   1. `<project>/.aida/config.toml` `[seats]`
//!   2. `~/.aida/config.toml` `[seats]`  (the user-global default)
//!   3. the built-in default ([`default_seat`])
//!
//! A missing file / section / key, or a value outside the configurable set,
//! falls through to the next layer — a config typo never blocks a worklist.
//! trace:STORY-620 | ai:claude

use std::collections::HashMap;
use std::path::Path;

/// Which seat a worklist bucket belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seat {
    Operator,
    Advisor,
}

impl Seat {
    /// Parse a config token (case-insensitive). `human` is accepted as an
    /// alias for `operator` (it is the command name the operator runs).
    pub fn parse(s: &str) -> Option<Seat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "operator" | "human" => Some(Seat::Operator),
            "advisor" => Some(Seat::Advisor),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Seat::Operator => "operator",
            Seat::Advisor => "advisor",
        }
    }
}

/// The bucket keys whose seat is configurable — the four buckets BOTH the
/// operator and advisor worklists can render. Reassigning any other key would
/// move it off one list without the other being able to show it, so the loader
/// ignores out-of-set keys.
pub const CONFIGURABLE_KEYS: &[&str] = &["to-groom", "decompose", "ready-to-close", "triage"];

/// Built-in default seat for a configurable bucket. All four default to the
/// advisor today (STORY-618's partition); kept as a fn so a future bucket can
/// carry a different default without touching callers.
pub fn default_seat(_key: &str) -> Seat {
    Seat::Advisor
}

/// A resolved seat policy: the configurable-bucket overrides merged across the
/// user-global and project config layers (project wins).
#[derive(Debug, Clone, Default)]
pub struct SeatPolicy {
    overrides: HashMap<String, Seat>,
}

impl SeatPolicy {
    /// The effective seat for a configurable bucket key.
    pub fn seat_of(&self, key: &str) -> Seat {
        self.overrides
            .get(key)
            .copied()
            .unwrap_or_else(|| default_seat(key))
    }

    /// True when this key's seat was set by config (vs. the built-in default).
    #[allow(dead_code)] // config-vs-default probe for a future "why this seat" surface; asserted in tests
    pub fn is_overridden(&self, key: &str) -> bool {
        self.overrides.contains_key(key)
    }

    /// Resolve from the user-global config first, then the project config so
    /// the project layer wins on a key both set. Missing files fall through.
    pub fn load(project_root: &Path) -> Self {
        let mut overrides = HashMap::new();
        if let Some(home) = crate::aida_home_dir() {
            apply_file(&mut overrides, &home.join(".aida").join("config.toml"));
        }
        apply_file(
            &mut overrides,
            &project_root.join(".aida").join("config.toml"),
        );
        SeatPolicy { overrides }
    }

    /// Build from a raw TOML string — used by tests so they don't touch disk.
    #[cfg(test)]
    pub fn from_toml_str(content: &str) -> Self {
        let mut overrides = HashMap::new();
        apply_str(&mut overrides, content);
        SeatPolicy { overrides }
    }
}

/// The seat a single config file assigns to a configurable bucket key, if any —
/// used by `aida config show` to attribute the source (project vs user-global).
pub fn seat_in_file(path: &Path, key: &str) -> Option<Seat> {
    let content = std::fs::read_to_string(path).ok()?;
    scan_seats_section(&content)
        .into_iter()
        .filter(|(k, _)| k == key)
        .find_map(|(_, v)| Seat::parse(&v))
}

fn apply_file(overrides: &mut HashMap<String, Seat>, path: &Path) {
    if let Ok(content) = std::fs::read_to_string(path) {
        apply_str(overrides, &content);
    }
}

fn apply_str(overrides: &mut HashMap<String, Seat>, content: &str) {
    for (key, val) in scan_seats_section(content) {
        // Only the configurable buckets are honored — silently ignore others so
        // a misconfigured key can't create a rendering hole.
        if CONFIGURABLE_KEYS.contains(&key.as_str()) {
            if let Some(seat) = Seat::parse(&val) {
                overrides.insert(key, seat);
            }
        }
    }
}

/// Extract `key = value` pairs from the `[seats]` section. Section-aware; stops
/// at the next `[section]`. Mirrors the hand-rolled scanner used by
/// `advisor::scan_advisor_section` so we don't pull a serde TOML dependency.
fn scan_seats_section(content: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut in_seats = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_seats = stripped.trim_end_matches(']').trim() == "seats";
            continue;
        }
        if in_seats {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'').trim();
                pairs.push((k.trim().to_string(), v.to_string()));
            }
        }
    }
    pairs
}

fn strip_inline_comment(s: &str) -> &str {
    let (mut dq, mut sq) = (false, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '#' if !dq && !sq => return &s[..i],
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_advisor_for_every_configurable_bucket() {
        let p = SeatPolicy::default();
        for key in CONFIGURABLE_KEYS {
            assert_eq!(p.seat_of(key), Seat::Advisor, "{key} defaults to advisor");
            assert!(!p.is_overridden(key));
        }
    }

    #[test]
    fn project_override_moves_a_bucket_to_the_operator() {
        let p = SeatPolicy::from_toml_str("[seats]\ntriage = \"operator\"\n");
        assert_eq!(p.seat_of("triage"), Seat::Operator);
        assert!(p.is_overridden("triage"));
        // Untouched buckets keep their default.
        assert_eq!(p.seat_of("to-groom"), Seat::Advisor);
    }

    #[test]
    fn human_alias_and_advisor_parse() {
        let p = SeatPolicy::from_toml_str(
            "[seats]\nready-to-close = \"human\"\ntriage = \"advisor\"\n",
        );
        assert_eq!(p.seat_of("ready-to-close"), Seat::Operator);
        assert_eq!(p.seat_of("triage"), Seat::Advisor);
    }

    #[test]
    fn out_of_set_keys_and_bad_values_are_ignored() {
        // A non-configurable key (reviews-awaiting) must not create an override.
        let p = SeatPolicy::from_toml_str(
            "[seats]\nreviews-awaiting = \"advisor\"\ntriage = \"nonsense\"\n",
        );
        assert!(!p.is_overridden("reviews-awaiting"));
        assert!(!p.is_overridden("triage"));
        assert_eq!(p.seat_of("triage"), Seat::Advisor);
    }

    #[test]
    fn section_aware_scanner_stops_at_next_section() {
        let p = SeatPolicy::from_toml_str(
            "[seats]\ntriage = \"operator\"\n\n[advisor]\ntriage = \"advisor\"\n",
        );
        // Only the [seats] value counts; the [advisor] line is a different
        // section and must not leak in.
        assert_eq!(p.seat_of("triage"), Seat::Operator);
    }
}
