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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aida_core::node::{NodeRegistry, NodeRegistryEntry};

/// Filename of the shared node roster under the store worktree.
const NODES_TOML_REL: &[&str] = &["registry", "nodes.toml"];

/// Filename of the shared per-user role roster under the store worktree.
/// trace:STORY-646 | ai:claude
const TEAM_TOML_REL: &[&str] = &["registry", "team.toml"];

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

// ── Team RBAC: per-user roles (STORY-646) ───────────────────────────────────
//
// GUARDRAIL, NOT SECURITY. The store is a shared git branch — anyone with push
// access to `aida-store` can edit any YAML directly, so this role roster can
// never be an access-control boundary. What it CAN be: a guardrail that stops
// *accidents* (an implementer accidentally approving a spec), an encoding of
// team structure, and an audit signal (bypasses show up in git history). The
// caveat is surfaced in `aida team set-role` --help, its output, and the docs.
// trace:STORY-646 | ai:claude

/// The shared per-user role roster — `registry/team.toml` on the `aida-store`
/// branch. Maps a `user_id` (the person, per `current_user_id`) to a role
/// string. An absent file / absent user = unranked → falls back to
/// `AIDA_SESSION_ROLE` / the current default (backward-compatible).
///
/// ```toml
/// [members]
/// alice = "advisor"
/// bob   = "implementer"
/// ```
/// trace:STORY-646 | ai:claude
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct TeamRoster {
    /// user_id -> role string. `BTreeMap` keeps the file deterministically
    /// sorted so a CAS round-trip is stable across writers.
    #[serde(default)]
    pub members: BTreeMap<String, String>,
}

impl TeamRoster {
    /// Absolute path to `registry/team.toml` under a store worktree root.
    fn path(store_root: &Path) -> PathBuf {
        let mut path = store_root.to_path_buf();
        for seg in TEAM_TOML_REL {
            path.push(seg);
        }
        path
    }

    /// Load the roster from `<store_root>/registry/team.toml`. A missing or
    /// unreadable file yields an empty roster — "no RBAC configured" is never
    /// an error (best-effort, backward-compatible). trace:STORY-646
    pub(crate) fn load(store_root: &Path) -> Self {
        let path = Self::path(store_root);
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_default()
    }

    /// The roster role for `user_id`, if one is recorded.
    pub(crate) fn role_for(&self, user_id: &str) -> Option<&str> {
        self.members.get(user_id).map(String::as_str)
    }

    /// Set (or replace) a user's role and serialize to TOML. The CAS write now
    /// lives in `aida_core::team::set_role_cas`; this remains for the round-trip
    /// unit test below. trace:STORY-650 | ai:claude
    #[cfg(test)]
    fn with_role_set(mut self, user_id: &str, role: &str) -> Self {
        self.members.insert(user_id.to_string(), role.to_string());
        self
    }
}

/// Pure resolution of a user's effective role for the guardrail.
///
/// Priority: the **roster** role for the user (durable, survives a forgotten
/// env var) → ELSE the session env (`AIDA_SESSION_ROLE`) → ELSE the read-side
/// default (`implementer`). So a rostered user gets their role with no env set;
/// a non-rostered user behaves exactly as today (backward-compatible).
///
/// Pure over its inputs (roster role + raw env value) so it is directly
/// unit-testable without touching the process env or the filesystem.
/// trace:STORY-646 | ai:claude
pub(crate) fn resolve_effective_role(
    roster_role: Option<&str>,
    env_role: Option<&str>,
) -> (String, RoleSource) {
    if let Some(r) = roster_role.map(str::trim).filter(|s| !s.is_empty()) {
        return (super::canonical_role_name(r), RoleSource::Roster);
    }
    match env_role.map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => (super::canonical_role_name(r), RoleSource::Env),
        None => ("implementer".to_string(), RoleSource::Default),
    }
}

/// Where an effective role came from — lets call sites tailor the refusal
/// message (a roster role is the durable team role; an env role is per-shell).
/// trace:STORY-646 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoleSource {
    /// From `registry/team.toml` (the durable team role).
    Roster,
    /// From the `AIDA_SESSION_ROLE` env var (per-shell, self-declared).
    Env,
    /// Neither set — the read-side `implementer` default.
    Default,
}

/// Resolve the effective role for `user_id` against the store at `store_root`,
/// reading `AIDA_SESSION_ROLE` from the process env. Best-effort: an
/// unreachable / unreadable store yields an empty roster, so resolution falls
/// straight through to the env / default (never blocks). trace:STORY-646
pub(crate) fn effective_role_for_user(store_root: &Path, user_id: &str) -> (String, RoleSource) {
    let roster = TeamRoster::load(store_root);
    let roster_role = roster.role_for(user_id).map(str::to_string);
    let env_role = std::env::var("AIDA_SESSION_ROLE").ok();
    resolve_effective_role(roster_role.as_deref(), env_role.as_deref())
}

/// Write `user_id = role` into `registry/team.toml` on the store with a
/// CAS push-wins loop (mirrors `git_ops::register_node_full`): pull → load →
/// merge our edit → save → commit → push; on a rejected push, hard-reset the
/// stale commit and retry. Solo (no `origin`) writes locally and lets the next
/// `aida push` upload. Returns the canonicalized role written. trace:STORY-646
pub(crate) fn set_role_cas(store_root: &Path, user_id: &str, role: &str) -> anyhow::Result<()> {
    // Delegate to the shared aida-core implementation so the CLI and the REST
    // `PUT /api/v2/team/:user/role` endpoint write team.toml identically.
    // trace:STORY-650 | ai:claude
    aida_core::team::set_role_cas(store_root, user_id, role)
        .map_err(|e| anyhow::anyhow!("setting team role failed: {}", e))
}

