//! Read-only checks over an explicitly supplied current authority snapshot.
//!
//! These borrowed views are not another grant format. The publication owner
//! must obtain them through guarded canonical reads and separately establish
//! actor/process/assignment/roster provenance. Success here is not a grant,
//! assignment or release capability, and must be resampled at final P4.
// trace:BUG-1808 | ai:codex

use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const MAX_ACTIVE_ANCESTORS: usize = 128;

/// Projection of the existing SeatGrant, with no persistence or issuance API.
pub(super) struct GrantView<'a> {
    pub id: Uuid,
    pub actor_session_id: Uuid,
    pub principal: &'a str,
    pub subject: &'a str,
    pub selected_seat: &'a str,
    pub parent_grant_id: Option<Uuid>,
    pub delegable_seats: &'a [&'a str],
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

pub(super) struct AuthoritySnapshot<'a> {
    pub grants: BTreeMap<Uuid, GrantView<'a>>,
    /// Exact current roster ceilings, resolved before this pure check.
    pub roster: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum PreflightRefusal {
    #[error("session grant evidence is missing")]
    MissingGrant(Uuid),
    #[error("session grant identity does not match its record")]
    Identity,
    #[error("session grant belongs to another subject")]
    Subject,
    #[error("session grant ancestry is cyclic or exceeds the supported bound")]
    Ancestry,
    #[error("session grant is revoked")]
    Revoked(Uuid),
    #[error("session grant is not currently valid")]
    Time(Uuid),
    #[error("session grant seat is outside its current roster ceiling")]
    Roster(Uuid),
    #[error("session grant delegation edge is invalid")]
    DelegationEdge(Uuid),
    #[error("launch planning did not resolve all required child seats")]
    MissingSeats,
    #[error("launcher grant cannot delegate the selected child seat")]
    ChildSeat(String),
}

/// A launcher preflights every statically selected seat in one read-only pass.
/// Assigned consumption is a different operation requiring the reciprocal
/// assignment validator; it must never manufacture a delegable seat here.
pub(super) fn check_delegation_snapshot(
    snapshot: &AuthoritySnapshot<'_>,
    caller_grant: Uuid,
    caller_actor: Uuid,
    caller_subject: &str,
    required_seats: &[&str],
    now: DateTime<Utc>,
) -> Result<(), PreflightRefusal> {
    if required_seats.is_empty() || required_seats.iter().any(|seat| seat.is_empty()) {
        return Err(PreflightRefusal::MissingSeats);
    }
    let caller = lookup(snapshot, caller_grant)?;
    if caller.actor_session_id != caller_actor || caller_actor.is_nil() {
        return Err(PreflightRefusal::Identity);
    }
    if caller.subject.is_empty() || caller.subject != caller_subject {
        return Err(PreflightRefusal::Subject);
    }
    let mut visited = BTreeSet::new();
    let mut current = caller;
    loop {
        if visited.len() == MAX_ACTIVE_ANCESTORS || !visited.insert(current.id) {
            return Err(PreflightRefusal::Ancestry);
        }
        if current.revoked_at.is_some() {
            return Err(PreflightRefusal::Revoked(current.id));
        }
        if current.subject.is_empty() {
            return Err(PreflightRefusal::Subject);
        }
        if current.issued_at > now
            || current.expires_at <= now
            || current.issued_at >= current.expires_at
        {
            return Err(PreflightRefusal::Time(current.id));
        }
        if current.principal.is_empty()
            || current.selected_seat.is_empty()
            || !snapshot
                .roster
                .get(current.principal)
                .is_some_and(|seats| seats.contains(current.selected_seat))
        {
            return Err(PreflightRefusal::Roster(current.id));
        }
        let Some(parent_id) = current.parent_grant_id else {
            break;
        };
        let parent = lookup(snapshot, parent_id)?;
        if parent.principal != current.principal
            || !parent.delegable_seats.contains(&current.selected_seat)
            || current
                .delegable_seats
                .iter()
                .any(|seat| !parent.delegable_seats.contains(seat))
            || parent.issued_at > current.issued_at
            || current.expires_at > parent.expires_at
        {
            return Err(PreflightRefusal::DelegationEdge(current.id));
        }
        current = parent;
    }
    for seat in required_seats {
        if !caller.delegable_seats.contains(seat)
            || !snapshot
                .roster
                .get(caller.principal)
                .is_some_and(|seats| seats.contains(seat))
        {
            return Err(PreflightRefusal::ChildSeat((*seat).to_owned()));
        }
    }
    Ok(())
}

