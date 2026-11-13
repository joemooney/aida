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
    #[cfg(test)]
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
fn read_change(
    ledger: &File,
    name: &str,
    change: &str,
    budget: &mut Budget,
) -> Result<Option<Vec<GateEvent>>, Unsafe> {
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
        return parse_events(name, &body, change, budget).map(Some);
    }
}

/// A bounded inventory of spec-to-change bindings, derived strictly from
/// events rather than the fallible by-spec hints. Discovery retains no review
/// payloads. Discovery and selection each have an aggregate work budget.
///
/// BUG-2517: the same single pass also builds the mis-key ownership index
/// (phase-3 witness fingerprints only, charged to the inventory budget), so
/// every selected vector is filtered against one index per discovery.
// trace:BUG-2531 | ai:codex
// trace:BUG-2517 | ai:claude
pub(crate) struct Discovery {
    ledger: File,
    by_spec: Option<File>,
    discovered: Vec<String>,
    owners: super::project::MiskeyOwners,
    bound: std::collections::HashMap<String, Vec<String>>,
    /// Selected ledgers are read once across a list operation. Their total
    /// retained payload is bounded by the caller's selection budget.
    selected: std::collections::HashMap<String, Vec<GateEvent>>,
}

/// Discover the gate ledger under `root`; `Ok(None)` when it does not exist.
// trace:BUG-2403 | ai:claude
pub(crate) fn discover(root: &Path) -> Result<Option<Discovery>, Unsafe> {
    discover_with_budget(
        root,
        &mut Budget {
            bytes: MAX_DISCOVERY_BYTES,
            events: MAX_SELECTION_EVENTS,
        },
    )
}

// trace:BUG-2531 | ai:codex
pub(crate) fn discover_with_budget(
    root: &Path,
    work: &mut Budget,
) -> Result<Option<Discovery>, Unsafe> {
    let Some((ledger, by_spec)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let mut discovered: Vec<String> = Vec::new();
    let mut bound: std::collections::HashMap<String, Vec<String>> = Default::default();
    // Inventory strings also have a finite aggregate budget, independent of
    // the (potentially much larger) unrelated review payloads being inspected.
    let mut inventory_budget = Budget::default();
    // trace:BUG-2517 | ai:claude
    let mut owners = super::project::MiskeyOwners::new(std::iter::empty());
    for name in list_pinned(&ledger, &ledger_path)? {
        let Some(change) = name
            .strip_suffix(".jsonl")
            .and_then(ChangeKey::parse)
            .filter(|c| format!("{}.jsonl", c.as_key()) == name)
        else {
            continue;
        };
        if discovered.len() >= MAX_CHANGE_LEDGERS {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: more than {MAX_CHANGE_LEDGERS} change ledgers"
            )));
        }
        let key = change.as_key();
        discovered.push(key.clone());
        // One budget for the entire scan, including retries. Never reset this
        // per file: 4096 individually legal files can otherwise cost 64 GiB.
        let Some(events) = read_change(&ledger, &name, &key, work)
            .map_err(|e| Unsafe(format!("refusing gate-ledger discovery: {e}")))?
        else {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: {name} vanished while it was read"
            )));
        };
        // trace:BUG-2517 | ai:claude
        inventory_budget.spend_bytes(owners.add(&events))?;
        for e in events.into_iter().filter(|e| e.verdict_record().is_some()) {
            if let Some(spec) = e.spec.as_deref() {
                let spec = spec.trim().to_ascii_uppercase();
                let changes = bound.entry(spec.clone()).or_default();
                if !changes.contains(&key) {
                    inventory_budget.spend_event()?;
                    inventory_budget.spend_bytes((spec.len() + key.len()) as u64)?;
                    changes.push(key.clone());
                }
            }
        }
    }
    Ok(Some(Discovery {
        ledger,
        owners,
        by_spec,
        discovered,
        bound,
        selected: Default::default(),
    }))
}

impl Discovery {
    /// Reuse the ownership snapshot for additional vectors without rescanning
    /// witnesses or rebuilding their semantic fingerprints.
    // trace:BUG-2517 | ai:codex
    pub(crate) fn filter_events(&self, events: &mut Vec<GateEvent>) {
        self.owners.filter(events);
    }

