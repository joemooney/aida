//! Where the shared SQLite read cache lives, and the one-time move out of the
//! legacy layout.
//!
//! The cache is `<project>/.aida/cache-db/cache.db`. Its directory holds only
//! the database and its sidecars (`-wal`, `-shm`, `.lock-info`,
//! `.refresh.lock`, `.refresh-request*`, `.refresh.log`, quarantined
//! `.malformed-*` copies) and nothing else, so a contained lane can be granted
//! exactly that directory (the STORY-2238 directory-leaf invariant; a native
//! vendor sandbox can grant directories only). Linked worktrees and seat clones
//! reach the main checkout's cache through ONE directory link,
//! `<worktree>/.aida/cache-db -> <main>/.aida/cache-db`, instead of per-file
//! links.
//!
//! The legacy layout put `cache.db` and its sidecars directly in `.aida/`
//! (beside grants, leases, mail and advisor state), and worktrees linked
//! `cache.db`, `cache.db-shm` and `cache.db-wal` individually.
//! [`migrate_legacy_cache_layout`] moves a project out of it. There is no
//! compatibility shim: nothing reads the legacy path after the move.
// trace:TASK-2707 | ai:claude

use std::path::{Path, PathBuf};

/// The dedicated directory under `.aida/` that holds the cache and nothing else.
/// (`.aida/cache/` already holds unrelated JSON caches, so it cannot be the
/// cache-only directory.)
pub const CACHE_DIR_NAME: &str = "cache-db";

/// The cache database's file name inside [`CACHE_DIR_NAME`].
pub const CACHE_FILE_NAME: &str = "cache.db";

/// Legacy `.aida/` entries that belonged to the cache: the database plus every
/// sidecar an older binary created beside it. Quarantined `cache.db.malformed-*`
/// copies are matched by prefix (see [`LEGACY_QUARANTINE_PREFIX`]).
pub const LEGACY_CACHE_ENTRIES: [&str; 9] = [
    "cache.db",
    "cache.db-wal",
    "cache.db-shm",
    "cache.db-journal",
    "cache.db.lock-info",
    "cache.db.refresh.lock",
    "cache.db.refresh-request",
    "cache.db.refresh-request.lock",
    "cache.db.refresh.log",
];

/// Prefix of legacy quarantined cache copies (`cache.db.malformed-<stamp>...`).
pub const LEGACY_QUARANTINE_PREFIX: &str = "cache.db.malformed-";

/// Legacy entries that are kept for diagnosis, so the migration moves them into
/// the cache directory instead of deleting them.
const LEGACY_KEPT_ENTRIES: [&str; 1] = ["cache.db.refresh.log"];

/// `<aida_dir>/cache-db`.
pub fn cache_dir_in(aida_dir: &Path) -> PathBuf {
    aida_dir.join(CACHE_DIR_NAME)
}

/// `<project_root>/.aida/cache-db/cache.db`. A pure path computation: it does
/// not migrate. Openers reach the migration through `Cache::open` and
/// `CachedGitBackend::default_cache_path`; a caller that must see a migrated
/// layout before opening (session start, a contained launch) calls
/// [`ensure_project_cache_dir`].
pub fn project_cache_path(project_root: &Path) -> PathBuf {
    cache_dir_in(&project_root.join(".aida")).join(CACHE_FILE_NAME)
}

/// [`project_cache_path`] for a reader that opens the database directly (not
/// through `Cache::open`): it first moves `<project_root>/.aida` out of the
/// legacy layout, so the reader never misses a cache still at the old path. It
/// creates no directory when there is nothing to migrate, and leaves a
/// symlinked `.aida` alone. Best effort, like the migration itself.
// trace:TASK-2707 | ai:claude
pub fn migrated_project_cache_path(project_root: &Path) -> PathBuf {
    let aida_dir = project_root.join(".aida");
    if std::fs::symlink_metadata(&aida_dir).is_ok_and(|m| m.file_type().is_dir()) {
        migrate_legacy_cache_layout(&aida_dir);
    }
    project_cache_path(project_root)
}

/// When `cache_path` is `<x>/.aida/cache-db/cache.db`, the `<x>/.aida`
/// directory whose layout governs it.
pub fn aida_dir_of_cache_path(cache_path: &Path) -> Option<&Path> {
    let dir = cache_path.parent()?;
    let aida = dir.parent()?;
    (cache_path.file_name()? == CACHE_FILE_NAME
        && dir.file_name()? == CACHE_DIR_NAME
        && aida.file_name()? == ".aida")
        .then_some(aida)
}