fn lookup<'a, 'b>(
    snapshot: &'a AuthoritySnapshot<'b>,
    id: Uuid,
) -> Result<&'a GrantView<'b>, PreflightRefusal> {
    let grant = snapshot
        .grants
        .get(&id)
        .ok_or(PreflightRefusal::MissingGrant(id))?;
    if id.is_nil() || grant.id != id || grant.actor_session_id.is_nil() {
        return Err(PreflightRefusal::Identity);
    }
    Ok(grant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const DELEGATIONS: &[&str] = &["driver", "implementer", "reviewer"];

    fn fixture(now: DateTime<Utc>) -> (AuthoritySnapshot<'static>, Uuid, Uuid, Uuid) {
        let root = Uuid::new_v4();
        let parent = Uuid::new_v4();
        let caller = Uuid::new_v4();
        let mut grants = BTreeMap::new();
        for (id, ancestor) in [(root, None), (parent, Some(root)), (caller, Some(parent))] {
            grants.insert(
                id,
                GrantView {
                    id,
                    actor_session_id: Uuid::new_v4(),
                    principal: "fixture-person",
                    subject: "fixture-child",
                    selected_seat: "driver",
                    parent_grant_id: ancestor,
                    delegable_seats: DELEGATIONS,
                    issued_at: now - Duration::minutes(1),
                    expires_at: now + Duration::minutes(1),
                    revoked_at: None,
                },
            );
        }
        (
            AuthoritySnapshot {
                grants,
                roster: BTreeMap::from([("fixture-person", DELEGATIONS.iter().copied().collect())]),
            },
            root,
            parent,
            caller,
        )
    }

    fn check(
        snapshot: &AuthoritySnapshot<'_>,
        caller: Uuid,
        now: DateTime<Utc>,
    ) -> Result<(), PreflightRefusal> {
        check_delegation_snapshot(
            snapshot,
            caller,
            snapshot.grants[&caller].actor_session_id,
            "fixture-child",
            &["implementer", "reviewer"],
            now,
        )
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn full_chain_and_all_selected_seats_are_required() {
        let now = Utc::now();
        let (mut snapshot, root, _, caller) = fixture(now);
        assert_eq!(check(&snapshot, caller, now), Ok(()));
        snapshot.grants.get_mut(&root).unwrap().revoked_at = Some(now);
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::Revoked(root))
        );
        snapshot.grants.get_mut(&root).unwrap().revoked_at = None;
        snapshot.grants.get_mut(&caller).unwrap().delegable_seats = &["implementer"];
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::ChildSeat("reviewer".into()))
        );
        // Actual mode's implementer-only plan does not require a reviewer.
        assert_eq!(
            check_delegation_snapshot(
                &snapshot,
                caller,
                snapshot.grants[&caller].actor_session_id,
                "fixture-child",
                &["implementer"],
                now
            ),
            Ok(())
        );
        snapshot.grants.get_mut(&caller).unwrap().delegable_seats = &[];
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::ChildSeat("implementer".into()))
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn missing_cyclic_or_mismatched_ancestry_never_means_root() {
        let now = Utc::now();
        let (mut snapshot, root, parent, caller) = fixture(now);
        let root_record = snapshot.grants.remove(&root).unwrap();
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::MissingGrant(root))
        );
        snapshot.grants.insert(root, root_record);
        snapshot.grants.get_mut(&root).unwrap().parent_grant_id = Some(caller);
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::Ancestry)
        );
        snapshot.grants.get_mut(&root).unwrap().parent_grant_id = None;
        snapshot.grants.get_mut(&parent).unwrap().principal = "foreign";
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::DelegationEdge(caller))
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn final_time_roster_subject_and_actor_are_not_frozen_preflight() {
        let now = Utc::now();
        let (mut snapshot, _, _, caller) = fixture(now);
        let actor = snapshot.grants[&caller].actor_session_id;
        assert_eq!(check(&snapshot, caller, now), Ok(()));
        assert_eq!(
            check(&snapshot, caller, now + Duration::minutes(1)),
            Err(PreflightRefusal::Time(caller))
        );
        assert_eq!(
            check_delegation_snapshot(
                &snapshot,
                caller,
                Uuid::new_v4(),
                "fixture-child",
                &["implementer"],
                now
            ),
            Err(PreflightRefusal::Identity)
        );
        assert_eq!(
            check_delegation_snapshot(
                &snapshot,
                caller,
                actor,
                "foreign-subject",
                &["implementer"],
                now
            ),
            Err(PreflightRefusal::Subject)
        );
        assert_eq!(
            check_delegation_snapshot(&snapshot, caller, actor, "fixture-child", &[], now),
            Err(PreflightRefusal::MissingSeats)
        );
        snapshot
            .roster
            .get_mut("fixture-person")
            .unwrap()
            .remove("driver");
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::Roster(caller))
        );
    }

    // trace:BUG-1808 | ai:codex
    #[test]
    fn ancestor_identity_and_delegation_cannot_be_amplified() {
        let now = Utc::now();
        let (mut snapshot, root, parent, caller) = fixture(now);
        assert_eq!(check(&snapshot, caller, now), Ok(()));
        snapshot.grants.get_mut(&root).unwrap().subject = "";
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::Subject)
        );
        snapshot.grants.get_mut(&root).unwrap().subject = "fixture-root";
        snapshot.grants.get_mut(&parent).unwrap().delegable_seats = &["driver"];
        assert_eq!(
            check(&snapshot, caller, now),
            Err(PreflightRefusal::DelegationEdge(caller))
        );
        snapshot.grants.get_mut(&parent).unwrap().delegable_seats = DELEGATIONS;
        assert_eq!(check(&snapshot, caller, now), Ok(()));
    }
}
