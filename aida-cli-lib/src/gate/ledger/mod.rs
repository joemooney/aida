//! M1: one append-only gate ledger per change (PR/MR), in the primary
//! checkout's shared state — `<main>/.aida/gate-ledger/PR-<N>.jsonl`.
//!
//! The change is the only primary key. A spec ID is an index to changes
//! (`by-spec/<SPEC>.json`, a derived cache), never a second place state lives.
//! Every event names the head it was recorded at and the change's patch
//! identity, so a gate can tell "the same change on a moved base" from "new
//! code".
//!
//! One writer kind: uncontained AIDA on the primary side ([`store::append`]).
//! A contained seat cannot write `<main>/.aida/` (BUG-1929 / BUG-2218) and has
//! no route to the ledger of its own: a mailbox submission cannot prove which
//! seat sent it, since grant files are readable from every seat. Its verdict
//! stays in its seat clone until the uncontained launcher that started it
//! collects it ([`crate::gate::review_verdict::collect_standalone_verdict`]),
//! checking the PR binding and that launch's own child grant, and appends it
//! here. No verdict or hold state persists in a seat clone after collection.
//!
//! Slice 1 carries review verdicts. The legacy `.aida/review-verdicts/`
//! files are read through ([`legacy`]) and preserved verbatim in a
//! `legacy-imported` event on a change's first ledger write; nothing here
//! deletes them.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
// mandate-gate

pub(crate) mod event;
pub(crate) mod legacy;
pub(crate) mod project;
pub(crate) mod store;

use std::path::{Path, PathBuf};

pub(crate) use event::{EventBody, GateEvent};

/// The forge's name for a change: a GitHub pull request or a GitLab merge
/// request.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChangeForge {
    Pr,
    Mr,
}

/// The ledger's primary key: forge kind plus number, spelled `PR-<N>` /
/// `MR-<N>`.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ChangeKey {
    pub forge: ChangeForge,
    pub number: u64,
}

impl ChangeKey {
    pub(crate) fn pr(number: u64) -> Self {
        Self {
            forge: ChangeForge::Pr,
            number,
        }
    }

    /// `PR-7` / `pr-7` / `MR-7`; anything else (a spec ID) is not a change.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().to_ascii_uppercase();
        let (forge, digits) = if let Some(d) = raw.strip_prefix("PR-") {
            (ChangeForge::Pr, d)
        } else if let Some(d) = raw.strip_prefix("MR-") {
            (ChangeForge::Mr, d)
        } else {
            return None;
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            forge,
            number: digits.parse().ok()?,
        })
    }

    pub(crate) fn as_key(&self) -> String {
        let prefix = match self.forge {
            ChangeForge::Pr => "PR",
            ChangeForge::Mr => "MR",
        };
        format!("{prefix}-{}", self.number)
    }
}

/// `<root>/.aida/gate-ledger`.
pub(crate) fn ledger_dir(root: &Path) -> PathBuf {
    root.join(".aida").join("gate-ledger")
}

/// The checkout whose ledger `start` shares: the main checkout of a linked
/// worktree (read from git's files, no subprocess) or of a verified seat
/// clone. An unverified seat clone is refused rather than given a ledger of
/// its own: a worktree-local gate store is exactly what AC5 removes.
///
/// Memoised per process like `hold_store_root`: readers call this inside
/// per-spec loops, and resolving a seat clone lists leases. A refusal is
/// never memoised, so a clone resolves afresh once its lease verifies.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn store_root(start: &Path) -> std::io::Result<PathBuf> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MEMO: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(start).cloned()) {
        return Ok(hit);
    }
    let resolved = match crate::seat_clone::main_for(start) {
        crate::seat_clone::MainFor::Unverified(clone) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} is a seat clone that does not verify against its main checkout; refusing to resolve its gate ledger",
                    clone.display()
                ),
            ))
        }
        crate::seat_clone::MainFor::Verified(main) => main,
        // A root with no `.git` stays unmemoised, as in `hold_store_root`.
        crate::seat_clone::MainFor::NotAClone
            if std::fs::symlink_metadata(start.join(".git")).is_err() =>
        {
            return Ok(start.to_path_buf())
        }
        crate::seat_clone::MainFor::NotAClone => {
            crate::gate::merge_hold_core::hold_store_root(start)
        }
    };
    if let Ok(mut m) = memo.lock() {
        m.insert(start.to_path_buf(), resolved.clone());
    }
    Ok(resolved)
}

