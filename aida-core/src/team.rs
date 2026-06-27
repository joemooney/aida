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

    /// Set (or replace) a user's role.
    fn with_role_set(mut self, user_id: &str, role: &str) -> Self {
        self.members.insert(user_id.to_string(), role.to_string());
        self
    }

    /// Migrate stray integer role keys (the STORY-653 bug: the dashboard wrote
    /// roles under the node's integer `user_id`, e.g. `1 = "advisor"`, but
    /// `effective_role`/queues/assignees key on the person-key string) to the
    /// person key, using the node roster to map `user_id → owner()`.
    ///
    /// Conservative: only a key that is a bare integer AND maps to exactly one
    /// distinct non-integer person key in the roster is migrated; the new key is
    /// only taken if it isn't already present (never clobbers a real role).
    /// Returns whether anything changed. An ambiguous / unmappable integer is
    /// left untouched (no data loss). trace:STORY-653 | ai:claude
    fn migrate_integer_keys(&mut self, nodes: &NodeRegistry) -> bool {
        use std::collections::BTreeSet;
        // Map each integer user_id (as a string) to the set of distinct person
        // keys it resolves to across the node roster.
        let mut int_to_persons: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for node in &nodes.nodes {
            let person = node.owner();
            // Only useful when the owner differs from the bare integer (i.e. the
            // node actually carries a user/email we can remap to).
            if person != node.user_id.to_string() {
                int_to_persons
                    .entry(node.user_id.to_string())
                    .or_default()
                    .insert(person);
            }
        }

        let mut changed = false;
        let int_keys: Vec<String> = self
            .members
            .keys()
            .filter(|k| k.parse::<u64>().is_ok())
            .cloned()
            .collect();
        for int_key in int_keys {
            let Some(persons) = int_to_persons.get(&int_key) else {
                continue; // can't determine the person → leave it (no data loss)
            };
            if persons.len() != 1 {
                continue; // ambiguous → leave it
            }
            let person = persons.iter().next().unwrap().clone();
            if self.members.contains_key(&person) {
                continue; // a real role already exists under the person key
            }
            if let Some(role) = self.members.remove(&int_key) {
                self.members.insert(person, role);
                changed = true;
            }
        }
        changed
    }
}

/// The guardrail-not-security caveat surfaced wherever a role is written.
///
/// The `aida-store` branch is a shared git branch — anyone with push access can
/// edit any YAML directly, so the role roster can never be an access-control
/// boundary. What it CAN be: a guardrail against *accidents* (an implementer
/// accidentally approving a spec), an encoding of team structure, and an audit
/// signal (bypasses show up in git history). Both `aida team set-role` and the
/// REST `PUT /api/v2/team/:user/role` endpoint surface this so the operator is
/// never misled about what the roster guarantees. trace:STORY-650 | ai:claude
pub const ROLE_GUARDRAIL_CAVEAT: &str =
    "Guardrail, not security: this records team structure and stops accidental \
     role-violating edits via the CLI/UI, but anyone with push access to the store can \
     still edit any spec directly with raw git. It is NOT an access-control boundary.";

/// The core role names a role write is always allowed to record. Project- or
/// machine-installed role files (`~/.aida/roles/`) add to this set; the CLI's
/// `known_role_names()` layers those on. The server validates against this core
/// set (it has no view of the caller's local roles dir). trace:STORY-650
pub fn core_role_names() -> [&'static str; 3] {
    ["advisor", "implementer", "human"]
}

