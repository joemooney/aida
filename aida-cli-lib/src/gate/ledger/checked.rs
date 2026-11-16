//! BUG-2403 (signoff amendments 6-7): the fallible, bounded, no-follow reader
//! the cross-alias verdict selector ([`crate::gate::review_alias`]) reads its
//! ledger evidence through.
//!
//! [`super::store::read_file`] and [`super::project::changes_for_spec`] are
//! lossy on purpose (a torn final line is skipped, a bad index reads as
//! empty): the single-label readers keep that. A selection that compares
//! several labels' evidence must not, because a skipped line can be the newer
//! refusal. Here every directory is opened without following links, every
//! file must be a singly linked regular file under a byte cap, every line
//! must parse with a supported version and the right change binding, and an
//! operation stops at an aggregate byte/event budget. Any failure is a named
//! [`Unsafe`] the selector refuses on — never an empty answer.
// trace:BUG-2403 | ai:claude
// mandate-gate

use super::bindings::{
    binding_key, change_ledger_name, Marker, BINDINGS_DIR, FIX_COMMAND, MARKER_FILE, MARKER_FORMAT,
};
use super::event::{GateEvent, EVENT_VERSION};
use super::project::{spec_key, SpecIndex};
use super::ChangeKey;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

/// One change ledger file.
pub(crate) const MAX_LEDGER_BYTES: u64 = 16 * 1024 * 1024;
/// One `by-spec` index file.
pub(crate) const MAX_INDEX_BYTES: u64 = 1024 * 1024;
/// One ledger line, including full review findings and attestation payloads.
/// Keep this below the unchanged file and aggregate selection budgets.
// trace:BUG-2507 | ai:codex
pub(crate) const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Changes one spec index may name.
pub(crate) const MAX_CHANGES_PER_SPEC: usize = 256;
/// Aggregate bytes one selection may read across every label.
pub(crate) const MAX_SELECTION_BYTES: u64 = 64 * 1024 * 1024;
/// Aggregate events one selection may parse across every label.
pub(crate) const MAX_SELECTION_EVENTS: usize = 200_000;

/// Aggregate discovery I/O, including unrelated payloads and torn retries.
/// This safety ceiling is independent of selected evidence, not a completeness
/// mechanism: crossing it refuses the entire lookup.
// trace:BUG-2531 | ai:codex
pub(crate) const MAX_DISCOVERY_BYTES: u64 = 128 * 1024 * 1024;

/// Evidence the checked reader refused: the selector must not answer past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unsafe(pub String);

impl std::fmt::Display for Unsafe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The finite budget of one selection operation (amendment 7).
#[derive(Debug)]
pub(crate) struct Budget {
    bytes: u64,
    events: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            bytes: MAX_SELECTION_BYTES,
            events: MAX_SELECTION_EVENTS,
        }
    }
}

impl Budget {
    pub(crate) fn with_limits(bytes: u64, events: usize) -> Self {
        Self { bytes, events }
    }

    fn spend_bytes(&mut self, n: u64) -> Result<(), Unsafe> {
        self.bytes = self.bytes.checked_sub(n).ok_or_else(|| {
            Unsafe("the selection's aggregate gate-ledger read budget is exhausted".into())
        })?;
        Ok(())
    }

    fn spend_event(&mut self) -> Result<(), Unsafe> {
        self.events = self.events.checked_sub(1).ok_or_else(|| {
            Unsafe("the selection's aggregate gate-ledger event budget is exhausted".into())
        })?;
        Ok(())
    }
}

fn openat(dir: &File, name: &str, flags: libc::c_int) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = std::ffi::CString::new(name)?;
    // SAFETY: `dir` stays open across openat; `name` is NUL-terminated; the
    // returned descriptor is owned exactly once by File.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | flags,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn missing(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
}

/// The pinned `<root>/.aida/gate-ledger` directory handle and, when present,
/// its `by-spec` child; `None` when the ledger does not exist. A linked or
/// non-directory component refuses.
fn ledger_dirs(root: &Path) -> Result<Option<(File, Option<File>)>, Unsafe> {
    use std::os::unix::fs::OpenOptionsExt;
    let unsafe_dir = |what: &str, e: std::io::Error| {
        Unsafe(format!(
            "refusing the gate ledger under {}: {what} is not a plain directory ({e})",
            root.display()
        ))
    };
    let root_dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(root)
        .map_err(|e| unsafe_dir("the store root", e))?;
    let aida = match openat(&root_dir, ".aida", libc::O_DIRECTORY) {
        Ok(d) => d,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(unsafe_dir(".aida", e)),
    };
    let ledger = match openat(&aida, "gate-ledger", libc::O_DIRECTORY) {
        Ok(d) => d,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(unsafe_dir("gate-ledger", e)),
    };
    let by_spec = match openat(&ledger, "by-spec", libc::O_DIRECTORY) {
        Ok(d) => Some(d),
        Err(e) if missing(&e) => None,
        Err(e) => return Err(unsafe_dir("gate-ledger/by-spec", e)),
    };
    Ok(Some((ledger, by_spec)))
}

/// `name` under `dir`, read whole: `None` when absent; refused unless it is a
/// singly linked regular file of at most `cap` bytes (a FIFO, device, link or
/// hard link never blocks or floods the reader).
fn read_entry(
    dir: &File,
    name: &str,
    cap: u64,
    budget: &mut Budget,
) -> Result<Option<Vec<u8>>, Unsafe> {
    Ok(read_entry_meta(dir, name, cap, budget)?.map(|(body, _)| body))
}

/// [`read_entry`], plus the metadata of the file it read, taken before the
/// read (a later append therefore always differs from it).
// trace:BUG-2543 | ai:claude
fn read_entry_meta(
    dir: &File,
    name: &str,
    cap: u64,
    budget: &mut Budget,
) -> Result<Option<(Vec<u8>, std::fs::Metadata)>, Unsafe> {
    use std::os::unix::fs::MetadataExt;
    let file = match openat(dir, name, 0) {
        Ok(f) => f,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(Unsafe(format!("refusing gate-ledger entry {name}: {e}"))),
    };
    let meta = file
        .metadata()
        .map_err(|e| Unsafe(format!("refusing gate-ledger entry {name}: {e}")))?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(Unsafe(format!(
            "refusing gate-ledger entry {name}: it must be a singly linked regular file of at most {cap} bytes"
        )));
    }
    // trace:BUG-2542 | ai:claude
    if meta.len() > cap {
        return Err(Unsafe(format!(
            "refusing gate-ledger entry {name}: it is {} bytes, over the {cap}-byte per-file cap",
            meta.len()
        )));
    }
    // Bound actual I/O before reading, including growth after metadata. One
    // sentinel byte detects overflow; the operation then refuses immediately.
    // trace:BUG-2531 | ai:codex
    if meta.len() > budget.bytes {
        return Err(Unsafe(format!(
            "the aggregate gate-ledger read budget is exhausted at {name} ({} bytes; {} remaining)",
            meta.len(),
            budget.bytes
        )));
    }
    let read_cap = cap.min(budget.bytes);
    let mut body = Vec::new();
    file.take(read_cap + 1)
        .read_to_end(&mut body)
        .map_err(|e| Unsafe(format!("refusing gate-ledger entry {name}: {e}")))?;
    #[cfg(test)]
    discovery_work_tests::READ_BYTES.with(|n| n.set(n.get() + body.len() as u64));
    if body.len() as u64 > cap {
        return Err(Unsafe(format!(
            "refusing gate-ledger entry {name}: it grew past {cap} bytes while read"
        )));
    }
    budget.spend_bytes(body.len() as u64)?;
    Ok(Some((body, meta)))
}

