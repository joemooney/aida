//! One grant issuer for launched children. No authority is inferred from cwd
//! or a role string: seat_authority revalidates the parent and its roster.
// trace:STORY-2326 | ai:codex
use crate::gate::seat_authority::{self, SeatGrant};
use anyhow::Result;
use std::path::Path;

/// Revalidate delegation and mint the child's own grant. Sandboxed children
/// require a keyring-held key; uncontained children retain the existing
/// warn-only proc binding fallback. Refusals precede child execution.
pub(crate) fn issue_launch_child(
    authority_root: &Path,
    requested_seat: &str,
    child_subject: &str,
    sandboxed: bool,
) -> Result<SeatGrant> {
    seat_authority::issue_child_for(authority_root, requested_seat, child_subject, sandboxed)
}

/// [`issue_launch_child`] for a delegator child (advisor or integrator):
/// the child inherits, or explicitly narrows, the validated parent's
/// delegable seats; worker seats never delegate (BUG-2467).
// trace:BUG-2467 STORY-2326 | ai:claude
pub(crate) fn issue_launch_child_with_delegation(
    authority_root: &Path,
    requested_seat: &str,
    child_subject: &str,
    sandboxed: bool,
    seats: &[String],
) -> Result<SeatGrant> {
    seat_authority::issue_launch_child_with_delegation(
        authority_root,
        requested_seat,
        child_subject,
        sandboxed,
        seats,
    )
}

/// Intake and clarify own this grant until their synchronous child has exited.
/// Drop also covers seat-assignment and spawn failures after minting.
// trace:STORY-2326 | ai:codex
pub(crate) struct AdvisorChildGrant(SeatGrant);

impl Drop for AdvisorChildGrant {
    fn drop(&mut self) {
        crate::session::revoke_headless_child_grant(&self.0.id);
    }
}

pub(crate) fn assign_advisor_child(
    root: &Path,
    vendor: crate::session::HeadlessVendor,
    contained: bool,
    command: &mut std::process::Command,
) -> Result<AdvisorChildGrant> {
    let grant = AdvisorChildGrant(issue_launch_child(
        root,
        "advisor",
        &crate::current_user_id(None),
        crate::session::sandboxed_child_grant(vendor, contained)?,
    )?);
    super::assign_seat(
        command,
        aida_core::launch_policy::LaunchKind::Fresh,
        super::SeatDecision::Child {
            role: "advisor",
            grant: &grant.0.id,
        },
    )?;
    Ok(grant)
}

/// A fixed host-supervised controller uses explicit persisted parent authority,
/// rather than the deputy's ambient selector. Keep identity/seat projection in
/// the shared launch owner.
// trace:STORY-2571 | ai:codex
pub(crate) fn review_watch_controller(
    root: &Path,
    parent_id: &str,
    parent_subject: &str,
) -> Result<(SeatGrant, std::process::Command)> {
    let grant = seat_authority::issue_review_watch_child(root, parent_id, parent_subject)?;
    let command = (|| {
        let mut command = super::control_child()?;
        super::assign_seat(
            &mut command,
            aida_core::launch_policy::LaunchKind::Fresh,
            super::SeatDecision::Child {
                role: "advisor",
                grant: &grant.id,
            },
        )?;
        command
            .env("AIDA_USER", &grant.subject)
            .env("AIDA_AGENT_NAME", &grant.subject);
        Ok(command)
    })();
    match command {
        Ok(command) => Ok((grant, command)),
        Err(error) => {
            let _ = seat_authority::revoke_by_id(&grant.id);
            Err(error)
        }
    }
}
