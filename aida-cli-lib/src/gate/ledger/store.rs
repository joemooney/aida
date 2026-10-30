//! Append and read one change's ledger file. Appends hold an exclusive
//! `flock` on `<CHANGE>.lock` and write every new line in one `O_APPEND`
//! write, so `seq` is monotonic and a reader never sees a reordered history.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
use super::{ledger_dir, legacy, ChangeKey, Delivered};
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

/// Open a ledger file for appending. A symlink in its place is refused, so
/// an append can never land outside the ledger directory.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
fn open_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|e| {
        let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        if linked {
            std::io::Error::new(
                e.kind(),
                format!("refusing to append to {}: it is a symlink", path.display()),
            )
        } else {
            e
        }
    })
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
/// (first write only). A verdict whose record the ledger already holds for
/// the same spec — written through two paths, or collected after the
/// reviewer wrote it in place — is not appended again. Returns the events as
/// appended, with `seq` and `at` assigned; writes nothing when there is
/// nothing to append.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append(
    root: &Path,
    change: ChangeKey,
    events: Vec<GateEvent>,
) -> std::io::Result<Vec<GateEvent>> {
    with_lock(&lock_file(root, change), || {
        append_locked(root, change, events, false)
    })
    .map_err(|e| named(&ledger_file(root, change), e))
}

/// `e`, naming the ledger file that did not take the write (BUG-1571: a
/// failed write names the artefact that did not land).
fn named(file: &Path, e: std::io::Error) -> std::io::Error {
    if e.to_string().contains(&file.display().to_string()) {
        return e;
    }
    std::io::Error::new(
        e.kind(),
        format!(
            "could not append to the gate ledger {}: {e}",
            file.display()
        ),
    )
}

/// Append the change's `legacy-imported` event now if it has none, so the
/// import reads the legacy files before a writer changes them.
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
    let mut pending: Vec<GateEvent> = Vec::new();
    for event in events {
        let known = event.verdict_record().is_some_and(|record| {
            existing
                .iter()
                .chain(pending.iter())
                .any(|e| e.verdict_record() == Some(record) && e.spec == event.spec)
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
    let mut file = open_append(&ledger_file(root, change))?;
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
    Ok(if appended.iter().any(|e| e.body == body) {
        Delivered::Appended
    } else {
        Delivered::Unchanged
    })
}

/// The unbound ledger of a spec no change has been recorded against yet:
/// `by-spec/<SPEC>.jsonl` beside the spec's index entry. It carries only
/// verdict events whose `change` is the spec key, and is read together with
/// the spec's change-bound records ([`super::project`]); once a change is
/// indexed for the spec, new records land on that change.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn unbound_file(root: &Path, spec: &str) -> PathBuf {
    ledger_dir(root)
        .join("by-spec")
        .join(format!("{}.jsonl", spec.trim().to_ascii_uppercase()))
}

/// Every parseable event on `spec`'s unbound ledger, in file order.
pub(crate) fn read_unbound(root: &Path, spec: &str) -> Vec<GateEvent> {
    let Ok(body) = std::fs::read_to_string(unbound_file(root, spec)) else {
        return Vec::new();
    };
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Append `record` as a verdict event on `spec`'s unbound ledger, unless
/// that ledger already holds it.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn append_unbound(
    root: &Path,
    spec: &str,
    record: &crate::gate::review_verdict::JsonObj,
) -> std::io::Result<Delivered> {
    let spec = spec.trim().to_ascii_uppercase();
    let safe = !spec.is_empty()
        && !spec.starts_with('.')
        && spec
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !safe || ChangeKey::parse(&spec).is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{spec}` is not a spec id"),
        ));
    }
    let file = unbound_file(root, &spec);
    with_lock(&file.with_extension("lock"), || {
        let existing = read_unbound(root, &spec);
        if existing.iter().any(|e| e.verdict_record() == Some(record)) {
            return Ok(Delivered::Unchanged);
        }
        let mut event = super::event::verdict_event_unbound(root, &spec, record);
        event.seq = existing.last().map(|e| e.seq).unwrap_or(0) + 1;
        event.at = chrono::Utc::now().to_rfc3339();
        let mut line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut out = open_append(&file)?;
        out.write_all(line.as_bytes())?;
        out.sync_data()?;
        Ok(Delivered::Appended)
    })
    .map_err(|e| named(&file, e))
}

/// AC2: bind `spec`'s unbound records to `change` now that a change exists
/// for it. Each unbound verdict newer than every verdict `change` already
/// holds for the spec is appended to `change` (indexed by the spec), in its
/// unbound order, so the change is the one key that answers for it. Older
/// unbound records stay readable through the spec. The unbound ledger is
/// left as written: binding copies, it never deletes evidence. Returns how
/// many records were bound.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn bind_unbound(root: &Path, spec: &str, change: ChangeKey) -> std::io::Result<usize> {
    let spec = spec.trim().to_ascii_uppercase();
    if spec.is_empty() || ChangeKey::parse(&spec).is_some() {
        return Ok(0);
    }
    let unbound = read_unbound(root, &spec);
    if unbound.is_empty() {
        return Ok(0);
    }
    with_lock(&lock_file(root, change), || {
        let existing = read_file(root, change);
        let newest_bound = existing
            .iter()
            .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
            .map(|e| e.at.clone())
            .max();
        let events: Vec<GateEvent> = unbound
            .into_iter()
            .filter(|e| e.verdict_record().is_some())
            .filter(|e| newest_bound.as_ref().is_none_or(|at| e.at > *at))
            .map(|mut e| {
                e.change = change.as_key();
                e.spec = Some(spec.clone());
                e
            })
            .collect();
        let appended = append_locked(root, change, events, false)?;
        Ok(appended
            .iter()
            .filter(|e| e.verdict_record().is_some())
            .count())
    })
    .map_err(|e| named(&ledger_file(root, change), e))
}
