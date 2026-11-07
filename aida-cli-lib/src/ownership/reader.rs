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
        aida_core::process_probe::ProbeSource::system().probe(id)
    };
    let host = HostFacts {
        node_id: aida_core::proc_identity::local_node_id(),
        host: writer.host,
        boot: writer.boot,
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
    if let Phase::Closed { why } = record.phase {
        return Ok(Some(why));
    }
    Ok(match owner(&record) {
        OwnerVerdict::Dead { proof } => Some(proof),
        _ => None,
    })
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
