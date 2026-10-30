//! Read side: the state a gate needs, folded from a change's events, and the
//! spec → change index.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::GateEvent;
use super::{ledger_dir, store, store_root, ChangeKey};
use crate::gate::review_verdict::{same_reviewed_sha, JsonObj};
use std::path::{Path, PathBuf};

/// A spec ID usable as a file name: no separators, no `..`, no change key.
fn spec_key(spec: &str) -> Option<String> {
    let key = spec.trim().to_ascii_uppercase();
    let safe = !key.is_empty()
        && !key.starts_with('.')
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    (safe && ChangeKey::parse(&key).is_none()).then_some(key)
}

fn index_file(root: &Path, spec: &str) -> Option<PathBuf> {
    Some(
        ledger_dir(root)
            .join("by-spec")
            .join(format!("{}.json", spec_key(spec)?)),
    )
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct SpecIndex {
    #[serde(default)]
    changes: Vec<String>,
}

/// Every key the ledger holds verdicts under, read from `start`: each
/// change with a ledger file (`PR-<N>`) and each indexed spec, sorted. A
/// reader that enumerates the legacy verdict directory unions these in, so
/// a ledger-only verdict (a collected review) is not skipped.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn recorded_keys(start: &Path) -> Vec<String> {
    let Ok(root) = store_root(start) else {
        return Vec::new();
    };
    let dir = ledger_dir(&root);
    let stems = |dir: PathBuf, ext: &str| -> Vec<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
            .filter_map(|p| Some(p.file_stem()?.to_str()?.to_string()))
            .collect()
    };
    let mut keys: Vec<String> = stems(dir.clone(), "jsonl")
        .into_iter()
        .filter_map(|k| ChangeKey::parse(&k).map(|c| c.as_key()))
        .chain(
            stems(dir.join("by-spec"), "json")
                .into_iter()
                .chain(stems(dir.join("by-spec"), "jsonl"))
                .filter_map(|k| spec_key(&k)),
        )
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// [`recorded_keys`] unioned with the stems of the legacy verdict files
/// `file_keys` (case-insensitively), sorted: the key set a directory-
/// scanning reader walks.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn keys_with_ledger(start: &Path, file_keys: Vec<String>) -> Vec<String> {
    let mut keys = file_keys;
    for key in recorded_keys(start) {
        if !keys.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
            keys.push(key);
        }
    }
    keys.sort();
    keys
}

/// Every verdict record on the ledger, with its change and seq, in key
/// order: what a corpus scan (finding classes, the verdict audit) adds to
/// the legacy files it walks.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn all_verdict_events(start: &Path) -> Vec<GateEvent> {
    let Ok(root) = store_root(start) else {
        return Vec::new();
    };
    recorded_keys(&root)
        .into_iter()
        .flat_map(|k| match ChangeKey::parse(&k) {
            Some(change) => events(&root, change),
            None => store::read_unbound(&root, &k),
        })
        .filter(|e| e.verdict_record().is_some())
        .collect()
}

/// The ledger file `event` was read from under the store root `root`: its
/// change's file, or the unbound ledger of the spec it names.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn event_file(root: &Path, event: &GateEvent) -> PathBuf {
    match ChangeKey::parse(&event.change) {
        Some(change) => store::ledger_file(root, change),
        None => store::unbound_file(root, &event.change),
    }
}

/// The changes the ledger has recorded `spec` against. A derived cache,
/// rebuilt from events; never the place state lives.
pub(crate) fn changes_for_spec(root: &Path, spec: &str) -> Vec<ChangeKey> {
    let Some(path) = index_file(root, spec) else {
        return Vec::new();
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<SpecIndex>(&b).ok())
        .map(|i| {
            i.changes
                .iter()
                .filter_map(|c| ChangeKey::parse(c))
                .collect()
        })
        .unwrap_or_default()
}

