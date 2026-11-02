//! TASK-2707 PR0: the cache's own directory and the one-time legacy move.
// trace:TASK-2707 | ai:claude

use super::*;
use rusqlite::Connection;

/// A WAL-mode SQLite database at `path` holding one marker row.
fn seed_db(path: &Path, marker: &str) {
    let conn = Connection::open(path).unwrap();
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
        .unwrap();
    conn.execute_batch("CREATE TABLE t (v TEXT)").unwrap();
    conn.execute("INSERT INTO t VALUES (?1)", [marker]).unwrap();
}

fn marker(path: &Path) -> String {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT v FROM t", [], |r| r.get(0))
        .unwrap()
}

fn aida_dir(root: &Path) -> PathBuf {
    let dir = root.join(".aida");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A legacy primary `.aida/`: the database, its WAL sidecars and the other
/// sidecars, plus unrelated state that must stay untouched.
fn legacy_primary(root: &Path) -> PathBuf {
    let aida = aida_dir(root);
    seed_db(&aida.join("cache.db"), "legacy-row");
    for name in [
        "cache.db.lock-info",
        "cache.db.refresh.lock",
        "cache.db.refresh-request",
        "cache.db.refresh-request.lock",
    ] {
        std::fs::write(aida.join(name), b"").unwrap();
    }
    std::fs::write(aida.join("cache.db.refresh.log"), b"log").unwrap();
    std::fs::write(aida.join("cache.db.malformed-2026-10-10"), b"q").unwrap();
    std::fs::write(aida.join("config.toml"), b"cfg").unwrap();
    std::fs::create_dir_all(aida.join("cache")).unwrap();
    std::fs::write(aida.join("cache/worktree-verdicts.json"), b"{}").unwrap();
    aida
}

#[test]
fn project_cache_path_is_the_dedicated_directory() {
    let root = Path::new("/p");
    assert_eq!(
        project_cache_path(root),
        PathBuf::from("/p/.aida/cache-db/cache.db")
    );
    assert_eq!(
        aida_dir_of_cache_path(&project_cache_path(root)),
        Some(Path::new("/p/.aida"))
    );
    for other in [
        "/p/.aida/cache.db",
        "/p/cache-db/cache.db",
        "/p/.aida/cache-db/x.db",
    ] {
        assert_eq!(aida_dir_of_cache_path(Path::new(other)), None, "{other}");
    }
}

#[test]
fn nothing_legacy_is_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = aida_dir(tmp.path());
    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::NothingToDo
    );
    assert!(!cache_dir_in(&aida).exists(), "a no-op creates nothing");
}

#[test]
fn primary_legacy_cache_moves_into_its_own_directory_once() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());

    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::Moved
    );

    let dir = cache_dir_in(&aida);
    assert_eq!(marker(&dir.join("cache.db")), "legacy-row");
    for name in LEGACY_CACHE_ENTRIES {
        if name == "cache.db.refresh.log" {
            continue;
        }
        assert!(
            std::fs::symlink_metadata(aida.join(name)).is_err(),
            "legacy {name} must be gone"
        );
    }
    assert_eq!(
        std::fs::read(dir.join("cache.db.refresh.log")).unwrap(),
        b"log"
    );
    assert!(dir.join("cache.db.malformed-2026-10-10").is_file());
    assert!(!aida.join("cache.db.malformed-2026-10-10").exists());
    // Unrelated state is untouched, and nothing but the cache lands in its dir.
    assert_eq!(std::fs::read(aida.join("config.toml")).unwrap(), b"cfg");
    assert!(aida.join("cache/worktree-verdicts.json").is_file());
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(
            name.starts_with("cache.db"),
            "stray {name} in the cache dir"
        );
    }

    // Idempotent: a second run finds nothing legacy and leaves the copy alone.
    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::NothingToDo
    );
    assert_eq!(marker(&dir.join("cache.db")), "legacy-row");
}

#[test]
fn an_existing_new_cache_is_never_overwritten_by_the_legacy_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());
    std::fs::create_dir_all(cache_dir_in(&aida)).unwrap();
    seed_db(&cache_dir_in(&aida).join("cache.db"), "new-row");

    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::Moved
    );
    assert_eq!(marker(&cache_dir_in(&aida).join("cache.db")), "new-row");
    assert!(!aida.join("cache.db").exists());
}

#[test]
fn a_corrupt_legacy_cache_is_dropped_not_copied() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = aida_dir(tmp.path());
    std::fs::write(aida.join("cache.db"), b"not a sqlite database at all").unwrap();

    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::Moved
    );
    assert!(cache_dir_in(&aida).is_dir());
    assert!(!cache_dir_in(&aida).join("cache.db").exists());
    assert!(!aida.join("cache.db").exists());
    let leftovers: Vec<_> = std::fs::read_dir(&aida)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".cache-db-migrate"))
        .collect();
    assert!(leftovers.is_empty(), "temp copies left: {leftovers:?}");
}