/// Write `user_id = role` into `registry/team.toml` on the store with a
/// CAS push-wins loop (mirrors `git_ops::register_node_full` and the aida-cli
/// `set_role_cas`): pull → load → merge our edit → save → commit → push; on a
/// rejected push, hard-reset the stale commit and retry. Solo (no `origin`)
/// writes locally and lets the next `aida push` upload. The CALLER is
/// responsible for validating/canonicalizing `role` first. trace:STORY-650
pub fn set_role_cas(store_root: &Path, user_id: &str, role: &str) -> std::io::Result<()> {
    use crate::git_ops;

    const MAX_RETRIES: u32 = 10;
    let registry_path = TeamRoster::path(store_root);
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "main".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");

    let io_err = |e: anyhow::Error| std::io::Error::other(e.to_string());

    for attempt in 0..MAX_RETRIES {
        // Step 1: pull latest (skip first attempt / solo).
        if attempt > 0 && !local_only {
            git_ops::pull_rebase(store_root, "origin", &branch).map_err(io_err)?;
        }

        // Step 2: load → migrate any stray integer keys to the person key →
        // merge our edit → save. The migration self-heals the STORY-653 bug
        // (roles written under the cryptic integer user_id) opportunistically on
        // the next role write. trace:STORY-653
        let mut roster = TeamRoster::load(store_root);
        let nodes = load_node_roster(store_root);
        roster.migrate_integer_keys(&nodes);
        let roster = roster.with_role_set(user_id, role);
        let content = toml::to_string_pretty(&roster)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&registry_path, content)?;

        // Step 3: stage + commit.
        git_ops::add(store_root, &["registry/team.toml"]).map_err(io_err)?;
        let msg = format!("chore(registry): set team role {} = {}", user_id, role);
        git_ops::commit(store_root, &msg).map_err(io_err)?;

        // Step 4: push (or stop here when solo).
        if local_only {
            return Ok(());
        }
        match git_ops::push(store_root, "origin", &branch) {
            Ok(true) => return Ok(()),
            Ok(false) => {
                // Push rejected — someone else wrote first. Discard our stale
                // commit + tree so the next pull --rebase applies cleanly.
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                continue;
            }
            Err(e) => return Err(io_err(e)),
        }
    }
    Err(std::io::Error::other(format!(
        "could not write the team role after {} attempts (store push kept being rejected) — \
         run `aida db sync --pull` and retry",
        MAX_RETRIES
    )))
}

/// Remove `user_id`'s entry from `registry/team.toml` with the same CAS
/// push-wins loop as [`set_role_cas`]. Used by `aida team unset-role` to clean
/// stray / duplicate keys (e.g. the orphaned integer `1` from a pre-STORY-653
/// roster). Returns `Ok(true)` if a member entry was removed, `Ok(false)` if
/// the user wasn't present (a friendly no-op — no commit is made). Solo (no
/// `origin`) writes locally and lets the next `aida push` upload.
// trace:STORY-654 | ai:claude
pub fn unset_role_cas(store_root: &Path, user_id: &str) -> std::io::Result<bool> {
    use crate::git_ops;

    const MAX_RETRIES: u32 = 10;
    let registry_path = TeamRoster::path(store_root);
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "main".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");

    let io_err = |e: anyhow::Error| std::io::Error::other(e.to_string());

    for attempt in 0..MAX_RETRIES {
        // Step 1: pull latest (skip first attempt / solo).
        if attempt > 0 && !local_only {
            git_ops::pull_rebase(store_root, "origin", &branch).map_err(io_err)?;
        }

        // Step 2: load → remove the key. No-op (and no commit) if absent.
        let mut roster = TeamRoster::load(store_root);
        if roster.members.remove(user_id).is_none() {
            return Ok(false);
        }
        let content = toml::to_string_pretty(&roster)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&registry_path, content)?;

        // Step 3: stage + commit.
        git_ops::add(store_root, &["registry/team.toml"]).map_err(io_err)?;
        let msg = format!("chore(registry): unset team role for {}", user_id);
        git_ops::commit(store_root, &msg).map_err(io_err)?;

        // Step 4: push (or stop here when solo).
        if local_only {
            return Ok(true);
        }
        match git_ops::push(store_root, "origin", &branch) {
            Ok(true) => return Ok(true),
            Ok(false) => {
                // Push rejected — discard our stale commit + tree so the next
                // pull --rebase applies cleanly.
                let _ = std::process::Command::new("git")
                    .args(["reset", "--hard", "HEAD~1"])
                    .current_dir(store_root)
                    .output();
                continue;
            }
            Err(e) => return Err(io_err(e)),
        }
    }
    Err(std::io::Error::other(format!(
        "could not remove the team role after {} attempts (store push kept being rejected) — \
         run `aida db sync --pull` and retry",
        MAX_RETRIES
    )))
}

