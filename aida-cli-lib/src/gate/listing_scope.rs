//! A listing-wide read snapshot for verdict listings (BUG-2543).
//!
//! `aida review list` (and lane liveness) resolve the current verdict of
//! every key, ~2,800 on a busy checkout. Each single-key read rediscovers the
//! whole gate ledger (the BUG-2517 mis-key ownership index) and re-resolves
//! the review-publication store, its tip, its spec index and the canonical
//! snapshot through git. Per key that is ~0.75 s, so a listing ran 30+ min.
//!
//! [`ListingScope::enter`] pins those per-root answers for the duration of
//! one listing: the first key computes them, every later key reuses them.
//! Outside a scope nothing is memoized, so single-key gate reads keep their
//! exact fresh-read semantics. Failures are never memoized except the
//! ownership index's fail-closed answer, which is what every key would get,
//! and an over-budget aggregate listing, whose memo only routes each key
//! back to its own bounded per-directory read.
// trace:BUG-2543 | ai:claude

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// The per-root answers one listing shares.
#[derive(Default)]
pub(crate) struct Memo {
    /// Ledger root -> mis-key ownership index and the identity of every
    /// ledger it covers; each read is verified against it and any change
    /// discards it (see `ledger::checked::OwnersSnapshot`).
    pub(crate) owners: HashMap<PathBuf, super::ledger::checked::OwnersSnapshot>,
    /// Project root -> publication store and its pinned tip.
    pub(crate) store_view: HashMap<PathBuf, Option<(PathBuf, String)>>,
    /// (store, tip, top) -> every committed name under `top`, sorted;
    /// `None` when that aggregate read failed or exceeded its budget, so
    /// each directory is read on its own exactly as a single-key read does.
    pub(crate) aggregate: HashMap<(PathBuf, String, &'static str), Option<Rc<Vec<String>>>>,
    /// Project root -> its hashed origin (publication repository identity).
    pub(crate) repository: HashMap<PathBuf, String>,
    /// (store, tip) -> the review node roster committed there.
    pub(crate) roster: HashMap<(PathBuf, String), Rc<super::review_publication::Roster>>,
    /// Project root -> canonical snapshot at the store head.
    pub(crate) snapshot: HashMap<PathBuf, Rc<super::canon::Snapshot>>,
}

thread_local! {
    static SCOPE: RefCell<Option<Memo>> = const { RefCell::new(None) };
}

/// While alive, single-key reads on this thread share one snapshot. Nested
/// scopes join the outermost one.
pub(crate) struct ListingScope {
    owner: bool,
}

impl ListingScope {
    pub(crate) fn enter() -> Self {
        SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            let owner = scope.is_none();
            if owner {
                *scope = Some(Memo::default());
            }
            Self { owner }
        })
    }
}

impl Drop for ListingScope {
    fn drop(&mut self) {
        if self.owner {
            SCOPE.with(|scope| *scope.borrow_mut() = None);
        }
    }
}

/// Read from the active scope; `None` when no scope is active or it holds
/// no answer yet. The borrow is released before the caller computes a miss,
/// so a computation may itself consult the scope.
pub(crate) fn get<T>(read: impl FnOnce(&Memo) -> Option<T>) -> Option<T> {
    SCOPE.with(|scope| scope.borrow().as_ref().and_then(read))
}

/// Record into the active scope; a no-op outside one.
pub(crate) fn put(write: impl FnOnce(&mut Memo)) {
    SCOPE.with(|scope| {
        if let Some(memo) = scope.borrow_mut().as_mut() {
            write(memo);
        }
    });
}

/// Whether a scope is active on this thread.
pub(crate) fn active() -> bool {
    SCOPE.with(|scope| scope.borrow().is_some())
}
