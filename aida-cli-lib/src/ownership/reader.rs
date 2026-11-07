//! Production ownership observations. Missing or unreadable evidence retains
//! ownership; consumers never substitute PID, TTL or forge-body heuristics.
// trace:STORY-2325 | ai:codex
use super::{dual, HostFacts, OwnerVerdict, Ownership, Phase, WorkKey};
use anyhow::Result;
use chrono::Utc;
use std::path::Path;

pub(crate) fn load(root: &Path, key: &WorkKey) -> Result<Option<Ownership>> {
    match super::store::OwnershipStore::for_project(root)? {
        Some(store) => store.load(key),
        None => Ok(None),
    }
}

pub(crate) fn owner(record: &Ownership) -> OwnerVerdict {
    let writer = dual::WriterFacts::here();
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
    record.holder.as_ref().map_or_else(
        || OwnerVerdict::Unknown {
            why: "no ownership holder".into(),
        },
        |holder| super::judge_owner(holder, &host, Utc::now()),
    )
}

/// Explicit release or positive death proof. Unknown and absent records retain
/// the marker/train; elapsed time alone never permits destructive cleanup.
pub(crate) fn ended(root: &Path, key: &WorkKey) -> Result<Option<String>> {
    let Some(record) = load(root, key)? else {
        return Ok(None);
    };
    Ok(ended_record(&record))
}

// trace:STORY-2325 | ai:codex
pub(crate) fn ended_record(record: &Ownership) -> Option<String> {
    if let Phase::Closed { why } = &record.phase {
        return Some(why.clone());
    }
    match owner(record) {
        OwnerVerdict::Dead { proof } => Some(proof),
        _ => None,
    }
}

pub(crate) fn lease_record(root: &Path, lease: &crate::SessionLease) -> Result<Option<Ownership>> {
    let reqs = crate::load_store_for_lookup(root);
    let resolve = |label: &str| {
        uuid::Uuid::parse_str(label.trim())
            .ok()
            .map(crate::gate::canon::SpecKey)
            .or_else(|| {
                reqs.as_ref()
                    .and_then(|s| dual::spec_key_in_store(s, label))
            })
    };
    let Some((key, _)) = dual::key_for_lease(lease, &resolve) else {
        return Ok(None);
    };
    load(root, &key)
}
