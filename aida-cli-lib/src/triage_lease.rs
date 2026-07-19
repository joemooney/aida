//! Per-scope disposition / triage lease (TASK-661, ADR-3 intake gate).
//!
//! ADR-3's authority gate (`status_requires_advisor_authority` +
//! `has_advisor_authority` in `main.rs`) checks *WHO* may dispose a draft
//! (advisor role OR interactive TTY) — it does NOT check *HOW MANY* advisors
//! are disposing the same scope concurrently. Two advisor sessions can both
//! pass the WHO gate and race the same draft inbox, each disposing from its
//! own divergent session memory, neither holding the authoritative
//! source-of-truth context. The result is conflicting dispositions, double
//! approval, and a fragmented source of truth.
//!
//! This module adds the HOW-MANY half: a per-scope exclusive disposition
//! lease, the same substrate-as-bouncer pattern the queue/session model
//! already uses for *work* scope (see `SessionLease` in `main.rs`). Acquiring
//! disposition rights over a scope (default scope = whole project, slug
//! `project`) takes an exclusive lease; a second advisor attempting to
//! dispose the same scope is *refused, naming the holder*. Leases are
//! per-scope, so as subsystem-scoped advisors arrive (SPIKE-10) non-
//! overlapping scopes dispose in parallel safely.
//!
//! Reaping reuses the same PID-liveness primitive the session-lease reaper
//! uses (`process_probe::pid_is_alive`): a stale lease whose holder process is
//! dead is treated as free, so a crashed advisor cannot lock triage forever.
//!
//! trace:TASK-661 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use aida_core::fs_atomic::write_atomic;

/// Default scope slug when an advisor disposes without naming a subsystem
/// scope — the whole project. One authoritative advisor per project until
/// subsystem-scoped advisors (SPIKE-10) partition it. trace:TASK-661
pub const DEFAULT_SCOPE: &str = "project";

/// A held disposition/triage lease — one disposing advisor per scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispositionLease {
    /// Resolved scope slug (filesystem-safe; the lease file is named after
    /// it). `project` for the whole-project default.
    pub scope: String,
    /// Owner / user id of the disposing advisor (shell user identity, same
    /// resolution as the queue — `current_user_id`). Named back to a second
    /// advisor on refusal.
    pub owner: String,
    /// PID of the process that acquired the lease. Used by the reaper:
    /// a dead PID means the holder crashed and the lease is free.
    pub pid: u32,
    /// Hostname where the lease was acquired (informational on refusal).
    pub hostname: String,
    /// ISO-8601 UTC acquisition time (informational on refusal).
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// Outcome of an acquire attempt — the pure decision, free of IO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireDecision {
    /// No live holder for this scope — the requester may take it. Carries the
    /// lease to persist.
    Granted(DispositionLease),
    /// A live holder already owns this scope — refused, naming the holder.
    Refused(DispositionLease),
    /// The requester already holds this scope's lease — idempotent re-acquire.
    AlreadyHeld(DispositionLease),
}

/// Pure core of the bouncer (TASK-661): decide whether `requester` may acquire
/// the disposition lease for `scope`, given the set of *live* leases (the
/// caller has already filtered out dead-PID leases via the reaper) and a
/// freshly-minted lease describing the requester.
///
/// Pure over its inputs so it can be unit-tested without touching the
/// filesystem or probing real PIDs:
///   - no live holder            → `Granted(new_lease)`
///   - the requester is the holder → `AlreadyHeld(existing)` (idempotent)
///   - someone else holds it     → `Refused(existing)`
///
/// Ownership for the idempotent case is keyed on `(owner, pid)`: the same
/// process re-acquiring is a no-op; the same user from a *different* live
/// process is still refused (two advisor shells, same login, both disposing).
/// trace:TASK-661 | ai:claude
pub fn decide_acquire(
    new_lease: DispositionLease,
    live_leases: &[DispositionLease],
) -> AcquireDecision {
    match live_leases.iter().find(|l| l.scope == new_lease.scope) {
        None => AcquireDecision::Granted(new_lease),
        // trace:TASK-951 | ai:claude — owner equality folds case so the same
        // human re-acquiring from a shell reporting different casing is still
        // recognised as the holder.
        Some(existing)
            if aida_core::node::canonical_user_id(&existing.owner)
                == aida_core::node::canonical_user_id(&new_lease.owner)
                && existing.pid == new_lease.pid =>
        {
            AcquireDecision::AlreadyHeld(existing.clone())
        }
        Some(existing) => AcquireDecision::Refused(existing.clone()),
    }
}

