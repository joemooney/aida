//! Write-ahead spec-to-change bindings: the completeness evidence verdict
//! selection reads instead of scanning every change ledger.
//!
//! `<ledger>/bindings/<SPEC>/<CHANGE>` is one empty regular file per pair. A
//! writer persists the binding of every spec its events carry BEFORE it
//! appends them, under the same locks as the append, and a failed binding
//! write fails the append. Every label is bound, whatever its spelling
//! ([`binding_key`]). A binding without an event is harmless extra work; an
//! event without a binding is forbidden. The `by-spec` indexes stay lossy
//! hints, but a writer leaves a label's hint naming exactly its bound
//! changes, and the bootstrap does the same. Selection reads the bindings
//! alone unless it holds evidence that one was lost
//! ([`super::checked::binding_lost`]): a hint naming a change the bindings
//! do not, or a label's bindings directory left without a hint. Only then
//! does it read every change ledger
//! ([`super::checked::recorded_changes`]), so a deleted binding does not
//! hide a review, while a label never recorded (no directory, no hint)
//! costs nothing. A binding and a hint that both lack the same change are
//! indistinguishable from a spec never recorded on it; only the deep
//! [`audit`] sees that.
//!
//! Only a format 2 writer binds, so binaries older than format 2 are fenced
//! out of the ledger before anything is certified
//! ([`super::store::fence_older_writers`]): their ledger-wide lock path is
//! replaced by a dangling link, and each of their writes fails on it.
//!
//! `<ledger>/bindings-complete.json` certifies that every change ledger that
//! predates the bindings has been bound. It is published by one explicit
//! maintenance action ([`bootstrap`], run by the doctor fix named in
//! [`FIX_COMMAND`]) under the exclusive ledger lock, LAST, so an interrupted
//! run leaves no marker and a rerun completes. A writer that finds no marker
//! and no change ledger at all publishes it itself: an empty ledger is
//! trivially complete. A missing or unsupported marker, or a marker without
//! the fence, makes selection refuse ([`super::checked`]); a writer that
//! finds the fence down withdraws the marker. [`audit`] names every
//! disagreement between a valid marker and the ledger, and the bootstrap
//! repairs each.
// trace:BUG-2531 | ai:claude
// mandate-gate

use super::project::spec_key;
use super::{ledger_dir, ChangeKey};
use std::path::{Path, PathBuf};

/// The bindings directory under the ledger directory.
pub(crate) const BINDINGS_DIR: &str = "bindings";
/// The completeness marker under the ledger directory.
pub(crate) const MARKER_FILE: &str = "bindings-complete.json";
/// The marker format this binary writes and trusts.
pub(crate) const MARKER_FORMAT: u32 = 1;
/// The one command that bootstraps the bindings and publishes the marker.
pub(crate) const FIX_COMMAND: &str = "aida doctor --heal --yes --category gate-ledger-bindings";

/// Bootstrap work ceiling: the scan streams (no payload is retained), so it
/// may read far more than one selection, but it is still finite.
pub(crate) const MAX_BOOTSTRAP_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub(crate) const MAX_BOOTSTRAP_EVENTS: usize = 20_000_000;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Marker {
    pub format: u32,
    #[serde(default)]
    pub completed_at: String,
}

pub(crate) fn bindings_dir(root: &Path) -> PathBuf {
    ledger_dir(root).join(BINDINGS_DIR)
}

pub(crate) fn marker_file(root: &Path) -> PathBuf {
    ledger_dir(root).join(MARKER_FILE)
}

fn refused(what: String) -> std::io::Error {
    std::io::Error::other(what)
}

fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// `dir` as a plain directory, created when absent (and its parent synced).
/// A link or non-directory in its place refuses. Returns whether it was
/// created by this call.
fn ensure_dir(dir: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => return Ok(false),
        Ok(_) => {
            return Err(refused(format!(
                "refusing gate-ledger binding directory {}: it is not a plain directory",
                dir.display()
            )))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    match std::fs::create_dir(dir) {
        Ok(()) => {}
        // A concurrent writer created it: recheck its type.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return ensure_dir(dir),
        Err(e) => return Err(e),
    }
    if let Some(parent) = dir.parent() {
        fsync_dir(parent)?;
    }
    Ok(true)
}

/// Create `<bindings>/<spec>/<change>` durably; an existing regular file is
/// already the binding. `spec` is an already normalized key.
fn write_binding(root: &Path, spec: &str, change: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    ensure_dir(&bindings_dir(root))?;
    let spec_dir = bindings_dir(root).join(spec);
    ensure_dir(&spec_dir)?;
    let path = spec_dir.join(change);
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path);
    match created {
        Ok(file) => {
            file.sync_all()?;
            fsync_dir(&spec_dir)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_file() {
                Ok(())
            } else {
                Err(refused(format!(
                    "refusing gate-ledger binding {}: it is not a regular file",
                    path.display()
                )))
            }
        }
        Err(e) => Err(e),
    }
}

/// Whether `name` is a canonical change-ledger file name (`PR-<N>.jsonl`).
pub(crate) fn change_ledger_name(name: &str) -> Option<String> {
    let change = name.strip_suffix(".jsonl").and_then(ChangeKey::parse)?;
    let key = change.as_key();
    (format!("{key}.jsonl") == name).then_some(key)
}