/// Every event in `body`, strictly: a torn, oversized, unparseable,
/// unsupported-version, out-of-order or wrongly bound line refuses.
fn parse_events(
    name: &str,
    body: &[u8],
    change: &str,
    budget: &mut Budget,
) -> Result<Vec<GateEvent>, Unsafe> {
    let text = std::str::from_utf8(body)
        .map_err(|_| Unsafe(format!("refusing gate-ledger entry {name}: not UTF-8")))?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(Unsafe(format!(
            "refusing gate-ledger entry {name}: its final line is torn"
        )));
    }
    let mut out: Vec<GateEvent> = Vec::new();
    for (n, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let bad = |why: &str| {
            Unsafe(format!(
                "refusing gate-ledger entry {name} line {}: {why}",
                n + 1
            ))
        };
        if line.len() > MAX_LINE_BYTES {
            return Err(bad(&format!(
                "the line is oversized ({} bytes; limit {MAX_LINE_BYTES})",
                line.len()
            )));
        }
        budget.spend_event()?;
        #[cfg(test)]
        discovery_work_tests::PARSE_ATTEMPTS.with(|n| n.set(n.get() + 1));
        let event: GateEvent =
            serde_json::from_str(line).map_err(|e| bad(&format!("does not parse ({e})")))?;
        if event.v == 0 || event.v > EVENT_VERSION {
            return Err(bad(&format!("unsupported event version {}", event.v)));
        }
        if !event.change.eq_ignore_ascii_case(change) {
            return Err(bad(&format!(
                "the event is bound to {}, not {change}",
                event.change
            )));
        }
        if out.last().is_some_and(|prev| prev.seq >= event.seq) {
            return Err(bad("its sequence number does not advance"));
        }
        out.push(event);
    }
    Ok(out)
}

/// Directory entries one discovery pass may list.
pub(crate) const MAX_LEDGER_ENTRIES: usize = 16_384;
/// Change ledgers one discovery pass may read.
pub(crate) const MAX_CHANGE_LEDGERS: usize = 4_096;
/// Re-reads of a ledger whose final line was torn by a concurrent append.
const TORN_RETRIES: usize = 2;

/// The names in the pinned directory `dir`, which `path` names: listed by
/// path (std has no portable read-by-descriptor) and accepted only when the
/// path still names that same pinned directory before and after the
/// listing, so a swapped or linked directory cannot feed the list. Bounded
/// by [`MAX_LEDGER_ENTRIES`]; any error refuses.
fn list_pinned(dir: &File, path: &Path) -> Result<Vec<String>, Unsafe> {
    use std::os::unix::fs::MetadataExt;
    let refuse = |e: String| {
        Unsafe(format!(
            "refusing gate-ledger discovery in {}: {e}",
            path.display()
        ))
    };
    let pinned = dir.metadata().map_err(|e| refuse(e.to_string()))?;
    let same = || -> Result<(), Unsafe> {
        let now = std::fs::symlink_metadata(path).map_err(|e| refuse(e.to_string()))?;
        if !now.is_dir() || now.dev() != pinned.dev() || now.ino() != pinned.ino() {
            return Err(refuse(
                "the directory was replaced while it was listed".into(),
            ));
        }
        Ok(())
    };
    same()?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path).map_err(|e| refuse(e.to_string()))? {
        let entry = entry.map_err(|e| refuse(e.to_string()))?;
        #[cfg(test)]
        discovery_work_tests::DIRECTORY_ENTRIES.with(|n| n.set(n.get() + 1));
        if names.len() >= MAX_LEDGER_ENTRIES {
            return Err(refuse(format!(
                "it holds more than {MAX_LEDGER_ENTRIES} entries"
            )));
        }
        names.push(
            entry
                .file_name()
                .into_string()
                .map_err(|n| refuse(format!("{n:?} is not a UTF-8 name")))?,
        );
    }
    same()?;
    names.sort();
    Ok(names)
}

/// One change ledger, read and parsed strictly; a final line torn by a
/// concurrent append is re-read a bounded number of times, then refuses.
/// Returns the events and the bytes read for them.
fn read_change(
    ledger: &File,
    name: &str,
    change: &str,
    budget: &mut Budget,
) -> Result<Option<(Vec<GateEvent>, u64, EntryStat)>, Unsafe> {
    let mut attempt = 0;
    loop {
        let Some((body, meta)) = read_entry_meta(ledger, name, MAX_LEDGER_BYTES, budget)? else {
            return Ok(None);
        };
        let torn = !body.is_empty() && !body.ends_with(b"\n");
        if torn && attempt < TORN_RETRIES {
            attempt += 1;
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }
        let bytes = body.len() as u64;
        let stat = EntryStat::of(&meta); // trace:BUG-2543 | ai:claude
        return parse_events(name, &body, change, budget).map(|e| Some((e, bytes, stat)));
    }
}

/// `<root>/.aida/gate-ledger-quarantine`: where an operator moves a change
/// ledger that blocks discovery. Outside the ledger directory, so discovery
/// and capacity probes skip it; the evidence is kept, not deleted.
// trace:BUG-2542 | ai:claude
pub(crate) fn quarantine_dir(root: &Path) -> std::path::PathBuf {
    root.join(".aida").join("gate-ledger-quarantine")
}

/// The shell command that quarantines the change ledger `name` under `root`.
// trace:BUG-2542 | ai:claude
pub(crate) fn quarantine_command(root: &Path, name: &str) -> String {
    let q = quarantine_dir(root);
    format!(
        "mkdir -p '{}' && mv '{}' '{}/'",
        q.display(),
        super::ledger_dir(root).join(name).display(),
        q.display()
    )
}

/// A refusal of the full bindings scan caused by one change ledger: it names
/// the file and the fix, because an unfinished bootstrap blocks review
/// selection for every spec.
// trace:BUG-2542 | ai:claude
fn discovery_refusal(root: &Path, name: &str, e: &Unsafe) -> Unsafe {
    Unsafe(format!(
        "refusing gate-ledger discovery: {e}. File: {}. This blocks review selection for every spec. \
         Fix: if that change is closed or merged, quarantine its ledger (`{}`); \
         `aida doctor` lists every oversized or budget-threatening change ledger",
        super::ledger_dir(root).join(name).display(),
        quarantine_command(root, name)
    ))
}

/// The full, strict scan the bindings bootstrap needs: every change ledger
/// and every spec its events carry. It reads every active ledger, so only
/// the maintenance bootstrap ([`super::bindings::bootstrap`]) and a
/// selection that holds evidence of a lost binding ([`binding_lost`],
/// [`recorded_changes`]) run it; any other selection reads bindings instead.
// trace:BUG-2531 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) changes: std::collections::BTreeSet<String>,
    pub(crate) bound: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    /// The pinned ledger directory's (device, inode) and, in listing order,
    /// every change ledger read with its stat identity: what a reuse of this
    /// scan re-validates ([`Scan::current`]).
    ledger_id: Option<(u64, u64)>,
    read: Vec<(String, EntryStat)>,
}

impl Scan {
    /// Whether a scan of `root` now would read exactly the files this one
    /// read: the same ledger directory, listing the same change ledgers,
    /// each with the stat identity it had when read.
    // trace:BUG-2531 | ai:claude
    fn current(&self, root: &Path) -> bool {
        let Ok(Some((ledger, _))) = ledger_dirs(root) else {
            return false;
        };
        if dir_id(&ledger).ok() != self.ledger_id {
            return false;
        }
        let ledger_path = super::ledger_dir(root);
        let Ok(names) = list_pinned(&ledger, &ledger_path) else {
            return false;
        };
        let mut names = names
            .into_iter()
            .filter(|n| change_ledger_name(n).is_some());
        self.read.iter().all(|(name, stat)| {
            names.next().as_deref() == Some(name.as_str())
                && std::fs::symlink_metadata(ledger_path.join(name))
                    .is_ok_and(|now| EntryStat::of(&now) == *stat)
        }) && names.next().is_none()
    }
}

type FullReads = std::collections::HashMap<std::path::PathBuf, std::rc::Rc<Scan>>;

thread_local! {
    /// The last full read of each store root on this thread, reused only
    /// while [`Scan::current`] holds.
    // trace:BUG-2531 | ai:claude
    static FULL_READS: std::cell::RefCell<FullReads> =
        std::cell::RefCell::new(Default::default());
}

