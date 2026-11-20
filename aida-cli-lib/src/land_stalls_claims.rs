//! STALE-CLAIM: a shared claim or a local lease that still blocks admission,
//! reads as dead, and that `aida doctor heal stale-leases` does not heal.
//!
//! The inventory is every record admission consults: the implementer and
//! reviewer claims under `coordination/leases/`, the drain and solo locks
//! under `coordination/`, and this clone's own leases. Each is asked the
//! question its own admission asks, so a record is reported for as long as
//! it blocks, however old it is. A claim blocks when its spec can still be
//! worked: nothing is admitted for a spec that is completed, rejected or
//! superseded, so a claim left on one blocks nothing.
// trace:TASK-3827 | ai:claude

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{Class, Item, Policy, ADVISOR_SEAT};
use crate::coordination::{self, Claim};
use crate::ownership::OwnerVerdict;

/// One record that reads as dead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimFact {
    pub scope: String,
    pub record: PathBuf,
    /// Tells this claim from a later one in the same record.
    pub generation: String,
    /// Why the record reads as dead.
    pub evidence: String,
    /// What its admission still refuses; `None` when it refuses nothing.
    pub blocks: Option<String>,
    /// Why the doctor does not heal it; `None` when it does.
    pub unhealable: Option<String>,
    /// Minutes it has read as dead: past its expiry, or since its last
    /// heartbeat when it has no expiry to pass.
    pub stale_mins: i64,
    /// Recorded on another host: its holder cannot be looked at from here.
    pub remote: bool,
}

pub(crate) fn classify_claims(claims: &[ClaimFact], policy: &Policy) -> Vec<Item> {
    claims
        .iter()
        .filter(|claim| {
            claim.stale_mins
                > if claim.remote {
                    policy.stale_claim_remote_mins
                } else {
                    policy.stale_claim_mins
                }
        })
        .filter_map(|claim| {
            let blocks = claim.blocks.as_deref()?;
            let why = claim.unhealable.as_deref()?;
            Some(Item {
                key: format!(
                    "{}:{}@{}",
                    Class::StaleClaim.label(),
                    claim.record.display(),
                    claim.generation
                ),
                class: Class::StaleClaim,
                pr: None,
                seat: ADVISOR_SEAT.into(),
                age_mins: Some(claim.stale_mins),
                summary: format!(
                    "the claim on {} blocks admission ({blocks}) and has read as dead for {} min ({}); `aida doctor heal stale-leases` does not heal it: {why}; record {}",
                    claim.scope,
                    claim.stale_mins,
                    claim.evidence,
                    claim.record.display()
                ),
                action: format!(
                    "confirm the holder of {} is gone, then release the record {}",
                    claim.scope,
                    claim.record.display()
                ),
            })
        })
        .collect()
}

/// A field of a shared record, as it is quoted in a routed line: on one line
/// and bounded. The store is written by every clone and lane.
fn quoted(text: &str) -> String {
    crate::land::one_line(text, 120)
}

/// Minutes `claim` has read as dead at `now`: past its expiry when it has
/// expired, else since its last heartbeat. `None` for an unreadable time.
fn stale_mins(claim: &Claim, now: DateTime<Utc>) -> Option<i64> {
    let heartbeat = DateTime::parse_from_rfc3339(claim.heartbeat_at.trim())
        .ok()?
        .with_timezone(&Utc);
    let age = (now - heartbeat).num_seconds().max(0);
    let ttl = i64::try_from(claim.ttl_secs).unwrap_or(i64::MAX);
    let dead_for = if age > ttl { age - ttl } else { age };
    Some(dead_for / 60)
}

/// Whether work can still be admitted for `scope`: `Some(false)` for a spec
/// that is closed, `None` when the scope names no spec the store knows.
fn spec_open(store: &aida_core::RequirementsStore, scope: &str) -> Option<bool> {
    use aida_core::RequirementStatus as Status;
    store.get_requirement_by_spec_id(scope.trim()).map(|spec| {
        !matches!(
            spec.status,
            Status::Completed | Status::Rejected | Status::Superseded
        )
    })
}

