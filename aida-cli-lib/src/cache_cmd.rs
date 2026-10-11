//! `aida cache` command cluster (`cache status` / `cache rebuild`).
//!
//! CLI-side management of the SQLite read-projection cache: rebuild it from the
//! git-canonical store, and report freshness by comparing the cache's recorded
//! HEAD SHA against the orphan store's actual HEAD. The cache engine itself
//! (rebuild / stale-detection) lives in `aida_core::db::cache` /
//! `cached_git_backend`; this file is only the command surface. Extracted
//! verbatim from `main.rs` (SPIKE-78); no behavior change.

use anyhow::Result;
use colored::Colorize;

use crate::CacheCommand;

pub(crate) fn handle_cache_command(
    cmd: &CacheCommand,
    backend: &aida_core::CachedGitBackend,
) -> Result<()> {
    use aida_core::DatabaseBackend;

    match cmd {
        CacheCommand::Rebuild { history: true } => {
            // trace:TASK-1507 | ai:claude
            println!("Rebuilding the history index from the store's git log...");
            let report = crate::history_cache::rebuild_full(backend.path())?;
            println!(
                "{}: History index rebuilt in {:.1}s. {} commit(s), {} event(s) at {}.",
                "OK".green(),
                report.elapsed.as_secs_f64(),
                report.commits,
                report.events,
                report.path.display()
            );
            for p in &report.pruned {
                println!(
                    "Removed an index file from another version: {}",
                    p.display()
                );
            }
        }
        CacheCommand::Rebuild { history: false } => {
            // trace:BUG-1940 | ai:claude
            let outcome = backend.rebuild_cache_recovering()?;
            print_quarantine_notice(&outcome);
            let n = outcome.count;
            println!(
                "{}: Cache rebuilt. {} requirement(s) projected from git store at {}.",
                "OK".green(),
                n,
                backend.cache().path().display()
            );
            // trace:TASK-1507 | ai:claude
            println!(
                "{}",
                "(the history index is separate; rebuild it with `aida cache rebuild --history`)"
                    .dimmed()
            );
        }
        CacheCommand::Status => {
            // trace:TASK-1526 | ai:codex
            backend.ensure_cache_fresh_with_schema_retry()?;
            let cache = backend.cache();
            let recorded_sha = cache.source_head_sha()?.unwrap_or_default();
            let actual_sha = aida_core::git_ops::head_sha(backend.path()).unwrap_or_default();
            let count = cache.requirement_count()?;
            let built_at = cache.built_at()?.unwrap_or_else(|| "(never)".into());
            // BUG-664: count object files without parsing them — a full
            // `list_requirements(true)` YAML-parses every object (~1s) just to
            // print a count; the directory walk is O(files) with no parse.
            let store_count = backend.inner().object_count()?;

            println!("Cache path:       {}", cache.path().display());
            println!("Cached requirements: {}", count);
            println!("Store requirements:  {}", store_count);
            println!("Last built:       {}", built_at);
            println!(
                "Cache HEAD SHA:   {}",
                if recorded_sha.is_empty() {
                    "(none)".to_string()
                } else {
                    recorded_sha.clone()
                }
            );
            println!(
                "Store HEAD SHA:   {}",
                if actual_sha.is_empty() {
                    "(no git head — non-git store?)".to_string()
                } else {
                    actual_sha.clone()
                }
            );
            let stale = recorded_sha != actual_sha || recorded_sha.is_empty();
            if stale && !actual_sha.is_empty() {
                println!(
                    "Status:           {} — run `aida cache rebuild`",
                    "STALE".yellow()
                );
            } else {
                println!("Status:           {}", "FRESH".green());
            }
            print_refresh_status(cache.path(), &actual_sha);
            print_history_status(&crate::history_cache::status(backend.path()));
        }
        CacheCommand::Verify { fix, json } => {
            return handle_cache_verify(backend, *fix, *json);
        }
        CacheCommand::Refresh {
            worker,
            if_requested,
            store,
            cache,
        } => {
            return handle_cache_refresh(backend, *worker, *if_requested, store, cache);
        }
    }
    Ok(())
}