/// Every change whose ledger carries an event for the normalized label
/// `spec`, from a full strict read of every change ledger: the answer no
/// lost binding or hint can shorten. A selection falls back to it only on
/// evidence that a binding of the label was lost ([`binding_lost`]): a
/// deleted binding would otherwise hide a newer refusal. A label never
/// recorded leaves no such evidence and is answered from its bindings.
///
/// The read streams (no payload is retained) under the bootstrap's work
/// ceiling, not the selection's budget; its result is reused on this thread
/// while every change ledger keeps the stat identity it was read with. A
/// change ledger that cannot be read strictly refuses, naming the file.
// trace:BUG-2531 | ai:claude
pub(crate) fn recorded_changes(root: &Path, spec: &str) -> Result<Vec<String>, Unsafe> {
    Ok(full_read(root)?
        .map(|scan| recorded_in(&scan, spec))
        .unwrap_or_default())
}

/// The full read [`recorded_changes`] answers from, reused on this thread
/// while it is current; `None` when the ledger does not exist.
// trace:BUG-2531 | ai:claude
fn full_read(root: &Path) -> Result<Option<std::rc::Rc<Scan>>, Unsafe> {
    let cached = FULL_READS.with(|c| c.borrow().get(root).cloned());
    let scan = match cached.filter(|s| s.current(root)) {
        Some(scan) => scan,
        None => {
            let mut work = Budget::with_limits(
                super::bindings::MAX_BOOTSTRAP_BYTES,
                super::bindings::MAX_BOOTSTRAP_EVENTS,
            );
            let Some(scan) = scan_bindings(root, &mut work)? else {
                return Ok(None);
            };
            let scan = std::rc::Rc::new(scan);
            FULL_READS.with(|c| c.borrow_mut().insert(root.to_path_buf(), scan.clone()));
            scan
        }
    };
    #[cfg(test)]
    discovery_work_tests::FULL_READ_ANSWERS.with(|n| n.set(n.get() + 1));
    Ok(Some(scan))
}

fn recorded_in(scan: &Scan, spec: &str) -> Vec<String> {
    scan.bound
        .get(spec)
        .map(|changes| changes.iter().cloned().collect())
        .unwrap_or_default()
}

/// Whether the label's records show that a binding existed and is gone.
/// A writer creates the label's bindings directory, binds, appends, then
/// names the change in the by-spec hint, so each of these is left only by a
/// deletion (or by a write cut short, where the full read costs time only):
///
/// - the hint names a change the bindings do not (`hint` is what the hint
///   names, `None` when the label has none);
/// - the bindings directory exists (`bound` is `Some`) without a hint: the
///   directory outlives its bindings and is the deletion's marker.
///
/// A label never recorded has neither directory nor hint and shows no
/// evidence, and a hint that merely names fewer changes than the bindings
/// is a lossy hint, not a lost binding. A binding and a hint entry deleted
/// together leave no evidence either; only the deep audit sees that.
// trace:BUG-2531 | ai:claude
pub(crate) fn binding_lost<T: PartialEq>(hint: Option<&[T]>, bound: Option<&[T]>) -> bool {
    match hint {
        Some(hint) => {
            let bound = bound.unwrap_or_default();
            hint.iter().any(|change| !bound.contains(change))
        }
        None => bound.is_some(),
    }
}

// trace:BUG-2531 | ai:codex
// trace:BUG-2531 | ai:claude
pub(crate) fn scan_bindings(root: &Path, work: &mut Budget) -> Result<Option<Scan>, Unsafe> {
    let Some((ledger, _)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let mut scan = Scan {
        ledger_id: Some(dir_id(&ledger)?),
        ..Default::default()
    };
    for name in list_pinned(&ledger, &ledger_path)? {
        let Some(key) = change_ledger_name(&name) else {
            continue;
        };
        if scan.changes.len() >= MAX_CHANGE_LEDGERS {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: more than {MAX_CHANGE_LEDGERS} change ledgers"
            )));
        }
        // One budget for the entire scan, including retries. Never reset this
        // per file: 4096 individually legal files can otherwise cost 64 GiB.
        let Some((events, _, stat)) = read_change(&ledger, &name, &key, work)
            .map_err(|e| discovery_refusal(root, &name, &e))?
        else {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: {name} vanished while it was read"
            )));
        };
        scan.read.push((name.clone(), stat));
        // Bind exactly what a writer binds: every spec an event carries,
        // whatever its spelling.
        for spec in events
            .iter()
            .filter_map(|e| e.spec.as_deref().map(binding_key))
        {
            scan.bound.entry(spec).or_default().insert(key.clone());
        }
        scan.changes.insert(key);
    }
    Ok(Some(scan))
}

/// Bytes the completeness marker may occupy.
const MAX_MARKER_BYTES: u64 = 4096;

/// A refusal that names the one fix: rebuilding the bindings.
// trace:BUG-2531 | ai:claude
fn not_bound(why: String) -> Unsafe {
    Unsafe(format!(
        "{why}; run `{FIX_COMMAND}` to rebuild the gate-ledger bindings"
    ))
}

/// Whether selection may trust the bindings: `Ok(true)` with a supported
/// marker and older writers fenced out, `Ok(false)` for a ledger with no
/// change ledger at all (nothing to bind), otherwise a refusal naming the
/// fix. Never a scan of payloads.
///
/// The fence is part of the certificate: a binary older than format 2
/// appends without binding, so a marker beside an open legacy lock path
/// (the fence was removed to let such a binary write) certifies nothing.
// trace:BUG-2531 | ai:claude
fn require_complete(root: &Path, ledger: &File, ledger_path: &Path) -> Result<bool, Unsafe> {
    let mut tiny = Budget::with_limits(MAX_MARKER_BYTES, 0);
    match read_entry(ledger, MARKER_FILE, MAX_MARKER_BYTES, &mut tiny)? {
        Some(body) => {
            let marker: Marker = serde_json::from_slice(&body).map_err(|e| {
                not_bound(format!(
                    "refusing gate-ledger selection: the bindings marker is unreadable ({e})"
                ))
            })?;
            if marker.format != MARKER_FORMAT {
                return Err(not_bound(format!(
                    "refusing gate-ledger selection: the bindings marker has unsupported format {}",
                    marker.format
                )));
            }
            if !super::store::writer_fenced(root) {
                return Err(not_bound(
                    "refusing gate-ledger selection: older aida binaries are not fenced out of this ledger, so it may hold reviews without a binding"
                        .into(),
                ));
            }
            Ok(true)
        }
        None => {
            let unbound = list_pinned(ledger, ledger_path)?
                .iter()
                .any(|n| change_ledger_name(n).is_some());
            if unbound {
                return Err(not_bound(
                    "refusing gate-ledger selection: the spec-to-change bindings of this ledger are not complete"
                        .into(),
                ));
            }
            Ok(false)
        }
    }
}

/// One change ledger read for this discovery.
struct Cached {
    events: Vec<GateEvent>,
    bytes: u64,
    /// The file's stat identity when read: what a shared discovery
    /// re-validates before each reuse. trace:BUG-2543 | ai:claude
    stat: EntryStat,
    /// Charged to a caller's selection budget (not only the work budget).
    selected: bool,
}