    /// Every event on `change`'s discovered ledger, read once per discovery
    /// under the caller's selection budget and filtered through the shared
    /// ownership index. Empty when discovery found no ledger for it.
    // trace:BUG-2517 | ai:codex
    pub(crate) fn change_events(
        &mut self,
        change: ChangeKey,
        budget: &mut Budget,
    ) -> Result<Vec<GateEvent>, Unsafe> {
        let key = change.as_key();
        if !self.discovered.contains(&key) {
            return Ok(Vec::new());
        }
        let mut events = self.select(&key, budget)?.to_vec();
        self.owners.filter(&mut events);
        Ok(events)
    }

    // trace:BUG-2531 | ai:codex
    fn select(&mut self, key: &str, budget: &mut Budget) -> Result<&[GateEvent], Unsafe> {
        if !self.selected.contains_key(key) {
            let name = format!("{key}.jsonl");
            let selected = read_change(&self.ledger, &name, key, budget)?.ok_or_else(|| {
                Unsafe(format!(
                    "refusing gate-ledger selection: {name} vanished while it was read"
                ))
            })?;
            self.selected.insert(key.to_string(), selected);
        }
        Ok(&self.selected[key])
    }

    /// Every verdict event for each spec label in `labels`, oldest first per
    /// label: the discovered change-bound events plus the label's unbound
    /// ledger (a record already bound to a change is read once, from the
    /// change). The label's index is checked against the discovery: a
    /// change it names whose ledger is missing, or a malformed index, refuses.
    // trace:BUG-2403 | ai:claude
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
            for key in self.bound.get(&spec).cloned().into_iter().flatten() {
                events.extend(
                    self.select(&key, budget)?
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
            if let Some(by_spec) = &self.by_spec {
                let index_name = format!("{spec}.json");
                if let Some(body) = read_entry(by_spec, &index_name, MAX_INDEX_BYTES, budget)? {
                    let index: SpecIndex = serde_json::from_slice(&body).map_err(|e| {
                        Unsafe(format!("refusing gate-ledger index {index_name}: {e}"))
                    })?;
                    if index.changes.len() > MAX_CHANGES_PER_SPEC {
                        return Err(Unsafe(format!(
                            "refusing gate-ledger index {index_name}: it names more than {MAX_CHANGES_PER_SPEC} changes"
                        )));
                    }
                    for c in &index.changes {
                        let key = ChangeKey::parse(c).map(|k| k.as_key()).ok_or_else(|| {
                            Unsafe(format!(
                                "refusing gate-ledger index {index_name}: {c:?} is not a change key"
                            ))
                        })?;
                        if !self.discovered.contains(&key) {
                            return Err(Unsafe(format!(
                                "refusing gate-ledger index {index_name}: it names {key}, whose ledger is missing"
                            )));
                        }
                    }
                }
                let unbound_name = format!("{spec}.jsonl");
                if let Some(body) = read_entry(by_spec, &unbound_name, MAX_LEDGER_BYTES, budget)? {
                    let unbound: Vec<GateEvent> =
                        parse_events(&unbound_name, &body, &spec, budget)?
                            .into_iter()
                            .filter(|e| e.verdict_record().is_some())
                            .filter(|u| {
                                !events
                                    .iter()
                                    .any(|b| b.verdict_record() == u.verdict_record())
                            })
                            .collect();
                    events.extend(unbound);
                }
            }
            // trace:BUG-2517 | ai:codex
            self.owners.filter(&mut events);
            events.sort_by(|a, b| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)));
            out.push((spec, events));
        }
        Ok(out)
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
    // append may have succeeded while indexing failed. Use pinned discovery.
    let Some(discovery) = discover_with_budget(root, budget)? else {
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
    discovery.filter_events(&mut events);
    Ok(events)
}