/// `aida cache refresh`: plain form refreshes strictly inline; `--worker` is
/// the detached single-flight worker (amendment A8: explicit paths, exits by
/// outcome); `--if-requested` is the schedule-tick form, a silent no-op
/// without a pending request.
// trace:TASK-1527 | ai:claude
fn handle_cache_refresh(
    ambient: &aida_core::CachedGitBackend,
    worker: bool,
    if_requested: bool,
    store: &Option<std::path::PathBuf>,
    cache: &Option<std::path::PathBuf>,
) -> Result<()> {
    use aida_core::db::{refresh_request, refresh_worker};

    // A worker trusts only its explicit paths, never the inferred project
    // (amendment A8). The scheduler and plain forms use the ambient backend.
    let explicit;
    let backend = match (store, cache) {
        (Some(store), Some(cache)) => {
            explicit = aida_core::CachedGitBackend::open(store, cache)?;
            &explicit
        }
        (None, None) => ambient,
        _ => anyhow::bail!("--store and --cache must be passed together"),
    };

    if worker {
        // The runtime cap (sketch: max(120 s, 2x the last full rebuild)) is a
        // watchdog that exits nonzero: the transaction rolls back under WAL,
        // the flock dies with the process, and the request file survives for
        // the next reader or tick.
        let cap = refresh_worker::worker_runtime_cap(backend);
        std::thread::spawn(move || {
            std::thread::sleep(cap);
            eprintln!(
                "refresh worker exceeded its runtime cap ({}s); exiting",
                cap.as_secs()
            );
            std::process::exit(3);
        });
        let cache_path = backend.cache().path().to_path_buf();
        return match refresh_worker::run_refresh_worker(backend) {
            Ok(outcome) => {
                eprintln!(
                    "{}: refresh worker: {:?} at {}",
                    chrono::Utc::now().to_rfc3339(),
                    outcome,
                    cache_path.display()
                );
                Ok(())
            }
            Err(e) => {
                refresh_request::record_failed_attempt(&cache_path, &format!("{e:#}"))?;
                Err(e)
            }
        };
    }

    if if_requested {
        let cache_path = backend.cache().path();
        let Some(request) = refresh_request::load(cache_path) else {
            return Ok(());
        };
        if request.spawning_suppressed(chrono::Utc::now()) {
            println!(
                "cache refresh pending at {} but suppressed after repeated worker failures{}; \
                 run `aida cache refresh` to refresh inline",
                request.target_head,
                request
                    .last_error()
                    .map(|e| format!(" (last: {e})"))
                    .unwrap_or_default()
            );
            return Ok(());
        }
        // The tick already runs us in a separate process: do the work inline
        // rather than spawning a third process.
        let outcome = refresh_worker::run_refresh_worker(backend).inspect_err(|e| {
            let _ = refresh_request::record_failed_attempt(cache_path, &format!("{e:#}"));
        })?;
        println!("cache refresh --if-requested: {outcome:?}");
        return Ok(());
    }

    // trace:BUG-1779 | ai:antigravity
    backend.ensure_cache_fresh_with_schema_retry()?;
    if let Some(head) = backend.cache().source_head_sha()? {
        refresh_request::clear_if_target_matches(backend.cache().path(), &head)?;
    } else {
        refresh_request::clear(backend.cache().path())?;
    }
    println!(
        "{}: cache fresh at {}",
        "OK".green(),
        backend
            .cache()
            .source_head_sha()?
            .unwrap_or_else(|| "(no head)".into())
    );
    Ok(())
}

