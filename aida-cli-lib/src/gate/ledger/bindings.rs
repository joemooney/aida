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

#[cfg(test)]
pub(crate) mod publish_rendezvous {
    //! Hold every publisher of one store root's marker between writing its
    //! temporary file and renaming it until `parties` have arrived (or a
    //! timeout passes), so a test can drive two first appends of one
    //! process into publication together. Keyed by root: other tests'
    //! publications never wait.
    // trace:BUG-2531 | ai:claude
    use std::path::{Path, PathBuf};
    use std::sync::{Condvar, Mutex};

    static WAITING: Mutex<Vec<(PathBuf, usize, usize)>> = Mutex::new(Vec::new());
    static ARRIVED: Condvar = Condvar::new();

    pub(crate) fn arm(root: &Path, parties: usize) {
        WAITING
            .lock()
            .unwrap()
            .push((root.to_path_buf(), parties, 0));
    }

    pub(crate) fn wait(root: &Path) {
        let mut waiting = WAITING.lock().unwrap();
        let Some(i) = waiting.iter().position(|(r, _, _)| r == root) else {
            return;
        };
        waiting[i].2 += 1;
        ARRIVED.notify_all();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let Some(entry) = waiting.iter().find(|(r, _, _)| r == root) else {
                return;
            };
            if entry.2 >= entry.1 {
                return;
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return;
            }
            waiting = ARRIVED.wait_timeout(waiting, deadline - now).unwrap().0;
        }
    }

    pub(crate) fn disarm(root: &Path) {
        WAITING.lock().unwrap().retain(|(r, _, _)| r != root);
    }
}

/// A temporary file name no other publisher uses: unique per process and,
/// within one, per call (two first appends of one process to different
/// changes can certify at once; they hold different per-change locks).
// trace:BUG-2531 | ai:claude
fn marker_tmp(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    path.with_extension(format!("tmp.{}.{n}", std::process::id()))
}

/// Publish the marker atomically and durably, only once no older writer
/// holds the old lock open: one that opened it before the fence went in can
/// still append without a binding after the fence's lock passes. Each call
/// writes its own temporary file (created new, never truncated), so
/// concurrent publishers never share an inode: each rename installs a
/// complete marker, and no handle writes into one already published.
// trace:BUG-2531 | ai:claude
fn publish_marker(root: &Path) -> std::io::Result<()> {
    use std::io::Write as _;
    super::store::require_older_writers_quiescent(root)?;
    let marker = Marker {
        format: MARKER_FORMAT,
        completed_at: chrono::Utc::now().to_rfc3339(),
    };
    let body = serde_json::to_vec_pretty(&marker).map_err(std::io::Error::other)?;
    let path = marker_file(root);
    let tmp = marker_tmp(&path);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        drop(file);
        #[cfg(test)]
        publish_rendezvous::wait(root);
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
/// that carries a spec no binding names or a binding whose change ledger
/// carries none of its label's events (a strict read of every change
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
                let scan = scan.unwrap_or_default();
                // A binding whose change ledger exists but carries none of
                // the label's events (an append that failed after binding)
                // reads as a lost binding on every selection of the label.
                // trace:BUG-2531 | ai:claude
                if let Ok(specs) = std::fs::read_dir(&dir) {
                    for spec in specs.flatten() {
                        let label = spec.file_name().to_string_lossy().into_owned();
                        let Ok(entries) = std::fs::read_dir(spec.path()) else {
                            continue;
                        };
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            let carried = scan.bound.get(&label).is_some_and(|c| c.contains(&name));
                            if ledgers.contains(&name) && !carried {
                                out.push(format!(
                                    "{label} is bound to {name}, whose ledger records no {label} events"
                                ));
                            }
                        }
                    }
                }
                for (spec, changes) in &scan.bound {
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

/// Rewrite every label's by-spec hint to exactly the changes the scan bound
/// it to, and remove each hint the scan bound nothing to: a hint that names
/// a change whose ledger is gone refuses every selection of its label. A
/// label bound only under a `~` key has no hint. The unbound ledgers
/// (`by-spec/<SPEC>.jsonl`) and the lock files beside the hints are
/// independent evidence and are left as they are.
// trace:BUG-2531 | ai:claude
fn rebuild_hints(
    root: &Path,
    bound: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) -> std::io::Result<()> {
    let mut kept = std::collections::BTreeSet::new();
    for (spec, changes) in bound {
        if spec_key(spec).as_deref() != Some(spec.as_str()) {
            continue;
        }
        super::project::write_index(root, spec, changes.iter().cloned().collect())?;
        kept.insert(format!("{spec}.json"));
    }
    let by_spec = ledger_dir(root).join("by-spec");
    if !std::fs::symlink_metadata(&by_spec).is_ok_and(|m| m.is_dir()) {
        return Ok(());
    }
    let mut removed = false;
    for entry in std::fs::read_dir(&by_spec)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.ends_with(".json") && !kept.contains(name) {
            remove_entry(&entry.path())?;
            removed = true;
        }
    }
    if removed {
        fsync_dir(&by_spec)?;
    }
    Ok(())
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
        // First remove what selection would refuse on, or would read as a
        // lost binding: a binding the scan did not find on its change (its
        // ledger is gone, or an append that failed after its binding left
        // the change without the spec's event) and every entry that is not
        // a binding at all. trace:BUG-2531 | ai:claude
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
            let scanned = spec
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|key| scan.bound.get(key));
            for entry in std::fs::read_dir(&spec)? {
                let entry = entry?;
                let bound = entry.file_name().to_str().is_some_and(|name| {
                    scanned.is_some_and(|changes| changes.contains(name))
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
                out.bindings += 1;
            }
        }
        // As a writer leaves them: each label's hint names exactly its bound
        // changes, so selection sees no evidence of a lost binding, and no
        // hint names a change whose ledger is gone. trace:BUG-2531 | ai:claude
        rebuild_hints(root, &scan.bound)?;
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
