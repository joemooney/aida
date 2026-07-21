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
        CacheCommand::Rebuild => {
            let n = backend.rebuild_cache()?;
            println!(
                "{}: Cache rebuilt. {} requirement(s) projected from git store at {}.",
                "OK".green(),
                n,
                backend.cache().path().display()
            );
        }
        CacheCommand::Status => {
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
        }
        CacheCommand::Verify { fix, json } => {
            return handle_cache_verify(backend, *fix, *json);
        }
    }
    Ok(())
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

    let store = backend.inner().load()?;
    let mut divergences = status_divergences(backend.cache(), &store)?;
    let mut rebuilt = false;

    if fix && !divergences.is_empty() {
        backend.rebuild_cache()?;
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