/// The Refresh section of `aida cache status` (story acceptance #5): the
/// refresh flock holder and the pending durable request, with amendment A7's
/// crash-loop stand-down made visible where it bites.
// trace:TASK-1527 | ai:claude
fn print_refresh_status(cache_path: &std::path::Path, store_head: &str) {
    // trace:BUG-1779 | ai:antigravity
    use aida_core::db::{cache_refresh::RefreshLock, refresh_request};
    let lock_line = match RefreshLock::try_acquire(cache_path) {
        // The probe guard drops at the end of this match: holding it for the
        // lifetime of a status print would block a real refresh.
        Ok(Some(_probe)) => "free".to_string(),
        Ok(None) => "held (a refresh is running)".yellow().to_string(),
        Err(e) => format!("(unreadable: {e})"),
    };
    println!("Refresh lock:     {lock_line}");
    match refresh_request::load(cache_path) {
        None => println!("Refresh request:  (none pending)"),
        Some(request) => {
            let age = chrono::DateTime::parse_from_rfc3339(&request.requested_at)
                .map(|at| {
                    let mins = (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_minutes();
                    format!("{mins}m ago")
                })
                .unwrap_or_else(|_| "at an unknown time".into());
            let satisfied = !store_head.is_empty() && request.target_head == store_head;
            println!(
                "Refresh request:  pending for {} (requested {age}{})",
                request.target_head,
                if satisfied {
                    "; already satisfied — clears on the next refresh run"
                } else {
                    ""
                }
            );
            let failures = request
                .attempts
                .iter()
                .filter(|a| a.error.is_some())
                .count();
            if failures > 0 {
                println!(
                    "Refresh workers:  {failures} failed attempt(s){}",
                    request
                        .last_error()
                        .map(|e| format!(", last: {e}"))
                        .unwrap_or_default()
                );
            }
            if request.spawning_suppressed(chrono::Utc::now()) {
                println!(
                    "Refresh spawning: {} — run `aida cache refresh` to refresh inline",
                    "SUPPRESSED after repeated worker failures".yellow()
                );
            }
        }
    }
}

/// The History section of `aida cache status`: where the history index
/// lives, how far it reaches, and whether it matches the store HEAD.
// trace:TASK-1507 | ai:claude
fn print_history_status(st: &crate::history_cache::HistoryCacheStatus) {
    println!();
    println!("History index:    {}", st.path.display());
    for line in history_status_lines(st) {
        println!("{line}");
    }
}

/// Pure rendering of the History section rows (after the path line), so the
/// wording is testable without capturing stdout.
// trace:TASK-1507 | ai:claude
pub(crate) fn history_status_lines(st: &crate::history_cache::HistoryCacheStatus) -> Vec<String> {
    let mut out = Vec::new();
    if !st.exists {
        out.push(
            "History status:   not built yet — it builds on the next `aida history` \
             query, or run `aida cache rebuild --history`"
                .to_string(),
        );
        return out;
    }
    if let Some(err) = &st.error {
        out.push(format!(
            "History status:   {} ({err}) — `aida history` reads git directly; \
             run `aida cache rebuild --history`",
            "UNREADABLE".yellow()
        ));
        return out;
    }
    out.push(format!(
        "History events:   {} event(s) from {} commit(s)",
        st.events, st.commits
    ));
    out.push(format!(
        "History reaches:  {}",
        if st.complete {
            "the start of the store history".to_string()
        } else {
            match &st.floor_commit_at {
                Some(at) => format!("back to {at} (still filling in older history)"),
                None => "nothing yet (still filling in older history)".to_string(),
            }
        }
    ));
    out.push(format!(
        "History updated:  {}",
        st.updated_at.as_deref().unwrap_or("(never)")
    ));
    out.push(format!(
        "History tip SHA:  {}",
        st.tip.as_deref().unwrap_or("(none)")
    ));
    if let Some(reason) = &st.last_reset_reason {
        out.push(format!("History last reset: {reason}"));
    }
    if st.indexer_running {
        out.push("History indexer:  running now".to_string());
    }
    // trace:BUG-1923 | ai:claude
    if let Some(lag) = st.lag {
        out.push(format!("History lag:      {lag} commit(s) behind HEAD"));
    }
    // trace:BUG-1942 | ai:claude
    if st.catch_up_stalled {
        let since = st
            .catch_up_stalled_at
            .as_deref()
            .map(|at| format!(" since {at}"))
            .unwrap_or_default();
        let retry = st.catch_up_next_retry_at.as_deref().map_or_else(
            || "on the next `aida history events` query".to_string(),
            |at| format!("after {at}"),
        );
        out.push(format!("History catch-up: paused{since}; retries {retry}"));
    }
    let fresh = st.tip.is_some() && st.tip == st.head;
    out.push(if fresh && st.complete {
        format!("History status:   {}", "FRESH".green())
    } else if fresh {
        format!(
            "History status:   {} — recent history is served from the index; \
             older queries read git directly until it finishes",
            "FILLING".yellow()
        )
    } else if st.catch_up_stalled {
        format!(
            "History status:   {} — catching up stalled; `aida history` reads git \
             directly until the next retry succeeds or `aida cache rebuild --history` \
             brings it up to date",
            "BEHIND".yellow()
        )
    } else {
        format!(
            // trace:STORY-1650 | ai:claude
            "History status:   {} — it catches up on the next `aida history events` query",
            "BEHIND".yellow()
        )
    });
    out
}

/// Cross-check the cache's projected status for every spec against the status
/// the git store projects for it, and report (or repair) the drift.
///
/// The HEAD-SHA check `cache status` prints catches a cache that is BEHIND the
/// store. It cannot catch a row that disagrees while the two HEADs match — a
/// silent projection lie, which is exactly how a Rejected epic rendered as
/// Draft in `list`/`show` long enough for an advisor to start decomposing
/// closed work. Status drift is the dangerous class (closed work looks open,
/// open work looks closed), so it gets an explicit sweep.
///
/// The store is loaded through `backend.inner()` — the RAW git backend — on
/// purpose: the cached backend's `load()` freshens the cache first, which would
/// repair the very drift being audited before it could be observed.
///
/// Exit contract: non-zero (via an error) when drift remains, so the sweep is
/// usable as a gate. `--fix` rebuilds and re-checks first.
// trace:BUG-771 | ai:claude
fn handle_cache_verify(backend: &aida_core::CachedGitBackend, fix: bool, json: bool) -> Result<()> {
    use aida_core::db::status_divergences;
    use aida_core::DatabaseBackend;

    // trace:BUG-1940 | ai:claude
    // A damaged cache file is reported as corruption with the fix, not as the
    // raw sqlite error the status comparison below would hit.
    let mut rebuilt = false;
    if let Some(problem) = backend.cache().integrity_problem()? {
        if !fix {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "corrupt": true,
                        "problem": problem,
                        "fix": "aida cache rebuild",
                    }))?
                );
            }
            anyhow::bail!(cache_corruption_message(Some(&problem)));
        }
        let outcome = backend.rebuild_cache_recovering()?;
        if !json {
            print_quarantine_notice(&outcome);
        }
        rebuilt = true;
    }

    let store = backend.inner().load()?;
    let mut divergences = status_divergences(backend.cache(), &store)?;

    if fix && !divergences.is_empty() {
        backend.rebuild_cache_recovering()?;
        rebuilt = true;
        // Re-derive from a freshly-read store so the re-check compares against
        // the same canonical bytes the rebuild projected from.
        let store = backend.inner().load()?;
        divergences = status_divergences(backend.cache(), &store)?;
    }

    if json {
        let rows: Vec<serde_json::Value> = divergences
            .iter()
            .map(|d| {
                serde_json::json!({
                    "spec_id": d.spec_id,
                    "uuid": d.id.to_string(),
                    "is_epic": d.is_epic,
                    "cached": d.cached,
                    "expected": d.expected,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checked": store.requirements.len(),
                "rebuilt": rebuilt,
                "diverged": rows.len(),
                "divergences": rows,
            }))?
        );
    } else if divergences.is_empty() {
        let suffix = if rebuilt { " after rebuild" } else { "" };
        println!(
            "{}: every cached status agrees with the store{}.",
            "OK".green(),
            suffix
        );
    } else {
        println!(
            "{}: {} spec(s) whose cached status disagrees with the store.",
            "DRIFT".red(),
            divergences.len()
        );
        for d in &divergences {
            println!(
                "  {:<14} cache={} store={}",
                d.spec_id,
                d.cached.yellow(),
                d.expected.green()
            );
        }
        if !fix {
            println!("\nRepair: `aida cache verify --fix` (rebuilds, then re-checks).");
        }
    }

    if divergences.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "{} cached status(es) disagree with the git store",
            divergences.len()
        )
    }
}