/// Remove `user_id`'s entry from `registry/team.toml` via the shared aida-core
/// CAS push-wins loop. Returns `Ok(true)` if an entry was removed, `Ok(false)`
/// if absent (friendly no-op). Used by `aida team unset-role` to clean stray /
/// duplicate keys. trace:STORY-654 | ai:claude
pub(crate) fn unset_role_cas(store_root: &Path, user_id: &str) -> anyhow::Result<bool> {
    aida_core::team::unset_role_cas(store_root, user_id)
        .map_err(|e| anyhow::anyhow!("removing team role failed: {}", e))
}

/// A roster member row joined with the role recorded for its user_id, for the
/// extended `aida team` view. trace:STORY-646 | ai:claude
pub(crate) fn roles_by_user(store_root: &Path) -> BTreeMap<String, String> {
    TeamRoster::load(store_root).members
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
            name: None,
            user: None,
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

    // ── STORY-646: effective_role + team.toml CAS round-trip ────────────────

    #[test]
    fn roster_role_wins_over_env() {
        // A rostered advisor stays advisor even with an implementer env set.
        let (role, src) = resolve_effective_role(Some("advisor"), Some("implementer"));
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Roster);
    }

    #[test]
    fn env_used_when_not_rostered() {
        let (role, src) = resolve_effective_role(None, Some("advisor"));
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Env);
    }

    #[test]
    fn default_when_neither_set() {
        let (role, src) = resolve_effective_role(None, None);
        assert_eq!(role, "implementer");
        assert_eq!(src, RoleSource::Default);
    }

    #[test]
    fn blank_roster_and_env_fall_through_to_default() {
        // Empty/whitespace strings are treated as unset at each tier.
        let (role, src) = resolve_effective_role(Some("  "), Some(""));
        assert_eq!(role, "implementer");
        assert_eq!(src, RoleSource::Default);
    }

    #[test]
    fn roster_role_dialog_canonicalizes_to_advisor() {
        // The deprecated `dialog` token normalizes to `advisor` like everywhere.
        let (role, src) = resolve_effective_role(Some("dialog"), None);
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Roster);
    }

    #[test]
    fn team_toml_round_trips() {
        let roster = TeamRoster::default()
            .with_role_set("alice", "advisor")
            .with_role_set("bob", "implementer");
        let toml_str = toml::to_string_pretty(&roster).unwrap();
        let parsed: TeamRoster = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.role_for("alice"), Some("advisor"));
        assert_eq!(parsed.role_for("bob"), Some("implementer"));
        assert_eq!(parsed.role_for("carol"), None);
        // `[members]` section is the on-disk shape the design doc specifies.
        assert!(toml_str.contains("[members]"), "got: {toml_str}");
        assert!(toml_str.contains("alice = \"advisor\""), "got: {toml_str}");
    }

    #[test]
    fn missing_team_toml_is_empty_roster() {
        let dir = tempfile::tempdir().unwrap();
        let roster = TeamRoster::load(dir.path());
        assert!(roster.members.is_empty());
        assert_eq!(roster.role_for("anyone"), None);
    }

    #[test]
    fn load_reads_what_was_written() {
        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        std::fs::write(
            registry.join("team.toml"),
            "[members]\nalice = \"advisor\"\n",
        )
        .unwrap();
        let roster = TeamRoster::load(dir.path());
        assert_eq!(roster.role_for("alice"), Some("advisor"));
    }

    #[test]
    fn dashboard_person_key_role_is_read_by_effective_role() {
        // The crux of STORY-653: the dashboard groups/keys on the person key
        // (`aida_core::team::person_key`), and a role written under that key is
        // exactly what `effective_role_for_user` reads back — so a role set in
        // the UI actually enforces. trace:STORY-653
        let entry = NodeRegistryEntry {
            id: "1".to_string(),
            user_id: 1, // the OLD cryptic integer the bug keyed on
            hostname: "imac".to_string(),
            email: Some("joe.mooney@gmail.com".to_string()),
            clone_path: Some(PathBuf::from("/home/joe/ai/aida")),
            name: Some("imac-joe-1".to_string()),
            user: Some("joe".to_string()),
            registered: Utc::now(),
        };
        let key = aida_core::team::person_key(&entry);
        assert_eq!(
            key, "joe",
            "person key is the owner string, not the integer"
        );

        let dir = tempfile::tempdir().unwrap();
        let registry = dir.path().join("registry");
        std::fs::create_dir_all(&registry).unwrap();
        // The UI writes the role under the person key (what TeamMemberDto.user_id
        // now carries).
        std::fs::write(
            registry.join("team.toml"),
            format!("[members]\n{key} = \"advisor\"\n"),
        )
        .unwrap();

        // effective_role_for_user, called with the same person key, reads it.
        let (role, src) = effective_role_for_user(dir.path(), &key);
        assert_eq!(role, "advisor");
        assert_eq!(src, RoleSource::Roster);

        // The OLD integer key (the bug) does NOT resolve — proving the fix is the
        // key, not luck.
        let (role_int, src_int) = effective_role_for_user(dir.path(), "1");
        assert_ne!(
            src_int,
            RoleSource::Roster,
            "integer key is not in the roster"
        );
        let _ = role_int;
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