/// Canonicalize a role string for display: the deprecated `dialog` token maps
/// to the canonical `advisor`. Other roles pass through unchanged. Mirrors the
/// aida-cli role-name normalization at the read boundary. trace:STORY-648
pub fn canonical_role(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("dialog") {
        "advisor".to_string()
    } else {
        raw.trim().to_string()
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

/// A team member for the dashboard: one row per **person identity**, with the
/// person's registered clones grouped together. trace:STORY-648 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberDto {
    /// The person's identity — the **person key** that roles / queues /
    /// assignees all key on: the node owner `$USER` string captured at
    /// registration (STORY-652's node `user` field = `current_user_id`),
    /// falling back to the email local-part, then the integer `user_id` for
    /// old nodes that lack it. This MATCHES the `registry/team.toml` role key,
    /// `effective_role_for_user`, and the spec `assignee` field — so a role or
    /// assignment set in the UI under this id actually enforces. A person may
    /// register several clones, all grouped under this id. trace:STORY-653
    pub user_id: String,
    /// A friendly display label for the person — their email if recorded, else
    /// the person key. Shown in the UI instead of the cryptic integer id.
    /// trace:STORY-653 | ai:claude
    pub display_label: String,
    /// The friendly node names (STORY-652) for this person's registered clones,
    /// e.g. `["imac-joe-1", "spock-joe-2"]`. trace:STORY-653 | ai:claude
    pub node_names: Vec<String>,
    /// The person's role from `registry/team.toml`, if any (canonicalized),
    /// resolved via the person key. trace:STORY-653
    pub role: Option<String>,
    /// Distinct hostnames this person is registered on.
    pub hosts: Vec<String>,
    /// Absolute clone paths this person has registered.
    pub clone_paths: Vec<String>,
    /// The most recent registration timestamp across the person's clones (RFC3339).
    pub last_seen: Option<String>,
    /// One coordination claim scope this person currently holds, if any (the
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

/// The person identity for a node — the key that roles / queues / assignees all
/// key on. Delegates to [`NodeRegistryEntry::owner`] (STORY-652): the registered
/// owner `$USER` string if present, else the email local-part, else the integer
/// `user_id` stringified for pre-STORY-652 rows. This is the join key between the
/// dashboard team layer and `effective_role_for_user` / the spec assignee, fixing
/// the integer-vs-string mismatch (STORY-653). trace:STORY-653 | ai:claude
pub fn person_key(node: &crate::node::NodeRegistryEntry) -> String {
    // trace:STORY-653 | ai:claude
    node.owner()
}

/// Build the dashboard team view: every registered clone grouped by the
/// **person key** (the owner identity that roles/queues/assignees use), each row
/// joined with that person's role + one active coordination claim it holds, plus
/// a friendly display label and the person's node names. Best-effort — absent
/// files yield an empty vec. trace:STORY-648 trace:STORY-653
pub fn build_team_members(store_root: &Path) -> Vec<TeamMemberDto> {
    let node_roster = load_node_roster(store_root);
    let roles = TeamRoster::load(store_root);
    let claims = list_coordination_claims(store_root);

    // Group nodes by the person key (a person may have several clones, possibly
    // with different integer user_ids across machines). trace:STORY-653
    let mut by_user: BTreeMap<String, TeamMemberDto> = BTreeMap::new();
    for node in node_roster.nodes {
        let user_id = person_key(&node);
        let node_name = node.display_name();
        let host = node.hostname.clone();
        let email = node.email.clone();
        let clone_path = node
            .clone_path
            .as_ref()
            .map(|p| canon(p))
            .unwrap_or_default();
        let registered = node.registered.to_rfc3339();

        // trace:TASK-951 | ai:claude — dedup the roster on the case-folded person
        // key so the same human whose clones report `Joe` on one machine and `joe`
        // on another collapses into ONE row. The DTO keeps the first-seen original
        // casing for display + role lookup; only the GROUPING key is folded.
        let dedup_key = crate::node::canonical_user_id(&user_id);
        let entry = by_user.entry(dedup_key).or_insert_with(|| TeamMemberDto {
            user_id: user_id.clone(),
            // Default the display label to the person key; a node carrying an
            // email upgrades it below (first email wins).
            display_label: user_id.clone(),
            node_names: Vec::new(),
            role: roles.role_for(&user_id).map(canonical_role),
            hosts: Vec::new(),
            clone_paths: Vec::new(),
            last_seen: None,
            active_claim: None,
        });

        // Prefer an email as the friendly display label (e.g.
        // `joe.mooney@gmail.com`); fall back to the person key. trace:STORY-653
        if entry.display_label == entry.user_id {
            if let Some(e) = email.as_deref().filter(|s| !s.is_empty()) {
                entry.display_label = e.to_string();
            }
        }
        if !node_name.is_empty() && !entry.node_names.contains(&node_name) {
            entry.node_names.push(node_name);
        }
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
        // Two clones for user 1, one for user 2. No `user`/`email` field → the
        // person key falls back to the integer user_id (pre-STORY-652 rows).
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
        // Node names are backfilled from host/owner/id for pre-STORY-652 rows.
        assert_eq!(u1.node_names.len(), 2, "two node names");
        // No email → display label falls back to the person key.
        assert_eq!(u1.display_label, "1");
        // last_seen is the most recent registration across the user's clones.
        assert_eq!(u1.last_seen.as_deref(), Some("2026-06-17T02:00:00+00:00"));
        let u2 = members.iter().find(|m| m.user_id == "2").unwrap();
        assert_eq!(u2.role.as_deref(), Some("implementer"));
    }

    #[test]
    fn person_key_prefers_owner_string_then_email_then_integer() {
        use crate::node::NodeRegistryEntry;
        use chrono::Utc;
        let base = NodeRegistryEntry {
            id: "1".to_string(),
            user_id: 7,
            hostname: "imac".to_string(),
            email: None,
            clone_path: None,
            name: None,
            user: None,
            registered: Utc::now(),
        };
        // Owner string wins.
        let mut n = base.clone();
        n.user = Some("joe".to_string());
        n.email = Some("joe.mooney@gmail.com".to_string());
        assert_eq!(person_key(&n), "joe");
        // No owner string → email local-part.
        let mut n = base.clone();
        n.email = Some("joe.mooney@gmail.com".to_string());
        assert_eq!(person_key(&n), "joe.mooney");
        // Neither → integer user_id.
        assert_eq!(person_key(&base), "7");
    }

    #[test]
    fn members_keyed_on_owner_string_with_email_label_and_node_names() {
        let dir = tempfile::tempdir().unwrap();
        // Two machines for the same human "joe" but DIFFERENT integer user_ids
        // (joe@imac vs joe@spock). The person key (owner string) collapses them
        // into one row — and the role keyed on "joe" is read back. trace:STORY-653
        write(
            &dir.path().join("registry/nodes.toml"),
            r#"
[[nodes]]
id = "1"
user_id = 1
user = "joe"
name = "imac-joe-1"
email = "joe.mooney@gmail.com"
hostname = "imac"
clone_path = "/home/joe/ai/aida"
registered = "2026-06-17T01:00:00Z"

[[nodes]]
id = "2"
user_id = 5
user = "joe"
name = "spock-joe-2"
hostname = "spock"
clone_path = "/home/joe/ai/aida-b"
registered = "2026-06-17T02:00:00Z"
"#,
        );
        // Role written under the person key "joe" (as the UI now does).
        write(
            &dir.path().join("registry/team.toml"),
            "[members]\njoe = \"advisor\"\n",
        );

        let members = build_team_members(dir.path());
        assert_eq!(members.len(), 1, "one person across two machines");
        let m = &members[0];
        assert_eq!(m.user_id, "joe", "keyed on the person key, not the integer");
        assert_eq!(
            m.role.as_deref(),
            Some("advisor"),
            "role keyed on the person key resolves"
        );
        assert_eq!(
            m.display_label, "joe.mooney@gmail.com",
            "email is the label"
        );
        assert_eq!(m.node_names, vec!["imac-joe-1", "spock-joe-2"]);
        assert_eq!(m.hosts.len(), 2);
    }

    // trace:TASK-951 | ai:claude — case-variant person keys (`Joe` vs `joe`) for
    // one human across two machines collapse into a single roster row.
    #[test]
    fn members_dedup_across_case_variant_person_keys() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("registry/nodes.toml"),
            r#"
[[nodes]]
id = "1"
user_id = 1
user = "Joe"
name = "imac-joe-1"
hostname = "imac"
clone_path = "/home/joe/ai/aida"
registered = "2026-06-17T01:00:00Z"

[[nodes]]
id = "2"
user_id = 5
user = "joe"
name = "spock-joe-2"
hostname = "spock"
clone_path = "/home/joe/ai/aida-b"
registered = "2026-06-17T02:00:00Z"
"#,
        );

        let members = build_team_members(dir.path());
        assert_eq!(
            members.len(),
            1,
            "Joe and joe are one human — folded to a single roster row"
        );
        let m = &members[0];
        // The first-seen original casing is preserved for display (not lowercased).
        assert_eq!(m.user_id, "Joe", "display keeps first-seen original casing");
        assert_eq!(m.hosts.len(), 2, "both machines' hosts roll up");
        assert_eq!(m.node_names, vec!["imac-joe-1", "spock-joe-2"]);
    }

    #[test]
    fn migrate_integer_keys_remaps_stray_integer_to_person_key() {
        use crate::node::{NodeRegistry, NodeRegistryEntry};
        use chrono::Utc;
        let mut nodes = NodeRegistry::default();
        nodes.nodes.push(NodeRegistryEntry {
            id: "1".to_string(),
            user_id: 1,
            hostname: "imac".to_string(),
            email: None,
            clone_path: None,
            name: None,
            user: Some("joe".to_string()),
            registered: Utc::now(),
        });

        // Stray integer key (the bug) → migrates to "joe".
        let mut roster = TeamRoster::default().with_role_set("1", "advisor");
        assert!(roster.migrate_integer_keys(&nodes));
        assert_eq!(roster.role_for("joe"), Some("advisor"));
        assert_eq!(roster.role_for("1"), None);

        // Idempotent: a second pass changes nothing.
        assert!(!roster.migrate_integer_keys(&nodes));

        // Ambiguous (one integer → two persons) is left untouched (no data loss).
        let mut nodes2 = nodes.clone();
        nodes2.nodes.push(NodeRegistryEntry {
            id: "2".to_string(),
            user_id: 1,
            hostname: "spock".to_string(),
            email: None,
            clone_path: None,
            name: None,
            user: Some("joey".to_string()),
            registered: Utc::now(),
        });
        let mut roster2 = TeamRoster::default().with_role_set("1", "advisor");
        assert!(!roster2.migrate_integer_keys(&nodes2));
        assert_eq!(roster2.role_for("1"), Some("advisor"));

        // A pre-existing person-key role is never clobbered.
        let mut roster3 = TeamRoster::default()
            .with_role_set("1", "advisor")
            .with_role_set("joe", "implementer");
        assert!(!roster3.migrate_integer_keys(&nodes));
        assert_eq!(roster3.role_for("joe"), Some("implementer"));
        assert_eq!(roster3.role_for("1"), Some("advisor"));
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

    /// STORY-654: `unset_role_cas` removes the member entry (local-only repo, so
    /// no push) and is a friendly no-op when the user is absent. The classic
    /// case: cleaning the orphaned integer "1" key while a real "joe" stays.
    /// trace:STORY-654 | ai:claude
    #[test]
    fn unset_role_removes_key_and_noop_when_absent() {
        use crate::git_ops;
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path();
        // A local-only git store (no origin) so the CAS loop writes + commits
        // locally and skips the push.
        git_ops::init(store).unwrap();
        git_ops::configure_user(store, "Test", "test@example.com").unwrap();
        write(
            &store.join("registry/team.toml"),
            "[members]\njoe = \"advisor\"\n1 = \"advisor\"\n",
        );

        // Remove the stray integer key.
        let removed = unset_role_cas(store, "1").unwrap();
        assert!(removed, "the stray '1' key existed → removed");
        let roster = TeamRoster::load(store);
        assert!(!roster.members.contains_key("1"), "stray key gone");
        assert_eq!(
            roster.role_for("joe"),
            Some("advisor"),
            "real role preserved"
        );

        // Removing again (now absent) is a friendly no-op.
        let removed_again = unset_role_cas(store, "1").unwrap();
        assert!(!removed_again, "absent key → no-op");

        // Removing a user that was never present is a no-op too.
        assert!(!unset_role_cas(store, "nobody").unwrap());
    }

    /// STORY-654: round-trip — set a role, then unset it, leaving an empty
    /// roster. trace:STORY-654 | ai:claude
    #[test]
    fn set_then_unset_role_roundtrip() {
        use crate::git_ops;
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path();
        git_ops::init(store).unwrap();
        git_ops::configure_user(store, "Test", "test@example.com").unwrap();

        set_role_cas(store, "alice", "implementer").unwrap();
        assert_eq!(
            TeamRoster::load(store).role_for("alice"),
            Some("implementer")
        );

        assert!(unset_role_cas(store, "alice").unwrap());
        assert!(TeamRoster::load(store).members.is_empty());
    }
}
