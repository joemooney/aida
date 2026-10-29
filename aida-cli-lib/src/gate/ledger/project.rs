//! Read side: the state a gate needs, folded from a change's events, and the
//! spec → change index. Every read first ingests the change's pending
//! mailbox submissions (when this caller can write the ledger).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
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

/// Every verdict the change has on record, oldest first: the legacy files
/// (as imported, or read live before the first ledger write), then the
/// ledger's own verdict events. Nothing a legacy file held is lost.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn verdict_history(root: &Path, change: ChangeKey) -> Vec<JsonObj> {
    let events = events(root, change);
    let legacy = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::LegacyImported { legacy } => Some(legacy.clone()),
            _ => None,
        })
        .unwrap_or_else(|| super::legacy::verdict_files(root, change));
    let as_obj = |body: &str| {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v.as_object().cloned())
    };
    // Archived rounds are older than the current file they were archived from.
    let (current, archived): (Vec<_>, Vec<_>) = legacy
        .iter()
        .partition(|f| f.path.ends_with(&format!("{}.json", change.as_key())));
    archived
        .iter()
        .chain(current.iter())
        .filter_map(|f| as_obj(&f.body))
        .chain(events.iter().filter_map(|e| e.verdict_record().cloned()))
        .collect()
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

/// The newest verdict record the ledger holds for `key`, read from `start`
/// (any checkout sharing the ledger).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn newest_verdict(start: &Path, key: &str) -> Option<JsonObj> {
    let root = store_root(start).ok()?;
    verdict_events(&root, key)
        .into_iter()
        .rev()
        .find_map(|e| e.verdict_record().cloned())
}

/// The ledger's verdict records for `key` taken at `sha`, newest first.
pub(crate) fn verdicts_at(start: &Path, key: &str, sha: &str) -> Vec<JsonObj> {
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
        .filter_map(|e| e.verdict_record().cloned())
        .collect()
}
