//! `[dispatch.routing]` config loader — TASK-1092.
//!
//! Reads the role -> ordered-vendor-list overrides from
//! `.aida/config.toml`'s `[dispatch.routing]` section and overlays them on
//! top of `aida_core::dispatch_routing::RoutingTable::default_routing()`.
//! Mirrors `locking_gate::LockingConfig`'s / `advisor::AdvisorConfig`'s
//! hand-rolled section-scanner pattern (no serde-toml dependency for a
//! handful of scalar/array keys) — this is the same pattern extended to
//! array-valued keys instead of a single scalar per key.
//!
//! **Additive only (TASK-1092 scope).** Nothing calls
//! [`DispatchRoutingConfig::load`] from a live dispatch/drive/drain/launch
//! path yet — it exists so the primitive (config shape + pure resolver) can
//! ship and be exercised by tests ahead of the follow-up that wires it into
//! an actual vendor-selection decision.
//!
//! trace:TASK-1092 | ai:claude

// Nothing outside this module's own tests calls `DispatchRoutingConfig` yet
// (see the scope note above) — silence the resulting dead_code warnings
// rather than force a premature call site. Mirrors `autopilot.rs`'s
// `#![allow(dead_code)]` for the same "shipped, not yet wired in" situation.
#![allow(dead_code)]

use std::path::Path;

use aida_core::dispatch_routing::{RoutingTable, Vendor};

/// `[dispatch.routing]` in `.aida/config.toml`. Missing file / section / keys
/// all fall through to `RoutingTable::default_routing()` — a config error or
/// an absent section never blocks a caller building a table.
// trace:TASK-1092 | ai:claude
#[derive(Debug, Clone)]
pub struct DispatchRoutingConfig {
    pub table: RoutingTable,
}

impl Default for DispatchRoutingConfig {
    fn default() -> Self {
        Self {
            table: RoutingTable::default_routing(),
        }
    }
}

impl DispatchRoutingConfig {
    /// Load `[dispatch.routing]` from `<project_root>/.aida/config.toml`,
    /// overlaying any `role = ["vendor", ...]` entries found on top of the
    /// shipped default table (a config that only overrides one role, e.g.
    /// just `implementer`, leaves the others at their defaults).
    pub fn load(project_root: &Path) -> Self {
        let mut table = RoutingTable::default_routing();
        if let Ok(content) = std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        {
            for (role, vendors) in parse_dispatch_routing_section(&content) {
                table.set_role(role, vendors);
            }
        }
        Self { table }
    }

    /// Build from a raw TOML string, bypassing the filesystem — used by the
    /// tests so they don't have to touch disk.
    #[cfg(test)]
    pub fn from_toml_str(content: &str) -> Self {
        let mut table = RoutingTable::default_routing();
        for (role, vendors) in parse_dispatch_routing_section(content) {
            table.set_role(role, vendors);
        }
        Self { table }
    }
}

/// Extract `role = ["vendor", ...]` pairs from `[dispatch.routing]`.
/// Section-aware; stops at the next `[section]`. A value that doesn't parse
/// as a `[...]` array, or that contains an unrecognized vendor name, drops
/// just that one malformed entry (or that one unrecognized element) rather
/// than failing the whole load — matching `advisor`/`locking_gate`'s
/// "config errors never block the caller" posture.
// trace:TASK-1092 | ai:claude
fn parse_dispatch_routing_section(content: &str) -> Vec<(String, Vec<Vendor>)> {
    let mut pairs = Vec::new();
    let mut in_section = false;
    for raw in content.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix('[') {
            in_section = stripped.trim_end_matches(']').trim() == "dispatch.routing";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let vendors = parse_vendor_list(val.trim());
        if !vendors.is_empty() {
            pairs.push((key.trim().to_string(), vendors));
        }
    }
    pairs
}

/// Parse a TOML-array-shaped value like `["claude", "codex"]` into the
/// `Vendor`s it names, skipping any element that doesn't parse (unknown
/// vendor name, stray punctuation) instead of failing the whole list.
fn parse_vendor_list(raw: &str) -> Vec<Vendor> {
    let Some(inner) = raw.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Vec::new();
    };
    inner
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').trim())
        .filter(|s| !s.is_empty())
        .filter_map(Vendor::parse)
        .collect()
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
    fn default_config_matches_default_routing_table() {
        let cfg = DispatchRoutingConfig::default();
        assert_eq!(cfg.table, RoutingTable::default_routing());
    }

    #[test]
    fn load_falls_back_to_default_when_config_file_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = DispatchRoutingConfig::load(tmp.path());
        assert_eq!(cfg.table, RoutingTable::default_routing());
    }

    #[test]
    fn load_falls_back_to_default_when_section_is_absent() {
        let cfg = DispatchRoutingConfig::from_toml_str("[agents]\nvendor = \"codex\"\n");
        assert_eq!(cfg.table, RoutingTable::default_routing());
    }

    #[test]
    fn section_overrides_one_role_and_leaves_others_default() {
        let cfg = DispatchRoutingConfig::from_toml_str(
            "[dispatch.routing]\nimplementer = [\"codex\", \"claude\"]\n",
        );
        assert_eq!(
            cfg.table.list_for_role("implementer").unwrap(),
            &[Vendor::Codex, Vendor::Claude][..]
        );
        // advisor wasn't mentioned in the config — stays at the shipped default.
        assert_eq!(
            cfg.table.list_for_role("advisor").unwrap(),
            RoutingTable::default_routing()
                .list_for_role("advisor")
                .unwrap()
        );
    }

    #[test]
    fn section_can_add_a_brand_new_role_not_in_the_default_table() {
        let cfg =
            DispatchRoutingConfig::from_toml_str("[dispatch.routing]\ntriager = [\"claude\"]\n");
        assert_eq!(
            cfg.table.list_for_role("triager").unwrap(),
            &[Vendor::Claude][..]
        );
    }

    #[test]
    fn unrecognized_vendor_names_are_dropped_not_fatal() {
        let cfg = DispatchRoutingConfig::from_toml_str(
            "[dispatch.routing]\nimplementer = [\"claude\", \"gemini\", \"codex\"]\n",
        );
        assert_eq!(
            cfg.table.list_for_role("implementer").unwrap(),
            &[Vendor::Claude, Vendor::Codex][..]
        );
    }

    #[test]
    fn stops_scanning_at_the_next_section() {
        let cfg = DispatchRoutingConfig::from_toml_str(
            "[dispatch.routing]\nimplementer = [\"codex\"]\n\n[agents]\nvendor = \"claude\"\n",
        );
        assert_eq!(
            cfg.table.list_for_role("implementer").unwrap(),
            &[Vendor::Codex][..]
        );
    }

    #[test]
    fn inline_comment_after_array_is_stripped() {
        let cfg = DispatchRoutingConfig::from_toml_str(
            "[dispatch.routing]\nimplementer = [\"codex\"] # prefer codex here\n",
        );
        assert_eq!(
            cfg.table.list_for_role("implementer").unwrap(),
            &[Vendor::Codex][..]
        );
    }

    #[test]
    fn load_reads_real_config_toml_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".aida")).unwrap();
        std::fs::write(
            tmp.path().join(".aida").join("config.toml"),
            "[dispatch.routing]\nreviewer = [\"codex\"]\n",
        )
        .unwrap();
        let cfg = DispatchRoutingConfig::load(tmp.path());
        assert_eq!(
            cfg.table.list_for_role("reviewer").unwrap(),
            &[Vendor::Codex][..]
        );
    }
}
