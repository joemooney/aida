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

/// `[locking]` posture in `.aida/config.toml` (STORY-711 slice 2). Gates
/// whether [`locking_gate`] ever turns a `Refused` verdict into anything
/// visible. Fail-safe-by-DEFAULT-OFF, per the plan's "Fail-safe default is
/// opt-in per posture" decision — requiring a lock everywhere would break
/// every current solo/manual flow, so adoption is a deliberate per-project
/// opt-in, not a silent behavior change.
// trace:STORY-711 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LockingPosture {
    /// No gating at all — a `Refused` verdict is silently treated as `Allow`.
    /// The default: a project that has never configured `[locking]` sees
    /// zero behavior change.
    #[default]
    Off,
    /// A `Refused` verdict is downgraded to a warning: the commit proceeds,
    /// but the caller is told a lock mismatch was observed.
    Warn,
    /// A `Refused` verdict blocks the action.
    Enforce,
}

impl LockingPosture {
    /// Parse a config/env value into a posture. Case-insensitive; unknown
    /// values return `None` so callers can fall back to the default rather
    /// than silently mis-parsing a typo into `Off`.
    // trace:STORY-711 | ai:claude
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(LockingPosture::Off),
            "warn" => Some(LockingPosture::Warn),
            "enforce" => Some(LockingPosture::Enforce),
            _ => None,
        }
    }
}

/// The action a caller (the commit-boundary bouncer, in slice 2) should take,
/// after composing [`verify_worktree_lock`]'s verdict with the `[locking]`
/// posture.
// trace:STORY-711 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateAction {
    /// Proceed silently.
    Allow,
    /// Proceed, but tell the caller a lock mismatch was observed (names the
    /// authorizing advisor).
    Warn { by: String },
    /// Refuse to proceed (names the authorizing advisor so the caller knows
    /// who to coordinate with).
    Refuse { by: String },
}

/// Compose [`verify_worktree_lock`] with the `[locking]` posture into the
/// action a caller should take. Pure and total — no IO — so the whole truth
/// table is exhaustively unit-testable without a live agent, filesystem, or
/// config file.
///
/// `Unlocked` and `Authorized` verdicts are ALWAYS `Allow`, regardless of
/// posture — posture only matters for a `Refused` verdict:
/// - `Off` → `Allow` (a mismatch is silently waved through — the default, so
///   an unconfigured project sees zero behavior change).
/// - `Warn` → `Warn { by }` (proceed, but surface the mismatch).
/// - `Enforce` → `Refuse { by }` (block).
// trace:STORY-711 | ai:claude
pub fn locking_gate(
    worktree_lock: Option<&str>,
    my_token: Option<&str>,
    posture: LockingPosture,
) -> GateAction {
    match verify_worktree_lock(worktree_lock, my_token) {
        LockVerdict::Unlocked | LockVerdict::Authorized => GateAction::Allow,
        LockVerdict::Refused { by } => match posture {
            LockingPosture::Off => GateAction::Allow,
            LockingPosture::Warn => GateAction::Warn { by },
            LockingPosture::Enforce => GateAction::Refuse { by },
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

    // ── locking_gate (STORY-711 slice 2): every (verdict, posture) combination ──

    #[test]
    fn unlocked_is_always_allow_regardless_of_posture() {
        for posture in [
            LockingPosture::Off,
            LockingPosture::Warn,
            LockingPosture::Enforce,
        ] {
            assert_eq!(locking_gate(None, None, posture), GateAction::Allow);
            assert_eq!(
                locking_gate(None, Some("advisor-a"), posture),
                GateAction::Allow
            );
        }
    }

    #[test]
    fn authorized_is_always_allow_regardless_of_posture() {
        for posture in [
            LockingPosture::Off,
            LockingPosture::Warn,
            LockingPosture::Enforce,
        ] {
            assert_eq!(
                locking_gate(Some("advisor-a"), Some("advisor-a"), posture),
                GateAction::Allow
            );
        }
    }

    #[test]
    fn refused_under_off_posture_is_allow() {
        // The load-bearing no-op: a project with no [locking] config (posture
        // defaults Off) never turns a lock mismatch into anything visible.
        assert_eq!(
            locking_gate(Some("advisor-a"), Some("advisor-b"), LockingPosture::Off),
            GateAction::Allow
        );
        // Even a caller with NO token at all — the fail-safe Refused case —
        // is waved through under Off.
        assert_eq!(
            locking_gate(Some("advisor-a"), None, LockingPosture::Off),
            GateAction::Allow
        );
    }

    #[test]
    fn refused_under_warn_posture_warns_naming_the_holder() {
        assert_eq!(
            locking_gate(Some("advisor-a"), Some("advisor-b"), LockingPosture::Warn),
            GateAction::Warn {
                by: "advisor-a".to_string()
            }
        );
        assert_eq!(
            locking_gate(Some("advisor-a"), None, LockingPosture::Warn),
            GateAction::Warn {
                by: "advisor-a".to_string()
            }
        );
    }

    #[test]
    fn refused_under_enforce_posture_refuses_naming_the_holder() {
        assert_eq!(
            locking_gate(
                Some("advisor-a"),
                Some("advisor-b"),
                LockingPosture::Enforce
            ),
            GateAction::Refuse {
                by: "advisor-a".to_string()
            }
        );
        assert_eq!(
            locking_gate(Some("advisor-a"), None, LockingPosture::Enforce),
            GateAction::Refuse {
                by: "advisor-a".to_string()
            }
        );
    }

    #[test]
    fn posture_parse_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(LockingPosture::parse("off"), Some(LockingPosture::Off));
        assert_eq!(LockingPosture::parse("OFF"), Some(LockingPosture::Off));
        assert_eq!(LockingPosture::parse("Warn"), Some(LockingPosture::Warn));
        assert_eq!(
            LockingPosture::parse(" enforce "),
            Some(LockingPosture::Enforce)
        );
        assert_eq!(LockingPosture::parse("bogus"), None);
    }
}