/// [`filter_miskeyed`] for the lossy single-label readers: when discovery is
/// incomplete (an unrelated ledger is malformed, unreadable or over budget)
/// every review copy a foreign change could own is withheld, while evidence
/// no witness could disqualify — holds, carry-over proofs, phase-3 records —
/// stays readable. Never presents raw foreign evidence; never rewrites.
// trace:BUG-2517 | ai:claude
pub(crate) fn filter_miskeyed_or_fail_closed(
    root: &Path,
    events: Vec<GateEvent>,
) -> Vec<GateEvent> {
    let mut events = events;
    match discover(root) {
        Ok(Some(discovery)) => discovery.filter_events(&mut events),
        // The directory can disappear after a legacy reader has read its
        // input vector. Absence then proves no ownership for those copies.
        // trace:BUG-2517 | ai:codex
        Ok(None) | Err(_) => super::project::MiskeyOwners::fail_closed(&mut events),
    }
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

impl Discovery {
    /// Read a member's unbound evidence and index through this operation's pinned
    /// by-spec directory, sharing the same budget as the change discovery.
    // trace:STORY-2324 | ai:claude
    // trace:BUG-2517 | ai:codex
    pub(crate) fn spec_entries(
        &self,
        label: &str,
        budget: &mut Budget,
    ) -> Result<(Vec<GateEvent>, Vec<ChangeKey>), Unsafe> {
        let spec =
            spec_key(label).ok_or_else(|| Unsafe(format!("{label:?} is not a spec label")))?;
        let Some(by_spec) = &self.by_spec else {
            return Ok((Vec::new(), Vec::new()));
        };
        let unbound_name = format!("{spec}.jsonl");
        let unbound = match read_entry(by_spec, &unbound_name, MAX_LEDGER_BYTES, budget)? {
            Some(body) => parse_events(&unbound_name, &body, &spec, budget)?
                .into_iter()
                .filter(|e| e.verdict_record().is_some())
                .collect(),
            None => Vec::new(),
        };
        let index_name = format!("{spec}.json");
        let mut changes = Vec::new();
        if let Some(body) = read_entry(by_spec, &index_name, MAX_INDEX_BYTES, budget)? {
            let index: SpecIndex = serde_json::from_slice(&body)
                .map_err(|e| Unsafe(format!("refusing gate-ledger index {index_name}: {e}")))?;
            if index.changes.len() > MAX_CHANGES_PER_SPEC {
                return Err(Unsafe(format!(
                    "refusing gate-ledger index {index_name}: it names more than {MAX_CHANGES_PER_SPEC} changes"
                )));
            }
            for c in &index.changes {
                let change = ChangeKey::parse(c).ok_or_else(|| {
                    Unsafe(format!(
                        "refusing gate-ledger index {index_name}: {c:?} is not a change key"
                    ))
                })?;
                // trace:BUG-2517 | ai:codex
                // A cached discovery must retain the strict reader's missing
                // ledger refusal, rather than treating absent evidence as empty.
                if !self.discovered.contains(&change.as_key()) {
                    return Err(Unsafe(format!(
                        "refusing gate-ledger index {index_name}: it names {}, whose ledger is missing",
                        change.as_key()
                    )));
                }
                changes.push(change);
            }
        }
        Ok((unbound, changes))
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
mod discovery_work_tests {
    use super::*;
    use std::cell::Cell;
    thread_local! {
        pub(super) static READ_BYTES: Cell<u64> = const { Cell::new(0) };
        pub(super) static PARSE_ATTEMPTS: Cell<usize> = const { Cell::new(0) };
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
        let err = discover_with_budget(root.path(), &mut budget)
            .err()
            .unwrap();
        assert!(err.0.contains("discovery"), "{err}");
        assert!(err.0.contains("read budget"), "{err}");
        assert_eq!(READ_BYTES.with(Cell::get), (size * 3) as u64);
        assert_eq!(PARSE_ATTEMPTS.with(Cell::get), 3);

        READ_BYTES.with(|n| n.set(0));
        PARSE_ATTEMPTS.with(|n| n.set(0));
        let mut budget = Budget::with_limits((size * 20) as u64, 2);
        let err = discover_with_budget(root.path(), &mut budget)
            .err()
            .unwrap();
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
        let err = discover_with_budget(root.path(), &mut budget)
            .err()
            .unwrap();
        assert!(err.0.contains("read budget"), "{err}");
        assert_eq!(READ_BYTES.with(Cell::get), (body.len() * 2) as u64);
        assert_eq!(PARSE_ATTEMPTS.with(Cell::get), 0);
    }
}
