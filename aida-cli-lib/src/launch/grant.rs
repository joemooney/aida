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
