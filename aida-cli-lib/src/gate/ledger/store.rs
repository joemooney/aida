//! Append and read one change's ledger file. Appends hold the ledger-wide
//! lock shared and an exclusive `flock` on `<CHANGE>.lock`, and write every
//! new line in one `O_APPEND` write, so `seq` is monotonic and a reader never sees a reordered history.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

use super::event::{EventBody, GateEvent};
use super::{ledger_dir, legacy, ChangeKey, Delivered};
use crate::gate::canon::GateSpec;
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub(crate) fn ledger_file(root: &Path, change: ChangeKey) -> PathBuf {
    ledger_dir(root).join(format!("{}.jsonl", change.as_key()))
}

fn lock_file(root: &Path, change: ChangeKey) -> PathBuf {
    ledger_dir(root).join(format!("{}.lock", change.as_key()))
}

/// `<root>/.aida/gate-ledger.lock`: the ledger-wide lock. It sits beside
/// the ledger directory, not in it, so `aida gate migrate` swapping the
/// directory never moves it.
// trace:STORY-2324 | ai:claude
pub(crate) fn ledger_lock_file(root: &Path) -> PathBuf {
    root.join(".aida").join("gate-ledger.lock")
}

/// A held ledger-wide lock; released on drop.
// trace:STORY-2324 | ai:claude
pub(crate) struct LedgerLock {
    file: std::fs::File,
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn open_lock(path: &Path) -> std::io::Result<std::fs::File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
}

/// Take the ledger-wide lock exclusively: no writer appends, indexes or
/// creates a ledger file until it drops. `aida gate migrate` holds it from
/// its plan through its swap (and through a rollback).
// trace:STORY-2324 | ai:claude
pub(crate) fn lock_ledger_exclusive(root: &Path) -> std::io::Result<LedgerLock> {
    use fs2::FileExt;
    let file = open_lock(&ledger_lock_file(root))?;
    file.lock_exclusive()?;
    Ok(LedgerLock { file })
}

