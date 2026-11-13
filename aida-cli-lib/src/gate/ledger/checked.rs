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
    change_ledger_name, Marker, BINDINGS_DIR, FIX_COMMAND, MARKER_FILE, MARKER_FORMAT,
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
    use std::os::unix::fs::MetadataExt;
    let file = match openat(dir, name, 0) {
        Ok(f) => f,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(Unsafe(format!("refusing gate-ledger entry {name}: {e}"))),
    };
    let meta = file
        .metadata()
        .map_err(|e| Unsafe(format!("refusing gate-ledger entry {name}: {e}")))?;
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > cap {
        return Err(Unsafe(format!(
            "refusing gate-ledger entry {name}: it must be a singly linked regular file of at most {cap} bytes"
        )));
    }
    // Bound actual I/O before reading, including growth after metadata. One
    // sentinel byte detects overflow; the operation then refuses immediately.
    // trace:BUG-2531 | ai:codex
    if meta.len() > budget.bytes {
        return Err(Unsafe(
            "the aggregate gate-ledger read budget is exhausted".into(),
        ));
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
    Ok(Some(body))
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
) -> Result<Option<(Vec<GateEvent>, u64)>, Unsafe> {
    let mut attempt = 0;
    loop {
        let Some(body) = read_entry(ledger, name, MAX_LEDGER_BYTES, budget)? else {
            return Ok(None);
        };
        let torn = !body.is_empty() && !body.ends_with(b"\n");
        if torn && attempt < TORN_RETRIES {
            attempt += 1;
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }
        let bytes = body.len() as u64;
        return parse_events(name, &body, change, budget).map(|e| Some((e, bytes)));
    }
}

/// The full, strict scan the bindings bootstrap needs: every change ledger
/// and every spec its events carry. It reads every active ledger, so only
/// the maintenance bootstrap ([`super::bindings::bootstrap`]) runs it;
/// selection reads bindings instead.
// trace:BUG-2531 | ai:claude
#[derive(Debug, Default)]
pub(crate) struct Scan {
    pub(crate) changes: std::collections::BTreeSet<String>,
    pub(crate) bound: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

// trace:BUG-2531 | ai:codex
// trace:BUG-2531 | ai:claude
pub(crate) fn scan_bindings(root: &Path, work: &mut Budget) -> Result<Option<Scan>, Unsafe> {
    let Some((ledger, _)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let mut scan = Scan::default();
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
        let Some((events, _)) = read_change(&ledger, &name, &key, work)
            .map_err(|e| Unsafe(format!("refusing gate-ledger discovery: {e}")))?
        else {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: {name} vanished while it was read"
            )));
        };
        // Bind exactly what a writer binds: every spec an event carries.
        for spec in events
            .iter()
            .filter_map(|e| e.spec.as_deref().and_then(spec_key))
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
/// marker, `Ok(false)` for a ledger with no change ledger at all (nothing to
/// bind), otherwise a refusal naming the fix. Never a scan of payloads.
// trace:BUG-2531 | ai:claude
fn require_complete(ledger: &File, ledger_path: &Path) -> Result<bool, Unsafe> {
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
    /// Charged to a caller's selection budget (not only the work budget).
    selected: bool,
}

/// Bound-only discovery (BUG-2531): an operation reads the requested
/// labels' write-ahead bindings and only the change ledgers they name, plus
/// the by-spec hints, which may add a change but never hide one. The
/// completeness marker is checked when the discovery opens.
///
/// BUG-2517: the mis-key ownership index is built from the same bound
/// ledgers. A copy is disqualified only by a phase-3 witness with the same
/// fingerprint, which includes the spec, so the witnesses of a candidate are
/// exactly on the changes bound to its spec. Each ledger is folded into the
/// index once per discovery.
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
    owners: super::project::MiskeyOwners,
    owned_specs: std::collections::HashSet<String>,
    owned_changes: std::collections::HashSet<String>,
    /// Discovery's own work (ownership reads), separate from selections.
    work: Budget,
    bound: std::collections::HashMap<String, Vec<String>>,
    /// Ledgers are read once across a list operation. Their total retained
    /// payload is bounded by the work and selection budgets.
    cache: std::collections::HashMap<String, Cached>,
}

/// Open bound-only discovery of the gate ledger under `root`; `Ok(None)`
/// when it does not exist.
// trace:BUG-2403 | ai:claude
pub(crate) fn discover(root: &Path) -> Result<Option<Discovery>, Unsafe> {
    discover_with_budget(
        root,
        Budget {
            bytes: MAX_DISCOVERY_BYTES,
            events: MAX_SELECTION_EVENTS,
        },
    )
}

