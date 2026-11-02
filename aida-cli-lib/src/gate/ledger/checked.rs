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
/// One ledger line.
pub(crate) const MAX_LINE_BYTES: usize = 1024 * 1024;
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
            return Err(bad("the line is oversized"));
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

/// Every verdict event the ledger holds for the spec label `spec`, oldest
/// first, read strictly through [`ledger_dirs`]/[`read_entry`]: the indexed
/// changes' events bound to that label, plus its unbound ledger (a record
/// already bound to a change is read once, from the change). Same set and
/// order as [`super::project`]'s lossy reader would give on clean evidence.
// trace:BUG-2403 | ai:claude
pub(crate) fn spec_verdict_events(
    root: &Path,
    spec: &str,
    budget: &mut Budget,
) -> Result<Vec<GateEvent>, Unsafe> {
    let Some(spec) = spec_key(spec) else {
        return Err(Unsafe(format!("{spec:?} is not a spec label")));
    };
    let Some((ledger, by_spec)) = ledger_dirs(root)? else {
        return Ok(Vec::new());
    };
    let Some(by_spec) = by_spec else {
        return Ok(Vec::new());
    };
    let index_name = format!("{spec}.json");
    let changes: Vec<ChangeKey> = match read_entry(&by_spec, &index_name, MAX_INDEX_BYTES, budget)?
    {
        None => Vec::new(),
        Some(body) => {
            let index: SpecIndex = serde_json::from_slice(&body)
                .map_err(|e| Unsafe(format!("refusing gate-ledger index {index_name}: {e}")))?;
            if index.changes.len() > MAX_CHANGES_PER_SPEC {
                return Err(Unsafe(format!(
                    "refusing gate-ledger index {index_name}: it names more than {MAX_CHANGES_PER_SPEC} changes"
                )));
            }
            index
                .changes
                .iter()
                .map(|c| {
                    ChangeKey::parse(c).ok_or_else(|| {
                        Unsafe(format!(
                            "refusing gate-ledger index {index_name}: {c:?} is not a change key"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?
        }
    };
    let mut bound = Vec::new();
    for change in changes {
        let key = change.as_key();
        let name = format!("{key}.jsonl");
        let Some(body) = read_entry(&ledger, &name, MAX_LEDGER_BYTES, budget)? else {
            return Err(Unsafe(format!(
                "refusing gate-ledger index {index_name}: it names {key}, whose ledger is missing"
            )));
        };
        bound.extend(
            parse_events(&name, &body, &key, budget)?
                .into_iter()
                .filter(|e| {
                    e.verdict_record().is_some() && e.spec.as_deref() == Some(spec.as_str())
                }),
        );
    }
    let unbound_name = format!("{spec}.jsonl");
    let unbound = match read_entry(&by_spec, &unbound_name, MAX_LEDGER_BYTES, budget)? {
        None => Vec::new(),
        Some(body) => parse_events(&unbound_name, &body, &spec, budget)?
            .into_iter()
            .filter(|e| e.verdict_record().is_some())
            .filter(|u| {
                !bound
                    .iter()
                    .any(|b| b.verdict_record() == u.verdict_record())
            })
            .collect(),
    };
    let mut out: Vec<GateEvent> = bound.into_iter().chain(unbound).collect();
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.seq.cmp(&b.seq)));
    Ok(out)
}
