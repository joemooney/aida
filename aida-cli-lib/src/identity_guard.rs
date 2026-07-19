//! Distinct-user identity guard — prevent queue/lease identity mixups on one
//! machine (TASK-1150).
//!
//! Background (BUG-89, TASK-951): the queue `user_id` and the disposition-lease
//! `owner` are the SHELL user identity resolved by `current_user_id`
//! (`--user` → `AIDA_USER` → `USER` → `USERNAME` → `"default"`). TASK-951 folds
//! case so `Joe` and `joe` are one person. The remaining gap: when a queue entry
//! or lease is owned by one identity but an operation runs under a *genuinely
//! different* identity on the same machine (`user-a` vs `user-b@corp.example` —
//! not merely a case variant), work can silently cross identities.
//!
//! This module is the DETECTION half: a pure predicate that decides whether two
//! identities are distinct (beyond case), and a policy layer that surfaces the
//! mismatch (warn by default, refuse when the operator opts in) instead of
//! letting the op cross identities silently. It deliberately does NOT unify
//! aliases — that is the person↔alias map (TASK-845, separate/deferred). This
//! guard only needs to *notice* distinct ids, not reconcile them.
//!
//! trace:TASK-1150 | ai:claude

use aida_core::node::canonical_user_id;

/// Pure core: are `a` and `b` DISTINCT user identities on one machine?
///
/// Returns `true` only when the two strings denote genuinely different people —
/// i.e. they differ *beyond* case and surrounding whitespace. Two ids that are
/// equal, or that fold to the same canonical form (`Joe` vs `joe`, per
/// TASK-951), are NOT distinct and return `false`.
///
/// This is the single decision the whole guard rests on, factored out pure so it
/// can be unit-tested without env vars, filesystem, or IO.
// trace:TASK-1150 | ai:claude
pub fn user_identities_distinct(a: &str, b: &str) -> bool {
    canonical_user_id(a) != canonical_user_id(b)
}

/// What the guard does when it detects a distinct-identity operation.
///
/// The safe default is [`GuardPolicy::Warn`] — this is a *detection* guard, so
/// it surfaces the mismatch loudly but does not block by default (a hard refuse
/// would break the legitimate operator who really did mean `--user <other>`).
/// Opt into refusing with `AIDA_IDENTITY_GUARD=refuse`; silence it entirely with
/// `AIDA_IDENTITY_GUARD=off`.
// trace:TASK-1150 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardPolicy {
    /// Surface the mismatch on stderr; let the op proceed. Default.
    Warn,
    /// Surface the mismatch and refuse the op (non-zero exit).
    Refuse,
    /// Suppress the guard entirely.
    Off,
}

impl GuardPolicy {
    /// Resolve the policy from `AIDA_IDENTITY_GUARD` (`warn` | `refuse` | `off`).
    /// Any unset/unrecognised value falls back to the safe [`GuardPolicy::Warn`]
    /// default.
    // trace:TASK-1150 | ai:claude
    pub fn from_env() -> Self {
        match std::env::var("AIDA_IDENTITY_GUARD")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("refuse") => GuardPolicy::Refuse,
            Some("off") => GuardPolicy::Off,
            _ => GuardPolicy::Warn,
        }
    }
}

/// The guard's decision for a single op, free of IO so it is unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Same identity (or policy `off`) — nothing to surface, proceed.
    Proceed,
    /// Distinct identities under policy `warn` — print `msg`, then proceed.
    Warn(String),
    /// Distinct identities under policy `refuse` — surface `msg` and abort.
    Refuse(String),
}

/// Pure evaluation: given the identity we are *operating as* and the identity
/// that *owns* the queue entry / lease we are about to take or mutate, plus the
/// resolved [`GuardPolicy`], decide what to do. `op` is a short human label for
/// the operation (e.g. `"queue add"`), woven into the surfaced message.
///
/// No IO, no env reads — every input is injected — so the whole matrix (same id,
/// case-variant, genuinely-distinct × warn/refuse/off) is testable directly.
// trace:TASK-1150 | ai:claude
pub fn evaluate(operating_as: &str, owner: &str, op: &str, policy: GuardPolicy) -> GuardOutcome {
    if policy == GuardPolicy::Off || !user_identities_distinct(operating_as, owner) {
        return GuardOutcome::Proceed;
    }
    let msg = format!(
        "identity mismatch on this machine: you are operating as '{operating_as}' but this \
         {op} targets '{owner}', a different user id. Work can silently cross identities. \
         If this is intentional, ignore this; otherwise use one consistent id \
         (e.g. export AIDA_USER=<name>).",
    );
    match policy {
        GuardPolicy::Warn => GuardOutcome::Warn(msg),
        GuardPolicy::Refuse => GuardOutcome::Refuse(msg),
        GuardPolicy::Off => unreachable!("handled above"),
    }
}