// trace:BUG-2531 | ai:claude
pub(crate) fn discover_with_budget(root: &Path, work: Budget) -> Result<Option<Discovery>, Unsafe> {
    let Some((ledger, by_spec)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let marked = require_complete(&ledger, &ledger_path)?;
    Ok(Some(Discovery {
        ledger,
        ledger_path,
        by_spec,
        marked,
        // trace:BUG-2517 | ai:claude
        owners: super::project::MiskeyOwners::new(std::iter::empty()),
        owned_specs: Default::default(),
        owned_changes: Default::default(),
        work,
        bound: Default::default(),
        cache: Default::default(),
    }))
}

impl Discovery {
    /// The changes bound to the normalized label `spec`, listed once.
    // trace:BUG-2531 | ai:claude
    fn bound_changes(&mut self, spec: &str) -> Result<Vec<String>, Unsafe> {
        if let Some(changes) = self.bound.get(spec) {
            return Ok(changes.clone());
        }
        let refuse = |what: String, e: std::io::Error| {
            not_bound(format!("refusing gate-ledger {what}: {e}"))
        };
        let mut changes = Vec::new();
        let bindings = match openat(&self.ledger, BINDINGS_DIR, libc::O_DIRECTORY) {
            Ok(d) => Some(d),
            Err(e) if missing(&e) && !self.marked => None,
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
        if let Some(dir) = spec_dir {
            let path = self.ledger_path.join(BINDINGS_DIR).join(spec);
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
        let Some((events, bytes)) = read_change(&self.ledger, &name, key, budget)? else {
            return Ok(false);
        };
        let reviewed: std::collections::BTreeSet<String> = events
            .iter()
            .filter(|e| e.verdict_record().is_some())
            .filter_map(|e| e.spec.as_deref().and_then(spec_key))
            .collect();
        for spec in reviewed {
            if !self.bound_changes(&spec)?.iter().any(|c| c == key) {
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

    /// Fold every ledger bound to `spec` into the ownership index.
    // trace:BUG-2531 | ai:claude
    // trace:BUG-2517 | ai:claude
    fn load_owners(&mut self, spec: &str) -> Result<(), Unsafe> {
        if self.owned_specs.contains(spec) {
            return Ok(());
        }
        for key in self.bound_changes(spec)? {
            if self.owned_changes.contains(&key) {
                continue;
            }
            if !self.load(&key, None)? {
                return Err(not_bound(format!(
                    "refusing gate-ledger ownership: {spec} is bound to {key}, whose ledger is missing"
                )));
            }
            let retained = self.owners.add(&self.cache[&key].events);
            self.work.spend_bytes(retained)?;
            self.owned_changes.insert(key);
        }
        self.owned_specs.insert(spec.to_string());
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
        self.owners.filter_bound(events);
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
    /// add a change, never hide one. A named change without a ledger refuses.
    // trace:BUG-2531 | ai:claude
    fn index_hint(&self, spec: &str, budget: &mut Budget) -> Result<Vec<String>, Unsafe> {
        let Some(by_spec) = &self.by_spec else {
            return Ok(Vec::new());
        };
        let index_name = format!("{spec}.json");
        let Some(body) = read_entry(by_spec, &index_name, MAX_INDEX_BYTES, budget)? else {
            return Ok(Vec::new());
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
        Ok(out)
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

    /// The bound changes of `spec` together with its index hints.
    // trace:BUG-2531 | ai:claude
    fn spec_changes(&mut self, spec: &str, budget: &mut Budget) -> Result<Vec<String>, Unsafe> {
        let mut changes = self.bound_changes(spec)?;
        for key in self.index_hint(spec, budget)? {
            if !changes.contains(&key) {
                changes.push(key);
            }
        }
        Ok(changes)
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

/// Read-only capacity probe for doctor; no event payloads are read.
// trace:BUG-2531 | ai:codex
pub(crate) fn active_bytes(root: &Path) -> Result<u64, Unsafe> {
    let Some((ledger, _)) = ledger_dirs(root)? else {
        return Ok(0);
    };
    let mut bytes = 0u64;
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
        bytes = bytes.saturating_add(meta.len());
    }
    Ok(bytes)
}

#[cfg(test)]
// trace:BUG-2531 | ai:codex
pub(crate) mod discovery_work_tests {
    use super::*;
    use std::cell::Cell;
    thread_local! {
        pub(crate) static READ_BYTES: Cell<u64> = const { Cell::new(0) };
        pub(crate) static PARSE_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
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
