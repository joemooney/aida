//! `aida own show [--compare]`: the records, the judge's verdict on each, and
//! (with `--compare`) today's lease reader beside the judge for every lease
//! (STORY-2325, amendment A6). Diagnostic only: nothing decides from it.
//!
//! PR-C may cut the readers over only when this report shows zero
//! unexplained differences. The expected ones are named here, each with the
//! reason the two rules differ by design.
//!
//! trace:STORY-2325 | ai:claude

use super::dual;
use super::store::OwnershipStore;
use super::types::{HostFacts, OwnerVerdict, Ownership, Phase, WorkKey};
use aida_core::liveness::LeaseState;
use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::path::Path;

/// How today's reader and the judge compare for one lease.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", content = "why", rename_all = "kebab-case")]
pub(crate) enum Outcome {
    Agree,
    Expected(String),
    Unexplained(String),
}

/// The expected-difference reasons, by name, so the report and the tests
/// share one wording.
// trace:STORY-2325 | ai:claude
pub(crate) mod expected {
    pub(crate) const REVIEW_UNRECORDED: &str =
        "a review lease with no recorded PR head: reviews are not recorded before PR-C";
    pub(crate) const UNRECORDED: &str =
        "no record yet: the lease predates the dual-write (run `aida own migrate`)";
    pub(crate) const HANDED_OFF: &str =
        "the work was handed to its PR; today's lease still names the seat";
    pub(crate) const LIVE_UNPROVEN: &str =
        "today reads it live from a working-directory scan or an unproven pid; the record holds no provable witness";
    pub(crate) const STALE_UNPROVEN: &str =
        "today's lenient rule reads a missing, foreign or unproven pid as dead; the judge abstains";
    pub(crate) const DORMANT_ALIVE: &str =
        "today finds no session in the tree; the recorded process is running";
    pub(crate) const DORMANT_DEAD: &str =
        "today waits out its 24h age clock; the judge already has proof the holder ended";
    pub(crate) const RELEASED: &str =
        "the record was released while today's lease remains (a release path that ends no lease)";
    pub(crate) const SUPERSEDED_LEASE: &str =
        "another current lease holds this work key; this retained lease is superseded";
    // trace:BUG-2425 | ai:claude
    pub(crate) const UNKNOWN_FOREIGN: &str =
        "today abstains: the lease's pids live in a pid namespace this reader cannot observe";
}

/// Multiple retained leases can resolve to one key (including native/display
/// aliases). A record matching one of them accounts for the older rows; a
/// session absent from the leases still needs a migration repair.
// trace:STORY-2325 | ai:codex
pub(crate) fn superseded_lease_outcome(
    lease_id: &str,
    today: LeaseState,
    rec: &Ownership,
    sessions_for_key: &[&str],
) -> Option<Outcome> {
    let holder = rec.holder.as_ref()?;
    (today == LeaseState::Stale
        && holder.session != lease_id
        && sessions_for_key.contains(&holder.session.as_str()))
    .then(|| Outcome::Expected(expected::SUPERSEDED_LEASE.into()))
}

/// Compare one lease's state today with its record and the judge's verdict
/// on that record's holder. Pure.
// trace:STORY-2325 | ai:claude
pub(crate) fn compare_one(
    lease_id: &str,
    today: LeaseState,
    rec: Option<&Ownership>,
    owner: Option<&OwnerVerdict>,
) -> Outcome {
    let Some(rec) = rec else {
        return Outcome::Expected(expected::UNRECORDED.to_string());
    };
    let Some(holder) = rec.holder.as_ref() else {
        return match rec.phase {
            Phase::InPr { .. } => Outcome::Expected(expected::HANDED_OFF.to_string()),
            _ if today == LeaseState::Stale => Outcome::Agree,
            _ => Outcome::Expected(expected::RELEASED.to_string()),
        };
    };
    if holder.session != lease_id {
        return Outcome::Unexplained(format!(
            "the record names session {}, not this lease; superseded ownership needs `aida own migrate`",
            holder.session
        ));
    }
    let Some(owner) = owner else {
        return Outcome::Unexplained("the record's holder was not judged".to_string());
    };
    match (today, owner) {
        (LeaseState::Live, OwnerVerdict::Alive { .. })
        | (LeaseState::Stale, OwnerVerdict::Dead { .. })
        | (LeaseState::Dormant, OwnerVerdict::Unknown { .. })
        | (LeaseState::Unknown, OwnerVerdict::Unknown { .. }) => Outcome::Agree,
        (LeaseState::Unknown, _) => Outcome::Expected(expected::UNKNOWN_FOREIGN.to_string()),
        (LeaseState::Live, OwnerVerdict::Unknown { .. }) => {
            Outcome::Expected(expected::LIVE_UNPROVEN.to_string())
        }
        (LeaseState::Stale, OwnerVerdict::Unknown { .. }) => {
            Outcome::Expected(expected::STALE_UNPROVEN.to_string())
        }
        (LeaseState::Dormant, OwnerVerdict::Alive { .. }) => {
            Outcome::Expected(expected::DORMANT_ALIVE.to_string())
        }
        (LeaseState::Dormant, OwnerVerdict::Dead { .. }) => {
            Outcome::Expected(expected::DORMANT_DEAD.to_string())
        }
        (LeaseState::Live, OwnerVerdict::Dead { proof }) => Outcome::Unexplained(format!(
            "today reads it live, but the judge has proof the holder ended: {proof}"
        )),
        (LeaseState::Stale, OwnerVerdict::Alive { witness }) => Outcome::Unexplained(format!(
            "today reads it stale, but the holder is running: {witness}"
        )),
    }
}