/// The changes bound to the normalized label `spec` under the pinned
/// `ledger` directory, strictly: every entry must be a regular file named by
/// a change key. `None` when the label has no bindings directory: it was
/// never bound. Without a marker (`marked` false) an absent bindings
/// directory binds nothing.
// trace:BUG-2531 | ai:claude
fn list_bound(
    ledger: &File,
    ledger_path: &Path,
    marked: bool,
    spec: &str,
) -> Result<Option<Vec<String>>, Unsafe> {
    let refuse =
        |what: String, e: std::io::Error| not_bound(format!("refusing gate-ledger {what}: {e}"));
    let mut changes = Vec::new();
    let bindings = match openat(ledger, BINDINGS_DIR, libc::O_DIRECTORY) {
        Ok(d) => Some(d),
        Err(e) if missing(&e) && !marked => None,
        Err(e) => return Err(refuse("bindings directory".into(), e)),
    };
    let spec_dir = match bindings
        .as_ref()
        .map(|b| openat(b, spec, libc::O_DIRECTORY))
    {
        None => None,
        Some(Ok(d)) => Some(d),
        Some(Err(e)) if missing(&e) => None,
        Some(Err(e)) => return Err(refuse(format!("bindings of {spec}"), e)),
    };
    let Some(dir) = spec_dir else {
        return Ok(None);
    };
    {
        let path = ledger_path.join(BINDINGS_DIR).join(spec);
        for name in list_pinned(&dir, &path)? {
            let key = ChangeKey::parse(&name)
                .map(|c| c.as_key())
                .filter(|k| *k == name)
                .ok_or_else(|| {
                    not_bound(format!(
                        "refusing gate-ledger binding {spec}/{name}: it does not name a change"
                    ))
                })?;
            let regular = openat(&dir, &name, 0)
                .and_then(|f| f.metadata())
                .map_err(|e| refuse(format!("binding {spec}/{name}"), e))?
                .is_file();
            if !regular {
                return Err(not_bound(format!(
                    "refusing gate-ledger binding {spec}/{name}: it is not a regular file"
                )));
            }
            changes.push(key);
        }
    }
    Ok(Some(changes))
}

/// Bound-only discovery (BUG-2531): an operation reads the requested
/// labels' write-ahead bindings and only the change ledgers they name, plus
/// the by-spec hints, which may add a change but never hide one. Only a
/// label with evidence of a lost binding ([`binding_lost`]) is answered from
/// a full read instead ([`Discovery::spec_changes`]); a label never recorded
/// reads no change ledger. The completeness marker is checked when the
/// discovery opens.
///
/// BUG-2517: the mis-key ownership index is built from the same bound
/// ledgers. A copy is disqualified only by a phase-3 witness with the same
/// fingerprint, which includes the spec, so the witnesses of a candidate are
/// exactly on the changes bound to its spec. Each ledger is folded into the
/// index once per [`Inventory`], which every discovery of one
/// [`DiscoveryScope`] shares while it stays current (BUG-2584).
// trace:BUG-2531 | ai:codex
// trace:BUG-2531 | ai:claude
// trace:BUG-2517 | ai:claude
pub(crate) struct Discovery {
    ledger: File,
    ledger_path: std::path::PathBuf,
    by_spec: Option<File>,
    /// The completeness marker was present (otherwise the ledger held no
    /// change ledger when the discovery opened).
    marked: bool,
    /// The ownership index; shared with the other discoveries of the active
    /// [`DiscoveryScope`] only after [`Inventory::current`] held at open.
    // trace:BUG-2584 | ai:claude
    inventory: std::rc::Rc<std::cell::RefCell<Inventory>>,
    /// Discovery's own work (ownership reads it makes itself), separate
    /// from selections. An inherited inventory costs it nothing, so a
    /// discovery that joins a shared inventory refuses on work only where a
    /// fresh one would.
    work: Budget,
    /// Per listed label, its bound changes; `None` when it has no bindings
    /// directory.
    bound: std::collections::HashMap<String, Option<Vec<String>>>,
    /// The store root, for the full read a selection falls back to, and
    /// what that read found beyond the bindings: per label, the changes
    /// whose ledger carries it although no binding names them.
    // trace:BUG-2531 | ai:claude
    root: std::path::PathBuf,
    recovered: std::collections::HashMap<String, Vec<String>>,
    /// The full read `recovered` came from, if any.
    full: Option<std::rc::Rc<Scan>>,
    /// Per label, its by-spec hint as read once by this discovery, with the
    /// hint file's stat identity (`None` when it has none).
    // trace:BUG-2531 | ai:claude
    hints: std::collections::HashMap<String, (Option<Vec<String>>, Option<EntryStat>)>,
    /// Ledgers are read once across a list operation. Their total retained
    /// payload is bounded by the work and selection budgets.
    cache: std::collections::HashMap<String, Cached>,
}

/// Every change a label's reviews may sit on, as a selection and the
/// ownership index both take them: its bindings, its hint and, on evidence
/// of a lost binding, every change a full read finds carrying it.
// trace:BUG-2531 | ai:claude
struct OwnerSet {
    changes: Vec<String>,
    /// What it was resolved from: the label's bindings, its hint's stat
    /// identity, and whether a full read was needed.
    folded: Folded,
    lost: bool,
}

/// What one label's ownership was folded from, re-validated before an
/// [`Inventory`] is reused.
// trace:BUG-2531 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
struct Folded {
    bound: Option<Vec<String>>,
    hint: Option<EntryStat>,
}

/// The mis-key ownership index of a ledger and exactly what it was folded
/// from: per spec, the bindings it was folded under, and per change ledger,
/// the stat identity it was read with. It retains no review payload, only
/// witness fingerprints, charged to the work budget of the discovery that
/// read them.
///
/// BUG-2584: a train's re-gate, carry-over proof and retries open several
/// strict discoveries of one ledger. Inside a [`DiscoveryScope`] they share
/// one inventory instead of re-reading and re-fingerprinting every ledger
/// bound to their specs, while each still reads its selected events fresh,
/// under its own selection budget, through freshly pinned directories.
// trace:BUG-2584 | ai:claude
// trace:BUG-2531 | ai:claude
pub(crate) struct Inventory {
    ledger_id: (u64, u64),
    marked: bool,
    owners: super::project::MiskeyOwners,
    specs: std::collections::HashMap<String, Folded>,
    changes: std::collections::HashMap<String, EntryStat>,
    retained: u64,
    /// The full read a label's ownership was recovered from (evidence of a
    /// lost binding): reused only while it is current. trace:BUG-2531 | ai:claude
    full: Option<std::rc::Rc<Scan>>,
    /// A population error left this index part-built: it is never reused.
    /// trace:BUG-2531 BUG-2584 | ai:claude
    poisoned: bool,
}

/// The stat identity of `spec`'s by-spec hint under `ledger_path`; `None`
/// when it has none. trace:BUG-2531 | ai:claude
fn hint_stat(ledger_path: &Path, spec: &str) -> Option<EntryStat> {
    std::fs::symlink_metadata(ledger_path.join("by-spec").join(format!("{spec}.json")))
        .ok()
        .map(|m| EntryStat::of(&m))
}

impl Inventory {
    // trace:BUG-2584 | ai:claude
    fn new(ledger_id: (u64, u64), marked: bool) -> Self {
        Self {
            ledger_id,
            marked,
            // trace:BUG-2517 | ai:claude
            owners: super::project::MiskeyOwners::new(std::iter::empty()),
            specs: Default::default(),
            changes: Default::default(),
            retained: 0,
            full: None,
            poisoned: false,
        }
    }

    /// Whether a discovery that opened `ledger` (pinned at `ledger_path`,
    /// directory identity `ledger_id`, marker state `marked`) would fold
    /// exactly this index: the same ledger directory and marker, every spec
    /// still bound to the changes it was folded under, with the same hint,
    /// the full read any spec was recovered from still current, and every
    /// folded ledger with the stat identity it was read with. Any append,
    /// binding, deleted binding, hint rewrite, hard link, replacement or
    /// non-regular file differs. Only stats and directory listings are
    /// read, never a payload, so a FIFO cannot block. A poisoned index
    /// (one a failed read left part-built) is never current.
    // trace:BUG-2584 | ai:claude
    // trace:BUG-2543 | ai:claude
    // trace:BUG-2531 | ai:claude
    fn current(
        &self,
        root: &Path,
        ledger: &File,
        ledger_path: &Path,
        ledger_id: (u64, u64),
        marked: bool,
    ) -> bool {
        !self.poisoned
            && self.ledger_id == ledger_id
            && self.marked == marked
            && self.retained <= MAX_DISCOVERY_BYTES
            && self.specs.iter().all(|(spec, folded)| {
                list_bound(ledger, ledger_path, marked, spec).is_ok_and(|now| now == folded.bound)
                    && hint_stat(ledger_path, spec) == folded.hint
            })
            && self.full.as_ref().is_none_or(|scan| scan.current(root))
            && self.changes.iter().all(|(key, stat)| {
                std::fs::symlink_metadata(ledger_path.join(format!("{key}.jsonl")))
                    .is_ok_and(|now| EntryStat::of(&now) == *stat)
            })
    }
}