/// Add `change` to `spec`'s index entry.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn index_spec(root: &Path, spec: &str, change: ChangeKey) -> std::io::Result<()> {
    let Some(path) = index_file(root, spec) else {
        return Ok(());
    };
    store::with_lock(&path.with_extension("lock"), || {
        let mut index: SpecIndex = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let key = change.as_key();
        if index.changes.contains(&key) {
            return Ok(());
        }
        index.changes.push(key);
        let body = serde_json::to_vec_pretty(&index).map_err(std::io::Error::other)?;
        aida_core::fs_atomic::write_atomic(&path, body)
    })
}

/// The change's events, in ledger order.
pub(crate) fn events(root: &Path, change: ChangeKey) -> Vec<GateEvent> {
    store::read_file(root, change)
}

/// Ledger verdict events for `key` — a change key, or a spec ID through the
/// index (only events recorded for that spec) — newest last.
fn verdict_events(root: &Path, key: &str) -> Vec<GateEvent> {
    if let Some(change) = ChangeKey::parse(key) {
        return events(root, change)
            .into_iter()
            .filter(|e| e.verdict_record().is_some())
            .collect();
    }
    let Some(spec) = spec_key(key) else {
        return Vec::new();
    };
    let bound: Vec<GateEvent> = changes_for_spec(root, &spec)
        .into_iter()
        .flat_map(|change| events(root, change))
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .collect();
    // An unbound record already bound to a change (AC2) is read once, from
    // the change. trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let unbound: Vec<GateEvent> = store::read_unbound(root, &spec)
        .into_iter()
        .filter(|u| {
            !bound
                .iter()
                .any(|b| b.verdict_record() == u.verdict_record())
        })
        .collect();
    let mut out: Vec<GateEvent> = bound.into_iter().chain(unbound).collect();
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)));
    out
}

/// The newest verdict event the ledger holds for `key`, read from `start`
/// (any checkout sharing the ledger).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn newest_verdict_event(start: &Path, key: &str) -> Option<GateEvent> {
    let root = store_root(start).ok()?;
    verdict_events(&root, key).pop()
}

/// Every verdict record the ledger holds for `key`, oldest first: the round
/// history a reader of a ledger-only verdict needs (the rework hold's
/// arming refusal, BUG-2186).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn verdict_records(start: &Path, key: &str) -> Vec<JsonObj> {
    let Ok(root) = store_root(start) else {
        return Vec::new();
    };
    verdict_events(&root, key)
        .into_iter()
        .filter_map(|e| e.verdict_record().cloned())
        .collect()
}

/// The newest verdict record the ledger holds for `key`.
pub(crate) fn newest_verdict(start: &Path, key: &str) -> Option<JsonObj> {
    newest_verdict_event(start, key).and_then(|e| e.verdict_record().cloned())
}

/// The ledger's verdict events for `key` taken at `sha`, newest first.
pub(crate) fn verdict_events_at(start: &Path, key: &str, sha: &str) -> Vec<GateEvent> {
    let Ok(root) = store_root(start) else {
        return Vec::new();
    };
    verdict_events(&root, key)
        .into_iter()
        .rev()
        .filter(|e| {
            e.head
                .as_deref()
                .is_some_and(|head| same_reviewed_sha(head, sha))
        })
        .collect()
}

/// The newest carry-over answer on `change`'s ledger for the approval at
/// `approved` and the head `head` (a `carry-over-proof` or
/// `carry-over-refused` event), read from `start`. Which base tip and proof
/// version it may stand for is the reader's check (`review_carryover`).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn carry_over_at(
    start: &Path,
    change: ChangeKey,
    approved: &str,
    head: &str,
) -> Option<GateEvent> {
    use super::EventBody;
    let root = store_root(start).ok()?;
    events(&root, change).into_iter().rev().find(|e| {
        let (a, h) = match &e.body {
            EventBody::CarryOverProof { proof, .. } => (&proof.approved_sha, &proof.new_head),
            EventBody::CarryOverRefused {
                approved_sha,
                new_head,
                ..
            } => (approved_sha, new_head),
            _ => return false,
        };
        same_reviewed_sha(a, approved) && same_reviewed_sha(h, head)
    })
}