/// A record whose holder names a session with no lease at all.
// trace:STORY-2325 | ai:claude
pub(crate) fn orphan_outcome(rec: &Ownership, owner: &OwnerVerdict) -> Option<Outcome> {
    let holder = rec.holder.as_ref()?;
    if holder.session.starts_with("merge-lease:") {
        return None;
    }
    // Keep the report read-only: eligibility alone is not a journaled release.
    // trace:STORY-2325 | ai:codex
    if super::judge::legacy_release_by_rule(holder, false) {
        return Some(Outcome::Unexplained(format!(
            "pre-admission session {} has no lease; release by rule needs `aida own migrate`",
            holder.session
        )));
    }
    Some(match owner {
        OwnerVerdict::Alive { witness } => Outcome::Unexplained(format!(
            "session {} has no lease, but its holder is running: {witness}",
            holder.session
        )),
        _ => Outcome::Unexplained(format!(
            "session {} has no lease, and its record was never released",
            holder.session
        )),
    })
}

fn describe_key(key: &WorkKey) -> String {
    match key {
        WorkKey::Spec { spec } => format!("spec {spec}"),
        WorkKey::Change { change } => format!("change PR-{}", change.n),
        WorkKey::Train { train } => format!("train PR-{}", train.n),
        WorkKey::Review { change, head } => {
            format!("review PR-{} @{}", change.n, &head[..head.len().min(10)])
        }
        WorkKey::MailScope { anchor } => format!("mail {}", anchor.0),
    }
}

fn describe_phase(phase: &Phase) -> String {
    match phase {
        Phase::Leased => "leased".to_string(),
        Phase::InPr { change, .. } => format!("in PR-{}", change.n),
        Phase::Reviewing { .. } => "reviewing".to_string(),
        Phase::Training { train } => format!("training on PR-{}", train.n),
        Phase::Closed { why } => format!("closed ({why})"),
    }
}

fn describe_owner(owner: &OwnerVerdict) -> String {
    match owner {
        OwnerVerdict::Alive { witness } => format!("alive: {witness}"),
        OwnerVerdict::Dead { proof } => format!("dead: {proof}"),
        OwnerVerdict::Unknown { why } => format!("unknown: {why}"),
    }
}

/// One `--compare` row.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Row {
    pub lease: String,
    pub scope: String,
    pub today: String,
    pub judge: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

