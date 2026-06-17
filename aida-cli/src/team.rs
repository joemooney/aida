//! Team identity & awareness (STORY-640, tier-1 of EPIC-47).
//!
//! Three surfaces, all keyed off the shared node roster
//! (`registry/nodes.toml` on the `aida-store` branch):
//!
//! 1. **`aida team`** — the roster: every registered node/clone with host,
//!    email, clone path, when it registered, and whether it currently holds a
//!    `coordination/` claim (lease / drain / solo).
//! 2. **`aida status` coordination view** — the active cross-clone
//!    `coordination/` claims (rendered by `main.rs`, sourced from
//!    [`crate::coordination`]).
//! 3. **distinct-identity guard + onboarding hint** — in a *team context* (a
//!    roster with more than one node, or any node other than this clone), the
//!    BUG-89 `"default"` shared identity is loudly flagged, and a fresh clone
//!    that joins an existing roster is nudged to set `AIDA_USER` + acquire a
//!    node id. Solo / single-node context is unchanged (no nag).
//!
//! **Best-effort + backward-compatible.** An absent / unreadable roster, or a
//! roster with ≤1 node, means "not a team" — every guard is silent and reads
//! never hard-block. trace:STORY-640 | ai:claude

use std::path::Path;

use aida_core::node::{NodeRegistry, NodeRegistryEntry};

/// Filename of the shared node roster under the store worktree.
const NODES_TOML_REL: &[&str] = &["registry", "nodes.toml"];

/// Load the shared node roster from `<store_root>/registry/nodes.toml`.
/// Returns an empty roster when the file is absent or unreadable — a missing
/// roster is "not a team", never an error. trace:STORY-640 | ai:claude
pub(crate) fn load_roster(store_root: &Path) -> NodeRegistry {
    let mut path = store_root.to_path_buf();
    for seg in NODES_TOML_REL {
        path.push(seg);
    }
    NodeRegistry::load(&path).unwrap_or_default()
}