/// Open bound-only discovery of the gate ledger under `root`; `Ok(None)`
/// when it does not exist. Inside a [`DiscoveryScope`] it joins the scope's
/// ownership [`Inventory`] of `root` while that is current, and otherwise
/// makes its own the scope's; an absent or refused discovery leaves none.
// trace:BUG-2403 | ai:claude
// trace:BUG-2584 | ai:claude
pub(crate) fn discover(root: &Path) -> Result<Option<Discovery>, Unsafe> {
    #[allow(unused_mut)]
    let mut work = Budget {
        bytes: MAX_DISCOVERY_BYTES,
        events: MAX_SELECTION_EVENTS,
    };
    #[cfg(test)]
    if let Some((bytes, events)) = discovery_work_tests::SHARED_WORK.with(|w| w.get()) {
        work = Budget::with_limits(bytes, events);
    }
    let found = open_discovery(root, work, true);
    if !matches!(found, Ok(Some(_))) {
        SHARED.with(|shared| {
            if let Some(m) = shared.borrow_mut().as_mut() {
                m.remove(root);
            }
        });
    }
    found
}

/// A discovery of its own (never shared) whose ownership reads charge `work`.
// trace:BUG-2531 | ai:claude
pub(crate) fn discover_with_budget(root: &Path, work: Budget) -> Result<Option<Discovery>, Unsafe> {
    open_discovery(root, work, false)
}