/// Run `aida own show`.
// trace:STORY-2325 | ai:claude
pub(crate) fn run(main: &Path, filter: Option<&str>, compare: bool, json: bool) -> Result<()> {
    let store = OwnershipStore::at(main.join(".aida").join(super::store::DIR));
    let (records, malformed) = store.list();
    let writer = dual::WriterFacts::here();
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    let probe = |id: &aida_core::process_probe::ProcessIdentity| {
        use aida_core::{
            proc_identity::{ProcIdentity, Verdict},
            process_probe::Probe,
        };
        let Some(id) = ProcIdentity::from_record(id, writer.pid_ns) else {
            return Probe::Unknown("insufficient identity".into());
        };
        match aida_core::proc_identity::observe(&id, true) {
            Verdict::Alive => Probe::Alive,
            Verdict::Dead(why) => Probe::Dead(format!("{why:?}")),
            Verdict::ForeignUnknown(why) => Probe::Unknown(format!("{why:?}")),
        }
    };
    let host = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: writer.host.clone(),
        boot: writer.boot.clone(),
        pid_ns: writer.pid_ns,
        probe: &probe,
    };
    let now = Utc::now();
    let judged = |rec: &Ownership| {
        rec.holder
            .as_ref()
            .map(|h| super::judge_owner(h, &host, now))
    };
    let shown: Vec<&Ownership> = records
        .iter()
        .filter(|r| {
            filter.is_none_or(|f| {
                describe_key(&r.key)
                    .to_lowercase()
                    .contains(&f.to_lowercase())
            })
        })
        .collect();

    let mut rows = Vec::new();
    if compare {
        let reqs = crate::load_store_for_lookup(main);
        let resolve = |label: &str| {
            if let Ok(id) = uuid::Uuid::parse_str(label.trim()) {
                return Some(crate::gate::canon::SpecKey(id));
            }
            reqs.as_ref()
                .and_then(|s| dual::spec_key_in_store(s, label))
        };
        let live = aida_core::liveness::probe_live_claude_sessions();
        let leases = crate::list_leases(main);
        let mut sessions_by_key: std::collections::BTreeMap<WorkKey, Vec<&str>> =
            Default::default();
        for lease in &leases {
            if let Some((key, _)) = dual::key_for_lease(lease, &resolve) {
                sessions_by_key.entry(key).or_default().push(&lease.id);
            }
        }
        for lease in &leases {
            let today = crate::lease_state_for(lease, &live, now);
            let rec = dual::key_for_lease(lease, &resolve)
                .and_then(|(k, _)| records.iter().find(|r| r.key == k));
            let owner = rec.and_then(judged);
            let superseded = rec.and_then(|rec| {
                superseded_lease_outcome(&lease.id, today, rec, sessions_by_key.get(&rec.key)?)
            });
            let outcome = match rec {
                None if lease.review_verb => {
                    Outcome::Expected(expected::REVIEW_UNRECORDED.to_string())
                }
                _ => {
                    superseded.unwrap_or_else(|| compare_one(&lease.id, today, rec, owner.as_ref()))
                }
            };
            rows.push(Row {
                lease: lease.id.clone(),
                scope: lease.scope.clone(),
                today: format!("{today:?}").to_lowercase(),
                judge: owner.as_ref().map_or_else(
                    || rec.map_or("no record".to_string(), |r| describe_phase(&r.phase)),
                    describe_owner,
                ),
                outcome,
            });
        }
        let ids: std::collections::BTreeSet<&str> = leases.iter().map(|l| l.id.as_str()).collect();
        for rec in &records {
            let Some(holder) = rec.holder.as_ref() else {
                continue;
            };
            if ids.contains(holder.session.as_str()) {
                continue;
            }
            let owner = super::judge_owner(holder, &host, now);
            if let Some(outcome) = orphan_outcome(rec, &owner) {
                rows.push(Row {
                    lease: holder.session.clone(),
                    scope: describe_key(&rec.key),
                    today: "no lease".to_string(),
                    judge: describe_owner(&owner),
                    outcome,
                });
            }
        }
    }

    if json {
        let recs: Vec<serde_json::Value> = shown
            .iter()
            .map(|r| {
                serde_json::json!({
                    "record": r,
                    "owner": judged(r).map(|o| describe_owner(&o)),
                })
            })
            .collect();
        let out = serde_json::json!({
            "records": recs,
            "malformed": malformed.iter().map(|(p, e)| serde_json::json!({"path": p, "error": e})).collect::<Vec<_>>(),
            "compare": compare.then_some(&rows),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("{} ownership record(s)", shown.len());
    for r in &shown {
        let holder = r.holder.as_ref().map_or_else(
            || "no holder".to_string(),
            |h| {
                format!(
                    "{} (session {}, {} witness(es))",
                    h.seat,
                    h.session,
                    h.witnesses.len()
                )
            },
        );
        let owner = judged(r).map_or_else(String::new, |o| format!(" — {}", describe_owner(&o)));
        println!(
            "  {} [gen {}] {}: {holder}{owner}",
            describe_key(&r.key),
            r.generation,
            describe_phase(&r.phase)
        );
    }
    for (path, err) in &malformed {
        println!("  malformed {}: {err}", path.display());
    }
    if compare {
        let agree = rows.iter().filter(|r| r.outcome == Outcome::Agree).count();
        let expected = rows
            .iter()
            .filter(|r| matches!(r.outcome, Outcome::Expected(_)))
            .count();
        let unexplained: Vec<&Row> = rows
            .iter()
            .filter(|r| matches!(r.outcome, Outcome::Unexplained(_)))
            .collect();
        println!(
            "compare: {} lease(s); {agree} agree, {expected} differ as expected, {} unexplained",
            rows.len(),
            unexplained.len()
        );
        let mut reasons: std::collections::BTreeMap<&str, usize> = Default::default();
        for r in &rows {
            if let Outcome::Expected(why) = &r.outcome {
                *reasons.entry(why.as_str()).or_insert(0) += 1;
            }
        }
        for (why, n) in reasons {
            println!("  expected ({n}): {why}");
        }
        for r in unexplained {
            if let Outcome::Unexplained(why) = &r.outcome {
                println!(
                    "  UNEXPLAINED {} ({}): today {}, judge {}: {why}",
                    r.lease, r.scope, r.today, r.judge
                );
            }
        }
    }
    Ok(())
}
