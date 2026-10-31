//! Where slice 3's hardening plugs into the claim path, stubbed so that
//! nothing it would judge runs unjudged. The 099 sketch names two slice-3
//! checks: a per-deputy command policy (allowlist / deny list beyond the
//! bypass flags), and a submit capability or delegated grant that proves who
//! submitted a job. This deputy enforces neither, so each hook REFUSES the
//! one case it cannot judge instead of letting the job through as if the
//! check had passed:
//!
//! - a job that presents a delegation/capability is refused (it would
//!   otherwise run as an ordinary, audit-only submit);
//! - while an operator-written command policy file exists, every claim is
//!   refused (it would otherwise be silently ignored).
//!
//! Every other job passes both hooks unchanged, exactly as in slice 1.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude

use super::{JobRecord, Spool};

/// A hook's answer for one job at claim time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookVerdict {
    Pass,
    /// Refuse the job with this `refused:<code> (why)` reason.
    Refuse(String),
}

/// Slice 3: the submitter's delegated grant / submit capability.
pub(crate) fn submitter_delegation(rec: &JobRecord) -> HookVerdict {
    if rec.delegation.is_some() {
        return HookVerdict::Refuse(
            "refused:delegation-unverifiable (the job carries a submit capability or delegated grant, and this deputy cannot verify one yet)"
                .into(),
        );
    }
    HookVerdict::Pass
}

/// Slice 3: the per-deputy command policy. Any entry at the policy path,
/// readable or not, refuses the claim.
pub(crate) fn command_policy(spool: &Spool) -> HookVerdict {
    let p = spool.command_policy_path();
    if std::fs::symlink_metadata(&p).is_ok() {
        return HookVerdict::Refuse(format!(
            "refused:command-policy-unsupported ({} exists, and this deputy cannot enforce a command policy yet; remove it to run jobs)",
            p.display()
        ));
    }
    HookVerdict::Pass
}

/// Both hooks, in order; the first refusal wins.
pub(crate) fn check(spool: &Spool, rec: &JobRecord) -> HookVerdict {
    match command_policy(spool) {
        HookVerdict::Pass => submitter_delegation(rec),
        refuse => refuse,
    }
}