// trace:BUG-2531 | ai:claude
// trace:BUG-2584 | ai:claude
fn open_discovery(root: &Path, work: Budget, share: bool) -> Result<Option<Discovery>, Unsafe> {
    let Some((ledger, by_spec)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let ledger_id = dir_id(&ledger)?; // trace:BUG-2543 | ai:claude
    let marked = require_complete(root, &ledger, &ledger_path)?;
    let joined = if share {
        SHARED.with(|shared| {
            let mut shared = shared.borrow_mut();
            let m = shared.as_mut()?;
            if let Some(inventory) = m.get(root).filter(|inventory| {
                inventory
                    .borrow()
                    .current(root, &ledger, &ledger_path, ledger_id, marked)
            }) {
                return Some(inventory.clone());
            }
            let inventory =
                std::rc::Rc::new(std::cell::RefCell::new(Inventory::new(ledger_id, marked)));
            m.insert(root.to_path_buf(), inventory.clone());
            Some(inventory)
        })
    } else {
        None
    };
    let inventory = joined.unwrap_or_else(|| {
        std::rc::Rc::new(std::cell::RefCell::new(Inventory::new(ledger_id, marked)))
    });
    Ok(Some(Discovery {
        ledger,
        ledger_path,
        by_spec,
        marked,
        inventory,
        work,
        bound: Default::default(),
        root: root.to_path_buf(),
        recovered: Default::default(),
        full: None,
        hints: Default::default(),
        cache: Default::default(),
    }))
}

impl Discovery {
    /// The changes bound to the normalized label `spec`, listed once.
    // trace:BUG-2531 | ai:claude
    fn bound_changes(&mut self, spec: &str) -> Result<Vec<String>, Unsafe> {
        Ok(self.listed(spec)?.unwrap_or_default())
    }

    /// [`Discovery::bound_changes`], telling a label without a bindings
    /// directory (`None`: never bound) from one whose directory is empty.
    // trace:BUG-2531 | ai:claude
    fn listed(&mut self, spec: &str) -> Result<Option<Vec<String>>, Unsafe> {
        if let Some(changes) = self.bound.get(spec) {
            return Ok(changes.clone());
        }
        let changes = list_bound(&self.ledger, &self.ledger_path, self.marked, spec)?;
        self.bound.insert(spec.to_string(), changes.clone());
        Ok(changes)
    }

    /// Read `key`'s ledger once into the cache; `false` when it has none. A
    /// selection read charges `selection` (once per ledger); an ownership
    /// read charges the discovery's work budget. Every reviewed spec the
    /// ledger carries must be bound to it, or the bindings are inconsistent.
    // trace:BUG-2531 | ai:claude
    fn load(&mut self, key: &str, selection: Option<&mut Budget>) -> Result<bool, Unsafe> {
        if let Some(cached) = self.cache.get_mut(key) {
            if let Some(budget) = selection {
                if !cached.selected {
                    budget.spend_bytes(cached.bytes)?;
                    for _ in 0..cached.events.len() {
                        budget.spend_event()?;
                    }
                    cached.selected = true;
                }
            }
            return Ok(true);
        }
        let selected = selection.is_some();
        let budget = match selection {
            Some(budget) => budget,
            None => &mut self.work,
        };
        let name = format!("{key}.jsonl");
        let Some((events, bytes, stat)) = read_change(&self.ledger, &name, key, budget)? else {
            return Ok(false);
        };
        let reviewed: std::collections::BTreeSet<String> = events
            .iter()
            .filter(|e| e.verdict_record().is_some())
            .filter_map(|e| e.spec.as_deref().map(binding_key))
            .collect();
        for spec in reviewed {
            // A change the full read recovered for a label is read although
            // its binding is gone. trace:BUG-2531 | ai:claude
            let recovered = self
                .recovered
                .get(&spec)
                .is_some_and(|changes| changes.iter().any(|c| c == key));
            if !recovered && !self.bound_changes(&spec)?.iter().any(|c| c == key) {
                return Err(not_bound(format!(
                    "refusing gate-ledger selection: {key} records {spec} reviews without a binding"
                )));
            }
        }
        self.cache.insert(
            key.to_string(),
            Cached {
                events,
                bytes,
                stat,
                selected,
            },
        );
        Ok(true)
    }

    // trace:BUG-2531 | ai:codex
    fn select(&mut self, key: &str, budget: &mut Budget) -> Result<Option<&[GateEvent]>, Unsafe> {
        if !self.load(key, Some(budget))? {
            return Ok(None);
        }
        Ok(Some(&self.cache[key].events))
    }

    /// Fold every ledger `spec`'s reviews may sit on into the ownership
    /// index: the same complete set a selection of the label reads
    /// ([`Discovery::owner_set`]), so a deleted binding of a witness's
    /// change cannot let a foreign copy through. Any error poisons the
    /// index, so no later discovery reuses it part-built.
    // trace:BUG-2531 | ai:claude
    // trace:BUG-2517 | ai:claude
    fn load_owners(&mut self, spec: &str) -> Result<(), Unsafe> {
        if self.inventory.borrow().specs.contains_key(spec) {
            return Ok(());
        }
        let folded = self.fold_owners(spec);
        if folded.is_err() {
            self.poison();
        }
        folded
    }

    /// Never reuse this discovery's index again: drop it from the active
    /// [`DiscoveryScope`] and continue on a fresh, unshared one, so a retry
    /// pays exactly what a fresh discovery pays.
    // trace:BUG-2531 BUG-2584 | ai:claude
    fn poison(&mut self) {
        let stale = self.inventory.clone();
        let (ledger_id, marked) = {
            let mut inventory = stale.borrow_mut();
            inventory.poisoned = true;
            (inventory.ledger_id, inventory.marked)
        };
        SHARED.with(|shared| {
            if let Some(m) = shared.borrow_mut().as_mut() {
                m.retain(|_, inventory| !std::rc::Rc::ptr_eq(inventory, &stale));
            }
        });
        self.inventory =
            std::rc::Rc::new(std::cell::RefCell::new(Inventory::new(ledger_id, marked)));
    }

    // trace:BUG-2531 | ai:claude
    // trace:BUG-2584 | ai:claude
    fn fold_owners(&mut self, spec: &str) -> Result<(), Unsafe> {
        let mut work = std::mem::take(&mut self.work);
        let set = self.owner_set(spec, &mut work);
        self.work = work;
        let set = set?;
        for key in set.changes {
            if self.inventory.borrow().changes.contains_key(&key) {
                continue;
            }
            if !self.load(&key, None)? {
                return Err(not_bound(format!(
                    "refusing gate-ledger ownership: {spec} is bound to {key}, whose ledger is missing"
                )));
            }
            let cached = &self.cache[&key];
            let retained = {
                // trace:BUG-2584 | ai:claude
                let mut inventory = self.inventory.borrow_mut();
                let retained = inventory.owners.add(&cached.events);
                inventory.retained = inventory.retained.saturating_add(retained);
                inventory.changes.insert(key, cached.stat.clone());
                retained
            };
            self.work.spend_bytes(retained)?;
        }
        let mut inventory = self.inventory.borrow_mut();
        if set.lost {
            inventory.full = self.full.clone();
        }
        inventory.specs.insert(spec.to_string(), set.folded);
        Ok(())
    }

    /// Filter `events` through the ownership index of the specs they carry,
    /// loading those specs' bound ledgers first. On a refusal `events` is
    /// left untouched.
    // trace:BUG-2517 | ai:codex
    // trace:BUG-2531 | ai:claude
    pub(crate) fn filter_events(&mut self, events: &mut Vec<GateEvent>) -> Result<(), Unsafe> {
        for spec in super::project::witness_specs(events) {
            self.load_owners(&spec)?;
        }
        // Every candidate's spec has a binding key, so the index holds every
        // witness that could disqualify it: no review is withheld unproven.
        self.inventory.borrow().owners.filter(events);
        Ok(())
    }

    /// Every event on `change`'s ledger, read once per discovery under the
    /// caller's selection budget and filtered through the ownership index.
    /// Empty when the change has no ledger.
    // trace:BUG-2517 | ai:codex
    pub(crate) fn change_events(
        &mut self,
        change: ChangeKey,
        budget: &mut Budget,
    ) -> Result<Vec<GateEvent>, Unsafe> {
        let key = change.as_key();
        let Some(events) = self.select(&key, budget)? else {
            return Ok(Vec::new());
        };
        let mut events = events.to_vec();
        self.filter_events(&mut events)?;
        Ok(events)
    }

    /// Validate a bound ledger without applying verdict-copy ownership rules:
    /// holds fold the physical change ledger, including its non-verdict events.
    // trace:BUG-2626 | ai:codex
    pub(crate) fn require_change(
        &mut self,
        change: ChangeKey,
        budget: &mut Budget,
    ) -> Result<(), Unsafe> {
        if self.select(&change.as_key(), budget)?.is_none() {
            return Err(not_bound(format!(
                "refusing hold discovery: bound ledger {} is missing",
                change.as_key()
            )));
        }
        Ok(())
    }

    fn ledger_exists(&self, key: &str) -> Result<bool, Unsafe> {
        match openat(&self.ledger, &format!("{key}.jsonl"), 0) {
            Ok(_) => Ok(true),
            Err(e) if missing(&e) => Ok(false),
            Err(e) => Err(Unsafe(format!(
                "refusing gate-ledger entry {key}.jsonl: {e}"
            ))),
        }
    }

    /// The changes `spec`'s by-spec index names, strictly: a hint that may
    /// add a change, never hide one; `None` when the label has no index. A
    /// named change without a ledger refuses.
    /// Read once per discovery (charged to the first caller's budget), with
    /// the hint file's stat identity.
    // trace:BUG-2531 | ai:claude
    fn index_hint(
        &mut self,
        spec: &str,
        budget: &mut Budget,
    ) -> Result<(Option<Vec<String>>, Option<EntryStat>), Unsafe> {
        if let Some(hint) = self.hints.get(spec) {
            return Ok(hint.clone());
        }
        let hint = self.read_index_hint(spec, budget)?;
        self.hints.insert(spec.to_string(), hint.clone());
        Ok(hint)
    }

    // trace:BUG-2531 | ai:claude
    fn read_index_hint(
        &self,
        spec: &str,
        budget: &mut Budget,
    ) -> Result<(Option<Vec<String>>, Option<EntryStat>), Unsafe> {
        let Some(by_spec) = &self.by_spec else {
            return Ok((None, None));
        };
        let index_name = format!("{spec}.json");
        let Some((body, meta)) = read_entry_meta(by_spec, &index_name, MAX_INDEX_BYTES, budget)?
        else {
            return Ok((None, None));
        };
        let index: SpecIndex = serde_json::from_slice(&body)
            .map_err(|e| Unsafe(format!("refusing gate-ledger index {index_name}: {e}")))?;
        if index.changes.len() > MAX_CHANGES_PER_SPEC {
            return Err(Unsafe(format!(
                "refusing gate-ledger index {index_name}: it names more than {MAX_CHANGES_PER_SPEC} changes"
            )));
        }
        let mut out = Vec::new();
        for c in &index.changes {
            let key = ChangeKey::parse(c).map(|k| k.as_key()).ok_or_else(|| {
                Unsafe(format!(
                    "refusing gate-ledger index {index_name}: {c:?} is not a change key"
                ))
            })?;
            // trace:BUG-2517 | ai:codex
            // Retain the strict reader's missing-ledger refusal rather than
            // treating absent evidence as empty.
            if !self.ledger_exists(&key)? {
                return Err(Unsafe(format!(
                    "refusing gate-ledger index {index_name}: it names {key}, whose ledger is missing"
                )));
            }
            out.push(key);
        }
        Ok((Some(out), Some(EntryStat::of(&meta))))
    }

    /// `spec`'s unbound verdict events, strictly.
    fn unbound(&self, spec: &str, budget: &mut Budget) -> Result<Vec<GateEvent>, Unsafe> {
        let Some(by_spec) = &self.by_spec else {
            return Ok(Vec::new());
        };
        let unbound_name = format!("{spec}.jsonl");
        Ok(
            match read_entry(by_spec, &unbound_name, MAX_LEDGER_BYTES, budget)? {
                Some(body) => parse_events(&unbound_name, &body, spec, budget)?
                    .into_iter()
                    .filter(|e| e.verdict_record().is_some())
                    .collect(),
                None => Vec::new(),
            },
        )
    }

    /// The bound changes of `spec` together with its index hints. Only on
    /// evidence that a binding of the label was lost ([`binding_lost`]: a
    /// deleted binding would hide a newer refusal) is every change whose
    /// ledger carries the label added from a full read
    /// ([`recorded_changes`]). A label never recorded, such as the unused
    /// alias of a two-label selection, reads no change ledger at all.
    // trace:BUG-2531 | ai:claude
    fn spec_changes(&mut self, spec: &str, budget: &mut Budget) -> Result<Vec<String>, Unsafe> {
        Ok(self.owner_set(spec, budget)?.changes)
    }

    /// [`Discovery::spec_changes`], with what it was resolved from. A label
    /// bound under a `~` key ([`binding_key`]) has no hint, so the absence
    /// of one is no evidence there.
    // trace:BUG-2531 | ai:claude
    fn owner_set(&mut self, spec: &str, budget: &mut Budget) -> Result<OwnerSet, Unsafe> {
        let listed = self.listed(spec)?;
        let keyed = spec_key(spec).as_deref() == Some(spec);
        let (hint, hint_stat) = if keyed {
            self.index_hint(spec, budget)?
        } else {
            (None, None)
        };
        let lost = keyed && binding_lost(hint.as_deref(), listed.as_deref());
        let mut changes = listed.clone().unwrap_or_default();
        if lost {
            let found = match self.recovered.get(spec) {
                Some(found) => found.clone(),
                None => {
                    let scan = full_read(&self.root)?;
                    let found: Vec<String> = scan
                        .as_deref()
                        .map(|scan| recorded_in(scan, spec))
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|key| !changes.contains(key))
                        .collect();
                    self.full = scan;
                    self.recovered.insert(spec.to_string(), found.clone());
                    found
                }
            };
            changes.extend(found);
        }
        for key in hint.unwrap_or_default() {
            if !changes.contains(&key) {
                changes.push(key);
            }
        }
        Ok(OwnerSet {
            changes,
            folded: Folded {
                bound: listed,
                hint: hint_stat,
            },
            lost,
        })
    }

    /// Every verdict event for each spec label in `labels`, oldest first per
    /// label: the events of every change bound to the label (or named by its
    /// index) plus the label's unbound ledger (a record already bound to a
    /// change is read once, from the change). A bound change whose ledger is
    /// missing, or a malformed index, refuses.
    // trace:BUG-2403 | ai:claude
    // trace:BUG-2531 | ai:claude
    pub(crate) fn events_for(
        &mut self,
        labels: &[String],
        budget: &mut Budget,
    ) -> Result<Vec<(String, Vec<GateEvent>)>, Unsafe> {
        let mut out = Vec::new();
        for label in labels {
            let spec =
                spec_key(label).ok_or_else(|| Unsafe(format!("{label:?} is not a spec label")))?;
            let mut events = Vec::new();
            for key in self.spec_changes(&spec, budget)? {
                let Some(found) = self.select(&key, budget)? else {
                    return Err(not_bound(format!(
                        "refusing gate-ledger selection: {spec} is bound to {key}, whose ledger is missing"
                    )));
                };
                events.extend(
                    found
                        .iter()
                        .filter(|e| {
                            e.verdict_record().is_some()
                                && e.spec
                                    .as_deref()
                                    .is_some_and(|s| s.trim().eq_ignore_ascii_case(&spec))
                        })
                        .cloned(),
                );
            }
            let unbound: Vec<GateEvent> = self
                .unbound(&spec, budget)?
                .into_iter()
                .filter(|u| {
                    !events
                        .iter()
                        .any(|b| b.verdict_record() == u.verdict_record())
                })
                .collect();
            events.extend(unbound);
            // trace:BUG-2517 | ai:codex
            self.filter_events(&mut events)?;
            events.sort_by(|a, b| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)));
            out.push((spec, events));
        }
        Ok(out)
    }

    /// A member's unbound evidence and the changes recorded against it (its
    /// bindings and index hints), read through this operation's pinned
    /// directories under the same selection budget.
    // trace:STORY-2324 | ai:claude
    // trace:BUG-2517 | ai:codex
    // trace:BUG-2531 | ai:claude
    pub(crate) fn spec_entries(
        &mut self,
        label: &str,
        budget: &mut Budget,
    ) -> Result<(Vec<GateEvent>, Vec<ChangeKey>), Unsafe> {
        let spec =
            spec_key(label).ok_or_else(|| Unsafe(format!("{label:?} is not a spec label")))?;
        let unbound = self.unbound(&spec, budget)?;
        let changes = self
            .spec_changes(&spec, budget)?
            .iter()
            .filter_map(|k| ChangeKey::parse(k))
            .collect();
        Ok((unbound, changes))
    }
}

