//! Cross-machine person-alias registry (TASK-845, tier of EPIC-47).
//!
//! One human routinely registers under different identity strings on different
//! machines/clones — `joe` on an iMac, `joe.mooney` on another host,
//! `Joe.Mooney@gd-ms.com` on a work host, `joe.mooney@gmail.com` on a personal
//! node. Each host mints its own ids, so the queue, the team roster, and the
//! block list all show what is really one person as several distinct owners.
//!
//! [`TASK-951`](crate::node::canonical_user_id) folds the **case-variant** part
//! of this divergence (`Joe` vs `joe`) at every comparison boundary. This module
//! adds the second normalization layer: an explicit, operator-curated **map**
//! that links the genuinely-different strings (`joe` ↔ `joe.mooney@gmail.com`)
//! to one canonical person. The two layers compose — callers case-fold FIRST,
//! then alias-resolve — so the surfaces collapse a person's aliases into one
//! row/queue/owner.
//!
//! The map is a SHARED record on the orphan `aida-store` branch
//! (`registry/aliases.toml`), the same source of truth the node roster
//! (STORY-640, `registry/nodes.toml`) and the role roster (STORY-646,
//! `registry/team.toml`) live on. Writes go through a CAS push-wins loop
//! (mirroring `team::set_role_cas`); reads are best-effort (absent / unreadable
//! / malformed file → an empty map, never an error — "no aliases configured" is
//! never a failure).
//!
//! Like TASK-951, this is a COMPARISON/DISPLAY layer only: stored owner / queue
//! / assignee strings are never rewritten — callers resolve to the canonical
//! person at the equality/lookup/grouping boundary, never the value they
//! persist or print as the raw identity.
//
// trace:TASK-845 | ai:claude

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::node::canonical_user_id;

/// `registry/aliases.toml` relative to the store worktree root.
const ALIASES_TOML_REL: &[&str] = &["registry", "aliases.toml"];