/// Normalize a raw scope string into a filesystem-safe slug. Empty / absent
/// scope → [`DEFAULT_SCOPE`]. Keeps alphanumerics, lowercases, and collapses
/// every other run into a single `-`. trace:TASK-661
pub fn scope_slug(raw: Option<&str>) -> String {
    let raw = raw.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return DEFAULT_SCOPE.to_string();
    }
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        DEFAULT_SCOPE.to_string()
    } else {
        slug
    }
}

/// Directory holding disposition-lease files: `.aida/triage-leases/`.
/// Sibling of `.aida/sessions/` (the session-lease dir), runtime per-clone
/// state, gitignored by the deny-by-default `.aida/*` rule. trace:TASK-661
pub fn leases_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("triage-leases")
}

fn lease_path(project_root: &Path, slug: &str) -> PathBuf {
    leases_dir(project_root).join(format!("{slug}.toml"))
}

/// Read all disposition leases on disk (live and stale). Tolerates a missing
/// dir and malformed files (skips them) — the same forgiving read the session
/// lease loader uses. trace:TASK-661
pub fn list_all(project_root: &Path) -> Vec<DispositionLease> {
    let dir = leases_dir(project_root);
    if !dir.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(lease) = toml::from_str::<DispositionLease>(&content) {
                    out.push(lease);
                }
            }
        }
    }
    out.sort_by(|a, b| a.scope.cmp(&b.scope));
    out
}

/// Read all leases, reaping (deleting) any whose holder PID is no longer
/// alive, and return only the live set. A crashed advisor's lease is freed so
/// triage isn't locked forever. The liveness probe is injected so the core
/// is testable; production callers pass `process_probe::pid_is_alive`.
/// trace:TASK-661 | ai:claude
pub fn live_leases_reaping(
    project_root: &Path,
    is_alive: impl Fn(u32) -> bool,
) -> Vec<DispositionLease> {
    let mut live = Vec::new();
    for lease in list_all(project_root) {
        if is_alive(lease.pid) {
            live.push(lease);
        } else {
            // Best-effort reap; ignore failure (another reaper may have won).
            let _ = std::fs::remove_file(lease_path(project_root, &lease.scope));
        }
    }
    live
}

/// Persist a granted/re-acquired lease atomically. trace:TASK-661
pub fn write_lease(project_root: &Path, lease: &DispositionLease) -> Result<()> {
    let dir = leases_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let content = toml::to_string_pretty(lease)?;
    write_atomic(&lease_path(project_root, &lease.scope), &content)?;
    Ok(())
}

