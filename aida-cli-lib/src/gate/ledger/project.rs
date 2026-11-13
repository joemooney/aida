//! Read side: the state a gate needs, folded from a change's events, and the
//! spec → change index.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::GateEvent;
use super::{ledger_dir, store, store_root, ChangeKey};
use crate::gate::review_verdict::{same_reviewed_sha, JsonObj};
use std::path::{Path, PathBuf};

/// A spec ID usable as a file name: no separators, no `..`, no change key.
pub(crate) fn spec_key(spec: &str) -> Option<String> {
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
pub(super) struct SpecIndex {
    #[serde(default)]
    pub(super) changes: Vec<String>,
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
    let mut events: Vec<_> = recorded_keys(&root)
        .into_iter()
        .flat_map(|k| match ChangeKey::parse(&k) {
            Some(change) => store::read_file(&root, change),
            None => store::read_unbound(&root, &k),
        })
        .filter(|e| e.verdict_record().is_some())
        .collect();
    // trace:BUG-2517 | ai:codex
    // Corpus scans already have all witnesses: avoid rediscovering the full
    // ledger separately for every change.
    let owners = MiskeyOwners::new(events.iter());
    owners.filter(&mut events);
    events
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
    store::with_lock(root, &path.with_extension("lock"), || {
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

/// The change's events, in ledger order. Incomplete ownership discovery
/// withholds every review copy a foreign change could own (historical foreign
/// copies have no spec-only marker) but keeps holds and other evidence no
/// witness can disqualify; callers needing diagnostics use the checked reader.
pub(crate) fn events(root: &Path, change: ChangeKey) -> Vec<GateEvent> {
    // trace:BUG-2517 | ai:codex
    let events = store::read_file(root, change)
        .into_iter()
        .filter(|e| !e.is_spec_only())
        .collect::<Vec<_>>();
    // trace:BUG-2517 | ai:claude
    super::checked::filter_miskeyed_or_fail_closed(root, events)
}

/// Ledger verdict events for `key` — a change key, or a spec ID through the
/// index (only events recorded for that spec) — newest last.
fn verdict_events(root: &Path, key: &str) -> Vec<GateEvent> {
    if let Some(change) = ChangeKey::parse(key) {
        return events(root, change)
            .into_iter()
            .filter(|e| e.verdict_record().is_some() && terminal_annotation_matches(e, key))
            .collect();
    }
    let Some(spec) = spec_key(key) else {
        return Vec::new();
    };
    // trace:BUG-2517 | ai:codex
    // One pinned discovery filters every related change: per-change
    // rediscovery would rescan the whole ledger once for each PR.
    let raw: Vec<GateEvent> = changes_for_spec(root, &spec)
        .into_iter()
        .flat_map(|change| store::read_file(root, change))
        .filter(|e| !e.is_spec_only())
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .collect();
    // As for change-keyed reads, incomplete discovery withholds every bound
    // copy a foreign change could own. trace:BUG-2517 | ai:claude
    let bound = super::checked::filter_miskeyed_or_fail_closed(root, raw);
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
    // trace:TASK-2964 | ai:codex
    // Merge independent ledgers by their next event's host time, retaining
    // append order within each ledger even when the host clock rolls back.
    // A global timestamp sort would make a spec lookup disagree with its PR.
    let mut streams: Vec<std::collections::VecDeque<GateEvent>> = Vec::new();
    for event in bound.into_iter().chain(unbound) {
        if let Some(stream) = streams.iter_mut().find(|stream| {
            stream
                .front()
                .is_some_and(|first| first.change == event.change)
        }) {
            stream.push_back(event);
        } else {
            streams.push(std::collections::VecDeque::from([event]));
        }
    }
    let mut out = Vec::new();
    while let Some(index) = streams
        .iter()
        .enumerate()
        .filter_map(|(index, stream)| stream.front().map(|event| (index, event)))
        .min_by(|(_, a), (_, b)| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)))
        .map(|(index, _)| index)
    {
        out.push(streams[index].pop_front().unwrap());
    }
    out.retain(|e| terminal_annotation_matches(e, key));
    out
}

// A spec-scoped closure cannot replace the PR review or another spec review.
// trace:BUG-2525 | ai:codex
fn terminal_annotation_matches(event: &GateEvent, key: &str) -> bool {
    event
        .verdict_record()
        .and_then(|r| r.get("terminal_wait_key"))
        .and_then(|v| v.as_str())
        .is_none_or(|target| target.eq_ignore_ascii_case(key))
}

/// The newest verdict event the ledger holds for `key`, read from `start`
/// (any checkout sharing the ledger).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn newest_verdict_event(start: &Path, key: &str) -> Option<GateEvent> {
    let root = store_root(start).ok()?;
    newest_of(verdict_events(&root, key))
}

/// Select a review round before applying lifecycle annotations. An annotation
/// carries no independent ordering authority against a legacy file: the source
/// round must first win ordinary selection, then the exact-source overlay applies.
// trace:BUG-2525 | ai:codex
pub(crate) fn newest_review_event(start: &Path, key: &str) -> Option<GateEvent> {
    let root = store_root(start).ok()?;
    newest_review_of(verdict_events(&root, key))
}

// Also used by the bounded alias reader with its already-checked snapshot.
// Raw change projection keeps using newest_of so it retains valid closures.
// trace:BUG-2525 | ai:codex
pub(crate) fn newest_review_of(events: Vec<GateEvent>) -> Option<GateEvent> {
    newest_of(
        events
            .into_iter()
            .filter(|event| {
                !event
                    .verdict_record()
                    .is_some_and(|record| record.contains_key("terminal_wait_source"))
            })
            .collect(),
    )
}

/// The latest lifecycle annotation before raw-review projection. Its source
/// must be checked against the effective legacy, signed, or ledger verdict.
/// Raw projection deliberately rejects annotations of independent reviews.
// trace:BUG-2525 | ai:codex
pub(crate) fn terminal_wait_annotation(start: &Path, key: &str) -> Option<JsonObj> {
    let root = store_root(start).ok()?;
    verdict_events(&root, key)
        .into_iter()
        .rev()
        .filter_map(|event| event.verdict_record().cloned())
        .find(|record| record.contains_key("terminal_wait_source"))
}

/// The answering event of one label's verdict events (oldest first): the
/// last, or the ordinary origin a proved phase-3 overlay duplicates.
// trace:BUG-2403 | ai:claude
pub(crate) fn newest_of(mut events: Vec<GateEvent>) -> Option<GateEvent> {
    // A lifecycle annotation is not a review of an independent ledger round.
    // This also protects raw effective-gate projections, which partition the
    // events themselves rather than reading one key through verdict_events.
    // trace:BUG-2525 | ai:codex
    while events.last().is_some_and(|event| {
        let Some(source) = event
            .verdict_record()
            .and_then(|r| r.get("terminal_wait_source"))
        else {
            return false;
        };
        let mut preceding = events
            .iter()
            .rev()
            .skip(1)
            .filter(|e| e.verdict_record().is_some());
        let prior = preceding.next();
        // The writer observed the receipt-bearing origin when a proved
        // phase-3 duplicate was the last raw event.
        let prior = match (preceding.next(), prior) {
            (Some(origin), Some(overlay)) if proved_phase3_origin(origin, overlay) => Some(origin),
            (_, prior) => prior,
        };
        prior
            .and_then(|event| event.verdict_record())
            .is_some_and(|prior| source != &serde_json::Value::Object(prior.clone()))
    }) {
        events.pop();
    }
    let newest = events.pop()?;
    // BUG-2403: a retained receipt-less phase-3 overlay answers with the
    // ordinary record it duplicates, when that pairing is proved.
    // trace:BUG-2403 | ai:claude
    Some(match events.last() {
        Some(origin) if proved_phase3_origin(origin, &newest) => origin.clone(),
        _ => newest,
    })
}

/// The `mode` the `--pr` handshake overlay of `aida review record` carries.
pub(crate) const PHASE3_MODE: &str = "orchestrator-phase-3";

/// Record fields a phase-3 overlay may differ from its origin in: the
/// coordination `mode`, the receipt older writers dropped, and the retained
/// round-history representation (every event stays readable through
/// [`verdict_records`]). Every other field must be present-or-absent alike
/// and equal.
const OVERLAY_ONLY_FIELDS: &[&str] = &["mode", "release_grant", "rounds", "unarchived_rounds"];

/// Fields whose presence is part of the proof: missing values are not equal.
const REQUIRED_FIELDS: &[&str] = &[
    "verdict",
    "reviewed_sha",
    "recorded_by",
    "recorded_at",
    crate::gate::review_verdict::ATTESTATION_KEY,
];

/// BUG-2403 (signoff amendment 5): `overlay` is a receipt-less phase-3
/// handshake that duplicates `origin`, the verdict event immediately before it
/// in the reader's candidate order (so no distinct review or refusal sits in
/// between), and `origin` is the ordinary receipt-bearing record of the same
/// act of review: same change, same spec label, same envelope head, and every
/// review-semantic field present and equal. Anything unproved — a missing
/// field, an unknown difference, a PRESENT receipt on the overlay, a
/// different recorder or time — is not a pairing, and the overlay keeps its
/// own (protective) standing. Preferring the origin grants nothing: its own
/// receipt still has to pass every authority check downstream.
// trace:BUG-2403 | ai:claude
pub(crate) fn proved_phase3_origin(origin: &GateEvent, overlay: &GateEvent) -> bool {
    let (Some(o), Some(v)) = (origin.verdict_record(), overlay.verdict_record()) else {
        return false;
    };
    let is_phase3 = |r: &JsonObj| r.get("mode").and_then(|m| m.as_str()) == Some(PHASE3_MODE);
    let envelope = origin.change == overlay.change
        && origin.seq < overlay.seq
        && origin.spec.is_some()
        && origin.spec == overlay.spec
        && origin.head.is_some()
        && origin.head == overlay.head;
    if !envelope
        || !is_phase3(v)
        || is_phase3(o)
        || v.contains_key("release_grant")
        || !o.get("release_grant").is_some_and(|g| g.is_object())
    {
        return false;
    }
    if !REQUIRED_FIELDS
        .iter()
        .all(|k| o.get(*k).is_some_and(|x| !x.is_null()) && o.get(*k) == v.get(*k))
    {
        return false;
    }
    o.keys()
        .chain(v.keys())
        .filter(|k| !OVERLAY_ONLY_FIELDS.contains(&k.as_str()))
        .all(|k| o.get(k) == v.get(k))
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
    carry_eligible: bool,
) -> Option<GateEvent> {
    use super::EventBody;
    let root = store_root(start).ok()?;
    events(&root, change).into_iter().rev().find(|e| {
        let (a, h, eligible) = match &e.body {
            EventBody::CarryOverProof {
                proof,
                carry_eligible,
                ..
            } => (&proof.approved_sha, &proof.new_head, *carry_eligible),
            EventBody::CarryOverRefused {
                approved_sha,
                new_head,
                carry_eligible,
                ..
            } => (approved_sha, new_head, *carry_eligible),
            _ => return false,
        };
        eligible == carry_eligible && same_reviewed_sha(a, approved) && same_reviewed_sha(h, head)
    })
}

/// Fingerprint the act of review, omitting only the documented handshake
/// overlay fields. Missing provenance never proves a misplaced copy.
// trace:BUG-2517 | ai:codex
fn review_fingerprint(event: &GateEvent) -> Option<String> {
    let record = event.verdict_record()?;
    // Matching numbers or payloads on different forges are not one change.
    let forge = match ChangeKey::parse(&event.change)?.forge {
        super::ChangeForge::Pr => "PR",
        super::ChangeForge::Mr => "MR",
    };
    let spec = event.spec.as_deref()?;
    let head = event.head.as_deref()?;
    if !REQUIRED_FIELDS
        .iter()
        .all(|k| record.get(*k).is_some_and(|v| !v.is_null()))
    {
        return None;
    }
    // Borrow payloads while constructing the canonical fingerprint: the
    // discovery owns only its serialized key, not a second review-body clone.
    // A sorted map preserves the existing JSON object's canonical key order.
    let semantic: std::collections::BTreeMap<&str, &serde_json::Value> = record
        .iter()
        .filter(|(k, _)| !OVERLAY_ONLY_FIELDS.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    serde_json::to_string(&(forge, spec, head, semantic)).ok()
}

// trace:BUG-2517 | ai:codex
#[derive(Default)]
pub(crate) struct MiskeyOwners {
    owners: std::collections::HashMap<String, std::collections::HashSet<String>>,
}

#[cfg(test)]
thread_local! {
    pub(super) static OWNERSHIP_WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

impl MiskeyOwners {
    /// Build once from the complete corpus, then reuse for every bucket and
    /// any additional event vector filtered through the same discovery.
    pub(crate) fn new<'a>(witnesses: impl IntoIterator<Item = &'a GateEvent>) -> Self {
        let mut index = Self::default();
        #[cfg(test)]
        OWNERSHIP_WORK.with(|work| {
            let (builds, visits) = work.get();
            work.set((builds + 1, visits));
        });
        index.add(witnesses);
        index
    }

    /// Fold more witnesses into this index (a streaming discovery adds each
    /// ledger as it is read, retaining no payloads). Returns the bytes newly
    /// retained, so the caller can charge them to its inventory budget.
    // trace:BUG-2517 | ai:claude
    pub(crate) fn add<'a>(&mut self, witnesses: impl IntoIterator<Item = &'a GateEvent>) -> u64 {
        let mut retained = 0u64;
        for witness in witnesses {
            #[cfg(test)]
            OWNERSHIP_WORK.with(|work| {
                let (builds, visits) = work.get();
                work.set((builds, visits + 1));
            });
            if ChangeKey::parse(&witness.change).is_none() || witness.is_spec_only() {
                continue;
            }
            if witness
                .verdict_record()
                .and_then(|r| r.get("mode"))
                .and_then(|v| v.as_str())
                != Some(PHASE3_MODE)
            {
                continue;
            }
            if let Some(key) = review_fingerprint(witness) {
                let len = (key.len() + witness.change.len()) as u64;
                if self
                    .owners
                    .entry(key)
                    .or_default()
                    .insert(witness.change.clone())
                {
                    retained += len;
                }
            }
        }
        retained
    }

    pub(crate) fn filter(&self, events: &mut Vec<GateEvent>) {
        events.retain(|event| match disqualifiable(event) {
            Candidate::Keep => true,
            Candidate::Drop => false,
            Candidate::Fingerprint(key) => !self
                .owners
                .get(&key)
                .is_some_and(|changes| changes.iter().any(|change| change != &event.change)),
        });
    }

    /// Without a complete ownership index, keep exactly the events no witness
    /// could disqualify: holds, carry-over proofs, phase-3 records and other
    /// unfingerprintable evidence stay; every review copy that a foreign
    /// change might own is withheld rather than presented as current. An
    /// unrelated malformed ledger therefore neither resurrects a historical
    /// foreign copy nor erases a change's hold state. Read-only.
    // trace:BUG-2517 | ai:claude
    pub(crate) fn fail_closed(events: &mut Vec<GateEvent>) {
        events.retain(|event| matches!(disqualifiable(event), Candidate::Keep));
    }
}

// trace:BUG-2517 | ai:claude
enum Candidate {
    Keep,
    Drop,
    Fingerprint(String),
}

/// Whether ownership witnesses can disqualify `event` as a misplaced copy.
// trace:BUG-2517 | ai:claude
fn disqualifiable(event: &GateEvent) -> Candidate {
    if ChangeKey::parse(&event.change).is_none() {
        return Candidate::Keep;
    }
    // A copy made before spec-only isolation was enforced is still local
    // evidence, never a change verdict or an ownership witness.
    if event.is_spec_only() {
        return Candidate::Drop;
    }
    if event
        .verdict_record()
        .and_then(|r| r.get("mode"))
        .and_then(|v| v.as_str())
        == Some(PHASE3_MODE)
    {
        return Candidate::Keep;
    }
    review_fingerprint(event).map_or(Candidate::Keep, Candidate::Fingerprint)
}
