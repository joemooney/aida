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

/// `<root>/.aida/gate-ledger.v2.lock`: the ledger-wide lock. It sits beside
/// the ledger directory, not in it, so `aida gate migrate` swapping the
/// directory never moves it. Format 2 moved it off `gate-ledger.lock`, which
/// is now the fence against older writers ([`fence_older_writers`]).
// trace:STORY-2324 | ai:claude
// trace:BUG-2531 | ai:claude
pub(crate) fn ledger_lock_file(root: &Path) -> PathBuf {
    root.join(".aida").join("gate-ledger.v2.lock")
}

/// `<root>/.aida/gate-ledger.lock`: the ledger-wide lock of every binary
/// older than format 2. Each of their ledger writes opens it first.
// trace:BUG-2531 | ai:claude
pub(crate) fn legacy_lock_file(root: &Path) -> PathBuf {
    root.join(".aida").join("gate-ledger.lock")
}

/// What the fence at [`legacy_lock_file`] points at: a path that never
/// exists, whose name tells an operator running an older binary why its
/// ledger write failed.
// trace:BUG-2531 | ai:claude
pub(crate) const WRITER_FENCE_TARGET: &str =
    "gate-ledger-is-format-2.this-aida-is-too-old-to-write-it/upgrade-aida";

/// Whether older binaries are fenced out of `root`'s ledger: their lock path
/// is the dangling fence link, so each of their ledger writes fails when it
/// opens (or tries to create) its lock. An older writer never binds the
/// specs it appends, so an unfenced ledger cannot be certified complete.
// trace:BUG-2531 | ai:claude
pub(crate) fn writer_fenced(root: &Path) -> bool {
    std::fs::read_link(legacy_lock_file(root)).is_ok_and(|t| t == Path::new(WRITER_FENCE_TARGET))
}

#[cfg(test)]
thread_local! {
    /// Stop a fence installation right after its link went in (interruption).
    pub(crate) static STOP_AFTER_FENCE_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Replace the older binaries' lock with the fence, then wait out every
/// older writer that already holds it. The caller holds the format 2 lock
/// exclusively and has already withdrawn the completeness marker. The link
/// goes in first, so no older writer can open the lock afterwards; one that
/// opened it earlier and holds it is waited for here. One that opened it but
/// has not locked it yet can still append after these waits: nothing is
/// certified while it holds the old lock open ([`older_writers_in_flight`]).
// trace:BUG-2531 | ai:claude
fn install_writer_fence(root: &Path) -> std::io::Result<()> {
    use fs2::FileExt;
    let legacy = legacy_lock_file(root);
    let held = match std::fs::symlink_metadata(&legacy) {
        Ok(m) if m.is_file() => Some(std::fs::File::open(&legacy)?),
        Ok(m) if m.file_type().is_symlink() => None,
        Ok(_) => {
            return Err(std::io::Error::other(format!(
                "refusing to fence older gate-ledger writers: {} is neither a lock file nor a link",
                legacy.display()
            )))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let aida = root.join(".aida");
    std::fs::create_dir_all(&aida)?;
    let tmp = aida.join(format!("gate-ledger.lock.fence.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(WRITER_FENCE_TARGET, &tmp)?;
    std::fs::rename(&tmp, &legacy).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    std::fs::File::open(&aida)?.sync_all()?;
    #[cfg(test)]
    if STOP_AFTER_FENCE_RENAME.with(|s| s.get()) {
        return Err(std::io::Error::other("injected fence interruption"));
    }
    if let Some(old) = held {
        for _ in 0..2 {
            old.lock_exclusive()?;
            FileExt::unlock(&old)?;
        }
    }
    Ok(())
}

/// Fence older writers out under an already held exclusive format 2 lock.
/// When the fence was not standing, an older binary may have appended since
/// the completeness marker was published, so the marker is withdrawn:
/// selection then refuses until the bindings are rebuilt. It is withdrawn,
/// durably, BEFORE the fence goes in: selection trusts a marker beside a
/// standing fence, and an older writer may still be draining behind the new
/// link (or this process may stop right after the rename).
// trace:BUG-2531 | ai:claude
fn fence_locked(root: &Path) -> std::io::Result<()> {
    if writer_fenced(root) {
        return Ok(());
    }
    super::bindings::withdraw_marker(root)?;
    install_writer_fence(root)
}

/// The processes that hold the older binaries' lock open: the regular file
/// at [`legacy_lock_file`], or the inode the fence replaced. An older writer
/// opens it before every ledger write and keeps it open until that write is
/// done, so while this is not empty an older binary may still append
/// without binding, even after the fence's lock passes (it may have opened
/// the lock and not yet locked it). Nothing is certified until it is empty.
///
/// Read from `/proc/<pid>/fd`; a process whose descriptors this user cannot
/// list (another user's) is not counted. Without a readable `/proc` the
/// quiescence cannot be verified and this refuses. Off Linux it is always
/// empty: only the fence's lock passes protect there.
// trace:BUG-2531 | ai:claude
pub(crate) fn older_writers_in_flight(root: &Path) -> std::io::Result<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let aida = root.join(".aida");
        let aida = std::fs::canonicalize(&aida).unwrap_or(aida);
        let live = aida
            .join("gate-ledger.lock")
            .as_os_str()
            .as_bytes()
            .to_vec();
        let mut deleted = live.clone();
        deleted.extend_from_slice(b" (deleted)");
        let mut out = Vec::new();
        let procs = std::fs::read_dir("/proc").map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot verify that no older aida writer is in flight: /proc is unreadable ({e})"),
            )
        })?;
        for entry in procs.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            let holds = fds.flatten().any(|fd| {
                std::fs::read_link(fd.path()).is_ok_and(|t| {
                    let t = t.as_os_str().as_bytes();
                    t == live.as_slice() || t == deleted.as_slice()
                })
            });
            if holds {
                out.push(pid);
            }
        }
        Ok(out)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Ok(Vec::new())
    }
}

