//! Cross-vendor role -> vendor routing — pure config model + resolver
//! (TASK-1092, SPIKE-76 slice 4).
//!
//! SPIKE-76 (`docs/plans/2026-07-02-spike-76-dispatch-resilience.md`,
//! "Cross-Vendor Fan-Out") found that dispatch defaults to all-Claude and has
//! no explicit per-role fallback order when a vendor is unavailable (binary
//! missing, rate-limited, paused). This module ships the primitive that
//! closes that gap: an ordered vendor-preference list per ROLE, plus a PURE
//! function that picks the first *available* vendor off that list.
//!
//! **Scope note (TASK-1092): additive only.** Nothing in the live
//! drive/drain/launch path calls [`resolve_vendor_for_role`] yet — no
//! `queue work`, `zen`, `burndown`, or `agent new` behavior changes as a
//! result of this module landing. Wiring the resolver into an actual
//! dispatch decision (and composing it with the TASK-1116 per-invocation
//! `--vendor` override, which should win when present) is deliberately left
//! as a follow-up.
//!
//! trace:TASK-1092 | ai:claude

use std::collections::HashMap;

/// A dispatchable coding-agent vendor. Mirrors the vendor strings already
/// used ad hoc across `aida-cli` (`session::HeadlessVendor`,
/// `compete::VendorAdapter`/`JudgeVendor`, `agents_config::KNOWN_VENDORS`)
/// with one small, crate-local enum scoped to routing decisions — those
/// call sites are not touched by this change.
// trace:TASK-1092 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vendor {
    Claude,
    Codex,
    Antigravity,
}

impl Vendor {
    /// Canonical lowercase name, matching the strings already used in config
    /// (`[agents] vendor = "codex"`) and on the CLI (`agent new <vendor>`).
    pub fn as_str(self) -> &'static str {
        match self {
            Vendor::Claude => "claude",
            Vendor::Codex => "codex",
            Vendor::Antigravity => "antigravity",
        }
    }

    /// Case-insensitive parse. Accepts `agy` as a shorthand alias for
    /// Antigravity (matches the `docs/plans/2026-07-02-spike-76-...` prose,
    /// which uses "AGY" throughout). Unrecognized input returns `None` —
    /// callers never guess.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(Vendor::Claude),
            "codex" => Some(Vendor::Codex),
            "antigravity" | "agy" => Some(Vendor::Antigravity),
            _ => None,
        }
    }
}

/// Role -> ordered vendor-preference list. Storage-agnostic: this struct is
/// built either from [`RoutingTable::default_routing`] or by overlaying
/// `.aida/config.toml`'s `[dispatch.routing]` section on top of it (the I/O
/// side lives in `aida-cli`, mirroring how `aida_core::lock::LockingPosture`
/// is the pure enum and `aida-cli::locking_gate::LockingConfig` is the
/// file-reading wrapper around it).
// trace:TASK-1092 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingTable {
    roles: HashMap<String, Vec<Vendor>>,
}

impl RoutingTable {
    /// SPIKE-76's default cross-vendor routing
    /// (`docs/plans/2026-07-02-spike-76-dispatch-resilience.md`,
    /// "Cross-Vendor Fan-Out"):
    ///
    ///   - **Claude** strongest for headless drain/resume paths and
    ///     advisor/review flows that rely on `defer`/`--resume` behavior.
    ///   - **Codex** for bounded implementation, independent cross-checks,
    ///     and fallback implementation from a pushed branch.
    ///   - **Antigravity** for draft-for-review, cross-validation, and
    ///     mechanical/bounded work (human-briefed, not unattended-merge
    ///     authority — `compete.rs` models it the same way).
    ///
    /// `implementer` lists all three (Antigravity last, since it's
    /// human-briefed rather than autonomous); `advisor`, `reviewer`, and
    /// `integrator` list Claude then Codex — SPIKE-76 doesn't recommend
    /// Antigravity for those flows.
    pub fn default_routing() -> Self {
        let mut roles = HashMap::new();
        roles.insert(
            "implementer".to_string(),
            vec![Vendor::Claude, Vendor::Codex, Vendor::Antigravity],
        );
        roles.insert("advisor".to_string(), vec![Vendor::Claude, Vendor::Codex]);
        roles.insert("reviewer".to_string(), vec![Vendor::Claude, Vendor::Codex]);
        roles.insert(
            "integrator".to_string(),
            vec![Vendor::Claude, Vendor::Codex],
        );
        Self { roles }
    }

    /// Build an empty table (no roles configured at all) — used as the base
    /// for tests and for a caller that wants to overlay config without the
    /// shipped defaults underneath.
    pub fn empty() -> Self {
        Self {
            roles: HashMap::new(),
        }
    }

    /// Set (or replace) one role's ordered vendor list.
    pub fn set_role(&mut self, role: impl Into<String>, vendors: Vec<Vendor>) {
        self.roles.insert(role.into(), vendors);
    }

    /// This role's ordered vendor-preference list, if the table has one.
    /// `None` for a role the table doesn't know about — callers (including
    /// [`resolve_vendor_for_role`]) treat that as "no routing opinion",
    /// never as "use some other role's list".
    pub fn list_for_role(&self, role: &str) -> Option<&[Vendor]> {
        self.roles.get(role).map(|v| v.as_slice())
    }
}