/// What can be seen of a shared claim's holder from this host.
enum Holder {
    /// Something still runs for it: the claim is not dead.
    Running,
    /// What is known, in words.
    NotSeen(String),
}

fn holder_of(claim: &Claim, here: &str) -> Holder {
    if claim.host != here {
        return Holder::NotSeen(format!(
            "recorded on host `{}`, so its holder cannot be looked at from `{here}`",
            quoted(&claim.host)
        ));
    }
    let Some(lease_id) = claim.lease_id.as_deref() else {
        return Holder::NotSeen("the record names no lease, so no worker can be looked up".into());
    };
    if claim.clone_path.is_empty() {
        return Holder::NotSeen("the record names no clone".into());
    }
    let clone = Path::new(&claim.clone_path);
    if crate::lane_liveness::lane_worker_may_be_running(clone, lease_id, claim.pid_namespace) {
        return Holder::Running;
    }
    match crate::leases::read_exact_lease(clone, lease_id) {
        Ok(Some(lease)) => match crate::ownership::reader::lease_owner(clone, &lease) {
            OwnerVerdict::Alive { .. } => Holder::Running,
            verdict => Holder::NotSeen(format!(
                "the owner of lease {lease_id} is {}",
                crate::ownership::reader::owner_label(&verdict)
            )),
        },
        Ok(None) => Holder::NotSeen(format!(
            "lease {lease_id} is no longer in the table of {}",
            quoted(&claim.clone_path)
        )),
        Err(why) => Holder::NotSeen(format!("lease {lease_id} could not be read ({why:#})")),
    }
}

fn shared_claim_fact(
    claim: &Claim,
    stale_reason: &str,
    root: &Path,
    store_root: &Path,
    store: &aida_core::RequirementsStore,
    here: &str,
    now: DateTime<Utc>,
) -> Option<ClaimFact> {
    // A reviewer's claim is reclaimed by age: an expired one blocks nothing.
    if claim.review_verb {
        return None;
    }
    let holder = match holder_of(claim, here) {
        Holder::Running => return None,
        Holder::NotSeen(what) => what,
    };
    // An implementer's claim is never freed by age alone.
    let blocks = (coordination::implementer_admission_refuses(claim, root)
        && spec_open(store, &claim.scope) == Some(true))
    .then(|| {
        format!(
            "implementer admission for {} refuses while it stands",
            quoted(&claim.scope)
        )
    });
    let healable = claim.claim_id.is_some() && coordination::exited_holder_is_dead(claim, now);
    let unhealable = (!healable).then(|| {
        if claim.claim_id.is_none() {
            "the record carries no claim id, and the doctor heals a shared claim only by its id"
                .to_string()
        } else if claim.host != here {
            format!(
                "it was recorded on host `{}`, and the doctor heals a claim only from its holder's host",
                quoted(&claim.host)
            )
        } else {
            "the exit of its holder could not be proven from its lease and its lane record"
                .to_string()
        }
    });
    Some(ClaimFact {
        record: coordination::claim_path(store_root, &claim.scope),
        scope: quoted(&claim.scope),
        generation: claim.started_at.clone(),
        evidence: format!("{stale_reason}; {holder}"),
        blocks,
        unhealable,
        stale_mins: stale_mins(claim, now)?,
        remote: claim.host != here,
    })
}

/// A drain or solo lock the lock's own admission still refuses although its
/// holder cannot be shown to run. The stale-lease healer never releases a
/// lock.
fn lock_claim_facts(store_root: &Path, here: &str, now: DateTime<Utc>) -> Vec<ClaimFact> {
    coordination::lock_claim_views(store_root, now)
        .into_iter()
        .filter_map(|view| {
            let claim = &view.claim;
            let evidence = coordination::claim_staleness(
                claim,
                now,
                here,
                coordination::strictly_alive,
                |_: &Claim| false,
            )?;
            Some(ClaimFact {
                scope: format!("the {} lock", quoted(&claim.scope)),
                record: view.path.clone(),
                generation: claim.started_at.clone(),
                evidence: format!(
                    "{evidence}; held by `{}` from {} on host `{}`",
                    quoted(&claim.agent),
                    quoted(&claim.clone_path),
                    quoted(&claim.host)
                ),
                blocks: view.refuses.then(|| {
                    format!(
                        "another clone asking for the {} is refused",
                        quoted(&claim.scope)
                    )
                }),
                unhealable: Some(
                    "it is a process lock, which the stale-lease healer never releases".into(),
                ),
                stale_mins: stale_mins(claim, now)?,
                remote: claim.host != here,
            })
        })
        .collect()
}