/// Refuse to certify the ledger while an older writer holds the old lock
/// open ([`older_writers_in_flight`]).
// trace:BUG-2531 | ai:claude
pub(crate) fn require_older_writers_quiescent(root: &Path) -> std::io::Result<()> {
    let pids = older_writers_in_flight(root)?;
    if pids.is_empty() {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!(
            "refusing to certify the gate-ledger bindings: an older aida (pid {}) still holds {} open and may append without a binding; let it finish or stop it, then run `{}`",
            pids.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
            legacy_lock_file(root).display(),
            super::bindings::FIX_COMMAND
        ),
    ))
}

/// Fence older writers out of `root`'s ledger before this process writes
/// it. One `readlink` when the fence stands; otherwise the fence goes in
/// under the exclusive format 2 lock, so a writer never proceeds while
/// another process is still waiting older writers out.
// trace:BUG-2531 | ai:claude
pub(crate) fn fence_older_writers(root: &Path) -> std::io::Result<()> {
    use fs2::FileExt;
    if writer_fenced(root) {
        return Ok(());
    }
    let file = open_lock(&ledger_lock_file(root))?;
    file.lock_exclusive()?;
    let _lock = LedgerLock { file };
    fence_locked(root)
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
/// its plan through its swap (and through a rollback). Older writers do not
/// take this lock, so they are fenced out before it is returned.
// trace:STORY-2324 | ai:claude
// trace:BUG-2531 | ai:claude
pub(crate) fn lock_ledger_exclusive(root: &Path) -> std::io::Result<LedgerLock> {
    use fs2::FileExt;
    let file = open_lock(&ledger_lock_file(root))?;
    file.lock_exclusive()?;
    let lock = LedgerLock { file };
    fence_locked(root)?;
    Ok(lock)
}

// Serialize the final source comparison and append against every ledger writer.
// trace:BUG-2525 | ai:codex
pub(crate) fn with_exclusive<T>(
    root: &Path,
    f: impl FnOnce() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let _lock = lock_ledger_exclusive(root)?;
    struct Depth;
    impl Drop for Depth {
        fn drop(&mut self) {
            SHARED_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
    SHARED_DEPTH.with(|d| d.set(d.get() + 1));
    let _depth = Depth;
    f()
}

thread_local! {
    /// How many [`with_lock`] frames this thread is inside: a nested writer
    /// (an append indexing its spec) already holds the shared lock.
    static SHARED_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// The per-path locks this thread holds through [`with_lock`], innermost
    /// last.
    // trace:BUG-2662 | ai:claude
    static HELD_LOCKS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
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
    // trace:BUG-2662 | ai:claude — re-entrant per thread: a review append
    // holds its spec's `by-spec` lock across the change append, which indexes
    // the spec under that same lock file. A second flock on a fresh
    // descriptor would wait on itself.
    if HELD_LOCKS.with(|held| held.borrow().iter().any(|p| p == path)) {
        return f();
    }
    let shared = if SHARED_DEPTH.with(|d| d.get()) == 0 {
        // Before this process writes: no older binary may write beside it.
        // trace:BUG-2531 | ai:claude
        fence_older_writers(root).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "the gate ledger under {} is not fenced against older aida binaries and this process could not fence it ({e}); an uncontained aida fences it on its first gate-ledger write",
                    root.display()
                ),
            )
        })?;
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
    struct Held;
    impl Drop for Held {
        fn drop(&mut self) {
            HELD_LOCKS.with(|held| {
                held.borrow_mut().pop();
            });
        }
    }
    HELD_LOCKS.with(|held| held.borrow_mut().push(path.to_path_buf()));
    let held = Held;
    let out = f();
    drop(held);
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

/// The text of the regular file at `path`; `None` when it is absent,
/// unreadable, not UTF-8 or not a regular file. A regular-file check comes
/// before the open, so a FIFO or device in a ledger's place never hangs a
/// lossy reader (the strict reader in `checked` refuses it); the opened
/// handle is checked again before any read.
// trace:BUG-2543 | ai:claude
pub(crate) fn read_regular(path: &Path) -> Option<String> {
    use std::io::Read as _;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut body = String::new();
    file.read_to_string(&mut body).ok()?;
    #[cfg(test)]
    super::checked::discovery_work_tests::LOSSY_READ_BYTES
        .with(|n| n.set(n.get() + body.len() as u64));
    Some(body)
}

/// Every parseable event on the change's ledger, in file order. A line that
/// does not parse (a torn final write) is skipped, never fatal.
pub(crate) fn read_file(root: &Path, change: ChangeKey) -> Vec<GateEvent> {
    // trace:BUG-2543 | ai:claude — never blocks on a FIFO.
    let Some(body) = read_regular(&ledger_file(root, change)) else {
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

/// A ledger holding events of a newer format was written under rules this
/// binary does not know (format 2 added write-ahead bindings): appending
/// could break them, so every append path refuses before it writes anything.
// trace:BUG-2531 | ai:claude
fn refuse_newer_format(existing: &[GateEvent], what: &str) -> std::io::Result<()> {
    match existing.iter().find(|e| e.v > super::event::EVENT_VERSION) {
        Some(newer) => Err(std::io::Error::other(format!(
            "refusing to append to the gate ledger of {what}: it holds format {} events, newer than the format {} this aida writes; upgrade aida",
            newer.v,
            super::event::EVENT_VERSION
        ))),
        None => Ok(()),
    }
}

fn append_locked(
    root: &Path,
    change: ChangeKey,
    events: Vec<GateEvent>,
    import: bool,
) -> std::io::Result<Vec<GateEvent>> {
    let existing = read_file(root, change);
    refuse_newer_format(&existing, &change.as_key())?;
    let mut pending: Vec<GateEvent> = Vec::new();
    for event in events {
        // trace:BUG-2548 | ai:codex — retries keep one durable escalation.
        let manual_known = match &event.body {
            EventBody::ManualReviewRequested { request_id, .. } => existing.iter().chain(pending.iter()).any(|e| {
                matches!(&e.body, EventBody::ManualReviewRequested { request_id: known, .. } if known == request_id)
            }),
            _ => false,
        };
        let known = manual_known
            || event.verdict_record().is_some_and(|record| {
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

/// Bind `pending`'s specs to the change, stamp `seq`/`at`, append them in
/// one write, then index their specs. The bindings are written ahead of the
/// append, under the same locks, and a failed binding fails the append: an
/// event without a binding would be invisible to selection.
// trace:BUG-2531 | ai:claude
fn write_events(
    root: &Path,
    change: ChangeKey,
    existing: &[GateEvent],
    pending: &mut [GateEvent],
) -> std::io::Result<()> {
    // Before the first change ledger exists, whatever its events carry: a
    // first append without a spec must not leave the ledger uncertified.
    super::bindings::certify_empty(root)?;
    let specs: std::collections::BTreeSet<&str> =
        pending.iter().filter_map(|e| e.spec.as_deref()).collect();
    for spec in specs {
        super::bindings::bind(root, spec, change)?;
    }
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
    sync_appended(&file)?;
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
    // trace:BUG-2543 | ai:claude — never blocks on a FIFO.
    let Some(body) = read_regular(&unbound_file(root, spec)) else {
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
    let event = super::event::verdict_event_unbound(root, gate_spec, record);
    append_unbound_checked(
        root,
        event,
        |now, _| {
            if let Some(pending) = pending {
                let current = now.unbound.iter().rev().find_map(|e| e.verdict_record());
                if current != Some(pending) && current != Some(record) {
                    return Err(std::io::Error::other("review changed during sweep delivery; refusing stale approval; record a fresh review"));
                }
            }
            Ok(None)
        },
        |_, wrote| wrote,
    )
}

/// Whether `spec` may name a file under `by-spec/`.
fn unbound_spec_name(spec: &str) -> bool {
    !spec.is_empty()
        && !spec.starts_with('.')
        && spec
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && ChangeKey::parse(spec).is_none()
}

/// The identity of a verdict record: a digest of its serialized body.
// trace:BUG-2662 | ai:claude
type RecordId = [u8; 32];

fn record_id(record: &crate::gate::review_verdict::JsonObj) -> RecordId {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    // Hashing cannot fail, and a JSON object always serializes.
    let _ = serde_json::to_writer(&mut hasher, record);
    hasher.finalize().into()
}

/// The identity of one physical verdict event: the ledger it is on (the
/// spec's unbound ledger, or the change's), its sequence number there, the
/// spec label it carries and its record. The same record body appended again,
/// or under another label, is another event.
// trace:BUG-2662 | ai:claude
type EventId = (bool, u64, Option<String>, RecordId);

fn event_ids(events: &[GateEvent], unbound: bool) -> impl Iterator<Item = EventId> + '_ {
    events.iter().filter_map(move |e| {
        let record = e.verdict_record()?;
        Some((unbound, e.seq, e.spec.clone(), record_id(record)))
    })
}

/// One strict observation of the review history a verdict joins: every event
/// on the destination change's ledger, whatever spec label it carries (a
/// change's verdict is selected across all of them), and every event on the
/// spec's unbound ledger. Both files are read whole through the checked
/// reader, within its budget: an unreadable file or a line that does not
/// parse is a refusal, never a shorter history.
// trace:BUG-2662 | ai:claude
pub(crate) struct Observed {
    change: Option<ChangeKey>,
    bound: Vec<GateEvent>,
    unbound: Vec<GateEvent>,
}

/// Read [`Observed`] for a verdict landing on `change` (`None`: on the
/// unbound ledger) under the spec label `spec`, as its event carries it.
// trace:BUG-2662 | ai:claude
pub(crate) fn observe(
    root: &Path,
    change: Option<ChangeKey>,
    spec: Option<&str>,
) -> std::io::Result<Observed> {
    let unbound = spec.filter(|s| unbound_spec_name(s));
    let (bound, unbound) =
        super::checked::read_review_ledgers(root, change, unbound, &mut Default::default())
            .map_err(unread)?;
    Ok(Observed {
        change,
        bound,
        unbound,
    })
}

/// The refusal of a review history the checked reader would not read whole.
fn unread(refused: super::checked::Unsafe) -> std::io::Error {
    std::io::Error::other(format!(
        "the review history could not be read whole ({refused}); nothing was recorded"
    ))
}

impl Observed {
    /// The ledger the verdict lands on.
    fn destination(&self) -> &[GateEvent] {
        match self.change {
            Some(_) => &self.bound,
            None => &self.unbound,
        }
    }

    /// The act standing at the destination: its newest verdict event, under
    /// any label.
    fn current(&self) -> Option<&GateEvent> {
        self.destination()
            .iter()
            .rev()
            .find(|e| e.verdict_record().is_some())
    }

    /// The record of the standing act, when it was recorded under `spec`.
    pub(crate) fn current_act(
        &self,
        spec: Option<&str>,
    ) -> Option<&crate::gate::review_verdict::JsonObj> {
        self.current()
            .filter(|e| e.spec.as_deref() == spec)
            .and_then(|e| e.verdict_record())
    }

    /// Whether `record` repeats the act standing at the destination under
    /// `spec` ([`crate::gate::review_verdict::repeats_recorded_act`]). With a
    /// `receipt`, the record is weighed as it would answer with the standing
    /// act's receipt, when that receipt still vouches for it.
    pub(crate) fn repeats(
        &self,
        spec: Option<&str>,
        record: &crate::gate::review_verdict::JsonObj,
        receipt: Option<&dyn Receipt>,
    ) -> bool {
        let Some(act) = self.current_act(spec) else {
            return false;
        };
        match receipt.and_then(|r| r.reuse(act)) {
            Some(attestation) => {
                let mut record = record.clone();
                record.insert(
                    crate::gate::review_verdict::ATTESTATION_KEY.to_string(),
                    attestation,
                );
                crate::gate::review_verdict::repeats_recorded_act(act, &record)
            }
            None => crate::gate::review_verdict::repeats_recorded_act(act, record),
        }
    }

    /// Whether the destination already holds exactly `record` under `spec`.
    fn holds(&self, spec: Option<&str>, record: &crate::gate::review_verdict::JsonObj) -> bool {
        self.destination()
            .iter()
            .any(|e| e.spec.as_deref() == spec && e.verdict_record() == Some(record))
    }

    /// Whether a verdict event observed, on either ledger, names human-review
    /// receipt `id`.
    fn names_receipt(&self, id: &str) -> bool {
        self.bound.iter().chain(&self.unbound).any(|e| {
            e.verdict_record()
                .and_then(|r| r.get(crate::gate::review_verdict::ATTESTATION_KEY))
                .and_then(|a| a.get("receipt_id"))
                .and_then(|v| v.as_str())
                == Some(id)
        })
    }

    /// The verdict events observed, as [`Standing`].
    pub(crate) fn standing(&self) -> Standing {
        Standing(
            event_ids(&self.bound, false)
                .chain(event_ids(&self.unbound, true))
                .collect(),
        )
    }
}

/// The verdict events of an [`Observed`] history, taken when an approval
/// began filing its follow-up findings. The approval is appended only while
/// no other event has joined them, so filing findings never turns an older
/// review into the newest one. Events are indexed by physical identity
/// ([`EventId`]), so a body the history already held, appended again under a
/// sibling spec or on the other ledger, is still a new event; admitting a
/// history is linear in its size.
// trace:BUG-2662 | ai:claude
pub(crate) struct Standing(std::collections::HashSet<EventId>);

impl Standing {
    /// `Ok` while every verdict event in `now` was standing, or is `record`
    /// itself where it would land: on the destination, under `spec`.
    fn admit(
        &self,
        now: &Observed,
        spec: Option<&str>,
        record: &crate::gate::review_verdict::JsonObj,
    ) -> std::io::Result<()> {
        let lands_unbound = now.change.is_none();
        let joined = |events: &[GateEvent], unbound: bool| {
            let own = record_id(record);
            event_ids(events, unbound).any(|id| {
                let itself = unbound == lands_unbound && id.2.as_deref() == spec && id.3 == own;
                !itself && !self.0.contains(&id)
            })
        };
        if !joined(&now.bound, false) && !joined(&now.unbound, true) {
            return Ok(());
        }
        Err(std::io::Error::other(
            "another review was recorded while this approval's follow-up findings were filed; refusing a stale approval, nothing was recorded; record a fresh review",
        ))
    }
}

/// The recording attempt a receipt is issued for, as the append sees it
/// under its locks.
// trace:BUG-2662 | ai:claude
pub(crate) struct Attempt<'a> {
    /// Where the record lands: the store, the destination ledger and the
    /// spec label, as one digest. The locks held serialize every attempt
    /// that shares it.
    pub(crate) slot: String,
    /// What is recorded: a digest of the record without the fields every
    /// recording of the same act changes (its timestamp, the retained round
    /// history, the attestation that names the receipt).
    pub(crate) act: String,
    named: &'a dyn Fn(&str) -> bool,
}

impl Attempt<'_> {
    /// Whether a verdict event of the history read under the locks names
    /// receipt `id`.
    pub(crate) fn names(&self, id: &str) -> bool {
        (self.named)(id)
    }
}

/// What became of the record a receipt was issued for, as proved under the
/// append's locks.
// trace:BUG-2662 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settled {
    /// The record is on the ledger and was synced.
    Landed,
    /// The record is on the ledger unsynced, or the ledger could not be read
    /// back: the receipt and its reservation stay, and a retry reconciles.
    Kept,
    /// The ledger was read whole and does not hold the record.
    Absent,
}

/// The human-review receipt of an act of review, settled with its append
/// ([`append_review`]) under the destination's locks. A repeat answers with
/// the standing act's receipt. A new receipt is reserved for its attempt
/// ([`Attempt`]) before it is persisted, so a recording that stopped between
/// the receipt and the ledger write is found by the next attempt at the same
/// destination: the same act adopts the receipt, another act removes it once
/// the history read under the locks proves no record names it. After the
/// write, a receipt is removed only when the record is proved absent
/// ([`Settled`]); in doubt it is kept.
// trace:BUG-2662 | ai:claude
pub(crate) trait Receipt {
    /// The attestation that answers with the receipt of `standing`, when that
    /// receipt still vouches for this act.
    fn reuse(&self, standing: &crate::gate::review_verdict::JsonObj) -> Option<serde_json::Value>;
    /// Persist the receipt of `attempt` (adopting the one a stopped recording
    /// of the same act left) and return the attestation that names it. With
    /// no attempt (a hand-off file, no ledger) the receipt is always new.
    fn issue(&self, attempt: Option<&Attempt<'_>>) -> std::io::Result<serde_json::Value>;
    /// Settle the receipt [`Receipt::issue`] persisted, if any.
    fn settle(&self, settled: Settled);
    /// The attestation naming the receipt this act issued and kept.
    fn issued(&self) -> Option<serde_json::Value>;
}

/// What a review append must find true of the history it joins, decided
/// under the locks its append holds.
// trace:BUG-2662 | ai:claude
#[derive(Default, Clone, Copy)]
pub(crate) struct ReviewRule<'a> {
    /// Refuse when a verdict event joined the history since this was read.
    pub(crate) standing: Option<&'a Standing>,
    /// One act of review is one event: a record that repeats the act
    /// standing at the destination under the same spec is not appended, and
    /// one the ledger already holds but that no longer stands is refused.
    pub(crate) once: bool,
    /// Copy this spec's unbound records onto the change, in the same append.
    pub(crate) bind: Option<&'a GateSpec>,
    /// Settle the act's human-review receipt with the append.
    pub(crate) receipt: Option<&'a dyn Receipt>,
}

#[cfg(test)]
thread_local! {
    /// BUG-2662 test seam: called by [`append_review`] with the store root
    /// before it takes its locks (`false`: a writer racing the append lands
    /// here) and again under them, after its final read (`true`).
    // trace:BUG-2662 | ai:claude
    #[allow(clippy::type_complexity)]
    pub(crate) static REVIEW_APPEND_HOOK: std::cell::RefCell<Option<Box<dyn Fn(&Path, bool)>>> =
        const { std::cell::RefCell::new(None) };
    /// BUG-2662 test seam: fail the next review append on this thread after
    /// its decision (and any receipt it issued), before the ledger write.
    pub(crate) static FAIL_NEXT_REVIEW_WRITE: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
    /// BUG-2662 test seam: the next review append on this thread stops as
    /// [`ReviewFault`] says.
    pub(crate) static NEXT_REVIEW_FAULT: std::cell::Cell<Option<ReviewFault>> =
        const { std::cell::Cell::new(None) };
}

/// Where [`NEXT_REVIEW_FAULT`] stops a review append.
// trace:BUG-2662 | ai:claude
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewFault {
    /// The recorder exits once its receipt is persisted, before the ledger
    /// write: nothing after that point runs, no settlement either.
    ExitAfterReceipt,
    /// The ledger write completes and its sync reports an I/O error.
    Unsynced,
}

/// Take [`NEXT_REVIEW_FAULT`] when it is `fault`.
#[cfg(test)]
fn review_fault(fault: ReviewFault) -> bool {
    NEXT_REVIEW_FAULT.with(|f| {
        let hit = f.get() == Some(fault);
        if hit {
            f.set(None);
        }
        hit
    })
}

/// The sync of a ledger append: `Err` under [`ReviewFault::Unsynced`].
fn sync_appended(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(test)]
    if review_fault(ReviewFault::Unsynced) {
        return Err(std::io::Error::other("injected review sync failure"));
    }
    file.sync_data()
}

#[cfg(test)]
fn review_append_hook(root: &Path, locked: bool) {
    // Taken out while it runs, so a hook that records does not re-enter.
    if let Some(hook) = REVIEW_APPEND_HOOK.with(|h| h.borrow_mut().take()) {
        hook(root, locked);
        REVIEW_APPEND_HOOK.with(|h| {
            h.borrow_mut().get_or_insert(hook);
        });
    }
}

/// The decision of [`append_review`] on its final observation `now`, under
/// its locks: `Some` when nothing is to be appended.
// trace:BUG-2662 | ai:claude
fn review_check(
    root: &Path,
    now: &Observed,
    spec: Option<&str>,
    record: &crate::gate::review_verdict::JsonObj,
    rule: &ReviewRule<'_>,
) -> std::io::Result<Option<Delivered>> {
    #[cfg(test)]
    review_append_hook(root, true);
    #[cfg(not(test))]
    let _ = root;
    if rule.once && now.repeats(spec, record, rule.receipt) {
        return Ok(Some(Delivered::Unchanged));
    }
    if let Some(standing) = rule.standing {
        standing.admit(now, spec, record)?;
    }
    if rule.once && now.holds(spec, record) {
        return Err(std::io::Error::other(
            "this review is already on record but a later review has replaced it; nothing was recorded; record a fresh review",
        ));
    }
    Ok(None)
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// [`Attempt::slot`] for a record landing on `change` (`None`: the unbound
/// ledger) of the store at `root` under `spec`.
// trace:BUG-2662 | ai:claude
fn attempt_slot(root: &Path, change: Option<ChangeKey>, spec: Option<&str>) -> String {
    use sha2::Digest as _;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = sha2::Sha256::new();
    hasher.update(root.as_os_str().as_encoded_bytes());
    hasher.update([0]);
    hasher.update(change.map_or_else(|| "unbound".to_string(), |c| c.as_key()));
    hasher.update([0]);
    hasher.update(spec.unwrap_or_default());
    hex(&hasher.finalize())
}

/// [`Attempt::act`] for `record`.
// trace:BUG-2662 | ai:claude
fn attempt_act(record: &crate::gate::review_verdict::JsonObj) -> String {
    let mut act = record.clone();
    for key in [
        "recorded_at",
        "rounds",
        "unarchived_rounds",
        "inherited_findings",
        crate::gate::review_verdict::ATTESTATION_KEY,
    ] {
        act.remove(key);
    }
    hex(&record_id(&act))
}

/// The last step before a review append writes: issue the act's receipt, if
/// it has one to settle, and name it in `event`. A receipt that could not be
/// issued whole is taken back here: nothing was written under these locks.
// trace:BUG-2662 | ai:claude
fn seal(
    root: &Path,
    now: &Observed,
    spec: Option<&str>,
    record: &crate::gate::review_verdict::JsonObj,
    event: &mut GateEvent,
    rule: &ReviewRule<'_>,
) -> std::io::Result<()> {
    if let Some(receipt) = rule.receipt {
        let named = |id: &str| now.names_receipt(id);
        let attempt = Attempt {
            slot: attempt_slot(root, now.change, spec),
            act: attempt_act(record),
            named: &named,
        };
        match receipt.issue(Some(&attempt)) {
            Ok(attestation) => event.attest(attestation),
            Err(e) => {
                receipt.settle(Settled::Absent);
                return Err(e);
            }
        }
    }
    #[cfg(test)]
    {
        if FAIL_NEXT_REVIEW_WRITE.with(|f| f.replace(false)) {
            if let Some(receipt) = rule.receipt {
                receipt.settle(Settled::Absent);
            }
            return Err(std::io::Error::other("injected review write failure"));
        }
        if review_fault(ReviewFault::ExitAfterReceipt) {
            return Err(std::io::Error::other("injected recorder exit"));
        }
    }
    Ok(())
}

/// Settle the receipt of a sealed review append with what its write did,
/// still under the append's locks. A write that reported an error is read
/// back strictly: a record that is on the ledger was appended (its sync is
/// what failed) and keeps its receipt; one proved absent loses it; and when
/// the ledger cannot be read back the receipt is kept for the retry.
// trace:BUG-2662 | ai:claude
fn settle(
    root: &Path,
    change: Option<ChangeKey>,
    spec: Option<&str>,
    sealed: Option<&crate::gate::review_verdict::JsonObj>,
    rule: &ReviewRule<'_>,
    wrote: std::io::Result<Delivered>,
) -> std::io::Result<Delivered> {
    let Some(receipt) = rule.receipt else {
        return wrote;
    };
    let error = match wrote {
        Ok(Delivered::Appended) => {
            receipt.settle(Settled::Landed);
            return Ok(Delivered::Appended);
        }
        // The ledger already held the sealed record, which names the receipt.
        Ok(Delivered::Unchanged) => {
            receipt.settle(Settled::Kept);
            return Ok(Delivered::Unchanged);
        }
        Err(error) => error,
    };
    let holds = sealed
        .ok_or_else(|| std::io::Error::other("no sealed record"))
        .and_then(|record| Ok(observe(root, change, spec)?.holds(spec, record)));
    match holds {
        Ok(true) => {
            receipt.settle(Settled::Kept);
            if let (Some(change), Some(spec)) = (change, spec) {
                let _ = super::project::index_spec(root, spec, change);
            }
            eprintln!(
                "warning: the review verdict is on the gate ledger, but its write reported an error ({error}); its human-review receipt is kept"
            );
            Ok(Delivered::Appended)
        }
        Ok(false) => {
            receipt.settle(Settled::Absent);
            Err(error)
        }
        Err(unread) => {
            receipt.settle(Settled::Kept);
            Err(std::io::Error::new(
                error.kind(),
                format!(
                    "{error}; the gate ledger could not be read back to tell whether the review verdict was written ({unread}), so its human-review receipt is kept; retry `aida review record`"
                ),
            ))
        }
    }
}

/// Append the verdict `event` to `change`'s ledger, or with no change to the
/// unbound ledger of the spec it names, as `rule` allows.
///
/// The final read of the history, the decision, the receipt the record names
/// (issued, and settled with what the write did) and the append share the
/// change's lock and the spec's `by-spec` lock, taken in that order (the
/// order a change append already indexes its spec in). No verdict can join
/// either ledger in between: every change append holds the first and every
/// unbound append the second. The spec's unbound records a change key binds
/// ([`ReviewRule::bind`]) are staged and written with the verdict in one
/// append, after the receipt is issued: a refused or failed recording leaves
/// the change's ledger as it was.
// trace:BUG-2662 | ai:claude
pub(crate) fn append_review(
    root: &Path,
    change: Option<ChangeKey>,
    mut event: GateEvent,
    rule: &ReviewRule<'_>,
) -> std::io::Result<Delivered> {
    let Some(record) = event.verdict_record().cloned() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a review append needs a verdict event",
        ));
    };
    let spec = event.spec.clone();
    let spec = spec.as_deref();
    #[cfg(test)]
    review_append_hook(root, false);
    let Some(change) = change else {
        return append_unbound_checked(
            root,
            event,
            |now, event| {
                if let Some(decided) = review_check(root, now, spec, &record, rule)? {
                    return Ok(Some(decided));
                }
                seal(root, now, spec, &record, event, rule).map(|()| None)
            },
            |event, wrote| settle(root, None, spec, event.verdict_record(), rule, wrote),
        );
    };
    let spec_lock = spec
        .filter(|s| unbound_spec_name(s))
        .map(|s| unbound_file(root, s).with_extension("lock"));
    let locked = || {
        let now = observe(root, Some(change), spec)?;
        if let Some(decided) = review_check(root, &now, spec, &record, rule)? {
            return Ok(decided);
        }
        let mut events = match rule.bind {
            Some(bind) => stage_unbound(root, bind, change),
            None => Vec::new(),
        };
        seal(root, &now, spec, &record, &mut event, rule)?;
        let sealed = event.verdict_record().cloned();
        let body = event.body.clone();
        events.push(event);
        let wrote = append_locked(root, change, events, false).map(|appended| {
            if appended.iter().any(|e| e.body == body) {
                Delivered::Appended
            } else {
                Delivered::Unchanged
            }
        });
        settle(root, Some(change), spec, sealed.as_ref(), rule, wrote)
    };
    with_lock(root, &lock_file(root, change), || match &spec_lock {
        Some(spec_lock) => with_lock(root, spec_lock, locked),
        None => locked(),
    })
}

/// Append the unbound verdict `event` (its `change` is the spec label)
/// unless that ledger already holds its record. The ledger is read once,
/// under the spec's lock, through the checked reader and within its budget
/// ([`super::checked::read_unbound_for_append`]): that one validated read
/// answers `check`, the duplicate test and the next sequence number, and a
/// ledger that cannot be read whole refuses the append. `check` runs first
/// and may finish `event`; `Some` ends the append. `settle` answers for the
/// write of the finished event, under the same lock.
// trace:BUG-2662 | ai:claude
fn append_unbound_checked(
    root: &Path,
    mut event: GateEvent,
    check: impl FnOnce(&Observed, &mut GateEvent) -> std::io::Result<Option<Delivered>>,
    settle: impl FnOnce(&GateEvent, std::io::Result<Delivered>) -> std::io::Result<Delivered>,
) -> std::io::Result<Delivered> {
    let spec = event.change.clone();
    if !unbound_spec_name(&spec) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{spec}` is not a spec id"),
        ));
    }
    let file = unbound_file(root, &spec);
    with_lock(root, &file.with_extension("lock"), || {
        let unbound = super::checked::read_unbound_for_append(root, &spec, &mut Default::default())
            .map_err(unread)?;
        refuse_newer_format(&unbound, &spec)?; // trace:BUG-2531 | ai:claude
        let now = Observed {
            change: None,
            bound: Vec::new(),
            unbound,
        };
        if let Some(decided) = check(&now, &mut event)? {
            return Ok(decided);
        }
        let existing = &now.unbound;
        if existing
            .iter()
            .any(|e| e.verdict_record() == event.verdict_record())
        {
            return settle(&event, Ok(Delivered::Unchanged));
        }
        event.seq = existing.last().map(|e| e.seq).unwrap_or(0) + 1;
        event.at = chrono::Utc::now().to_rfc3339();
        let wrote = (|| {
            let mut line = serde_json::to_string(&event).map_err(std::io::Error::other)?;
            line.push('\n');
            let mut out = open_append(&file)?;
            out.write_all(line.as_bytes())?;
            sync_appended(&out)?;
            Ok(Delivered::Appended)
        })();
        settle(&event, wrote)
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
// trace:BUG-2517 | ai:codex
pub(crate) fn bind_unbound(
    root: &Path,
    gate_spec: &GateSpec,
    change: ChangeKey,
) -> std::io::Result<usize> {
    with_lock(root, &lock_file(root, change), || {
        let events = stage_unbound(root, gate_spec, change);
        if events.is_empty() {
            return Ok(0);
        }
        let appended = append_locked(root, change, events, false)?;
        Ok(appended
            .iter()
            .filter(|e| e.verdict_record().is_some())
            .count())
    })
    .map_err(|e| named(&ledger_file(root, change), e))
}

/// The copies [`bind_unbound`] appends to `change`, selected under the
/// change's lock (the caller holds it) and not yet written. A review append
/// writes them with its verdict, in one append ([`append_review`]).
// trace:BUG-2662 | ai:claude
fn stage_unbound(root: &Path, gate_spec: &GateSpec, change: ChangeKey) -> Vec<GateEvent> {
    let spec = gate_spec.label.clone();
    if spec.is_empty() || ChangeKey::parse(&spec).is_some() {
        return Vec::new();
    }
    let bound_elsewhere: Vec<GateEvent> = super::project::changes_for_spec(root, &spec)
        .into_iter()
        .filter(|other| *other != change)
        .flat_map(|other| read_file(root, other))
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .collect();
    let unbound: Vec<GateEvent> = read_unbound(root, &spec)
        .into_iter()
        .filter(|e| !e.is_spec_only())
        .filter(|u| {
            !bound_elsewhere
                .iter()
                .any(|b| b.verdict_record() == u.verdict_record())
        })
        .collect();
    if unbound.is_empty() {
        return Vec::new();
    }
    let existing = read_file(root, change);
    let newest_bound = existing
        .iter()
        .filter(|e| e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str()))
        .map(|e| e.at.clone())
        .max();
    unbound
        .into_iter()
        .filter(|e| e.verdict_record().is_some())
        .filter(|e| newest_bound.as_ref().is_none_or(|at| e.at > *at))
        .map(|mut e| {
            e.change = change.as_key();
            e.spec = Some(spec.clone());
            e.spec_key = gate_spec.key.or(e.spec_key);
            e
        })
        .collect()
}

// trace:BUG-2530 | ai:codex
// Keep the test gate explicit so production-input scans exclude this module.
#[cfg(test)]
#[cfg(target_os = "linux")]
mod bug2530_tests {
    use super::*;
    use fs2::FileExt;

    #[test]
    fn bug2530_reviewer_records_with_read_only_migration_lock() {
        const CHILD: &str = "AIDA_BUG2530_LOCK_CHILD";
        let environment = aida_core::exec_context::Environment::capture();
        if environment.get(CHILD).is_some() {
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