/// Ignore an old spec-routed copy when the PR-keyed handshake proves that
/// the very same act of review belongs to another change. Current heads
/// alone cannot disqualify historical reviews (including force-pushed heads).
/// This is read-only: neither ledger nor sequence history is repaired.
// trace:BUG-2517 | ai:codex
pub(crate) fn filter_miskeyed(
    root: &Path,
    events: Vec<GateEvent>,
    budget: &mut Budget,
) -> Result<Vec<GateEvent>, Unsafe> {
    // The by-spec files are hints, not completeness evidence: an earlier
    // append may have succeeded while indexing failed. Use the bindings.
    let Some(mut discovery) = discover_with_budget(root, std::mem::take(budget))? else {
        // Input may have been read before the directory disappeared. An
        // absent corpus cannot establish ownership of those review copies.
        // trace:BUG-2517 | ai:codex
        if !events.is_empty() {
            return Err(Unsafe(
                "refusing gate-ledger ownership: ledger disappeared after input was read".into(),
            ));
        }
        return Ok(events);
    };
    let mut events = events;
    let filtered = discovery.filter_events(&mut events);
    *budget = std::mem::take(&mut discovery.work);
    filtered?;
    Ok(events)
}

/// [`filter_miskeyed`] for the lossy single-label readers: when discovery is
/// incomplete (a bound ledger is malformed, unreadable or over budget, or
/// the bindings are not complete) every review copy a foreign change could
/// own is withheld, while evidence no witness could disqualify — holds,
/// carry-over proofs, phase-3 records — stays readable. Never presents raw
/// foreign evidence; never rewrites. Events no witness could disqualify need
/// no discovery at all.
// trace:BUG-2517 | ai:claude
// trace:BUG-2531 | ai:claude
pub(crate) fn filter_miskeyed_or_fail_closed(
    root: &Path,
    events: Vec<GateEvent>,
) -> Vec<GateEvent> {
    let mut events = events;
    if super::project::needs_witnesses(&events) {
        // Inside a [`DiscoveryScope`] the discovery joins the scope's
        // ownership inventory while it is current; it refuses exactly where
        // a fresh one does. trace:BUG-2543 BUG-2584 | ai:claude
        if let Ok(Some(mut discovery)) = discover(root) {
            let mut filtered = events.clone();
            if discovery.filter_events(&mut filtered).is_ok() {
                return filtered;
            }
        }
    }
    // The directory can disappear after a legacy reader has read its
    // input vector. Absence then proves no ownership for those copies.
    // trace:BUG-2517 | ai:codex
    super::project::MiskeyOwners::fail_closed(&mut events);
    events
}

/// The stat identity of one ledger file: which file (device, inode, link
/// count, type), its length and its change times. Length, inode and link
/// count are exact, so an append, a replacement, a hard link or a
/// non-regular file always differs. None of its content is retained.
// trace:BUG-2543 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryStat {
    dev: u64,
    ino: u64,
    nlink: u64,
    mode: u32,
    len: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}

