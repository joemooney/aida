//! Own-checkout guard for `aida integrate` (BUG-650 / TASK-1050, STORY-718
//! slice 2b).
//!
//! # The problem (BUG-650)
//!
//! `aida integrate` is meant to run as a persistent background loop
//! (`--watch`). When it is launched from the advisor's interactive checkout,
//! its merge drives run in that same worktree — contending the advisor's
//! `harness-worktree` lease. Slice 2a (TASK-1036) made `--watch` event-driven
//! and focus-scoped but explicitly gated UNATTENDED use on this guard.
//!
//! # The fix (option-c)
//!
//! Give the integrator its OWN checkout: a dedicated warm-pool worktree pinned
//! to the default branch under a distinct lease scope, so the integrator's
//! drives never sit in (or lease-contend) a real session's worktree. The
//! integrator never holds a feature branch — it merges PRs through the forge —
//! so a detached-HEAD pool worktree at the default ref is exactly right, and a
//! worktree (not a clone) keeps `target/` warm for the post-merge build.
//!
//! Two pieces:
//!   * [`ensure_integrator_checkout`] — acquire/reuse the dedicated worktree.
//!   * [`guard_not_shared_checkout`] — refuse to drive from (or relocate the
//!     drives out of) a checkout a live NON-integrator session occupies.
//!
//! The repo-wide drain lock (`drain_lock`, BUG-538) is untouched: it stays the
//! single merge authority. This module only changes WHERE the integrator's
//! working tree lives, not whether a second drain may run.
//!
//! trace:TASK-1050 trace:BUG-650 | ai:claude

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;

/// The integrator's dedicated worktree lease scope. Deliberately distinct from
/// any spec-id form AND from the generic `harness-worktree` scope a real
/// session takes, so the integrator's own checkout is never mistaken for — and
/// never contends — an advisor/implementer/agent session lease (BUG-650).
pub(crate) const INTEGRATOR_LEASE_SCOPE: &str = "aida-integrator";

/// Acquire (or reuse) the dedicated integrator worktree for `project_root`.
///
/// Reuse first: a previously-acquired integrator-leased pool worktree that is
/// still registered and on disk is reset to the default ref and handed back —
/// keeping its `target/` warm. The repo-wide drain lock guarantees a single
/// live integrator, so reuse needs no extra locking. Otherwise a fresh pool
/// worktree is acquired under [`INTEGRATOR_LEASE_SCOPE`]; `acquire` resets it to
/// the furthest-ahead default ref (detached HEAD), which is exactly what the
/// integrator wants — it never checks out a feature branch.
// trace:TASK-1050 trace:BUG-650 trace:STORY-714 | ai:claude
pub(crate) fn ensure_integrator_checkout(project_root: &Path) -> Result<PathBuf> {
    // Reuse an existing integrator-leased pool worktree when one is present.
    if let Ok(pool) = aida_core::worktree_pool::read_state(project_root) {
        if let Some(entry) = pool
            .entries
            .iter()
            .find(|e| e.lease_holder.as_deref() == Some(INTEGRATOR_LEASE_SCOPE) && e.path.is_dir())
        {
            let base_ref = aida_core::git_ops::furthest_ahead_default_ref(project_root)?;
            aida_core::git_ops::reset_worktree_to(&entry.path, &base_ref)
                .with_context(|| format!("reset integrator worktree {}", entry.path.display()))?;
            return Ok(entry.path.clone());
        }
    }

    // Otherwise acquire a fresh dedicated worktree under the integrator lease
    // scope. `acquire` pins it to the default ref (detached HEAD) and stamps the
    // durable lease, so a later run reuses it via the branch above.
    let opts = aida_core::worktree_pool::AcquireOptions {
        lease_holder: Some(INTEGRATOR_LEASE_SCOPE.to_string()),
        max_trees: crate::worktree_pool_config_max_trees(project_root),
        lease_ttl_secs: Some(crate::worktree_pool_config_lease_ttl_secs(project_root)),
        post_create_hooks: crate::worktree_pool_global_hooks("post_create"),
    };
    aida_core::worktree_pool::acquire(project_root, &opts)
        .context("acquire a dedicated integrator worktree (BUG-650)")
}