/// The shared person-alias map — `registry/aliases.toml` on the `aida-store`
/// branch. Each entry maps an **alias** (a case-folded identity string a host
/// reports) to the **canonical** person key it belongs to (also case-folded).
///
/// On-disk shape (every key/value already case-folded):
///
/// ```toml
/// [aliases]
/// "joe.mooney" = "joe"
/// "joe.mooney@gd-ms.com" = "joe"
/// "joe.mooney@gmail.com" = "joe"
/// ```
///
/// The canonical person is the chosen representative of a linked set; resolving
/// any member yields it. Linking is bidirectional and idempotent (see
/// [`AliasRegistry::link`]).
// trace:TASK-845 | ai:claude
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AliasRegistry {
    /// alias (case-folded) → canonical person key (case-folded).
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

impl AliasRegistry {
    /// `registry/aliases.toml` under the store worktree root.
    fn path(store_root: &Path) -> PathBuf {
        let mut p = store_root.to_path_buf();
        for seg in ALIASES_TOML_REL {
            p.push(seg);
        }
        p
    }

    /// Load the alias map from the store. A missing / unreadable / malformed
    /// file yields an empty map — "no aliases configured" is never an error.
    // trace:TASK-845 | ai:claude
    #[cfg(feature = "native")]
    pub fn load(store_root: &Path) -> Self {
        let path = Self::path(store_root);
        let Ok(content) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&content).unwrap_or_default()
    }

    /// Resolve a RAW identity string to its canonical person, composing
    /// TASK-951's case-fold (applied first) with this alias map (applied
    /// second). When the case-folded id is a known alias, the mapped canonical
    /// person is returned; otherwise the case-folded id is returned unchanged
    /// (so a not-yet-linked identity resolves to itself). Pure — no I/O.
    ///
    /// This is the single composition point the surfaces call, so the
    /// "case-fold first, then alias-resolve" ordering lives in exactly one
    /// place.
    // trace:TASK-845 | ai:claude
    pub fn resolve(&self, raw: &str) -> String {
        let folded = canonical_user_id(raw);
        self.aliases.get(&folded).cloned().unwrap_or(folded)
    }

    /// All identity strings (aliases + their canonical) that resolve to the
    /// same canonical person as `raw`, case-folded and sorted. Always includes
    /// the canonical itself; an unlinked id yields just `[its-canonical]`. Used
    /// to gather every owner string belonging to one person (e.g. to union a
    /// person's queues or blocks).
    // trace:TASK-845 | ai:claude
    pub fn members_of(&self, raw: &str) -> Vec<String> {
        let canonical = self.resolve(raw);
        let mut out: Vec<String> = vec![canonical.clone()];
        for (alias, target) in &self.aliases {
            if *target == canonical && *alias != canonical {
                out.push(alias.clone());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Link two identity strings as the same canonical person. Bidirectional
    /// and idempotent: both inputs are case-folded; the two linked SETS are
    /// merged and re-pointed at a single canonical representative (the
    /// lexicographically smallest member of the union, so the choice is stable
    /// regardless of argument order). Any prior members of either set are
    /// re-pointed too, so chained links (`a↔b`, then `b↔c`) collapse `a`, `b`,
    /// `c` to one person. Returns whether the map changed.
    ///
    /// Self-link (`a` == `a` after folding) is a no-op.
    // trace:TASK-845 | ai:claude
    pub fn link(&mut self, a: &str, b: &str) -> bool {
        let fa = canonical_user_id(a);
        let fb = canonical_user_id(b);
        if fa.is_empty() || fb.is_empty() {
            return false;
        }
        if fa == fb {
            return false;
        }
        // Gather every member of both ids' existing sets.
        let mut members: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for seed in [&fa, &fb] {
            members.insert(seed.clone());
            // The seed's current canonical (if it's an alias) and that
            // canonical's whole set.
            let canonical = self
                .aliases
                .get(seed)
                .cloned()
                .unwrap_or_else(|| seed.clone());
            members.insert(canonical.clone());
            for (alias, target) in &self.aliases {
                if *target == canonical || *alias == canonical {
                    members.insert(alias.clone());
                    members.insert(target.clone());
                }
            }
        }
        // Canonical = lexicographically smallest member (stable, order-free).
        let canonical = members.iter().next().cloned().unwrap_or(fa);

        let before = self.aliases.clone();
        // Re-point every non-canonical member at the canonical; drop a stale
        // canonical→canonical self-entry.
        for m in &members {
            if *m == canonical {
                self.aliases.remove(m);
            } else {
                self.aliases.insert(m.clone(), canonical.clone());
            }
        }
        self.aliases != before
    }

    /// Group the recorded aliases by canonical person: canonical → its sorted
    /// alias list (excluding the canonical itself). Powers `aida identity list`.
    // trace:TASK-845 | ai:claude
    pub fn people(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (alias, canonical) in &self.aliases {
            if alias != canonical {
                out.entry(canonical.clone())
                    .or_default()
                    .push(alias.clone());
            }
        }
        for v in out.values_mut() {
            v.sort();
            v.dedup();
        }
        out
    }

    /// Save the alias map to the store worktree (`registry/aliases.toml`),
    /// creating the parent dir. The CALLER is responsible for the git
    /// add/commit/push (see [`link_cas`]).
    // trace:TASK-845 | ai:claude
    #[cfg(feature = "native")]
    pub fn save(&self, store_root: &Path) -> std::io::Result<()> {
        let path = Self::path(store_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, content)
    }
}

/// Link `a` and `b` as the same canonical person in `registry/aliases.toml` on
/// the store, with a CAS push-wins loop (mirrors `team::set_role_cas`): pull →
/// load → merge our link → save → commit → push; on a rejected push, hard-reset
/// the stale commit and retry. Solo (no `origin`) writes locally and lets the
/// next `aida push` upload. Returns whether the map changed (a redundant link
/// commits nothing).
// trace:TASK-845 | ai:claude
#[cfg(feature = "native")]
pub fn link_cas(store_root: &Path, a: &str, b: &str) -> std::io::Result<bool> {
    use crate::git_ops;

    const MAX_RETRIES: u32 = 10;
    let registry_path = AliasRegistry::path(store_root);
    if let Some(parent) = registry_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch = git_ops::current_branch(store_root).unwrap_or_else(|_| "aida-store".to_string());
    let local_only = !git_ops::has_remote(store_root, "origin");

    let io_err = |e: anyhow::Error| std::io::Error::other(e.to_string());

    for attempt in 0..MAX_RETRIES {
        // Step 1: pull latest (skip first attempt / solo).
        if attempt > 0 && !local_only {
            git_ops::pull_rebase(store_root, "origin", &branch).map_err(io_err)?;
        }

        // Step 2: load → merge our link → save. A redundant link is a no-op:
        // nothing changed → no commit.
        let mut registry = AliasRegistry::load(store_root);
        if !registry.link(a, b) {
            return Ok(false);
        }
        registry.save(store_root)?;

        // Step 3: stage + commit.
        git_ops::add(store_root, &["registry/aliases.toml"]).map_err(io_err)?;
        let msg = format!(
            "chore(registry): link identity {} = {}",
            canonical_user_id(a),
            canonical_user_id(b)
        );
        git_ops::commit(store_root, &msg).map_err(io_err)?;

        // Step 4: push (or stop here when solo).
        if local_only {
            return Ok(true);
        }
        match git_ops::push(store_root, "origin", &branch) {
            Ok(true) => return Ok(true),
            Ok(false) => {
                // Push rejected — discard our stale commit + tree so the next
                // pull --rebase applies cleanly, then retry.
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
        "could not write the identity link after {} attempts (store push kept being rejected) — \
         run `aida db sync --pull` and retry",
        MAX_RETRIES
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_resolves_to_case_folded_self() {
        let reg = AliasRegistry::default();
        // No links → resolve is just TASK-951's case-fold.
        assert_eq!(reg.resolve("Joe"), "joe");
        assert_eq!(reg.resolve("joe.mooney@gmail.com"), "joe.mooney@gmail.com");
    }

    #[test]
    fn link_makes_two_ids_resolve_to_one_person() {
        let mut reg = AliasRegistry::default();
        assert!(reg.link("joe", "joe.mooney@gmail.com"));
        // Both resolve to the same canonical person.
        assert_eq!(reg.resolve("joe"), reg.resolve("joe.mooney@gmail.com"));
        // Canonical = lexicographically smallest = "joe".
        assert_eq!(reg.resolve("joe.mooney@gmail.com"), "joe");
    }

    #[test]
    fn link_is_bidirectional_and_order_independent() {
        let mut a = AliasRegistry::default();
        a.link("joe", "joe.mooney@gmail.com");
        let mut b = AliasRegistry::default();
        b.link("joe.mooney@gmail.com", "joe");
        assert_eq!(a, b, "link is order-independent");
    }

    #[test]
    fn link_is_idempotent() {
        let mut reg = AliasRegistry::default();
        assert!(reg.link("joe", "joe.mooney"));
        // Re-linking the same pair changes nothing.
        assert!(!reg.link("joe", "joe.mooney"));
        assert!(!reg.link("joe.mooney", "joe"));
    }

    #[test]
    fn self_link_is_noop() {
        let mut reg = AliasRegistry::default();
        assert!(!reg.link("joe", "joe"));
        assert!(!reg.link("Joe", "joe")); // same after case-fold
        assert!(reg.aliases.is_empty());
    }

    #[test]
    fn chained_links_collapse_to_one_person() {
        let mut reg = AliasRegistry::default();
        reg.link("joe", "joe.mooney");
        reg.link("joe.mooney", "joe.mooney@gd-ms.com");
        reg.link("joe.mooney@gd-ms.com", "joe.mooney@gmail.com");
        // All four resolve to the single canonical person.
        let canonical = reg.resolve("joe");
        assert_eq!(reg.resolve("joe.mooney"), canonical);
        assert_eq!(reg.resolve("joe.mooney@gd-ms.com"), canonical);
        assert_eq!(reg.resolve("joe.mooney@gmail.com"), canonical);
        assert_eq!(canonical, "joe"); // smallest of the set
    }

    #[test]
    fn case_fold_and_alias_resolve_compose() {
        let mut reg = AliasRegistry::default();
        // Link is stored case-folded; resolve must fold a mixed-case input
        // FIRST, then alias-resolve.
        reg.link("joe", "Joe.Mooney@gd-ms.com");
        assert_eq!(reg.resolve("JOE.MOONEY@GD-MS.COM"), "joe");
        assert_eq!(reg.resolve("Joe"), "joe");
    }

    #[test]
    fn members_of_gathers_the_whole_person() {
        let mut reg = AliasRegistry::default();
        reg.link("joe", "joe.mooney");
        reg.link("joe", "joe.mooney@gmail.com");
        let mut members = reg.members_of("joe.mooney@gmail.com");
        members.sort();
        assert_eq!(
            members,
            vec![
                "joe".to_string(),
                "joe.mooney".to_string(),
                "joe.mooney@gmail.com".to_string(),
            ]
        );
        // An unlinked id is its own sole member.
        assert_eq!(reg.members_of("stranger"), vec!["stranger".to_string()]);
    }

    #[test]
    fn people_groups_aliases_by_canonical() {
        let mut reg = AliasRegistry::default();
        reg.link("joe", "joe.mooney");
        reg.link("joe", "joe.mooney@gmail.com");
        reg.link("anna", "anna.smith");
        let people = reg.people();
        assert_eq!(
            people.get("joe"),
            Some(&vec![
                "joe.mooney".to_string(),
                "joe.mooney@gmail.com".to_string()
            ])
        );
        assert_eq!(people.get("anna"), Some(&vec!["anna.smith".to_string()]));
    }

    #[cfg(feature = "native")]
    #[test]
    fn load_save_roundtrips_through_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = AliasRegistry::default();
        reg.link("joe", "joe.mooney@gmail.com");
        reg.save(dir.path()).unwrap();
        let loaded = AliasRegistry::load(dir.path());
        assert_eq!(reg, loaded);
        assert_eq!(loaded.resolve("joe.mooney@gmail.com"), "joe");
    }

    #[cfg(feature = "native")]
    #[test]
    fn missing_file_is_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let reg = AliasRegistry::load(dir.path());
        assert!(reg.aliases.is_empty());
        // Resolve still works (degrades to case-fold).
        assert_eq!(reg.resolve("Joe"), "joe");
    }
}