impl EntryStat {
    fn of(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
            nlink: meta.nlink(),
            mode: meta.mode(),
            len: meta.len(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}

// trace:BUG-2543 | ai:claude
fn dir_id(dir: &File) -> Result<(u64, u64), Unsafe> {
    use std::os::unix::fs::MetadataExt;
    let meta = dir
        .metadata()
        .map_err(|e| Unsafe(format!("refusing gate-ledger discovery: {e}")))?;
    Ok((meta.dev(), meta.ino()))
}

type SharedDiscoveries =
    std::collections::HashMap<std::path::PathBuf, std::rc::Rc<std::cell::RefCell<Inventory>>>;

thread_local! {
    /// The active [`DiscoveryScope`]'s one ownership inventory per store root.
    // trace:BUG-2543 BUG-2551 BUG-2584 | ai:claude
    static SHARED: std::cell::RefCell<Option<SharedDiscoveries>> =
        const { std::cell::RefCell::new(None) };
}

/// One gate-ledger inventory shared by strict discoveries and single-key
/// verdict reads on this thread while alive (listings, reconciliation, trains).
///
/// Not a snapshot: every discovery opens its directories afresh (the same
/// path checks: a linked or non-directory component refuses) and reads
/// every selected event fresh from disk under its own selection budget; only
/// the ownership [`Inventory`] is reused, and only while
/// [`Inventory::current`] holds when a discovery opens: the same ledger
/// directory and marker, the same bindings for every spec it folded, and an
/// unchanged stat identity for every ledger it folded. Any difference (an
/// append by this or any other writer, a new binding, a hard link, a
/// replacement by a symlink or FIFO) starts a new inventory, which becomes
/// the shared one. A failed or absent discovery leaves none, so every
/// refusal is exactly the fresh path's. Nested scopes join the outermost one.
// trace:BUG-2543 BUG-2551 | ai:claude
// trace:BUG-2584 | ai:claude
pub(crate) struct DiscoveryScope {
    owner: bool,
}

impl DiscoveryScope {
    pub(crate) fn enter() -> Self {
        SHARED.with(|shared| {
            let mut shared = shared.borrow_mut();
            let owner = shared.is_none();
            if owner {
                *shared = Some(Default::default());
            }
            Self { owner }
        })
    }
}

impl Drop for DiscoveryScope {
    fn drop(&mut self) {
        if self.owner {
            SHARED.with(|shared| *shared.borrow_mut() = None);
        }
    }
}

/// STORY-2324: every event on `change`'s ledger under `root`, read strictly
/// (all kinds, not just verdicts); empty when the ledger or the file does
/// not exist.
// trace:STORY-2324 | ai:claude
pub(crate) fn read_change_events(
    root: &Path,
    change: ChangeKey,
    budget: &mut Budget,
) -> Result<Vec<GateEvent>, Unsafe> {
    match discover(root)? {
        Some(mut d) => d.change_events(change, budget),
        None => Ok(Vec::new()),
    }
}

/// Active change-ledger sizes, largest first; no event payloads are read.
// trace:BUG-2531 | ai:codex
// trace:BUG-2542 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct Capacity {
    pub(crate) total: u64,
    pub(crate) files: Vec<(String, u64)>,
}

/// Read-only capacity probe for doctor; no event payloads are read.
// trace:BUG-2531 | ai:codex
// trace:BUG-2542 | ai:claude
pub(crate) fn capacity(root: &Path) -> Result<Capacity, Unsafe> {
    let Some((ledger, _)) = ledger_dirs(root)? else {
        return Ok(Capacity::default());
    };
    let mut out = Capacity::default();
    for name in list_pinned(&ledger, &super::ledger_dir(root))? {
        if !name
            .strip_suffix(".jsonl")
            .and_then(ChangeKey::parse)
            .is_some_and(|c| format!("{}.jsonl", c.as_key()) == name)
        {
            continue;
        }
        let file = openat(&ledger, &name, 0)
            .map_err(|e| Unsafe(format!("cannot inspect gate-ledger entry {name}: {e}")))?;
        let meta = file.metadata().map_err(|e| Unsafe(e.to_string()))?;
        if !meta.is_file() {
            return Err(Unsafe(format!(
                "gate-ledger entry {name} is not a regular file"
            )));
        }
        out.total = out.total.saturating_add(meta.len());
        out.files.push((name, meta.len()));
    }
    out.files
        .sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

#[cfg(test)]
// trace:BUG-2531 | ai:codex
pub(crate) mod discovery_work_tests {
    use super::*;
    use std::cell::Cell;
    thread_local! {
        pub(crate) static DIRECTORY_ENTRIES: Cell<usize> = const { Cell::new(0) };
        pub(crate) static LOSSY_READ_BYTES: Cell<u64> = const { Cell::new(0) };
        pub(crate) static READ_BYTES: Cell<u64> = const { Cell::new(0) };
        pub(crate) static PARSE_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
        /// Selections answered from a full read ([`recorded_changes`]).
        pub(crate) static FULL_READ_ANSWERS: Cell<usize> = const { Cell::new(0) };
        /// A work budget for [`discover`] (bytes, events) in place of the
        /// production ceiling.
        pub(crate) static SHARED_WORK: Cell<Option<(u64, usize)>> = const { Cell::new(None) };
    }

    fn fixture() -> (tempfile::TempDir, usize) {
        let root = tempfile::tempdir().unwrap();
        let dir = super::super::ledger_dir(root.path());
        std::fs::create_dir_all(&dir).unwrap();
        let mut size = 0;
        for n in 100..120 {
            let mut event = GateEvent::new(
                ChangeKey::pr(n),
                super::super::event::EventBody::Verdict {
                    record: serde_json::from_value(serde_json::json!({
                        "verdict": "request_changes", "findings": ["x".repeat(4096)]
                    }))
                    .unwrap(),
                },
            );
            event.spec = Some("STORY-UNRELATED-1".into());
            event.seq = 1;
            event.at = "2026-11-12T00:00:00Z".into();
            let body = format!("{}\n", serde_json::to_string(&event).unwrap());
            size = body.len();
            std::fs::write(dir.join(format!("PR-{n}.jsonl")), body).unwrap();
        }
        // No by-spec index: it must neither skip refusals nor permit unbounded
        // work to prove absence for a different label.
        (root, size)
    }

    #[test]
    fn bug_2531_discovery_bounds_actual_unrelated_reads_and_parses() {
        let (root, size) = fixture();
        READ_BYTES.with(|n| n.set(0));
        PARSE_ATTEMPTS.with(|n| n.set(0));
        let mut budget = Budget::with_limits((size * 3) as u64, 100);
        let err = scan_bindings(root.path(), &mut budget).err().unwrap();
        assert!(err.0.contains("discovery"), "{err}");
        assert!(err.0.contains("read budget"), "{err}");
        assert_eq!(READ_BYTES.with(Cell::get), (size * 3) as u64);
        assert_eq!(PARSE_ATTEMPTS.with(Cell::get), 3);

        READ_BYTES.with(|n| n.set(0));
        PARSE_ATTEMPTS.with(|n| n.set(0));
        let mut budget = Budget::with_limits((size * 20) as u64, 2);
        let err = scan_bindings(root.path(), &mut budget).err().unwrap();
        assert!(err.0.contains("event budget"), "{err}");
        assert_eq!(READ_BYTES.with(Cell::get), (size * 3) as u64);
        assert_eq!(PARSE_ATTEMPTS.with(Cell::get), 2);
    }

    // trace:BUG-2542 | ai:claude
    #[test]
    fn bug_2542_oversized_ledger_refusal_names_the_file_and_the_fix() {
        let (root, _) = fixture();
        let dir = super::super::ledger_dir(root.path());
        std::fs::File::create(dir.join("PR-3888.jsonl"))
            .unwrap()
            .set_len(MAX_LEDGER_BYTES + 1)
            .unwrap();
        // trace:BUG-2531 | ai:claude — only the bindings bootstrap scans
        // every ledger; selection no longer reads the unrelated one at all.
        let scan = |root: &Path| scan_bindings(root, &mut Budget::default());
        let err = scan(root.path()).err().unwrap();
        let path = dir.join("PR-3888.jsonl");
        assert!(err.0.contains(&path.display().to_string()), "{err}");
        assert!(err.0.contains("over the"), "{err}");
        assert!(err.0.contains("every spec"), "{err}");
        assert!(
            err.0
                .contains(&quarantine_command(root.path(), "PR-3888.jsonl")),
            "{err}"
        );
        assert!(err.0.contains("aida doctor"), "{err}");
        // Following the named fix restores discovery for every other change.
        let q = quarantine_dir(root.path());
        std::fs::create_dir_all(&q).unwrap();
        std::fs::rename(&path, q.join("PR-3888.jsonl")).unwrap();
        assert!(scan(root.path()).unwrap().is_some());
    }

    #[test]
    fn bug_2531_torn_retries_share_the_discovery_read_budget() {
        let root = tempfile::tempdir().unwrap();
        let dir = super::super::ledger_dir(root.path());
        std::fs::create_dir_all(&dir).unwrap();
        let body = b"{\"torn\":true";
        std::fs::write(dir.join("PR-100.jsonl"), body).unwrap();
        READ_BYTES.with(|n| n.set(0));
        PARSE_ATTEMPTS.with(|n| n.set(0));
        let mut budget = Budget::with_limits((body.len() * 2) as u64, 100);
        let err = scan_bindings(root.path(), &mut budget).err().unwrap();
        assert!(err.0.contains("read budget"), "{err}");
        assert_eq!(READ_BYTES.with(Cell::get), (body.len() * 2) as u64);
        assert_eq!(PARSE_ATTEMPTS.with(Cell::get), 0);
    }
}