/// What the own-checkout guard decided about the launch cwd.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CheckoutGuard {
    /// cwd is not occupied by a live non-integrator session — drive as-is.
    Proceed,
    /// cwd is occupied by a live `<scope>` session, but a dedicated integrator
    /// checkout is available — the drives relocate into it (the spawned
    /// `aida queue work --from-pr` runs there), so they don't contend the
    /// occupant's lease.
    Relocate { occupied_by: String },
    /// cwd is occupied by a live `<scope>` session AND no dedicated checkout is
    /// available to relocate into — refuse with guidance (BUG-650 option-c).
    Refuse { message: String },
}

/// Pure decision core for [`guard_not_shared_checkout`]. `covering` is the set
/// of `(scope, is_live)` for every session lease whose worktree covers the cwd;
/// `dedicated_available` is whether a dedicated integrator checkout could be
/// provided. A live lease whose scope is anything other than the integrator's
/// own means a real session is working in this checkout (BUG-650).
// trace:TASK-1050 trace:BUG-650 | ai:claude
pub(crate) fn decide_checkout_guard(
    covering: &[(String, bool)],
    dedicated_available: bool,
) -> CheckoutGuard {
    let occupant = covering
        .iter()
        .find(|(scope, live)| *live && scope != INTEGRATOR_LEASE_SCOPE)
        .map(|(scope, _)| scope.clone());
    match occupant {
        None => CheckoutGuard::Proceed,
        Some(scope) if dedicated_available => CheckoutGuard::Relocate { occupied_by: scope },
        Some(scope) => CheckoutGuard::Refuse {
            message: shared_checkout_refusal(&scope),
        },
    }
}

/// The clear-error message for a shared-checkout refusal (BUG-650 option-c).
fn shared_checkout_refusal(scope: &str) -> String {
    format!(
        "refusing to integrate from a shared checkout: a live `{scope}` session holds this \
         worktree, so `aida integrate` would contend its harness-worktree lease (BUG-650). \
         Give the integrator its own checkout — run it from a dedicated clone or warm-pool \
         worktree — then retry."
    )
}

/// Guard `aida integrate` against driving merges from a shared checkout.
///
/// If the launch `cwd` is (or is inside) the dedicated integrator checkout, or
/// no live non-integrator session covers it, this is a no-op. Otherwise the
/// decision in [`decide_checkout_guard`] applies: relocate the drives into the
/// dedicated checkout (the caller spawns the drive there), or — when no
/// dedicated checkout is available — refuse with guidance.
// trace:TASK-1050 trace:BUG-650 | ai:claude
pub(crate) fn guard_not_shared_checkout(
    project_root: &Path,
    cwd: &Path,
    dedicated: &Path,
) -> Result<()> {
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let canon_dedicated = dedicated
        .canonicalize()
        .unwrap_or_else(|_| dedicated.to_path_buf());
    // Already in our own dedicated checkout — nothing to guard.
    if canon_cwd == canon_dedicated || canon_cwd.starts_with(&canon_dedicated) {
        return Ok(());
    }

    let covering = covering_leases(project_root, &canon_cwd);
    match decide_checkout_guard(&covering, dedicated.is_dir()) {
        CheckoutGuard::Proceed => Ok(()),
        CheckoutGuard::Relocate { occupied_by } => {
            eprintln!(
                "  {} integrate is driving merges in its own checkout {} \
                 (this one is held by a live `{}` session — BUG-650)",
                crate::glyph(crate::glyphs::Glyph::Check).green(),
                dedicated.display(),
                occupied_by,
            );
            Ok(())
        }
        CheckoutGuard::Refuse { message } => anyhow::bail!(message),
    }
}

