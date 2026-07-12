//! Advisor-directed worktree lock — pure model + verifier (STORY-711 slice 1,
//! generalizes BUG-637).
//!
//! Binds a **worktree** to an authorizing **advisor** so an implementer agent
//! can verify-or-refuse before it acts. The full design (why a lease field and
//! not a new `.aida/locks/` dir, the two-slice split, the fail-safe posture)
//! is `docs/plans/2026-07-12-story-711-advisor-lock.md`.
//!
//! This module is the **pure core only**: given the `authorized_by` value
//! recorded on a worktree's lease (if any) and the token the calling agent
//! carries, decide the verdict. It touches no filesystem and no process —
//! callers (the `aida lock` CLI in slice 1; an automatic pre-work gate in
//! slice 2) read the lease, then hand both values to [`verify_worktree_lock`].
//!
//! Slice 1 ships this verifier plus a *manual* `aida lock` CLI that can call
//! it — nothing today calls it automatically, so this module changes zero
//! existing behavior. trace:STORY-711 | ai:claude

/// The result of checking a worktree's lock against the calling agent's token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockVerdict {
    /// The worktree carries no `authorized_by` lock — nothing to check
    /// against. Slice 1 never gates on this; it's informational.
    Unlocked,
    /// The worktree is locked, and the caller's token matches the authorizing
    /// advisor — proceed.
    Authorized,
    /// The worktree is locked by a DIFFERENT advisor than the caller's token
    /// (or the caller carries no token at all — fail-safe: an agent that
    /// cannot confirm authorization refuses). Carries the authorizing
    /// advisor's id so the refusal can name who to ask.
    Refused { by: String },
}

/// Decide the lock verdict for a worktree.
///
/// `authorized_by` is the lease's recorded `authorized_by` value (`None` when
/// the worktree carries no lock). `my_token` is the calling agent's
/// authorization token (e.g. from a brief or its role-context snapshot;
/// `None` when the agent carries none).
///
/// Four arms, fail-safe by construction:
///   - no lock                              → `Unlocked`
///   - locked, token matches                → `Authorized`
///   - locked, token is a DIFFERENT advisor  → `Refused { by }`
///   - locked, caller has no token at all    → `Refused { by }` (fail-safe:
///     an agent that cannot prove authorization is treated the same as one
///     proven wrong, never waved through by default)
///
/// Pure and total — no IO, so it's exhaustively unit-testable without a live
/// agent or filesystem.
// trace:STORY-711 | ai:claude
pub fn verify_worktree_lock(authorized_by: Option<&str>, my_token: Option<&str>) -> LockVerdict {
    let Some(locked_by) = authorized_by.filter(|s| !s.is_empty()) else {
        return LockVerdict::Unlocked;
    };
    match my_token.filter(|s| !s.is_empty()) {
        Some(token) if token == locked_by => LockVerdict::Authorized,
        _ => LockVerdict::Refused {
            by: locked_by.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_lock_is_unlocked_regardless_of_token() {
        assert_eq!(verify_worktree_lock(None, None), LockVerdict::Unlocked);
        assert_eq!(
            verify_worktree_lock(None, Some("advisor-a")),
            LockVerdict::Unlocked
        );
    }

    #[test]
    fn matching_token_is_authorized() {
        assert_eq!(
            verify_worktree_lock(Some("advisor-a"), Some("advisor-a")),
            LockVerdict::Authorized
        );
    }

    #[test]
    fn mismatched_token_is_refused_naming_the_holder() {
        let v = verify_worktree_lock(Some("advisor-a"), Some("advisor-b"));
        assert_eq!(
            v,
            LockVerdict::Refused {
                by: "advisor-a".to_string()
            }
        );
    }

    #[test]
    fn missing_token_under_a_present_lock_is_refused_fail_safe() {
        // An agent that cannot confirm authorization refuses — it must NOT be
        // waved through just because it carries no token at all.
        let v = verify_worktree_lock(Some("advisor-a"), None);
        assert_eq!(
            v,
            LockVerdict::Refused {
                by: "advisor-a".to_string()
            }
        );
    }

    #[test]
    fn empty_string_lock_value_treated_as_unlocked() {
        // Defensive: an empty `authorized_by` (should never be written, but a
        // hand-edited or legacy-migrated lease could carry one) is treated
        // the same as absent — never a lock nobody can ever match.
        assert_eq!(verify_worktree_lock(Some(""), None), LockVerdict::Unlocked);
    }

    #[test]
    fn empty_string_token_is_never_a_match() {
        let v = verify_worktree_lock(Some("advisor-a"), Some(""));
        assert_eq!(
            v,
            LockVerdict::Refused {
                by: "advisor-a".to_string()
            }
        );
    }
}
