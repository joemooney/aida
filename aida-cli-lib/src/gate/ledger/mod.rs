//! M1: one append-only gate ledger per change (PR/MR), in the primary
//! checkout's shared state — `<main>/.aida/gate-ledger/PR-<N>.jsonl`.
//!
//! The change is the only primary key. A spec ID is an index to changes
//! (`by-spec/<SPEC>.json`, a derived cache), never a second place state lives.
//! Every event names the head it was recorded at and the change's patch
//! identity, so a gate can tell "the same change on a moved base" from "new
//! code".
//!
//! One writer kind: uncontained AIDA on the primary side appends directly
//! ([`store::append`]). A contained seat cannot write `<main>/.aida/` outside
//! its mailbox (BUG-1929 / BUG-2218), so it submits events through the
//! mailbox ([`spool`]); every append and every read on the primary side first
//! drains those submissions, validates the recorder attestation, and appends
//! either the event or a `rejected-event` naming why.
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
pub(crate) mod spool;
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
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn store_root(start: &Path) -> std::io::Result<PathBuf> {
    match crate::seat_clone::main_for(start) {
        crate::seat_clone::MainFor::Unverified(clone) => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} is a seat clone that does not verify against its main checkout; refusing to resolve its gate ledger",
                clone.display()
            ),
        )),
        _ => Ok(crate::gate::merge_hold_core::hold_store_root(start)),
    }
}

/// Where a recorded event went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Delivered {
    /// Appended to the ledger directly (primary-side writer).
    Appended,
    /// Submitted through the mailbox for the primary side to ingest.
    Spooled(PathBuf),
    /// Identical to the newest event of its kind already on the ledger.
    Unchanged,
}

/// Record one event from `start` (any checkout, worktree or seat clone).
/// A contained caller, or one whose direct append is refused by the
/// filesystem boundary, submits through the mailbox instead.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn record_event(
    start: &Path,
    event: GateEvent,
    contained: bool,
) -> std::io::Result<Delivered> {
    let root = store_root(start)?;
    if contained {
        return spool::submit(&root, &event).map(Delivered::Spooled);
    }
    match store::append_one(&root, event.clone()) {
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            spool::submit(&root, &event).map(Delivered::Spooled)
        }
        other => other,
    }
}

/// Ledger half of a verdict write: append the record as a `verdict` event
/// for the change its path names. A spec-keyed path has no change key and is
/// skipped (slice 2 resolves spec → change). Best-effort with a loud warning:
/// the legacy file just written stays the compatibility record in slice 1.
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
pub(crate) fn mirror_verdict_write(
    path: &Path,
    record: &crate::gate::review_verdict::JsonObj,
    spec: Option<&str>,
) {
    let Some((root, change)) = legacy::change_of_verdict_path(path) else {
        return;
    };
    let contained = crate::session::aida_contained_env().unwrap_or(false);
    let event = event::verdict_event(&root, change, record, spec);
    if let Err(e) = record_event(&root, event, contained) {
        eprintln!(
            "warning: recorded {} but could not add it to the {} gate ledger: {e}",
            path.display(),
            change.as_key()
        );
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