/// A lease of this clone whose holder nobody can establish: neither the
/// ownership record nor the lease table says it runs or is gone, no lane
/// worker is recorded as running for it, and the doctor has no proof to end
/// it by.
fn local_lease_facts(
    root: &Path,
    store: &aida_core::RequirementsStore,
    now: DateTime<Utc>,
    unknown: &mut Vec<String>,
) -> Vec<ClaimFact> {
    let leases = match crate::leases::list_leases_strict_all(root) {
        Ok(leases) => leases,
        Err(why) => {
            unknown.push(format!(
                "stale leases: this clone's leases could not be read ({})",
                crate::land::one_line(&why, 200)
            ));
            return Vec::new();
        }
    };
    crate::leases::classify_session_leases(root, &leases)
        .into_iter()
        .filter_map(|(lease, state, _)| {
            if state != aida_core::liveness::LeaseState::Unknown
                || lease.review_verb
                || crate::leases::is_ending_retained(&lease)
            {
                return None;
            }
            let OwnerVerdict::Unknown { why } = crate::ownership::reader::lease_owner(root, &lease)
            else {
                return None;
            };
            // The doctor ends a lease whose creator is provably gone.
            let creator_gone = lease.creator_pid.is_some_and(|pid| {
                crate::ownership::reader::local_claim_process_is_dead(
                    pid,
                    lease.creator_pid_start_time.as_deref(),
                    lease.pid_namespace,
                )
            });
            if creator_gone
                || crate::lane_liveness::lane_worker_may_be_running(
                    root,
                    &lease.id,
                    lease.pid_namespace,
                )
            {
                return None;
            }
            let idle = (now - crate::leases::lease_heartbeat(&lease))
                .num_minutes()
                .max(0);
            Some(ClaimFact {
                record: crate::leases::lease_path(root, &lease.id),
                generation: lease.started_at.to_rfc3339(),
                evidence: format!(
                    "lease {} on branch `{}`: its holder is unknown ({}); last sign of life {idle} min ago",
                    lease.id,
                    lease.branch,
                    crate::land::one_line(&why, 200)
                ),
                blocks: (spec_open(store, &lease.scope) == Some(true)).then(|| {
                    format!(
                        "a pickup of {} on this clone is refused while the lease stands",
                        lease.scope
                    )
                }),
                unhealable: Some(
                    "its holder cannot be established, and the doctor ends only a lease it can prove stale"
                        .into(),
                ),
                stale_mins: idle,
                remote: false,
                scope: lease.scope,
            })
        })
        .collect()
}

/// Every record that reads as dead at `now`, with what it blocks and whether
/// the doctor heals it. `root` is the clone the guard runs in.
pub(crate) fn claim_facts(
    root: &Path,
    store_root: &Path,
    store: &aida_core::RequirementsStore,
    now: DateTime<Utc>,
    unknown: &mut Vec<String>,
) -> Vec<ClaimFact> {
    let here = coordination::hostname();
    let (_live, stale) =
        coordination::partition_claims(coordination::list_claims(store_root), now, &here);
    let mut facts: Vec<ClaimFact> = stale
        .iter()
        .filter_map(|(claim, reason)| {
            shared_claim_fact(claim, reason, root, store_root, store, &here, now)
        })
        .collect();
    facts.extend(lock_claim_facts(store_root, &here, now));
    facts.extend(local_lease_facts(root, store, now, unknown));
    facts
}
