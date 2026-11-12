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
    let mut body = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut body)
        .map_err(|e| Unsafe(format!("refusing gate-ledger entry {name}: {e}")))?;
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

/// One bounded, strict discovery pass over a store's gate ledger: every
/// change ledger in the pinned ledger directory read once, its verdict
/// events grouped by the spec label each is bound to.
///
/// BUG-2403 (signoff amendments 6-7): the `by-spec` index is only a
/// discovery hint — `store::write_events` appends before it indexes and only
/// warns when indexing fails — so completeness is proved by DISCOVERY, not
/// by index success. A caller selecting for many specs (a list view) builds
/// one `Discovery` and asks it per spec, so no file is read twice per
/// operation. Listing, file count and aggregate bytes/events are bounded;
/// exhausting any bound refuses rather than answering from a partial read.
// trace:BUG-2403 | ai:claude
pub(crate) struct Discovery {
    /// `None` when the ledger has no `by-spec` directory.
    by_spec: Option<File>,
    discovered: Vec<String>,
    changes: std::collections::HashMap<ChangeKey, Vec<GateEvent>>,
    owners: super::project::MiskeyOwners,
    /// Upper-cased spec label -> its change-bound verdict events.
    bound: std::collections::HashMap<String, Vec<GateEvent>>,
}

/// Discover the gate ledger under `root`; `Ok(None)` when it does not exist.
// trace:BUG-2403 | ai:claude
// trace:BUG-2517 | ai:codex
pub(crate) fn discover(root: &Path, budget: &mut Budget) -> Result<Option<Discovery>, Unsafe> {
    let Some((ledger, by_spec)) = ledger_dirs(root)? else {
        return Ok(None);
    };
    let ledger_path = super::ledger_dir(root);
    let mut discovered: Vec<String> = Vec::new();
    let mut changes = std::collections::HashMap::new();
    let mut bound: std::collections::HashMap<String, Vec<GateEvent>> = Default::default();
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
        let Some(events) = read_change(&ledger, &name, &key, budget)? else {
            return Err(Unsafe(format!(
                "refusing gate-ledger discovery: {name} vanished while it was read"
            )));
        };
        changes.insert(change, events.clone());
        for e in events.into_iter().filter(|e| e.verdict_record().is_some()) {
            if let Some(spec) = e.spec.as_deref() {
                bound
                    .entry(spec.trim().to_ascii_uppercase())
                    .or_default()
                    .push(e);
            }
        }
    }
    // trace:BUG-2517 | ai:codex
    let owners = super::project::MiskeyOwners::new(bound.values().flatten());
    for events in bound.values_mut() {
        owners.filter(events);
    }
    for events in changes.values_mut() {
        owners.filter(events);
    }
    Ok(Some(Discovery {
        owners,
        changes,
        by_spec,
        discovered,
        bound,
    }))
}

impl Discovery {
    // trace:BUG-2517 | ai:codex
    pub(crate) fn change_events(&self, change: ChangeKey) -> Vec<GateEvent> {
        self.changes.get(&change).cloned().unwrap_or_default()
    }

    /// Every verdict event for each spec label in `labels`, oldest first per
    /// label: the discovered change-bound events plus the label's unbound
    /// ledger (a record already bound to a change is read once, from the
    /// change). The label's index is checked against the discovery: a
    /// change it names whose ledger is missing, or a malformed index, refuses.
    // trace:BUG-2403 | ai:claude
    pub(crate) fn events_for(
        &self,
        labels: &[String],
        budget: &mut Budget,
    ) -> Result<Vec<(String, Vec<GateEvent>)>, Unsafe> {
        let mut out = Vec::new();
        for label in labels {
            let spec =
                spec_key(label).ok_or_else(|| Unsafe(format!("{label:?} is not a spec label")))?;
            let mut events = self.bound.get(&spec).cloned().unwrap_or_default();
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
    let Some(discovery) = discover(root, budget)? else {
        return Ok(events);
    };
    let mut events = events;
    discovery.owners.filter(&mut events);
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
    match discover(root, &mut Budget::default()) {
        Ok(None) => {}
        Ok(Some(discovery)) => discovery.owners.filter(&mut events),
        Err(_) => super::project::MiskeyOwners::fail_closed(&mut events),
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
    Ok(discover(root, budget)?
        .map(|d| d.change_events(change))
        .unwrap_or_default())
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
                if !self.changes.contains_key(&change) {
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
