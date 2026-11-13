//! Write-ahead spec-to-change bindings: the completeness evidence verdict
//! selection reads instead of scanning every change ledger.
//!
//! `<ledger>/bindings/<SPEC>/<CHANGE>` is one empty regular file per pair. A
//! writer persists the binding of every spec its events carry BEFORE it
//! appends them, under the same locks as the append, and a failed binding
//! write fails the append. A binding without an event is harmless extra
//! work; an event without a binding is forbidden. The `by-spec` indexes stay
//! lossy hints.
//!
//! `<ledger>/bindings-complete.json` certifies that every change ledger that
//! predates the bindings has been bound. It is published by one explicit
//! maintenance action ([`bootstrap`], run by the doctor fix named in
//! [`FIX_COMMAND`]) under the exclusive ledger lock, LAST, so an interrupted
//! run leaves no marker and a rerun completes. A writer that finds no marker
//! and no change ledger at all publishes it itself: an empty ledger is
//! trivially complete. A missing or unsupported marker makes selection
//! refuse ([`super::checked`]).
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

/// Bind `spec` to `change` before an append that carries it. Called under
/// the append's locks; any error must fail the append. A spec that is not
/// a valid label key cannot be selected by label and needs no binding
/// (ownership filtering withholds such copies instead).
pub(crate) fn bind(root: &Path, spec: &str, change: ChangeKey) -> std::io::Result<()> {
    let Some(spec) = spec_key(spec) else {
        return Ok(());
    };
    // An empty ledger is trivially complete: certify it before the first
    // change ledger exists. Every later ledger is written by a binder.
    if std::fs::symlink_metadata(marker_file(root)).is_err() && !has_change_ledgers(root)? {
        ensure_dir(&bindings_dir(root))?;
        publish_marker(root)?;
    }
    write_binding(root, &spec, &change.as_key())
}

#[cfg(test)]
thread_local! {
    /// Stop a bootstrap after its bindings, before its marker (interruption).
    pub(crate) static STOP_BEFORE_MARKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The changes bound to `spec`, read lossily for the single-label readers
/// (which then filter through the strict reader): unreadable means none.
pub(crate) fn bound_changes_lossy(root: &Path, spec: &str) -> Vec<ChangeKey> {
    let Some(spec) = spec_key(spec) else {
        return Vec::new();
    };
    let mut out: Vec<ChangeKey> = std::fs::read_dir(bindings_dir(root).join(spec))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(ChangeKey::parse))
        .collect();
    out.sort_by_key(|c| c.as_key());
    out
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

/// Scan every change ledger strictly under the exclusive ledger lock, bind
/// every spec its events carry, remove bindings whose change ledger no
/// longer exists, and publish the marker last. Idempotent: an interrupted
/// run leaves no new marker, and a rerun completes it.
pub(crate) fn bootstrap(root: &Path) -> std::io::Result<Bootstrapped> {
    super::store::with_exclusive(root, || {
        if std::fs::symlink_metadata(ledger_dir(root)).is_err() {
            return Ok(Bootstrapped::default());
        }
        let mut work =
            super::checked::Budget::with_limits(MAX_BOOTSTRAP_BYTES, MAX_BOOTSTRAP_EVENTS);
        let scan = super::checked::scan_bindings(root, &mut work)
            .map_err(|e| refused(format!("cannot bootstrap the gate-ledger bindings: {e}")))?
            .unwrap_or_default();
        let mut out = Bootstrapped {
            ledgers: scan.changes.len(),
            ..Default::default()
        };
        ensure_dir(&bindings_dir(root))?;
        for (spec, changes) in &scan.bound {
            for change in changes {
                write_binding(root, spec, change)?;
                out.bindings += 1;
            }
        }
        // A binding whose ledger is gone (a first append that failed after
        // its binding, or a removed ledger) would refuse selection forever.
        for spec in std::fs::read_dir(bindings_dir(root))? {
            let spec = spec?;
            if !spec.file_type()?.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(spec.path())? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if !scan.changes.contains(name) {
                    std::fs::remove_file(entry.path())?;
                    out.pruned += 1;
                }
            }
        }
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
