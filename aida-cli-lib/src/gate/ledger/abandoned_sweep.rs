//! BUG-2786 slice 1: a delivery-pending barrier (`sweep_pending`) whose
//! reviewer session has ended is not left standing as a provisional
//! request-changes. Once it is older than [`SWEEP_ABANDON_AFTER`] and the
//! session registry shows its reviewer gone, it is finalized when the
//! barrier still carries a complete approval (every finding deferred), and
//! otherwise expired as `sweep_abandoned`: a record that reads as "needs
//! review", never as approval or request-changes. Run from the review
//! watcher's per-change pass; no daemon of its own.
// trace:BUG-2786 | ai:claude
// mandate-gate

use super::{store, store_root, ChangeKey, GateEvent, GateSpec};
use crate::gate::kernel_deps::review_blocking::{self as blocking, Effective};
use crate::gate::review_verdict::{self as rv, JsonObj};
use std::path::Path;

/// How long a barrier may stand after its reviewer session ended.
pub(crate) const SWEEP_ABANDON_AFTER: chrono::Duration = chrono::Duration::minutes(20);

/// What the pass did to one change's barriers.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SweepExpiry {
    pub finalized: usize,
    pub abandoned: usize,
}

/// The session ids a barrier names its reviewer by: `recorded_by:
/// reviewer:<id>` and the attestation's `session:<id>` tokens.
fn reviewer_sessions(record: &JsonObj) -> Vec<String> {
    let mut ids: Vec<String> = record
        .get("recorded_by")
        .and_then(|v| v.as_str())
        .and_then(|r| r.trim().strip_prefix("reviewer:"))
        .map(|id| vec![id.trim().to_string()])
        .unwrap_or_default();
    if let Some(tokens) = record
        .get(rv::ATTESTATION_KEY)
        .and_then(|a| a.get("identity"))
        .and_then(|v| v.as_array())
    {
        ids.extend(
            tokens
                .iter()
                .filter_map(|t| t.as_str()?.strip_prefix("session:"))
                .map(|id| id.trim().to_string()),
        );
    }
    ids.retain(|id| !id.is_empty());
    ids.sort();
    ids.dedup();
    ids
}

/// Session-registry evidence that every session the barrier names has
/// ended: its lease is gone, or the registry classifies it stale. A barrier
/// naming no session, or an unreadable lease, is never treated as ended.
pub(crate) fn reviewer_session_ended(root: &Path, record: &JsonObj) -> bool {
    let ids = reviewer_sessions(record);
    !ids.is_empty()
        && ids
            .iter()
            .all(|id| match crate::leases::read_exact_lease(root, id) {
                Ok(None) => true,
                Ok(Some(lease)) => {
                    crate::ownership::reader::current_lease_state(&lease)
                        == crate::LeaseState::Stale
                }
                Err(_) => false,
            })
}

/// The approval a barrier stands for, when the barrier alone proves it: it
/// stood in for an `approved` verdict ([`rv::BARRIER_OF`]), with a reviewed
/// head and findings that are all deferred (the recorder already normalizes
/// such a record to an approval). Any other barrier is incomplete: its
/// fail-closed verdict hides what the reviewer decided. A barrier of a
/// rejected or request-changes record, or one naming no original verdict,
/// is never an approval (fail closed). The stored findings are re-decided
/// under the `policy` held now, so a policy that changed since the barrier
/// was written cannot be bypassed.
fn complete_approval(barrier: &JsonObj, policy: &Effective) -> Option<JsonObj> {
    // trace:BUG-2786 | ai:claude
    if barrier.get(rv::BARRIER_OF).and_then(|v| v.as_str()) != Some("approved") {
        return None;
    }
    let mut restored = barrier.clone();
    restored.remove("sweep_pending");
    restored.remove(rv::BARRIER_OF);
    restored.insert("verdict".into(), "approved".into());
    let restored = rv::redecide_replay_record(restored, policy, rv::VerdictKind::Approved).ok()?;
    let head = restored.get("reviewed_sha").and_then(|v| v.as_str());
    let findings = restored.get("findings").and_then(|v| v.as_array());
    (head.is_some_and(|h| !h.trim().is_empty())
        && findings.is_some_and(|f| !f.is_empty())
        && rv::disposition_verdict(&restored).approves())
    .then_some(restored)
}