fn has_change_ledgers(root: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(ledger_dir(root))? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .and_then(change_ledger_name)
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Publish the marker atomically and durably.
fn publish_marker(root: &Path) -> std::io::Result<()> {
    use std::io::Write as _;
    let marker = Marker {
        format: MARKER_FORMAT,
        completed_at: chrono::Utc::now().to_rfc3339(),
    };
    let body = serde_json::to_vec_pretty(&marker).map_err(std::io::Error::other)?;
    let path = marker_file(root);
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        fsync_dir(&ledger_dir(root))
    };
    write().inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The bindings directory name of the spec label `spec`, for every label an
/// event can carry. A label usable as a file name is its own key (what
/// selection asks for). Any other label (one with a separator, or one that
/// reads as a change key) cannot be selected by label, but a review recorded
/// under it can still be a misplaced copy whose witness sits on another
/// change, so it is bound too, under `~<sha256 of the normalized label>`:
/// the ownership filter finds every witness, and no review is withheld for
/// want of a key. `~` never starts a label key.
// trace:BUG-2531 | ai:claude
pub(crate) fn binding_key(spec: &str) -> String {
    use sha2::{Digest as _, Sha256};
    spec_key(spec).unwrap_or_else(|| {
        let mut h = Sha256::new();
        h.update(spec.trim().to_ascii_uppercase().as_bytes());
        format!("~{:x}", h.finalize())
    })
}

/// Bind `spec` to `change` before an append that carries it. Called under
/// the append's locks; any error must fail the append. Every spec label is
/// bound, whatever its spelling ([`binding_key`]).
pub(crate) fn bind(root: &Path, spec: &str, change: ChangeKey) -> std::io::Result<()> {
    write_binding(root, &binding_key(spec), &change.as_key())
}

/// Withdraw the completeness marker: the ledger may hold events no binding
/// names (an older binary could write it), so selection must refuse until
/// the bootstrap has scanned it again. Absent is already withdrawn.
// trace:BUG-2531 | ai:claude
pub(crate) fn withdraw_marker(root: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(marker_file(root)) {
        Ok(()) => fsync_dir(&ledger_dir(root)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Certify a ledger that holds no change ledger yet: it is trivially
/// complete, and every later ledger is written by a binder. Called under the
/// append's locks before every append, whether or not its events carry a
/// spec; a ledger that already holds change ledgers is left to the
/// bootstrap.
pub(crate) fn certify_empty(root: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(marker_file(root)).is_err() && !has_change_ledgers(root)? {
        ensure_dir(&bindings_dir(root))?;
        publish_marker(root)?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Stop a bootstrap after its bindings, before its marker (interruption).
    pub(crate) static STOP_BEFORE_MARKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The changes bound to `spec`, read lossily for the single-label readers
/// (which then filter through the strict reader): an unreadable entry is
/// skipped. `None` when the label has no bindings directory (never bound).
pub(crate) fn bound_changes_lossy(root: &Path, spec: &str) -> Option<Vec<ChangeKey>> {
    let dir = bindings_dir(root).join(spec_key(spec)?);
    // An entry that is not a directory is never a label's bindings; a
    // directory that cannot be listed still shows the label was bound.
    // trace:BUG-2531 | ai:claude
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let mut out: Vec<ChangeKey> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(ChangeKey::parse))
        .collect();
    out.sort_by_key(|c| c.as_key());
    Some(out)
}

/// What [`bootstrap`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Bootstrapped {
    pub ledgers: usize,
    pub bindings: usize,
    pub pruned: usize,
}

/// The state of the completeness marker, for doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MarkerState {
    /// No ledger directory: nothing to bind.
    NoLedger,
    Complete,
    /// No marker, but no change ledger either.
    Trivial,
    Missing,
    Unsupported(String),
}

pub(crate) fn marker_state(root: &Path) -> std::io::Result<MarkerState> {
    if std::fs::symlink_metadata(ledger_dir(root)).is_err() {
        return Ok(MarkerState::NoLedger);
    }
    match std::fs::read(marker_file(root)) {
        Ok(body) => Ok(match serde_json::from_slice::<Marker>(&body) {
            Ok(m) if m.format == MARKER_FORMAT => MarkerState::Complete,
            Ok(m) => MarkerState::Unsupported(format!("format {}", m.format)),
            Err(e) => MarkerState::Unsupported(e.to_string()),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(if has_change_ledgers(root)? {
            MarkerState::Missing
        } else {
            MarkerState::Trivial
        }),
        Err(e) => Err(e),
    }
}

/// Remove whatever sits at `path` (a file, a link, or a directory tree).
fn remove_entry(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// The canonical keys of the change ledgers in the ledger directory.
fn change_ledgers(root: &Path) -> std::io::Result<std::collections::BTreeSet<String>> {
    let mut out = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(ledger_dir(root))? {
        if let Some(key) = entry?.file_name().to_str().and_then(change_ledger_name) {
            out.insert(key);
        }
    }
    Ok(out)
}

/// Every way the bindings disagree with the ledger they certify, as
/// selection would refuse it: older writers not fenced out, a bindings tree
/// that is not plain directories of regular files named by changes, a
/// binding whose change ledger is gone and, when `deep`, a change ledger
/// that carries a spec no binding names (a strict read of every change
/// ledger, bounded like the bootstrap). Empty when they agree. Read without
/// the ledger lock, so an append in flight can show as a binding whose
/// ledger is missing; the bootstrap, under the lock, then finds nothing.
// trace:BUG-2531 | ai:claude
pub(crate) fn audit(root: &Path, deep: bool) -> std::io::Result<Vec<String>> {
    let mut out = Vec::new();
    if std::fs::symlink_metadata(ledger_dir(root)).is_err() {
        return Ok(out);
    }
    if !super::store::writer_fenced(root) {
        out.push("older aida binaries are not fenced out of the ledger".to_string());
    }
    let ledgers = change_ledgers(root)?;
    let dir = bindings_dir(root);
    match std::fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => {
            for spec in std::fs::read_dir(&dir)? {
                let spec = spec?;
                let label = spec.file_name().to_string_lossy().into_owned();
                if !std::fs::symlink_metadata(spec.path())?.is_dir() {
                    out.push(format!("bindings entry {label} is not a directory"));
                    continue;
                }
                for entry in std::fs::read_dir(spec.path())? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let canonical = ChangeKey::parse(&name).is_some_and(|c| c.as_key() == name);
                    if !canonical || !std::fs::symlink_metadata(entry.path())?.is_file() {
                        out.push(format!(
                            "binding {label}/{name} is not a regular file named by a change"
                        ));
                    } else if !ledgers.contains(&name) {
                        out.push(format!(
                            "{label} is bound to {name}, whose ledger is missing"
                        ));
                    }
                }
            }
        }
        Ok(_) => out.push("the bindings directory is not a plain directory".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.push("the bindings directory is missing".to_string())
        }
        Err(e) => return Err(e),
    }
    if deep {
        let mut work =
            super::checked::Budget::with_limits(MAX_BOOTSTRAP_BYTES, MAX_BOOTSTRAP_EVENTS);
        match super::checked::scan_bindings(root, &mut work) {
            Ok(scan) => {
                for (spec, changes) in &scan.unwrap_or_default().bound {
                    for change in changes {
                        let bound = std::fs::symlink_metadata(dir.join(spec).join(change))
                            .is_ok_and(|m| m.is_file());
                        if !bound {
                            out.push(format!("{change} records {spec} events without a binding"));
                        }
                    }
                }
            }
            Err(e) => out.push(format!(
                "the bindings cannot be checked against the change ledgers: {e}"
            )),
        }
    }
    Ok(out)
}

/// Scan every change ledger strictly under the exclusive ledger lock (which
/// fences older writers out), rebuild the bindings tree to exactly what the
/// scan found — every spec its events carry is bound; a binding whose change
/// ledger no longer exists, and anything that is not a binding, is removed;
/// each label's by-spec hint names its bound changes; no label directory is
/// left empty — and publish the marker last. The marker is withdrawn first, so selection
/// refuses while the tree is rebuilt, an interrupted run leaves no marker,
/// and a rerun completes it. Safe to run on a ledger whose marker is valid:
/// it repairs every inconsistency [`audit`] reports.
pub(crate) fn bootstrap(root: &Path) -> std::io::Result<Bootstrapped> {
    super::store::with_exclusive(root, || {
        if std::fs::symlink_metadata(ledger_dir(root)).is_err() {
            return Ok(Bootstrapped::default());
        }
        withdraw_marker(root)?;
        let mut work =
            super::checked::Budget::with_limits(MAX_BOOTSTRAP_BYTES, MAX_BOOTSTRAP_EVENTS);
        let scan = super::checked::scan_bindings(root, &mut work)
            .map_err(|e| refused(format!("cannot bootstrap the gate-ledger bindings: {e}")))?
            .unwrap_or_default();
        let mut out = Bootstrapped {
            ledgers: scan.changes.len(),
            ..Default::default()
        };
        // First remove what selection would refuse on: a binding whose
        // ledger is gone (a first append that failed after its binding, or
        // a removed ledger) and every entry that is not a binding at all.
        let dir = bindings_dir(root);
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| !m.is_dir()) {
            std::fs::remove_file(&dir)?;
            out.pruned += 1;
        }
        ensure_dir(&dir)?;
        for spec in std::fs::read_dir(&dir)? {
            let spec = spec?.path();
            if !std::fs::symlink_metadata(&spec)?.is_dir() {
                std::fs::remove_file(&spec)?;
                out.pruned += 1;
                continue;
            }
            for entry in std::fs::read_dir(&spec)? {
                let entry = entry?;
                let bound = entry.file_name().to_str().is_some_and(|name| {
                    scan.changes.contains(name)
                        && std::fs::symlink_metadata(entry.path()).is_ok_and(|m| m.is_file())
                });
                if !bound {
                    remove_entry(&entry.path())?;
                    out.pruned += 1;
                }
            }
            fsync_dir(&spec)?;
        }
        fsync_dir(&dir)?;
        for (spec, changes) in &scan.bound {
            for change in changes {
                write_binding(root, spec, change)?;
                // As a writer leaves them: the label's hint names its bound
                // changes, so selection sees no evidence of a lost binding.
                // trace:BUG-2531 | ai:claude
                if let Some(change) = ChangeKey::parse(change) {
                    super::project::index_spec(root, spec, change)?;
                }
                out.bindings += 1;
            }
        }
        // A label's directory left empty by the pruning above would read as
        // a deletion marker on every selection. trace:BUG-2531 | ai:claude
        for spec in std::fs::read_dir(&dir)? {
            let spec = spec?.path();
            if std::fs::read_dir(&spec)?.next().is_none() {
                remove_entry(&spec)?;
            }
        }
        fsync_dir(&dir)?;
        #[cfg(test)]
        if STOP_BEFORE_MARKER.with(|s| s.get()) {
            return Err(refused("injected bootstrap interruption".into()));
        }
        publish_marker(root)?;
        Ok(out)
    })
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
mod tests;