thread_local! {
    /// How many [`with_lock`] frames this thread is inside: a nested writer
    /// (an append indexing its spec) already holds the shared lock.
    static SHARED_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Run `f` under the ledger-wide lock of `root`'s ledger in shared mode and
/// an exclusive lock on `path` (a per-change, per-spec or index lock under
/// that ledger), creating both if needed. Every ledger writer comes through
/// here, so none can write while `aida gate migrate` holds the ledger-wide
/// lock exclusively; the per-path lock is opened only after the shared one
/// is held, so a writer that waited out a swap locks the new tree's file.
// trace:STORY-2324 | ai:claude
pub(crate) fn with_lock<T>(
    root: &Path,
    path: &Path,
    f: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    use fs2::FileExt;
    let shared = if SHARED_DEPTH.with(|d| d.get()) == 0 {
        // trace:BUG-2530 | ai:codex — a native reviewer can write the
        // ledger directory, but its permanent sibling lock is read-only.
        // Linux flock needs no write access for a shared lock. Preserve the
        // same inode/exclusive-migration protocol; only a missing lock needs
        // the ordinary host-side creation path.
        let path = ledger_lock_file(root);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => open_lock(&path)?,
            Err(e) => return Err(e),
        };
        file.lock_shared()?;
        Some(LedgerLock { file })
    } else {
        None
    };
    struct Depth;
    impl Drop for Depth {
        fn drop(&mut self) {
            SHARED_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
    SHARED_DEPTH.with(|d| d.set(d.get() + 1));
    let depth = Depth;
    let file = open_lock(path)?;
    file.lock_exclusive()?;
    let out = f();
    let _ = FileExt::unlock(&file);
    drop(depth);
    drop(shared);
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
    with_lock(root, &lock_file(root, change), || {
        append_locked(root, change, events, false)
    })
    .map_err(|e| named(&ledger_file(root, change), e))
}

// trace:STORY-2698 | ai:codex
/// Finish a delivery only while its pending records are still current. Store
/// delivery must not turn an older review into the newest review after a race.
pub(crate) fn append_delivered(
    root: &Path,
    change: ChangeKey,
    events: Vec<GateEvent>,
    pending: &[GateEvent],
) -> std::io::Result<Vec<GateEvent>> {
    if events.len() != pending.len() || events.is_empty() {
        return Err(std::io::Error::other(
            "delivery must match its pending verdict batch",
        ));
    }
    with_lock(root, &lock_file(root, change), || {
        let existing = read_file(root, change);
        for (expected, delivered) in pending.iter().zip(&events) {
            let current = existing
                .iter()
                .rev()
                .find(|e| e.spec == expected.spec && e.verdict_record().is_some())
                .and_then(|e| e.verdict_record());
            if current != expected.verdict_record() && current != delivered.verdict_record() {
                return Err(std::io::Error::other("review changed during sweep delivery; refusing stale approval; record a fresh review"));
            }
        }
        append_locked(root, change, events, false)
    })
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
    with_lock(root, &lock_file(root, change), || {
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
// trace:STORY-2324 | ai:claude
pub(crate) fn append_unbound(
    root: &Path,
    gate_spec: &GateSpec,
    record: &crate::gate::review_verdict::JsonObj,
) -> std::io::Result<Delivered> {
    append_unbound_expected(root, gate_spec, record, None)
}

// trace:STORY-2698 | ai:codex
pub(crate) fn append_unbound_expected(
    root: &Path,
    gate_spec: &GateSpec,
    record: &crate::gate::review_verdict::JsonObj,
    pending: Option<&crate::gate::review_verdict::JsonObj>,
) -> std::io::Result<Delivered> {
    let spec = gate_spec.label.clone();
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
    with_lock(root, &file.with_extension("lock"), || {
        let existing = read_unbound(root, &spec);
        if let Some(pending) = pending {
            let current = existing.iter().rev().find_map(|e| e.verdict_record());
            if current != Some(pending) && current != Some(record) {
                return Err(std::io::Error::other("review changed during sweep delivery; refusing stale approval; record a fresh review"));
            }
        }
        if existing.iter().any(|e| e.verdict_record() == Some(record)) {
            return Ok(Delivered::Unchanged);
        }
        let mut event = super::event::verdict_event_unbound(root, gate_spec, record);
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
/// unbound order, so the change is the one key that answers for it. A record
/// some other change of the spec already holds stays with that change: it
/// answers for the change it was bound to, not for a later one. Older
/// unbound records stay readable through the spec. The unbound ledger is
/// left as written: binding copies, it never deletes evidence. Returns how
/// many records were bound. Each bound copy carries `gate_spec`'s key (or
/// the key its unbound original was written with).
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// trace:STORY-2324 | ai:claude
pub(crate) fn bind_unbound(
    root: &Path,
    gate_spec: &GateSpec,
    change: ChangeKey,
) -> std::io::Result<usize> {
    let spec = gate_spec.label.clone();
    if spec.is_empty() || ChangeKey::parse(&spec).is_some() {
        return Ok(0);
    }
    let bound_elsewhere: Vec<GateEvent> = super::project::changes_for_spec(root, &spec)
        .into_iter()
        .filter(|other| *other != change)
        .flat_map(|other| read_file(root, other))
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .collect();
    let unbound: Vec<GateEvent> = read_unbound(root, &spec)
        .into_iter()
        .filter(|u| {
            !bound_elsewhere
                .iter()
                .any(|b| b.verdict_record() == u.verdict_record())
        })
        .collect();
    if unbound.is_empty() {
        return Ok(0);
    }
    with_lock(root, &lock_file(root, change), || {
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
                e.spec_key = gate_spec.key.or(e.spec_key);
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

// trace:BUG-2530 | ai:codex
#[cfg(all(test, target_os = "linux"))]
mod bug2530_tests {
    use super::*;
    use fs2::FileExt;

    #[test]
    fn bug2530_reviewer_records_with_read_only_migration_lock() {
        const CHILD: &str = "AIDA_BUG2530_LOCK_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let root = std::env::current_dir().unwrap();
            let home = ledger_dir(&root).join("home");
            let _env = crate::test_env::AmbientGuard::hermetic_and(
                &root,
                None,
                &[
                    ("AIDA_HOME", Some(home.to_str().unwrap())),
                    ("AIDA_CONTAINED", None),
                ],
            );
            assert!(
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(ledger_lock_file(&root))
                    .is_err(),
                "the migration lock must actually be read-only"
            );
            let hold = crate::gate::merge_hold_core::refusal_hold(
                7,
                "STORY-1",
                "request-changes",
                Some("a".repeat(40)),
                Some("reviewer".into()),
            );
            crate::gate::merge_hold_core::place_hold(&root, &hold).unwrap();
            let record =
                serde_json::json!({"verdict":"request-changes", "reviewed_sha":"a".repeat(40),
                "recorded_at":chrono::Utc::now().to_rfc3339()})
                .as_object()
                .unwrap()
                .clone();
            crate::gate::review_verdict::write_verdict_object(
                &crate::gate::review_verdict::verdict_path(&root, "PR-7"),
                &record,
            )
            .unwrap();
            assert!(crate::gate::merge_hold_core::hold_is_open(&root, 7));
            assert!(crate::gate::review_verdict::read_verdict_for_sha(
                &root,
                "PR-7",
                &"a".repeat(40)
            )
            .unwrap()
            .kind
            .requires_changes());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(ledger_dir(root)).unwrap();
        let exclusive = lock_ledger_exclusive(root).unwrap();
        let reader = std::fs::File::open(ledger_lock_file(root)).unwrap();
        assert!(
            reader.try_lock_shared().is_err(),
            "read-only readers still respect migration"
        );
        drop(exclusive);
        reader.try_lock_shared().unwrap();
        FileExt::unlock(&reader).unwrap();
        let output = std::process::Command::new("bwrap")
            .args(["--die-with-parent", "--ro-bind", "/", "/", "--bind"])
            .arg(ledger_dir(root)).arg(ledger_dir(root))
            .arg("--chdir").arg(root)
            .arg(crate::aida_bin::current_executable_path().unwrap())
            .args(["--exact", "gate::ledger::store::bug2530_tests::bug2530_reviewer_records_with_read_only_migration_lock", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}