impl Default for RoutingTable {
    fn default() -> Self {
        Self::default_routing()
    }
}

/// PURE: resolve which vendor should handle `role`, given the vendors
/// actually `available` right now.
///
/// Returns the FIRST vendor in `role`'s configured preference list that also
/// appears in `available` — i.e. primary-available wins, primary-unavailable
/// falls to the next entry, and so on down the list.
///
/// Returns `None` in two cases, both left for the caller to decide
/// (park/defer/error — this function never guesses):
///   - the role has no routing entry in `routing` at all (unknown role: no
///     sensible default exists without knowing what kind of work the role
///     does, so this deliberately does NOT fall back to another role's list
///     or to a shipped default here — the caller-supplied `routing` is
///     already `RoutingTable::default_routing()` unless overridden, so an
///     unknown role is either a typo or an intentionally-unrouted new role);
///   - the role has a list, but none of its vendors are in `available`
///     (exhaustion — every vendor the role is willing to use is down).
///
/// Never returns a vendor that isn't both listed for the role AND available.
// trace:TASK-1092 | ai:claude
pub fn resolve_vendor_for_role(
    role: &str,
    available: &[Vendor],
    routing: &RoutingTable,
) -> Option<Vendor> {
    let candidates = routing.list_for_role(role)?;
    candidates.iter().copied().find(|v| available.contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_round_trips_through_as_str_and_parse() {
        for v in [Vendor::Claude, Vendor::Codex, Vendor::Antigravity] {
            assert_eq!(Vendor::parse(v.as_str()), Some(v));
        }
    }

    #[test]
    fn vendor_parse_is_case_insensitive_and_accepts_agy_alias() {
        assert_eq!(Vendor::parse("CODEX"), Some(Vendor::Codex));
        assert_eq!(Vendor::parse(" Claude "), Some(Vendor::Claude));
        assert_eq!(Vendor::parse("agy"), Some(Vendor::Antigravity));
        assert_eq!(Vendor::parse("AGY"), Some(Vendor::Antigravity));
    }

    #[test]
    fn vendor_parse_rejects_unknown_names() {
        assert_eq!(Vendor::parse("gemini"), None);
        assert_eq!(Vendor::parse(""), None);
    }

    #[test]
    fn default_routing_lists_all_three_for_implementer_claude_first() {
        let table = RoutingTable::default_routing();
        let list = table.list_for_role("implementer").unwrap();
        assert_eq!(
            list,
            &[Vendor::Claude, Vendor::Codex, Vendor::Antigravity][..]
        );
    }

    #[test]
    fn default_routing_advisor_reviewer_integrator_are_claude_then_codex() {
        let table = RoutingTable::default_routing();
        for role in ["advisor", "reviewer", "integrator"] {
            assert_eq!(
                table.list_for_role(role).unwrap(),
                &[Vendor::Claude, Vendor::Codex][..],
                "role {role}"
            );
        }
    }

    #[test]
    fn resolve_vendor_for_role_returns_primary_when_available() {
        let table = RoutingTable::default_routing();
        let available = [Vendor::Claude, Vendor::Codex, Vendor::Antigravity];
        assert_eq!(
            resolve_vendor_for_role("implementer", &available, &table),
            Some(Vendor::Claude)
        );
    }

    #[test]
    fn resolve_vendor_for_role_falls_back_to_next_when_primary_unavailable() {
        let table = RoutingTable::default_routing();
        let available = [Vendor::Codex, Vendor::Antigravity];
        assert_eq!(
            resolve_vendor_for_role("implementer", &available, &table),
            Some(Vendor::Codex)
        );
    }

    #[test]
    fn resolve_vendor_for_role_falls_all_the_way_to_last_choice() {
        let table = RoutingTable::default_routing();
        let available = [Vendor::Antigravity];
        assert_eq!(
            resolve_vendor_for_role("implementer", &available, &table),
            Some(Vendor::Antigravity)
        );
    }

    #[test]
    fn resolve_vendor_for_role_returns_none_when_no_listed_vendor_is_available() {
        let table = RoutingTable::default_routing();
        // advisor's list is [Claude, Codex] — neither is available.
        let available = [Vendor::Antigravity];
        assert_eq!(resolve_vendor_for_role("advisor", &available, &table), None);
    }

    #[test]
    fn resolve_vendor_for_role_never_picks_an_unlisted_vendor() {
        let mut table = RoutingTable::empty();
        table.set_role("reviewer", vec![Vendor::Claude]);
        // Codex/Antigravity are available, but NOT in reviewer's list —
        // must not be substituted in.
        let available = [Vendor::Codex, Vendor::Antigravity];
        assert_eq!(
            resolve_vendor_for_role("reviewer", &available, &table),
            None
        );
    }

    #[test]
    fn resolve_vendor_for_role_returns_none_for_a_role_with_no_routing_entry() {
        let table = RoutingTable::default_routing();
        let available = [Vendor::Claude, Vendor::Codex, Vendor::Antigravity];
        assert_eq!(
            resolve_vendor_for_role("some-unknown-role", &available, &table),
            None
        );
    }

    #[test]
    fn resolve_vendor_for_role_with_no_available_vendors_at_all_is_none() {
        let table = RoutingTable::default_routing();
        assert_eq!(resolve_vendor_for_role("implementer", &[], &table), None);
    }
}