/// Canonicalize a clone path to a stable string for comparison (mirrors the
/// coordination module's clone-identity key). Falls back to the lexical path.
fn canon(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// True when the roster describes a real TEAM relative to `our_clone`: either
/// more than one node is registered, or the single registered node is some
/// OTHER clone (we joined an existing store but haven't acquired a node id).
///
/// Pure over the registry + our clone path so it's directly unit-testable.
/// A clone is "ours" when its recorded `clone_path` canonicalizes to ours;
/// entries with no recorded path can't be attributed, so they count toward the
/// team size (conservative — better a stray nag than a missed collision).
/// trace:STORY-640 | ai:claude
pub(crate) fn is_team_context(registry: &NodeRegistry, our_clone: &str) -> bool {
    if registry.nodes.len() > 1 {
        return true;
    }
    // Exactly one node: a team only if that node is NOT us (we joined someone
    // else's store). Our own single node = solo, no nag.
    match registry.nodes.first() {
        Some(node) => !clone_matches(node, our_clone),
        None => false,
    }
}

/// True when `node`'s recorded clone path matches `our_clone` (both canonical).
fn clone_matches(node: &NodeRegistryEntry, our_clone: &str) -> bool {
    match &node.clone_path {
        Some(p) => {
            let np = p
                .canonicalize()
                .unwrap_or_else(|_| p.clone())
                .display()
                .to_string();
            np == our_clone
        }
        None => false,
    }
}

/// True when this clone has its OWN entry in the roster (a clone_path that
/// matches ours). A fresh clone that joined an existing store but never ran
/// `aida node acquire` has no own entry → the onboarding hint fires.
/// trace:STORY-640 | ai:claude
pub(crate) fn clone_is_registered(registry: &NodeRegistry, our_clone: &str) -> bool {
    registry.nodes.iter().any(|n| clone_matches(n, our_clone))
}

/// The distinct-identity guard decision (BUG-89 hardening). Pure + testable.
///
/// In a team context, the BUG-89 `"default"` fallback (no `$USER` / `$AIDA_USER`
/// / `$USERNAME`) collides queues + attribution across machines — every member
/// must carry a distinct `AIDA_USER`. Returns:
///
/// - `Ok` — no guard needed (solo context, or a distinct user id is set).
/// - `Warn` — a team member is on the shared `"default"` identity: surface a
///   loud warning. Reads still proceed; writes proceed unless the caller opts
///   into refusal via `AIDA_TEAM_REQUIRE_USER` (handled at the call site).
///
/// trace:STORY-640 | ai:claude
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IdentityVerdict {
    /// Identity is fine — distinct user, or not a team.
    Ok,
    /// Team context + shared `"default"` identity — flag it.
    DefaultInTeam,
}

/// Decide the identity verdict from the resolved user id + whether we're in a
/// team context. The `"default"` literal is the BUG-89 fallback that
/// `current_user_id` produces when no env var is set. trace:STORY-640
pub(crate) fn identity_verdict(user_id: &str, team_context: bool) -> IdentityVerdict {
    if team_context && user_id == "default" {
        IdentityVerdict::DefaultInTeam
    } else {
        IdentityVerdict::Ok
    }
}

/// A roster row joined with whether the node currently holds a `coordination/`
/// claim. trace:STORY-640 | ai:claude
pub(crate) struct TeamMember {
    pub entry: NodeRegistryEntry,
    /// Scopes this node currently holds a live claim on (lease scope, "drain",
    /// "solo loop"). Matched by clone_path against the coordination registry.
    pub active_claims: Vec<String>,
    /// True for the row that is THIS clone.
    pub is_self: bool,
}

/// Build the joined team view: every roster entry annotated with the
/// `coordination/` claims it currently holds (matched by clone path) and a
/// self-marker. Claim lookup is best-effort — an unreadable coordination tree
/// just yields empty claim lists. trace:STORY-640 | ai:claude
pub(crate) fn build_team_view(store_root: &Path, our_clone: &str) -> Vec<TeamMember> {
    let registry = load_roster(store_root);
    // One read of the coordination tree; bucket claims by canonical clone path.
    let mut claims = crate::coordination::list_claims(store_root);
    claims.extend(crate::coordination::list_lock_claims(store_root));

    registry
        .nodes
        .into_iter()
        .map(|entry| {
            let entry_clone = entry
                .clone_path
                .as_ref()
                .map(|p| {
                    p.canonicalize()
                        .unwrap_or_else(|_| p.clone())
                        .display()
                        .to_string()
                })
                .unwrap_or_default();
            let active_claims: Vec<String> = if entry_clone.is_empty() {
                Vec::new()
            } else {
                claims
                    .iter()
                    .filter(|c| c.clone_path == entry_clone)
                    .map(|c| c.scope.clone())
                    .collect()
            };
            let is_self = !entry_clone.is_empty() && entry_clone == our_clone;
            TeamMember {
                entry,
                active_claims,
                is_self,
            }
        })
        .collect()
}

/// Resolve the project root (the parent of the store worktree) to a canonical
/// clone-path string. Used by the call sites that only hold `store_root`.
pub(crate) fn our_clone_path(store_root: &Path) -> String {
    let project_root = store_root.parent().unwrap_or(store_root);
    canon(project_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::node::NodeRegistry;
    use chrono::Utc;
    use std::path::PathBuf;

    fn node(id: &str, clone: Option<&str>) -> NodeRegistryEntry {
        NodeRegistryEntry {
            id: id.to_string(),
            user_id: 1,
            hostname: "imac".to_string(),
            email: None,
            clone_path: clone.map(PathBuf::from),
            registered: Utc::now(),
        }
    }

    #[test]
    fn empty_roster_is_not_a_team() {
        let r = NodeRegistry::default();
        assert!(!is_team_context(&r, "/home/joe/ai/aida"));
    }

    #[test]
    fn single_own_node_is_solo() {
        // The one node is us (path matches) → solo, no nag.
        let mut r = NodeRegistry::default();
        r.nodes.push(node("1", Some("/home/joe/ai/aida")));
        // canonicalize on a non-existent path falls back to lexical, so the
        // comparison holds without the path existing on disk.
        assert!(!is_team_context(&r, "/home/joe/ai/aida"));
    }

    #[test]
    fn single_foreign_node_is_a_team() {
        // The one node is someone else's clone → we joined an existing store.
        let mut r = NodeRegistry::default();
        r.nodes.push(node("1", Some("/home/joe/ai/aida-other")));
        assert!(is_team_context(&r, "/home/joe/ai/aida"));
    }

    #[test]
    fn two_nodes_is_a_team() {
        let mut r = NodeRegistry::default();
        r.nodes.push(node("1", Some("/home/joe/ai/aida")));
        r.nodes.push(node("2", Some("/home/joe/ai/aida-b")));
        assert!(is_team_context(&r, "/home/joe/ai/aida"));
    }

    #[test]
    fn identity_guard_fires_only_for_default_in_team() {
        // Team + default → flag.
        assert_eq!(
            identity_verdict("default", true),
            IdentityVerdict::DefaultInTeam
        );
        // Team + distinct user → ok.
        assert_eq!(identity_verdict("alice", true), IdentityVerdict::Ok);
        // Solo + default → ok (no nag for a solo dev).
        assert_eq!(identity_verdict("default", false), IdentityVerdict::Ok);
        // Solo + distinct → ok.
        assert_eq!(identity_verdict("joe", false), IdentityVerdict::Ok);
    }

    #[test]
    fn clone_is_registered_detects_own_entry() {
        let mut r = NodeRegistry::default();
        r.nodes.push(node("1", Some("/home/joe/ai/aida-other")));
        assert!(!clone_is_registered(&r, "/home/joe/ai/aida"));
        r.nodes.push(node("2", Some("/home/joe/ai/aida")));
        assert!(clone_is_registered(&r, "/home/joe/ai/aida"));
    }

    #[test]
    fn node_with_no_clone_path_counts_toward_team() {
        // A pre-EPIC-9 entry has no clone_path; it can't be matched as "ours",
        // so a single such node reads as a team (conservative).
        let mut r = NodeRegistry::default();
        r.nodes.push(node("1", None));
        assert!(is_team_context(&r, "/home/joe/ai/aida"));
    }
}