/// One-line, actionable description of a corrupt cache.
// trace:BUG-1940 | ai:claude
pub(crate) fn cache_corruption_message(problem: Option<&str>) -> String {
    match problem {
        Some(problem) => format!(
            "the local AIDA cache (.aida/cache.db) is corrupt ({problem}). Fix: aida cache rebuild"
        ),
        None => {
            "the local AIDA cache (.aida/cache.db) is corrupt. Fix: aida cache rebuild".to_string()
        }
    }
}

/// Replace an error tagged at its source as cache-database corruption
/// ([`aida_core::db::CacheCorruption`]) with the one-line fix, keeping the
/// original sqlite cause visible on a second line. Untagged errors — including
/// corruption in other sqlite files such as the history index or token ledger,
/// and every lock/busy error — pass through untouched.
// trace:BUG-1940 | ai:claude
pub(crate) fn with_cache_corruption_hint(err: anyhow::Error) -> anyhow::Error {
    let Some(corruption) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<aida_core::db::CacheCorruption>())
    else {
        return err;
    };
    let cause = err
        .chain()
        .find(|cause| cause.downcast_ref::<rusqlite::Error>().is_some())
        .or_else(|| err.chain().last())
        .map(|cause| cause.to_string())
        .unwrap_or_default();
    anyhow::anyhow!("{corruption}. Fix: aida cache rebuild\n  cause: {cause}")
}

