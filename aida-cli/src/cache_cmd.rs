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
    }
    Ok(())
}
