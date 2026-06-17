//! Shared read models for the team dashboard (STORY-648, slice A of EPIC-47).
//!
//! The roster (`registry/nodes.toml`), the per-user role roster
//! (`registry/team.toml`, STORY-646), and the cross-clone coordination claims
//! (`coordination/leases/*.toml` + `coordination/{drain,solo}.lock.toml`,
//! EPIC-46) are all written by aida-cli today. The web dashboard
//! (`aida-server`) is a second consumer of the SAME on-disk substrate, so this
//! module hosts the **pure read side** — the parsing + the DTOs — in aida-core
//! (which both aida-cli and aida-server depend on) so the two surfaces can't
//! drift (the recurring STORY-82 hazard).
//!
//! Everything here is best-effort and backward-compatible: an absent /
//! unreadable file yields an empty result, never an error. The dashboard shows
//! "no team data yet" instead of a 500. trace:STORY-648 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs_forge::TS;

use crate::node::NodeRegistry;

/// `registry/nodes.toml` relative to the store worktree root.
const NODES_TOML_REL: &[&str] = &["registry", "nodes.toml"];
/// `registry/team.toml` relative to the store worktree root.
const TEAM_TOML_REL: &[&str] = &["registry", "team.toml"];
/// Subdir holding one file per leased scope (`coordination/leases/<scope>.toml`).
const LEASES_SUBDIR: &[&str] = &["coordination", "leases"];
/// Subdir holding the per-repo process locks (`coordination/*.lock.toml`).
const LOCKS_SUBDIR: &str = "coordination";
/// The two per-repo process-lock file names + their human label.
const LOCK_FILES: &[(&str, &str)] = &[("drain.lock.toml", "drain"), ("solo.lock.toml", "solo")];

/// The shared per-user role roster — `registry/team.toml` on the `aida-store`
/// branch. Maps a `user_id` to a role string. This is the read-side mirror of
/// the aida-cli `TeamRoster`; the on-disk TOML shape is identical
/// (`[members]\nalice = "advisor"`). trace:STORY-646 trace:STORY-648
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TeamRoster {
    #[serde(default)]
    pub members: BTreeMap<String, String>,
}

impl TeamRoster {
    fn path(store_root: &Path) -> PathBuf {
        let mut p = store_root.to_path_buf();
        for seg in TEAM_TOML_REL {
            p.push(seg);
        }
        p
    }

    /// Load the role roster. A missing / unreadable / malformed file yields an
    /// empty roster — "no RBAC configured" is never an error. trace:STORY-648
    pub fn load(store_root: &Path) -> Self {
        let path = Self::path(store_root);
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_default()
    }

    /// The roster role for `user_id`, if recorded (raw, un-canonicalized).
    pub fn role_for(&self, user_id: &str) -> Option<&str> {
        self.members.get(user_id).map(String::as_str)
    }
}

/// Canonicalize a role string for display: the deprecated `dialog` token maps
/// to the canonical `advisor`. Other roles pass through unchanged. Mirrors the
/// aida-cli role-name normalization at the read boundary. trace:STORY-648
fn canonical_role(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else {
        raw.to_string()
    }
}

/// Load the shared node roster from `registry/nodes.toml`. An absent /
/// unreadable file yields an empty roster (not a team). trace:STORY-648
pub fn load_node_roster(store_root: &Path) -> NodeRegistry {
    let mut path = store_root.to_path_buf();
    for seg in NODES_TOML_REL {
        path.push(seg);
    }
    NodeRegistry::load(&path).unwrap_or_default()
}

/// Canonicalize a clone path to a stable comparison string (mirrors the
/// coordination claim's clone-identity key). Falls back to the lexical path.
fn canon(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// A team member for the dashboard: one row per `user_id`, with the user's
/// registered clones grouped together. trace:STORY-648 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberDto {
    /// The person's identity (the `current_user_id` / `nodes.toml` user id as a
    /// string). A user may register several clones, all grouped under this id.
    pub user_id: String,
    /// The user's role from `registry/team.toml`, if any (canonicalized).
    pub role: Option<String>,
    /// Distinct hostnames this user is registered on.
    pub hosts: Vec<String>,
    /// Absolute clone paths this user has registered.
    pub clone_paths: Vec<String>,
    /// The most recent registration timestamp across the user's clones (RFC3339).
    pub last_seen: Option<String>,
    /// One coordination claim scope this user currently holds, if any (the
    /// roster "active now" signal). The full claim set is on `/coordination`.
    pub active_claim: Option<String>,
}

