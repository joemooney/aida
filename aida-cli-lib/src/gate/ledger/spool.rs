//! The contained writers' transport (sketch signoff, Q1): a contained seat
//! can write `<main>/.aida/mailbox/` and nothing else under `<main>/.aida/`,
//! so it submits a gate event as a file under
//! `mailbox/gate-events/<CHANGE>/`. Mailbox readers only read `*.json`
//! directly under the mailbox, so this subdirectory is invisible to them.
//!
//! The ledger keeps exactly one writer kind. A submission is evidence, not
//! authority: the primary side drains it under the change's lock, validates
//! the recorder attestation, and appends the event or a `rejected-event`
//! naming why. A seat that rewrites or deletes submissions can withhold its
//! own events, never forge or erase ledger history.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
use super::ChangeKey;
use crate::gate::review_verdict::{same_reviewed_sha, RecorderAttestation, ATTESTATION_KEY};
use std::path::{Path, PathBuf};

/// A submission larger than this is not read.
const MAX_SUBMISSION_BYTES: u64 = 1024 * 1024;

pub(crate) fn spool_root(root: &Path) -> PathBuf {
    aida_core::mailbox::local_mailbox_dir(root).join("gate-events")
}

pub(crate) fn spool_dir(root: &Path, change: ChangeKey) -> PathBuf {
    spool_root(root).join(change.as_key())
}

/// Submit `event` for the primary side to ingest. Returns the spool file.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn submit(root: &Path, event: &GateEvent) -> std::io::Result<PathBuf> {
    let change = ChangeKey::parse(&event.change).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{}` is not a change key", event.change),
        )
    })?;
    let dir = spool_dir(root, change);
    std::fs::create_dir_all(&dir)?;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("{nanos:024}-{}-{n:06}.json", std::process::id()));
    let body = serde_json::to_vec_pretty(event).map_err(std::io::Error::other)?;
    aida_core::fs_atomic::write_atomic(&path, body)?;
    Ok(path)
}

/// Regular `*.json` files in the change's spool, oldest first. A spool
/// directory that is a symlink is not read.
fn pending_files(root: &Path, change: ChangeKey) -> Vec<PathBuf> {
    let dir = spool_dir(root, change);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        _ => return Vec::new(),
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
}

pub(crate) fn has_pending(root: &Path, change: ChangeKey) -> bool {
    !pending_files(root, change).is_empty()
}

fn submission_id(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Read a submission: a regular file (never a link), size-capped.
fn read_submission(path: &Path) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("unreadable: {e}"))?;
    if !meta.is_file() {
        return Err("not a regular file".to_string());
    }
    if meta.len() > MAX_SUBMISSION_BYTES {
        return Err(format!("larger than {MAX_SUBMISSION_BYTES} bytes"));
    }
    std::fs::read_to_string(path).map_err(|e| format!("unreadable: {e}"))
}

/// Whether a submitted event may go on the ledger. Slice 1 accepts verdicts
/// whose recorder attestation authorizes exactly the head they name, and
/// which carry that same attestation inside the record. Approval gates still
/// re-validate the attestation against the grant stores on read.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn validate(event: &GateEvent, change: ChangeKey) -> Result<(), String> {
    if ChangeKey::parse(&event.change) != Some(change) {
        return Err(format!(
            "submitted for `{}` under {}",
            event.change,
            change.as_key()
        ));
    }
    let EventBody::Verdict { record } = &event.body else {
        return Err("only verdict events may be submitted through the mailbox".to_string());
    };
    let head = event
        .head
        .as_deref()
        .filter(|h| !h.trim().is_empty())
        .ok_or("names no head")?;
    let actor = event
        .actor
        .as_ref()
        .ok_or("carries no recorder attestation")?;
    if !actor.authorizes(Some(head)) {
        return Err(format!(
            "the recorder attestation does not authorize a verdict at {head}"
        ));
    }
    let in_record: Option<RecorderAttestation> = record
        .get(ATTESTATION_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    if in_record.as_ref() != Some(actor) {
        return Err("the record's attestation differs from the submitter's".to_string());
    }
    let reviewed = record
        .get("reviewed_sha")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !same_reviewed_sha(reviewed, head) {
        return Err(format!(
            "the record reviewed `{reviewed}`, the event names {head}"
        ));
    }
    Ok(())
}

/// The change's pending submissions as ledger events — each accepted event,
/// or a `rejected-event` — and the spool files they came from. A submission
/// the ledger already holds (its spool file outlived an earlier ingest)
/// yields no event; its file is still returned for removal.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn drain(
    root: &Path,
    change: ChangeKey,
    existing: &[GateEvent],
) -> (Vec<GateEvent>, Vec<PathBuf>) {
    let mut events = Vec::new();
    let mut consumed = Vec::new();
    for path in pending_files(root, change) {
        let id = submission_id(&path);
        consumed.push(path.clone());
        if existing
            .iter()
            .any(|e| e.submission.as_deref() == Some(id.as_str()))
        {
            continue;
        }
        let raw = read_submission(&path);
        let parsed = raw.as_ref().map_err(Clone::clone).and_then(|b| {
            serde_json::from_str::<GateEvent>(b).map_err(|e| format!("not a gate event: {e}"))
        });
        let mut event = match parsed.and_then(|e| validate(&e, change).map(|()| e)) {
            Ok(event) => event,
            Err(reason) => {
                let submitted = match &raw {
                    Ok(body) => serde_json::from_str(body)
                        .unwrap_or_else(|_| serde_json::Value::String(body.clone())),
                    Err(_) => serde_json::Value::Null,
                };
                GateEvent::new(change, EventBody::RejectedEvent { reason, submitted })
            }
        };
        event.submission = Some(id);
        events.push(event);
    }
    (events, consumed)
}
