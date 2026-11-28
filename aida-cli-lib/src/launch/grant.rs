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

/// The one issuance for a child whose environment is reduced to the contained
/// allowlist (which keeps `AIDA_USER` but drops `USER`). The grant is minted
/// for this launcher's identity and that same subject is exported to the
/// child, so its own grant check presents exactly the grant's subject.
/// Without the export a child of a shell with no `AIDA_USER` presents the
/// `default` fallback and is refused with `SubjectMismatch`. The production
/// contained launch and the conformance diagnostic both call this.
// trace:TASK-3933 | ai:claude
pub(crate) fn issue_contained_child(
    authority_root: &Path,
    requested_seat: &str,
    sandboxed: bool,
    command: &mut std::process::Command,
) -> Result<SeatGrant> {
    let grant = issue_launch_child(
        authority_root,
        requested_seat,
        &crate::current_user_id(None),
        sandboxed,
    )?;
    command.env("AIDA_USER", &grant.subject);
    Ok(grant)
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
    let command = review_watch_controller_command(&grant);
    match command {
        Ok(command) => Ok((grant, command)),
        Err(error) => {
            let _ = seat_authority::revoke_by_id(&grant.id);
            Err(error)
        }
    }
}

// Restart an exited controller with its existing authority, retaining ancestry
// for surviving reviewers without minting an unbounded chain of grants.
// trace:BUG-2640 | ai:codex
pub(crate) fn review_watch_controller_command(grant: &SeatGrant) -> Result<std::process::Command> {
    let mut command = super::control::service_child()?;
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
}

#[cfg(test)]
mod task_3933_tests {
    use crate::gate::seat_authority::{grant_check_for, GrantCheck, GrantRejection};
    use crate::test_env::{AmbientGuard, HERMETIC_TEST_USER};
    use std::collections::BTreeMap;

    /// What a contained child presents: `current_user_id`'s order over the
    /// environment the contained launch forwards (USER is not forwarded).
    fn presented(child: &BTreeMap<String, String>) -> String {
        ["AIDA_USER", "USER", "USERNAME"]
            .iter()
            .find_map(|key| child.get(*key).cloned())
            .unwrap_or_else(|| "default".into())
    }

    /// AC4: from a launcher shell with USER but no AIDA_USER (the dell1 agy
    /// conformance rerun), the shared contained issuer used by both the
    /// production contained launch and `conformance-launch` makes the child
    /// present exactly the grant's subject. Without its binding the same
    /// child presents the fallback and the unchanged subject check refuses
    /// it, naming both subjects only in redacted form (AC1, AC3).
    // trace:TASK-3933 | ai:claude
    #[test]
    fn task_3933_contained_child_presents_its_grant_subject() {
        let root = tempfile::tempdir().unwrap();
        let _ambient = AmbientGuard::hermetic_with_seat_and(
            root.path(),
            "advisor",
            &["implementer"],
            &[
                ("AIDA_USER", None),
                ("USER", Some(HERMETIC_TEST_USER)),
                ("USERNAME", None),
            ],
        );
        let mut command = std::process::Command::new("true");
        let grant =
            super::issue_contained_child(root.path(), "implementer", false, &mut command).unwrap();
        assert_eq!(
            grant.subject,
            crate::current_user_id(None),
            "production subject input"
        );
        let child = super::super::contained::selected_environment(&command);
        assert_eq!(presented(&child), grant.subject);
        assert!(
            matches!(
                grant_check_for(root.path(), &grant.id, &presented(&child)),
                GrantCheck::Valid(_)
            ),
            "the contained child's own grant check accepts its subject"
        );

        // Negative control: the pre-fix diagnostic issuance with no binding.
        let unbound = std::process::Command::new("true");
        let bare = super::issue_launch_child(
            root.path(),
            "implementer",
            &crate::current_user_id(None),
            false,
        )
        .unwrap();
        let child = super::super::contained::selected_environment(&unbound);
        assert_eq!(presented(&child), "default");
        let GrantCheck::Rejected { reason, .. } =
            grant_check_for(root.path(), &bare.id, &presented(&child))
        else {
            panic!("an unbound contained child must be refused");
        };
        assert!(
            matches!(reason, GrantRejection::SubjectMismatch { .. }),
            "{reason:?}"
        );
        let evidence = reason
            .subject_mismatch_evidence(&aida_core::exec_context::Environment::from_pairs([]))
            .expect("a subject mismatch names both subjects");
        assert!(
            evidence.starts_with("subject_mismatch expected=[kind=named sha256="),
            "{evidence}"
        );
        assert!(
            evidence.contains("presented=[kind=fallback sha256="),
            "{evidence}"
        );
        assert!(evidence.ends_with("source=fallback]"), "{evidence}");
        assert!(!evidence.contains(HERMETIC_TEST_USER), "{evidence}");
        assert!(!evidence.contains(&bare.id), "{evidence}");
        for id in [&grant.id, &bare.id] {
            crate::session::revoke_headless_child_grant(id);
        }
    }

    /// AC2: `conformance-launch` and the production contained launch both
    /// issue through [`super::issue_contained_child`]; the diagnostic keeps no
    /// issuance of its own.
    // trace:TASK-3933 | ai:claude
    #[test]
    fn task_3933_diagnostic_and_production_share_the_contained_issuer() {
        let agy = include_str!("agy.rs");
        let diagnostic = &agy[..agy.find("#[cfg(test)]").unwrap()];
        assert!(diagnostic.contains("super::issue_contained_child(repo, \"implementer\", true"));
        assert!(!diagnostic.contains("issue_launch_child("));
        assert!(include_str!("../session/headless.rs")
            .contains("crate::launch::issue_contained_child("));
    }
}