/// A cross-clone coordination claim for the dashboard. Read-side mirror of the
/// aida-cli `Claim`; the on-disk TOML shape is identical. trace:STORY-648
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoordinationClaimDto {
    /// `"lease"` (a leased spec/worktree scope), `"drain"`, or `"solo"`.
    pub kind: String,
    /// The leased scope (a SPEC-ID / worktree scope) for a lease; `None` for the
    /// drain/solo process locks (their scope is the kind itself).
    pub scope: Option<String>,
    /// The node id holding the claim.
    pub holder_user: String,
    /// Host the holder runs on.
    pub host: String,
    /// Absolute path of the holding clone.
    pub clone_path: String,
    /// Agent / command context, if recorded.
    pub agent: Option<String>,
    /// RFC3339 UTC when the claim was taken.
    pub started_at: String,
    /// RFC3339 UTC of the last heartbeat.
    pub heartbeat_at: String,
    /// Seconds since the heartbeat (computed at read time).
    pub age_secs: i64,
    /// True when the heartbeat has aged past the claim's TTL (reclaimable).
    pub stale: bool,
}

/// The on-disk coordination claim TOML shape (read-only subset). Mirrors the
/// aida-cli `Claim` serialization; only the fields the dashboard surfaces are
/// kept. Unknown fields (`pid`, `process_backed`, `review_verb`, …) are ignored
/// by serde so this stays forward-compatible. trace:STORY-648 | ai:claude
#[derive(Debug, Clone, Deserialize)]
struct RawClaim {
    scope: String,
    #[serde(default)]
    node_id: String,
    #[serde(default)]
    clone_path: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    started_at: String,
    #[serde(default)]
    heartbeat_at: String,
    #[serde(default)]
    ttl_secs: u64,
}

/// Parse a coordination claim file, if present and well-formed. A missing file
/// or a parse error yields `None` (treated as "no claim"). trace:STORY-648
fn read_raw_claim(path: &Path) -> Option<RawClaim> {
    let raw = std::fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

/// Seconds since `heartbeat_at` relative to `now`. An unparseable timestamp
/// yields `0` (age unknown → not stale by age alone). trace:STORY-648
fn heartbeat_age_secs(heartbeat_at: &str, now: DateTime<Utc>) -> i64 {
    DateTime::parse_from_rfc3339(heartbeat_at)
        .ok()
        .map(|hb| {
            now.signed_duration_since(hb.with_timezone(&Utc))
                .num_seconds()
                .max(0)
        })
        .unwrap_or(0)
}

/// Build a [`CoordinationClaimDto`] from a raw claim + its kind, computing
/// age/stale against `now`. trace:STORY-648 | ai:claude
fn claim_to_dto(
    raw: RawClaim,
    kind: &str,
    scope_is_self: bool,
    now: DateTime<Utc>,
) -> CoordinationClaimDto {
    let age_secs = heartbeat_age_secs(&raw.heartbeat_at, now);
    // A claim is stale once its heartbeat ages past its TTL (the reclaim
    // backstop). A zero TTL (older / malformed record) never reads stale.
    let stale = raw.ttl_secs > 0 && age_secs > raw.ttl_secs as i64;
    CoordinationClaimDto {
        kind: kind.to_string(),
        // For lease claims `scope` is the leased SPEC/worktree; for the drain/
        // solo process locks the scope label IS the kind, so don't duplicate it.
        scope: if scope_is_self { None } else { Some(raw.scope) },
        holder_user: raw.node_id,
        host: raw.host,
        clone_path: raw.clone_path,
        agent: if raw.agent.is_empty() {
            None
        } else {
            Some(raw.agent)
        },
        started_at: raw.started_at,
        heartbeat_at: raw.heartbeat_at,
        age_secs,
        stale,
    }
}

/// List the active cross-clone coordination claims on the store: the per-scope
/// lease claims plus the drain/solo process locks. Computes age/stale at read
/// time. Returns an empty vec when no coordination tree / claims exist (never
/// an error). trace:STORY-648 | ai:claude
pub fn list_coordination_claims(store_root: &Path) -> Vec<CoordinationClaimDto> {
    list_coordination_claims_at(store_root, Utc::now())
}

/// [`list_coordination_claims`] with an injectable `now` for deterministic
/// age/stale tests. trace:STORY-648 | ai:claude
pub fn list_coordination_claims_at(
    store_root: &Path,
    now: DateTime<Utc>,
) -> Vec<CoordinationClaimDto> {
    let mut out = Vec::new();

    // Lease claims: one file per scope under coordination/leases/.
    let mut leases_dir = store_root.to_path_buf();
    for seg in LEASES_SUBDIR {
        leases_dir.push(seg);
    }
    if let Ok(entries) = std::fs::read_dir(&leases_dir) {
        let mut raws: Vec<RawClaim> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("toml"))
            .filter_map(|p| read_raw_claim(&p))
            .collect();
        raws.sort_by(|a, b| a.scope.cmp(&b.scope));
        for raw in raws {
            out.push(claim_to_dto(raw, "lease", false, now));
        }
    }

    // Process locks: coordination/drain.lock.toml + coordination/solo.lock.toml.
    let locks_dir = store_root.join(LOCKS_SUBDIR);
    for (file, kind) in LOCK_FILES {
        if let Some(raw) = read_raw_claim(&locks_dir.join(file)) {
            out.push(claim_to_dto(raw, kind, true, now));
        }
    }

    out
}

