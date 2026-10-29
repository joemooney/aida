//! Append and read one change's ledger file. Appends hold an exclusive
//! `flock` on `<CHANGE>.lock` and write every new line in one `O_APPEND`
//! write, so `seq` is monotonic and a reader never sees a reordered history.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
use super::{ledger_dir, legacy, spool, ChangeKey, Delivered};
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub(crate) fn ledger_file(root: &Path, change: ChangeKey) -> PathBuf {
    ledger_dir(root).join(format!("{}.jsonl", change.as_key()))
}

fn lock_file(root: &Path, change: ChangeKey) -> PathBuf {
    ledger_dir(root).join(format!("{}.lock", change.as_key()))
}

/// Run `f` under an exclusive lock on `path`, creating it if needed.
pub(crate) fn with_lock<T>(
    path: &Path,
    f: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    use fs2::FileExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.lock_exclusive()?;
    let out = f();
    let _ = FileExt::unlock(&file);
    out
}

/// Every parseable event on the change's ledger, in file order. A line that
/// does not parse (a torn final write) is skipped, never fatal.
pub(crate) fn read_file(root: &Path, change: ChangeKey) -> Vec<GateEvent> {
    let Ok(body) = std::fs::read_to_string(ledger_file(root, change)) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append `events` to the change's ledger, after the change's legacy import
/// (first write only) and any pending mailbox submissions. A direct verdict
/// whose record the ledger already holds — written through two paths, or the
/// collected copy of an ingested submission — is not appended again. Returns
/// the events as appended, with `seq` and `at` assigned. Writes nothing when
/// there is nothing to append; consumed spool files are removed either way.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append(
    root: &Path,
    change: ChangeKey,
    events: Vec<GateEvent>,
) -> std::io::Result<Vec<GateEvent>> {
    with_lock(&lock_file(root, change), || {
        append_locked(root, change, events, false)
    })
}

/// Append the change's `legacy-imported` event now if it has none (plus any
/// pending submissions), so the import reads the legacy files before a
/// writer changes them.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn import_legacy(root: &Path, change: ChangeKey) -> std::io::Result<()> {
    with_lock(&lock_file(root, change), || {
        append_locked(root, change, Vec::new(), true).map(|_| ())
    })
}

fn append_locked(
    root: &Path,
    change: ChangeKey,
    events: Vec<GateEvent>,
    import: bool,
) -> std::io::Result<Vec<GateEvent>> {
    let existing = read_file(root, change);
    let (submitted, consumed) = spool::drain(root, change, &existing);
    let mut pending: Vec<GateEvent> = submitted;
    for event in events {
        let known = event.verdict_record().is_some_and(|record| {
            existing
                .iter()
                .chain(pending.iter())
                .any(|e| e.verdict_record() == Some(record))
        });
        if !known {
            pending.push(event);
        }
    }
    let imported = existing
        .iter()
        .any(|e| matches!(e.body, EventBody::LegacyImported { .. }));
    if !imported && (import || !pending.is_empty()) {
        pending.insert(0, legacy::import_event(root, change));
    }
    if !pending.is_empty() {
        write_events(root, change, &existing, &mut pending)?;
    }
    // The submissions are on the ledger now (or were already, when an
    // earlier removal failed: the drain skips a submission whose id the
    // ledger holds), so their spool files are consumed.
    for path in consumed {
        let _ = std::fs::remove_file(path);
    }
    Ok(pending)
}

/// Stamp `seq`/`at` on `pending` and append them in one write, then index
/// their specs.
fn write_events(
    root: &Path,
    change: ChangeKey,
    existing: &[GateEvent],
    pending: &mut [GateEvent],
) -> std::io::Result<()> {
    let mut seq = existing.last().map(|e| e.seq).unwrap_or(0);
    let now = chrono::Utc::now().to_rfc3339();
    let mut lines = String::new();
    for event in pending.iter_mut() {
        seq += 1;
        event.seq = seq;
        event.at = now.clone();
        event.change = change.as_key();
        let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
        lines.push_str(&line);
        lines.push('\n');
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_file(root, change))?;
    file.write_all(lines.as_bytes())?;
    file.sync_data()?;
    for event in pending.iter() {
        if let Some(spec) = event.spec.as_deref() {
            if let Err(e) = super::project::index_spec(root, spec, change) {
                eprintln!(
                    "warning: could not index {spec} → {} in the gate ledger: {e}",
                    change.as_key()
                );
            }
        }
    }
    Ok(())
}

/// Append one event; `Unchanged` when the ledger already held its record.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append_one(root: &Path, event: GateEvent) -> std::io::Result<Delivered> {
    let change = ChangeKey::parse(&event.change).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{}` is not a change key", event.change),
        )
    })?;
    let body = event.body.clone();
    let appended = append(root, change, vec![event])?;
    Ok(
        if appended
            .iter()
            .any(|e| e.submission.is_none() && e.body == body)
        {
            Delivered::Appended
        } else {
            Delivered::Unchanged
        },
    )
}

/// Drain the change's pending mailbox submissions onto its ledger. A caller
/// that cannot write the ledger (a contained reader) reads it as it stands.
pub(crate) fn ingest(root: &Path, change: ChangeKey) {
    if spool::has_pending(root, change) {
        let _ = append(root, change, Vec::new());
    }
}
