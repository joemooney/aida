//! Inline maintenance tick for an already-built history index.
//!
//! The indexer takes ONLY IndexLock: never RefreshLock or the store write lock.
//! It never spawns a detached worker, builds a missing index, or increments the
//! foreground stall counter. Schedule bookkeeping belongs to the scheduler;
//! this job advances history against one pinned store head within its budget.
// trace:STORY-1661 | ai:codex

use crate::history_cache::{self, Budget, IndexLock};
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Duration;

const RUNTIME_CAP: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    Disabled,
    NotBuilt,
    Busy,
    /// This checkout is behind the shared index, or diverged while the tip is
    /// live on another checkout: nothing to do here, and nothing written.
    // trace:STORY-1661 | ai:claude
    Skipped,
    UpToDate,
    Advanced {
        appended: usize,
        lag: usize,
    },
}

// trace:STORY-1661 | ai:codex
pub(crate) fn run_tick(store: &Path) -> Result<TickOutcome> {
    if !history_cache::cache_enabled() {
        return Ok(TickOutcome::Disabled);
    }
    run_tick_at(store, &history_cache::history_db_path(store), RUNTIME_CAP)
}

fn run_tick_at(store: &Path, db: &Path, cap: Duration) -> Result<TickOutcome> {
    if !history_cache::cache_enabled() {
        return Ok(TickOutcome::Disabled);
    }
    if !db.is_file() {
        return Ok(TickOutcome::NotBuilt);
    }
    let budget = Budget::for_duration(cap);
    let Some(_lock) = IndexLock::try_acquire(db)? else {
        return Ok(TickOutcome::Busy);
    };
    let Some((appended, lag)) = history_cache::catch_up_background_at(store, db, budget)? else {
        return Ok(TickOutcome::Skipped); // trace:STORY-1661 | ai:claude
    };
    if appended == 0 && lag == 0 {
        Ok(TickOutcome::UpToDate)
    } else {
        Ok(TickOutcome::Advanced { appended, lag })
    }
}

// Early dispatch is essential: no requirements-cache backend is constructed.
// trace:STORY-1661 | ai:codex
pub(crate) fn handle_tick(explicit_store: Option<&Path>) -> Result<()> {
    if !history_cache::cache_enabled() {
        return Ok(());
    }
    let store = match explicit_store {
        Some(store) => store.to_path_buf(),
        None => {
            let root = crate::find_aida_project_root_from(&std::env::current_dir()?)?;
            crate::detect_distributed_store_from(&root)
                .context("no git-canonical store found for the history tick")?
        }
    };
    match run_tick(&store)? {
        TickOutcome::Advanced { appended, lag } => {
            println!("History index: appended {appended} commit(s); {lag} still behind.");
        }
        // Busy, including an inherited-lock window, is a successful skipped tick.
        TickOutcome::Disabled
        | TickOutcome::NotBuilt
        | TickOutcome::Busy
        | TickOutcome::Skipped
        | TickOutcome::UpToDate => {}
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/story_1661_history_indexer_tests.rs"]
pub(crate) mod tests;