/// CLI enforcement wrapper: resolve the policy from the environment, evaluate the
/// guard for `operating_as` vs `owner`, and act — print a warning to stderr and
/// return `Ok(())` under the default warn policy, or `Err(..)` under refuse.
/// A same-identity (or case-variant) op is a silent no-op.
// trace:TASK-1150 | ai:claude
pub fn enforce(operating_as: &str, owner: &str, op: &str) -> anyhow::Result<()> {
    use colored::Colorize;
    match evaluate(operating_as, owner, op, GuardPolicy::from_env()) {
        GuardOutcome::Proceed => Ok(()),
        GuardOutcome::Warn(msg) => {
            eprintln!("{} {msg}", "warning:".yellow().bold());
            Ok(())
        }
        GuardOutcome::Refuse(msg) => anyhow::bail!(
            "{msg}\n(set AIDA_IDENTITY_GUARD=warn to downgrade this to a warning, \
             or AIDA_IDENTITY_GUARD=off to silence it)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the pure mismatch predicate -------------------------------------

    #[test]
    fn same_id_is_not_distinct() {
        assert!(!user_identities_distinct("user-a", "user-a"));
    }

    #[test]
    fn case_and_whitespace_variants_are_not_distinct() {
        // TASK-951 fold: same person, different shell casing/whitespace.
        assert!(!user_identities_distinct("User-A", "user-a"));
        assert!(!user_identities_distinct("  user-a  ", "user-a"));
        assert!(!user_identities_distinct(
            "User-A@Corp.Example",
            "user-a@corp.example"
        ));
    }

    #[test]
    fn genuinely_different_ids_are_distinct() {
        assert!(user_identities_distinct("user-a", "user-b"));
        // The BUG-89 hazard shape: short login vs full-address alias.
        assert!(user_identities_distinct("user-a", "user-a@corp.example"));
    }

    // --- policy resolution -----------------------------------------------

    #[test]
    fn policy_maps_recognised_values() {
        // Pure mapping check (mirrors from_env's match without mutating the
        // process env, which would race parallel tests).
        let map = |v: &str| match v.trim().to_ascii_lowercase().as_str() {
            "refuse" => GuardPolicy::Refuse,
            "off" => GuardPolicy::Off,
            _ => GuardPolicy::Warn,
        };
        assert_eq!(map("refuse"), GuardPolicy::Refuse);
        assert_eq!(map("OFF"), GuardPolicy::Off);
        assert_eq!(map("banana"), GuardPolicy::Warn);
        assert_eq!(map(""), GuardPolicy::Warn);
    }

    // --- the guard fires / no-ops per identity + policy ------------------

    #[test]
    fn guard_is_noop_on_same_identity() {
        // Even under the strictest policy, an aligned op just proceeds.
        assert_eq!(
            GuardOutcome::Proceed,
            evaluate("user-a", "user-a", "queue add", GuardPolicy::Refuse)
        );
        // Case-variant of the same person is still aligned.
        assert_eq!(
            GuardOutcome::Proceed,
            evaluate("User-A", "user-a", "lease release", GuardPolicy::Refuse)
        );
    }

    #[test]
    fn guard_warns_on_cross_identity_by_default() {
        match evaluate("user-a", "user-b", "queue add", GuardPolicy::Warn) {
            GuardOutcome::Warn(msg) => {
                assert!(msg.contains("user-a"), "names the operating identity");
                assert!(msg.contains("user-b"), "names the owner identity");
                assert!(msg.contains("queue add"), "names the op");
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn guard_refuses_on_cross_identity_when_opted_in() {
        match evaluate(
            "user-a",
            "user-b@corp.example",
            "lease release",
            GuardPolicy::Refuse,
        ) {
            GuardOutcome::Refuse(msg) => {
                assert!(msg.contains("user-b@corp.example"));
            }
            other => panic!("expected Refuse, got {other:?}"),
        }
    }

    #[test]
    fn guard_off_suppresses_even_cross_identity() {
        assert_eq!(
            GuardOutcome::Proceed,
            evaluate("user-a", "user-b", "queue add", GuardPolicy::Off)
        );
    }

    #[test]
    fn enforce_is_ok_on_same_identity_and_under_warn() {
        // Same identity → Ok regardless of ambient policy.
        assert!(enforce("user-a", "user-a", "queue add").is_ok());
    }
}