/// What recording an event did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Delivered {
    /// Appended to the ledger.
    Appended,
    /// The ledger already held this record.
    Unchanged,
}

/// Record one event from `start` (any checkout sharing the ledger). A
/// contained caller is refused: only uncontained primary-side code writes
/// the ledger.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn record_event(start: &Path, event: GateEvent) -> std::io::Result<Delivered> {
    if crate::session::aida_contained_env().unwrap_or(false) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot write the gate ledger; the launcher that started it collects its verdict",
        ));
    }
    store::append_one(&store_root(start)?, event)
}

/// Before a PR-keyed verdict file is written: take the change's one-time
/// legacy import now, so it captures the files as they were and the record
/// about to be written lands on the ledger once, as its own `verdict` event.
/// A contained writer cannot write the ledger; its submission is ingested
/// after the primary side's own import. Best-effort, like the mirror.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn before_verdict_write(path: &Path) {
    let Some((root, change)) = legacy::change_of_verdict_path(path) else {
        return;
    };
    if crate::session::aida_contained_env().unwrap_or(false) {
        return;
    }
    let imported = store_root(&root).and_then(|store| store::import_legacy(&store, change));
    if let Err(e) = imported {
        eprintln!(
            "warning: could not import the legacy verdicts of {} into its gate ledger: {e}",
            change.as_key()
        );
    }
}

/// Ledger half of a verdict write: append the record as a `verdict` event
/// for the change its path names. A spec-keyed path has no change key and is
/// skipped (slice 2 resolves spec → change). A contained writer skips it too:
/// its file stays in its seat clone for the launcher to collect. Best-effort
/// with a loud warning: the legacy file just written stays the compatibility
/// record in slice 1.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn mirror_verdict_write(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&str>,
) {
    let Some((root, change)) = legacy::change_of_verdict_path(path) else {
        return;
    };
    if crate::session::aida_contained_env().unwrap_or(false) {
        return;
    }
    let event = event::verdict_event(&root, change, record, spec);
    if let Err(e) = record_event(&root, event) {
        eprintln!(
            "warning: recorded {} but could not add it to the {} gate ledger: {e}",
            path.display(),
            change.as_key()
        );
    }
}

/// A verdict file written outside the record path (the review skill's
/// heredoc) at a PR-keyed path: append it to that change's ledger as it
/// stands. The file is left as written and nothing else is written beside
/// it. Any other path, or a contained caller, records nothing.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn adopt_verdict_file(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
) -> std::io::Result<()> {
    let Some((root, change)) = legacy::change_of_verdict_path(path) else {
        return Ok(());
    };
    if crate::session::aida_contained_env().unwrap_or(false) {
        return Ok(());
    }
    record_event(&root, event::verdict_event(&root, change, record, None)).map(|_| ())
}

/// A collected standalone verdict: the reviewer's record for `change`, then
/// one record per spec the PR credits, appended from the launcher's side in
/// one locked write. Returns how many events were new.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn record_collected_verdict(
    launcher_root: &Path,
    change: ChangeKey,
    record: &crate::gate::review_verdict::JsonObj,
    spec_records: &[(String, crate::gate::review_verdict::JsonObj)],
) -> std::io::Result<usize> {
    if crate::session::aida_contained_env().unwrap_or(false) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "a contained seat cannot collect a verdict onto the gate ledger",
        ));
    }
    let root = store_root(launcher_root)?;
    let mut events = vec![event::verdict_event(&root, change, record, None)];
    for (spec, body) in spec_records {
        events.push(event::verdict_event(&root, change, body, Some(spec)));
    }
    let appended = store::append(&root, change, events)?;
    Ok(appended
        .iter()
        .filter(|e| e.verdict_record().is_some())
        .count())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