/// Migrate `<project_root>/.aida` and make sure its cache directory exists, so
/// it can be linked into a worktree or granted to a sandbox. Best effort.
///
/// A symlinked `.aida` is left alone (nothing is written through it); the
/// contained launchers refuse such a layout themselves.
pub fn ensure_project_cache_dir(project_root: &Path) -> PathBuf {
    let aida_dir = project_root.join(".aida");
    let dir = cache_dir_in(&aida_dir);
    if !std::fs::symlink_metadata(&aida_dir).is_ok_and(|m| m.file_type().is_dir()) {
        return dir;
    }
    migrate_legacy_cache_layout(&aida_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// What [`migrate_legacy_cache_layout`] did. Informational; every arm leaves a
/// usable layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLayoutMigration {
    /// No legacy entry was present.
    NothingToDo,
    /// A worktree's legacy per-file links were replaced by one directory link.
    RelinkedWorktree,
    /// The legacy database was copied into the cache directory (or a copy was
    /// already there) and the legacy files were removed.
    Moved,
    /// The cache directory is populated, but a live connection or refresh still
    /// holds the legacy files, so they stay until a later run finds them idle.
    LegacyInUse,
}

/// Move one `.aida` directory out of the legacy cache layout. Idempotent, best
/// effort (never fails the caller), and cheap when nothing is legacy: a single
/// `lstat` of `<aida_dir>/cache.db`.
///
/// - A legacy `cache.db` SYMLINK (a worktree or seat clone set up by an older
///   binary) is replaced by a `cache-db` directory link to the target checkout's
///   cache directory, after migrating that checkout first. The legacy
///   `cache.db{,-shm,-wal}` links are removed only when they are links.
/// - A legacy `cache.db` FILE is copied into `cache-db/cache.db` through
///   SQLite's `VACUUM INTO` (a consistent snapshot even while another process
///   writes), published with a no-clobber hard link so two racing migrators
///   cannot replace a database a third process already opened. A corrupt
///   legacy database is not copied; the next open builds a fresh cache from the
///   store. The legacy files are deleted only while no other process holds a
///   SQLite connection on them (a shared lock on the `-shm` DMS byte) or the
///   refresh lock, so a seat still running an older binary is never pulled out
///   from under; a later run removes them once idle.
// trace:TASK-2707 | ai:claude
pub fn migrate_legacy_cache_layout(aida_dir: &Path) -> CacheLayoutMigration {
    let legacy = aida_dir.join("cache.db");
    let Ok(meta) = std::fs::symlink_metadata(&legacy) else {
        return CacheLayoutMigration::NothingToDo;
    };
    if meta.file_type().is_symlink() {
        relink_legacy_worktree(aida_dir, &legacy)
    } else if meta.file_type().is_file() {
        move_legacy_database(aida_dir, &legacy)
    } else {
        CacheLayoutMigration::NothingToDo
    }
}

fn relink_legacy_worktree(aida_dir: &Path, legacy_link: &Path) -> CacheLayoutMigration {
    let Some(target_aida) = legacy_link_target(legacy_link)
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
    else {
        return CacheLayoutMigration::NothingToDo;
    };
    let link = cache_dir_in(aida_dir);
    let target_dir = cache_dir_in(&target_aida);
    if target_aida != aida_dir && std::fs::symlink_metadata(&link).is_err() {
        // Migrate the checkout the legacy link pointed at first (the chain was
        // followed to its real file, so that checkout holds the database), so
        // the new link never points at a directory still to be filled.
        migrate_legacy_cache_layout(&target_aida);
        if !target_dir.is_dir() {
            // The target checkout has no cache directory (its migration could
            // not run, or it holds no cache at all): link nothing and keep the
            // legacy links, which still reach whatever the old layout shares.
            return CacheLayoutMigration::NothingToDo;
        }
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target_dir, &link).is_ok();
        #[cfg(not(unix))]
        let linked = false;
        if !linked && std::fs::symlink_metadata(&link).is_err() {
            // Could not place the link (a read-only worktree): keep the legacy
            // links so this worktree still reaches the old shared file.
            return CacheLayoutMigration::NothingToDo;
        }
    }
    if !link.is_dir() {
        // The directory link does not resolve to a cache directory: retiring
        // the legacy links would strand this worktree, so keep them.
        return CacheLayoutMigration::NothingToDo;
    }
    for name in ["cache.db", "cache.db-shm", "cache.db-wal"] {
        let path = aida_dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(&path);
        }
    }
    CacheLayoutMigration::RelinkedWorktree
}

/// The absolute target of a legacy `cache.db` link, followed through link
/// chains without requiring the target to exist.
fn legacy_link_target(link: &Path) -> Option<PathBuf> {
    let mut current = link.to_path_buf();
    for _ in 0..8 {
        let meta = std::fs::symlink_metadata(&current).ok();
        if !meta.is_some_and(|m| m.file_type().is_symlink()) {
            return Some(current);
        }
        let target = std::fs::read_link(&current).ok()?;
        current = if target.is_absolute() {
            target
        } else {
            current.parent()?.join(target)
        };
    }
    None
}