/// Collect `(scope, is_live)` for every session lease whose worktree covers
/// `canon_cwd`. Pool leases (the integrator's own worktree reservation) live in
/// the worktree-pool registry, not in `.aida/sessions/`, so they never appear
/// here — only real session leases do.
fn covering_leases(project_root: &Path, canon_cwd: &Path) -> Vec<(String, bool)> {
    let now = chrono::Utc::now();
    let live_sessions = crate::process_probe::probe_live_claude_sessions();
    crate::list_leases(project_root)
        .into_iter()
        .filter(|l| crate::lease_covers_cwd(l, canon_cwd))
        .map(|l| {
            let live = matches!(
                crate::lease_state_for(&l, &live_sessions, now),
                crate::LeaseState::Live
            );
            (l.scope.clone(), live)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // BUG-650 option-c: a live NON-integrator session occupying the cwd, with no
    // dedicated checkout to fall back to, must REFUSE with a clear, actionable
    // error — never silently drive merges from a shared checkout.
    #[test]
    fn ensure_integrator_checkout_refuses_shared_checkout_with_clear_error() {
        let covering = vec![("STORY-718".to_string(), true)];
        let guard = decide_checkout_guard(&covering, /* dedicated_available */ false);
        match guard {
            CheckoutGuard::Refuse { message } => {
                assert!(message.contains("shared checkout"), "msg: {message}");
                assert!(
                    message.contains("STORY-718"),
                    "names the occupant: {message}"
                );
                assert!(message.contains("BUG-650"), "cites the bug: {message}");
            }
            other => panic!("expected a clear-error refusal, got {other:?}"),
        }
    }

    // With a dedicated checkout available, the same shared-checkout situation
    // RELOCATES the drives instead of refusing.
    #[test]
    fn shared_checkout_relocates_when_a_dedicated_checkout_is_available() {
        let covering = vec![("BUG-466".to_string(), true)];
        assert_eq!(
            decide_checkout_guard(&covering, true),
            CheckoutGuard::Relocate {
                occupied_by: "BUG-466".to_string()
            }
        );
    }

    // A checkout with no live session — or only a DEAD/stale lease — is not
    // shared; the integrator may drive in place.
    #[test]
    fn proceeds_when_no_live_non_integrator_session_covers_cwd() {
        assert_eq!(decide_checkout_guard(&[], false), CheckoutGuard::Proceed);
        let dead = vec![("STORY-718".to_string(), false)];
        assert_eq!(decide_checkout_guard(&dead, false), CheckoutGuard::Proceed);
    }

    // The integrator's OWN checkout takes a DISTINCT lease scope: not a spec-id
    // form, and not the generic harness-worktree scope a real session takes —
    // so a covering lease in the integrator's own scope never reads as "shared".
    #[test]
    fn integrator_checkout_takes_a_distinct_lease_scope() {
        // Distinct from the generic harness-worktree scope (the advisor's).
        assert_ne!(
            INTEGRATOR_LEASE_SCOPE,
            crate::worktree_lease::HARNESS_WORKTREE_SCOPE
        );
        // Not a spec-id form, so it can't collide with a spec-scoped lease.
        assert!(crate::worktree_lease::spec_id_from_branch(INTEGRATOR_LEASE_SCOPE).is_none());
        // A live lease in the integrator's own scope is NOT treated as a shared
        // (non-integrator) occupant.
        let own = vec![(INTEGRATOR_LEASE_SCOPE.to_string(), true)];
        assert_eq!(decide_checkout_guard(&own, false), CheckoutGuard::Proceed);
    }

    // Slice 2b must NOT weaken the repo-wide drain lock (BUG-538): a second
    // acquisition against a live first holder is still refused.
    #[test]
    fn acquire_drain_lock_still_refuses_a_second_authority() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _held = crate::drain_lock::acquire_drain_lock(root, "queue integrate").unwrap();
        let err = crate::drain_lock::acquire_drain_lock(root, "second integrate")
            .expect_err("a live drain lock must refuse the second authority");
        let msg = err.to_string();
        assert!(msg.contains("a drain is already running"), "msg: {msg}");
    }
}