// trace:BUG-1940 | ai:claude
fn print_quarantine_notice(outcome: &aida_core::db::CacheRebuildOutcome) {
    let Some(main) = outcome.quarantined.first() else {
        return;
    };
    let problem = outcome
        .problem
        .as_deref()
        .unwrap_or("integrity check failed");
    println!(
        "{}: the cache was corrupt ({}); moved it aside to {} (with any -wal/-shm files) and rebuilt it from the git store.",
        "NOTE".yellow(),
        problem,
        main.display()
    );
}

#[cfg(test)]
mod bug_1940_tests {
    use super::*;

    fn corrupt() -> anyhow::Error {
        anyhow::Error::from(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("database disk image is malformed".into()),
        ))
    }

    // trace:BUG-1940 | ai:claude
    #[test]
    fn untagged_sqlite_corruption_from_another_db_is_not_rewritten() {
        let err = corrupt().context("Failed to read history-v3.db");
        let out = with_cache_corruption_hint(err);
        assert!(!out.to_string().contains("aida cache rebuild"), "{out}");
        assert!(out.to_string().contains("history-v3.db"));
    }

    // trace:BUG-1940 | ai:claude
    #[test]
    fn tagged_cache_corruption_gets_the_fix_and_keeps_the_cause() {
        let err = aida_core::db::tag_cache_corruption(
            std::path::Path::new("/p/.aida/cache.db"),
            corrupt(),
        )
        .context("list requirements");
        let out = with_cache_corruption_hint(err).to_string();
        assert!(
            out.contains("/p/.aida/cache.db is corrupt. Fix: aida cache rebuild"),
            "{out}"
        );
        assert!(
            out.contains("cause: database disk image is malformed"),
            "{out}"
        );
    }
}
