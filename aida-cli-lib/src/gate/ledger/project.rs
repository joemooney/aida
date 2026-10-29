//! Read side: the state a gate needs, folded from a change's events, and the
//! spec → change index. Every read first ingests the change's pending
//! mailbox submissions (when this caller can write the ledger).
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

/// The change's events after ingesting its pending submissions.
pub(crate) fn events(root: &Path, change: ChangeKey) -> Vec<GateEvent> {
    store::ingest(root, change);
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
    let mut out: Vec<GateEvent> = changes_for_spec(root, &spec)
        .into_iter()
        .flat_map(|change| events(root, change))
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .collect();
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