fn move_legacy_database(aida_dir: &Path, legacy: &Path) -> CacheLayoutMigration {
    let dir = cache_dir_in(aida_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return CacheLayoutMigration::NothingToDo;
    }
    sweep_stale_migration_temps(aida_dir);
    let target = dir.join(CACHE_FILE_NAME);
    if std::fs::symlink_metadata(&target).is_err() {
        copy_snapshot(legacy, aida_dir, &target);
    }
    if legacy_cache_in_use(aida_dir) {
        return CacheLayoutMigration::LegacyInUse;
    }
    retire_legacy_files(aida_dir, &dir);
    CacheLayoutMigration::Moved
}

/// Prefix of the private `VACUUM INTO` temp files [`copy_snapshot`] writes
/// beside the legacy database: `.cache-db-migrate.<pid>.<nanos>`.
const MIGRATION_TEMP_PREFIX: &str = ".cache-db-migrate.";

/// A migration temp file younger than this is never swept, even when its owner
/// pid looks dead: a migrator in another PID namespace (a contained lane) has a
/// pid this process cannot see.
const MIGRATION_TEMP_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(600);

/// Remove `.cache-db-migrate.<pid>.*` temp files left by a migrator that died
/// mid-copy: the owner pid is no longer alive and the file is not recent. A
/// live migrator's temp file is never touched.
// trace:TASK-2707 | ai:claude
fn sweep_stale_migration_temps(aida_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(aida_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(pid) = name
            .strip_prefix(MIGRATION_TEMP_PREFIX)
            .and_then(|rest| rest.split('.').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age >= MIGRATION_TEMP_MIN_AGE);
        if meta.file_type().is_file() && old_enough && !crate::liveness::pid_is_alive(pid) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// `VACUUM INTO` a private temp file beside the legacy database, then publish it
/// at `target` only if nothing is there yet. Any failure leaves `target` absent,
/// and the next open creates and refreshes a fresh cache.
fn copy_snapshot(legacy: &Path, aida_dir: &Path, target: &Path) {
    let tmp = aida_dir.join(format!(
        "{MIGRATION_TEMP_PREFIX}{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let copied = (|| -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(legacy)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy().as_ref()])?;
        conn.close().map_err(|(_, e)| e)
    })();
    if copied.is_ok() {
        // No-clobber publish: `link` fails when a racing migrator won.
        let _ = std::fs::hard_link(&tmp, target);
    }
    let _ = std::fs::remove_file(&tmp);
}

/// Whether another process still uses the legacy cache in `aida_dir`: a live
/// SQLite connection (every WAL-mode connection holds a shared POSIX lock on the
/// `-shm` dead-man-switch byte for its lifetime) or a held refresh lock (flock).
/// Unknown states count as in use.
#[cfg(unix)]
fn legacy_cache_in_use(aida_dir: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    // SQLite unix VFS: UNIX_SHM_BASE (120) + SQLITE_SHM_NLOCK (8).
    const SHM_DMS_BYTE: libc::off_t = 128;
    if let Ok(shm) = std::fs::File::open(aida_dir.join("cache.db-shm")) {
        // SAFETY: `flock` is plain data; an all-zero value is valid.
        let mut probe: libc::flock = unsafe { std::mem::zeroed() };
        probe.l_type = libc::F_WRLCK as _;
        probe.l_whence = libc::SEEK_SET as _;
        probe.l_start = SHM_DMS_BYTE;
        probe.l_len = 1;
        // SAFETY: a valid open fd and a pointer to a live `flock`.
        let rc = unsafe { libc::fcntl(shm.as_raw_fd(), libc::F_GETLK, &mut probe) };
        if rc != 0 || i32::from(probe.l_type) != libc::F_UNLCK {
            return true;
        }
    }
    for lock in ["cache.db.refresh.lock", "cache.db.refresh-request.lock"] {
        let Ok(file) = std::fs::File::open(aida_dir.join(lock)) else {
            continue;
        };
        // SAFETY: a valid open fd.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return true;
        }
        // SAFETY: releases the lock just taken on the same fd.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    }
    false
}

#[cfg(not(unix))]
fn legacy_cache_in_use(_aida_dir: &Path) -> bool {
    false
}

fn retire_legacy_files(aida_dir: &Path, cache_dir: &Path) {
    let quarantined: Vec<String> = std::fs::read_dir(aida_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(LEGACY_QUARANTINE_PREFIX))
        .collect();
    let kept = LEGACY_KEPT_ENTRIES
        .iter()
        .map(|s| s.to_string())
        .chain(quarantined);
    for name in kept {
        let from = aida_dir.join(&name);
        let to = cache_dir.join(&name);
        if std::fs::symlink_metadata(&from).is_ok_and(|m| m.file_type().is_file())
            && std::fs::symlink_metadata(&to).is_err()
        {
            let _ = std::fs::rename(&from, &to);
        }
    }
    // The database goes last, so an interrupted run is retried (the next
    // migration still sees the legacy `cache.db` and finishes the sweep).
    for name in LEGACY_CACHE_ENTRIES.iter().rev() {
        let path = aida_dir.join(name);
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file()) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
#[path = "cache_location_tests.rs"]
mod tests;