/// The barrier expired: kept whole for the audit trail, marked
/// `sweep_abandoned` with a verdict word that parses as neither approval
/// nor refusal.
fn abandoned(barrier: &JsonObj, now: chrono::DateTime<chrono::Utc>) -> JsonObj {
    let mut body = barrier.clone();
    body.remove("sweep_pending");
    body.insert("sweep_abandoned".into(), true.into());
    body.insert("verdict".into(), "sweep-abandoned".into());
    body.insert("sweep_abandoned_at".into(), now.to_rfc3339().into());
    body
}

/// Expire `change`'s barriers older than `bound` whose reviewer `ended`.
/// `finalize` records a complete approval through the normal writer; when
/// it fails, or `policy` no longer approves it, the barrier is abandoned
/// instead, so it never stands forever.
pub(crate) fn expire_with(
    start: &Path,
    change: ChangeKey,
    now: chrono::DateTime<chrono::Utc>,
    bound: chrono::Duration,
    policy: &Effective,
    ended: &dyn Fn(&JsonObj) -> bool,
    finalize: &dyn Fn(&GateEvent, &JsonObj) -> std::io::Result<()>,
) -> std::io::Result<SweepExpiry> {
    let root = store_root(start)?;
    let events = store::read_file(&root, change);
    let mut newest: Vec<&GateEvent> = Vec::new();
    // Unbound spec-only records hold no change authority; they are not read.
    let records = events
        .iter()
        .filter(|e| e.verdict_record().is_some() && !e.is_spec_only());
    for event in records {
        match newest.iter_mut().find(|e| e.spec == event.spec) {
            Some(slot) => *slot = event,
            None => newest.push(event),
        }
    }
    let mut done = SweepExpiry::default();
    for event in newest {
        let Some(barrier) = event.verdict_record() else {
            continue;
        };
        let aged = chrono::DateTime::parse_from_rfc3339(&event.at)
            .is_ok_and(|at| now.signed_duration_since(at) >= bound);
        if !rv::delivery_pending(&serde_json::Value::Object(barrier.clone()))
            || !aged
            || !ended(barrier)
        {
            continue;
        }
        if let Some(approval) = complete_approval(barrier, policy) {
            match finalize(event, &approval) {
                Ok(()) => {
                    done.finalized += 1;
                    continue;
                }
                Err(e) => eprintln!(
                    "{}: could not finalize the abandoned review sweep: {e}; marking it for re-review",
                    change.as_key()
                ),
            }
        }
        let mut expired = event.clone();
        expired.body = super::EventBody::Verdict {
            record: abandoned(barrier, now),
        };
        // Refuses when the record changed since it was read (a late reviewer
        // finished): that newer record stands.
        store::append_delivered(&root, change, vec![expired], std::slice::from_ref(event))?;
        done.abandoned += 1;
    }
    Ok(done)
}

/// Finalize through the single verdict writer, under the barrier's own
/// change and spec slot: it re-runs the two-phase findings delivery.
pub(crate) fn finalize_via_writer(
    start: &Path,
    event: &GateEvent,
    approval: &JsonObj,
) -> std::io::Result<()> {
    let spec = event.spec.as_ref().map(|label| GateSpec {
        label: label.clone(),
        key: event.spec_key.clone(),
    });
    let path = rv::verdict_path(start, &event.change);
    super::record_verdict_at(&path, approval, spec.as_ref()).map(|_| ())
}

/// The review watcher's per-change step: production liveness (session
/// registry) and finalization (the single verdict writer). Best effort: a
/// failure is reported and the barrier waits for the next pass.
/// The caller must be uncontained: a contained seat cannot write the ledger.
pub(crate) fn expire_reported(start: &Path, change: ChangeKey) {
    let finalize =
        |event: &GateEvent, approval: &JsonObj| finalize_via_writer(start, event, approval);
    let ended = |record: &JsonObj| reviewer_session_ended(start, record);
    match expire_with(
        start,
        change,
        chrono::Utc::now(),
        SWEEP_ABANDON_AFTER,
        &blocking::effective_warned(start),
        &ended,
        &finalize,
    ) {
        Ok(SweepExpiry {
            finalized: 0,
            abandoned: 0,
        }) => {}
        Ok(done) => eprintln!(
            "{}: exited reviewer's pending review sweep: {} finalized, {} marked for re-review",
            change.as_key(),
            done.finalized,
            done.abandoned
        ),
        Err(e) => eprintln!(
            "{}: could not expire an exited reviewer's pending sweep: {e}",
            change.as_key()
        ),
    }
}

#[cfg(test)]
#[path = "abandoned_sweep_tests.rs"]
mod tests;