/// A held refresh lock (an older binary mid-refresh) keeps the legacy files;
/// the next run after release finishes the sweep.
#[cfg(unix)]
#[test]
fn a_held_legacy_refresh_lock_defers_the_legacy_sweep() {
    use std::os::unix::io::AsRawFd;
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());
    let held = std::fs::File::open(aida.join("cache.db.refresh.lock")).unwrap();
    // SAFETY: a valid open fd.
    assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX) }, 0);

    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::LegacyInUse
    );
    assert_eq!(marker(&cache_dir_in(&aida).join("cache.db")), "legacy-row");
    assert!(aida.join("cache.db").is_file(), "in-use legacy stays");

    drop(held);
    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::Moved
    );
    assert!(!aida.join("cache.db").exists());
}

/// A live SQLite connection in ANOTHER process (a seat still running an older
/// binary) keeps the legacy files: it holds the `-shm` dead-man-switch lock.
/// Needs `python3` with its sqlite3 module; skips without it.
#[cfg(unix)]
#[test]
fn a_live_legacy_connection_in_another_process_defers_the_sweep() {
    use std::io::{BufRead, BufReader};
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());
    let script = "import sqlite3, sys\n\
                  c = sqlite3.connect(sys.argv[1])\n\
                  c.execute('SELECT count(*) FROM t').fetchall()\n\
                  print('ready', flush=True)\n\
                  sys.stdin.read()\n";
    let Ok(mut child) = std::process::Command::new("python3")
        .args(["-c", script])
        .arg(aida.join("cache.db"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        eprintln!("skipping: python3 unavailable");
        return;
    };
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    if line.trim() != "ready" {
        let _ = child.kill();
        eprintln!("skipping: python3 sqlite3 unavailable");
        return;
    }

    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::LegacyInUse
    );
    assert!(aida.join("cache.db").is_file());
    assert_eq!(marker(&cache_dir_in(&aida).join("cache.db")), "legacy-row");

    drop(child.stdin.take());
    child.wait().unwrap();
    assert_eq!(
        migrate_legacy_cache_layout(&aida),
        CacheLayoutMigration::Moved
    );
    assert!(!aida.join("cache.db").exists());
}

/// A worktree (or seat clone) an older binary linked file by file gets one
/// directory link to the main checkout's cache directory, and main is migrated
/// first so the link never points at an empty directory.
#[cfg(unix)]
#[test]
fn legacy_worktree_file_links_become_one_directory_link() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let main_aida = legacy_primary(&main);
    let wt_aida = aida_dir(&tmp.path().join("wt"));
    for name in ["cache.db", "cache.db-shm", "cache.db-wal"] {
        std::os::unix::fs::symlink(main_aida.join(name), wt_aida.join(name)).unwrap();
    }

    assert_eq!(
        migrate_legacy_cache_layout(&wt_aida),
        CacheLayoutMigration::RelinkedWorktree
    );
    let link = cache_dir_in(&wt_aida);
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_link(&link).unwrap(), cache_dir_in(&main_aida));
    for name in ["cache.db", "cache.db-shm", "cache.db-wal"] {
        assert!(
            std::fs::symlink_metadata(wt_aida.join(name)).is_err(),
            "{name}"
        );
    }
    assert_eq!(marker(&link.join("cache.db")), "legacy-row");
    assert!(!main_aida.join("cache.db").exists(), "main migrated too");

    assert_eq!(
        migrate_legacy_cache_layout(&wt_aida),
        CacheLayoutMigration::NothingToDo
    );
}

/// Racing migrators publish exactly one database and leave no temp copies.
#[test]
fn concurrent_migrators_publish_one_database() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());
    std::thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| migrate_legacy_cache_layout(&aida));
        }
    });
    migrate_legacy_cache_layout(&aida);
    assert_eq!(marker(&cache_dir_in(&aida).join("cache.db")), "legacy-row");
    assert!(!aida.join("cache.db").exists());
    let leftovers: Vec<_> = std::fs::read_dir(&aida)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".cache-db-migrate"))
        .collect();
    assert!(leftovers.is_empty(), "temp copies left: {leftovers:?}");
}

/// Opening the cache at its project path performs the move, so no opener ever
/// reads the legacy file.
#[test]
fn cache_open_at_the_project_path_migrates_first() {
    let tmp = tempfile::tempdir().unwrap();
    let aida = legacy_primary(tmp.path());
    let path = project_cache_path(tmp.path());
    drop(crate::db::Cache::open(&path).unwrap());
    assert!(path.is_file());
    assert!(!aida.join("cache.db").exists());
    assert_eq!(marker(&path), "legacy-row");
}