/// Build the dashboard team view: every registered clone grouped by `user_id`,
/// each row joined with the user's role + one active coordination claim it
/// holds. Best-effort — absent files yield an empty vec. trace:STORY-648
pub fn build_team_members(store_root: &Path) -> Vec<TeamMemberDto> {
    let node_roster = load_node_roster(store_root);
    let roles = TeamRoster::load(store_root);
    let claims = list_coordination_claims(store_root);

    // Group nodes by user_id (a user may have several clones).
    let mut by_user: BTreeMap<String, TeamMemberDto> = BTreeMap::new();
    for node in node_roster.nodes {
        let user_id = node.user_id.to_string();
        let host = node.hostname.clone();
        let clone_path = node
            .clone_path
            .as_ref()
            .map(|p| canon(p))
            .unwrap_or_default();
        let registered = node.registered.to_rfc3339();

        let entry = by_user
            .entry(user_id.clone())
            .or_insert_with(|| TeamMemberDto {
                user_id: user_id.clone(),
                role: roles.role_for(&user_id).map(canonical_role),
                hosts: Vec::new(),
                clone_paths: Vec::new(),
                last_seen: None,
                active_claim: None,
            });

        if !host.is_empty() && !entry.hosts.contains(&host) {
            entry.hosts.push(host);
        }
        if !clone_path.is_empty() && !entry.clone_paths.contains(&clone_path) {
            entry.clone_paths.push(clone_path.clone());
        }
        // last_seen = the most recent registration across the user's clones.
        match &entry.last_seen {
            Some(prev) if prev.as_str() >= registered.as_str() => {}
            _ => entry.last_seen = Some(registered),
        }
        // active_claim: the first coordination claim whose holding clone matches
        // one of this user's registered clones.
        if entry.active_claim.is_none() && !clone_path.is_empty() {
            if let Some(c) = claims.iter().find(|c| c.clone_path == clone_path) {
                entry.active_claim = c.scope.clone().or_else(|| Some(c.kind.clone()));
            }
        }
    }

    by_user.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn empty_store_yields_empty_views() {
        let dir = tempfile::tempdir().unwrap();
        assert!(build_team_members(dir.path()).is_empty());
        assert!(list_coordination_claims(dir.path()).is_empty());
    }

    #[test]
    fn missing_team_toml_is_empty_roster() {
        let dir = tempfile::tempdir().unwrap();
        let roster = TeamRoster::load(dir.path());
        assert!(roster.members.is_empty());
        assert_eq!(roster.role_for("anyone"), None);
    }

    #[test]
    fn dialog_role_canonicalizes_to_advisor() {
        assert_eq!(canonical_role("dialog"), "advisor");
        assert_eq!(canonical_role("advisor"), "advisor");
        assert_eq!(canonical_role("implementer"), "implementer");
    }

    #[test]
    fn members_grouped_by_user_with_roles() {
        let dir = tempfile::tempdir().unwrap();
        // Two clones for user 1, one for user 2.
        write(
            &dir.path().join("registry/nodes.toml"),
            r#"
[[nodes]]
id = "1"
user_id = 1
hostname = "imac"
clone_path = "/home/joe/ai/aida"
registered = "2026-06-17T01:00:00Z"

[[nodes]]
id = "2"
user_id = 1
hostname = "laptop"
clone_path = "/home/joe/ai/aida-b"
registered = "2026-06-17T02:00:00Z"

[[nodes]]
id = "3"
user_id = 2
hostname = "imac"
clone_path = "/home/joe/ai/aida-c"
registered = "2026-06-17T03:00:00Z"
"#,
        );
        write(
            &dir.path().join("registry/team.toml"),
            "[members]\n1 = \"advisor\"\n2 = \"implementer\"\n",
        );

        let members = build_team_members(dir.path());
        assert_eq!(members.len(), 2, "two distinct users");
        let u1 = members.iter().find(|m| m.user_id == "1").unwrap();
        assert_eq!(u1.role.as_deref(), Some("advisor"));
        assert_eq!(u1.hosts.len(), 2, "two distinct hosts");
        assert_eq!(u1.clone_paths.len(), 2, "two distinct clones");
        // last_seen is the most recent registration across the user's clones.
        assert_eq!(u1.last_seen.as_deref(), Some("2026-06-17T02:00:00+00:00"));
        let u2 = members.iter().find(|m| m.user_id == "2").unwrap();
        assert_eq!(u2.role.as_deref(), Some("implementer"));
    }

    #[test]
    fn coordination_claims_compute_age_and_stale() {
        let dir = tempfile::tempdir().unwrap();
        // A lease claim with a fresh heartbeat (within TTL) → not stale.
        write(
            &dir.path().join("coordination/leases/fr-1-abc.toml"),
            r#"
scope = "FR-1"
node_id = "2"
clone_path = "/home/joe/ai/aida-b"
host = "laptop"
pid = 4242
agent = "claude-implementer"
started_at = "2026-06-17T11:59:00Z"
heartbeat_at = "2026-06-17T11:59:00Z"
ttl_secs = 1800
process_backed = false
review_verb = false
"#,
        );
        // A drain lock whose heartbeat aged past its TTL → stale.
        write(
            &dir.path().join("coordination/drain.lock.toml"),
            r#"
scope = "drain"
node_id = "1"
clone_path = "/home/joe/ai/aida"
host = "imac"
pid = 1234
agent = "burndown run"
started_at = "2026-06-17T11:00:00Z"
heartbeat_at = "2026-06-17T11:00:00Z"
ttl_secs = 1800
process_backed = true
review_verb = false
"#,
        );

        let now = DateTime::parse_from_rfc3339("2026-06-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let claims = list_coordination_claims_at(dir.path(), now);
        assert_eq!(claims.len(), 2);

        let lease = claims.iter().find(|c| c.kind == "lease").unwrap();
        assert_eq!(lease.scope.as_deref(), Some("FR-1"));
        assert_eq!(lease.holder_user, "2");
        assert_eq!(lease.age_secs, 60);
        assert!(!lease.stale, "fresh heartbeat within TTL is not stale");
        assert_eq!(lease.agent.as_deref(), Some("claude-implementer"));

        let drain = claims.iter().find(|c| c.kind == "drain").unwrap();
        assert_eq!(drain.scope, None, "process-lock scope folds into kind");
        assert_eq!(drain.age_secs, 3600);
        assert!(drain.stale, "heartbeat older than TTL reads stale");
    }

    #[test]
    fn member_active_claim_joined_from_coordination() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("registry/nodes.toml"),
            r#"
[[nodes]]
id = "1"
user_id = 1
hostname = "imac"
clone_path = "/home/joe/ai/aida"
registered = "2026-06-17T01:00:00Z"
"#,
        );
        write(
            &dir.path().join("coordination/leases/fr-1-abc.toml"),
            r#"
scope = "FR-1"
node_id = "1"
clone_path = "/home/joe/ai/aida"
host = "imac"
pid = 4242
agent = "claude"
started_at = "2026-06-17T11:59:00Z"
heartbeat_at = "2026-06-17T11:59:00Z"
ttl_secs = 1800
"#,
        );
        let members = build_team_members(dir.path());
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].active_claim.as_deref(), Some("FR-1"));
    }
}
