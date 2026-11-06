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