/// Release (delete) the lease for `slug` iff `owner` holds it. Returns
/// `Ok(true)` when a lease was removed, `Ok(false)` when none was held by
/// this owner (nothing to release / held by someone else). trace:TASK-661
pub fn release(project_root: &Path, slug: &str, owner: &str) -> Result<bool> {
    let path = lease_path(project_root, slug);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(false);
    };
    let Ok(lease) = toml::from_str::<DispositionLease>(&content) else {
        return Ok(false);
    };
    // trace:TASK-951 | ai:claude — owner match folds case so the holder can
    // release from a shell whose casing differs.
    if aida_core::node::canonical_user_id(&lease.owner) != aida_core::node::canonical_user_id(owner)
    {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(scope: &str, owner: &str, pid: u32) -> DispositionLease {
        DispositionLease {
            scope: scope.to_string(),
            owner: owner.to_string(),
            pid,
            hostname: "testhost".to_string(),
            started_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn grants_when_no_live_holder() {
        let new = lease("project", "alice", 100);
        let decision = decide_acquire(new.clone(), &[]);
        assert_eq!(decision, AcquireDecision::Granted(new));
    }

    #[test]
    fn refuses_naming_holder_when_another_advisor_holds_same_scope() {
        let held = lease("project", "alice", 100);
        let requester = lease("project", "bob", 200);
        let decision = decide_acquire(requester, std::slice::from_ref(&held));
        match decision {
            AcquireDecision::Refused(holder) => {
                assert_eq!(holder.owner, "alice", "refusal must name the holder");
                assert_eq!(holder, held);
            }
            other => panic!("expected Refused, got {other:?}"),
        }
    }

    #[test]
    fn second_advisor_same_user_different_process_is_still_refused() {
        // Two advisor shells under the same login both disposing the project
        // scope: the second is refused — same-user is not the same authority
        // unless it's the same process re-acquiring.
        let held = lease("project", "alice", 100);
        let requester = lease("project", "alice", 999);
        let decision = decide_acquire(requester, std::slice::from_ref(&held));
        assert!(matches!(decision, AcquireDecision::Refused(_)));
    }

    #[test]
    fn same_process_reacquire_is_idempotent() {
        let held = lease("project", "alice", 100);
        let again = lease("project", "alice", 100);
        let decision = decide_acquire(again, std::slice::from_ref(&held));
        assert_eq!(decision, AcquireDecision::AlreadyHeld(held));
    }

    #[test]
    fn non_overlapping_scopes_grant_concurrently() {
        // SPIKE-10 path: subsystem-scoped advisors. A lease on scope `cli`
        // does not block acquiring scope `server`.
        let cli_held = lease("cli", "alice", 100);
        let server_req = lease("server", "bob", 200);
        let decision = decide_acquire(server_req.clone(), std::slice::from_ref(&cli_held));
        assert_eq!(decision, AcquireDecision::Granted(server_req));
    }

    #[test]
    fn scope_slug_defaults_to_project() {
        assert_eq!(scope_slug(None), "project");
        assert_eq!(scope_slug(Some("")), "project");
        assert_eq!(scope_slug(Some("   ")), "project");
    }

    #[test]
    fn scope_slug_is_filesystem_safe() {
        assert_eq!(scope_slug(Some("EPIC-35")), "epic-35");
        assert_eq!(scope_slug(Some("aida-cli/src")), "aida-cli-src");
        assert_eq!(scope_slug(Some("  Foo Bar  ")), "foo-bar");
    }

    #[test]
    fn write_list_release_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let l = lease("project", "alice", 100);
        write_lease(root, &l).unwrap();

        let all = list_all(root);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0], l);

        // Wrong owner can't release.
        assert!(!release(root, "project", "bob").unwrap());
        assert_eq!(list_all(root).len(), 1);

        // Holder releases.
        assert!(release(root, "project", "alice").unwrap());
        assert!(list_all(root).is_empty());
        // Releasing again is a no-op.
        assert!(!release(root, "project", "alice").unwrap());
    }

    #[test]
    fn reaper_drops_dead_pid_leases_keeps_live() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_lease(root, &lease("project", "alice", 100)).unwrap();
        write_lease(root, &lease("cli", "bob", 200)).unwrap();

        // pid 100 dead, pid 200 alive.
        let live = live_leases_reaping(root, |pid| pid == 200);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].scope, "cli");

        // The dead lease was reaped from disk.
        let on_disk = list_all(root);
        assert_eq!(on_disk.len(), 1);
        assert_eq!(on_disk[0].scope, "cli");
    }

    #[test]
    fn crashed_holder_frees_the_scope_for_a_new_advisor() {
        // End-to-end of the reap path: a dead holder's lease is reaped, so
        // decide_acquire over the reaped (live) set grants a new advisor.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write_lease(root, &lease("project", "crashed", 100)).unwrap();

        let live = live_leases_reaping(root, |_| false); // everyone dead
        let new = lease("project", "fresh", 200);
        assert_eq!(
            decide_acquire(new.clone(), &live),
            AcquireDecision::Granted(new)
        );
    }
}
