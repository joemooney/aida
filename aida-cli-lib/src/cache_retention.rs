//! Explicit removal of obsolete derived cache files.
// trace:BUG-2598 | ai:codex
use anyhow::Result;
use fs2::FileExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) const CATEGORY: &str = "cache-retention";
const RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);

fn obsolete_history(name: &str) -> Option<String> {
    let (version, suffix) = name.strip_prefix("history-v")?.split_once(".db")?;
    if !["", "-wal", "-shm"].contains(&suffix) {
        return None;
    }
    let (schema, decoder) = version.split_once('-')?;
    let version = (schema.parse::<u32>().ok()?, decoder.parse::<u32>().ok()?);
    let current = (
        crate::history_cache::HISTORY_SCHEMA_VERSION,
        crate::history_cache::HISTORY_DECODER_VERSION,
    );
    let base = format!("history-v{}-{}.db", version.0, version.1);
    (version < current && name == format!("{base}{suffix}")).then_some(base)
}

fn candidates(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in [root.join(".aida"), root.join(".aida/cache-db")] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = path.symlink_metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let old_backup = name.starts_with("cache.db.malformed-")
                && meta
                    .modified()
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > RETENTION);
            if old_backup || obsolete_history(name).is_some() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

pub(crate) fn scan(root: &Path) -> Vec<crate::DoctorFinding> {
    candidates(root)
        .into_iter()
        .map(|path| crate::DoctorFinding {
            category: CATEGORY.into(),
            id: path.display().to_string(),
            summary: format!("obsolete cache file {}", path.display()),
            action: "run `aida doctor cache-retention --fix` to remove obsolete cache files".into(),
            safe_heal: false,
        })
        .collect()
}

pub(crate) fn run(root: &Path, fix: bool) -> Result<()> {
    for path in candidates(root) {
        if !fix {
            println!(
                "obsolete cache file {}; run `aida doctor cache-retention --fix`",
                path.display()
            );
            continue;
        }
        // Match the old indexer's lock, retain its inode, and never touch a
        // currently running indexer. Backups are immutable quarantine outputs.
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let _lock = if let Some(base) = obsolete_history(name) {
            let lock_path = path.with_file_name(format!("{base}.lock"));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = options.open(lock_path)?;
            if !file.metadata()?.is_file() || file.try_lock_exclusive().is_err() {
                println!("skipped busy cache file {}", path.display());
                continue;
            }
            Some(file)
        } else {
            None
        };
        // Revalidate age and type after taking the lock.
        if candidates(root).contains(&path) {
            std::fs::remove_file(&path)?;
            println!("removed obsolete cache file {}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug2598_retention_reports_then_explicitly_removes_only_obsolete_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".aida/cache-db");
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "history-v6-4.db",
            "history-v6-4.db-wal",
            "history-v6-6.db",
            "history-v99-1.db",
            "cache.db",
            "cache.db.malformed-recent",
            "unrelated",
        ] {
            std::fs::write(dir.join(name), b"cache").unwrap();
        }
        let old = dir.join("cache.db.malformed-old");
        let file = std::fs::File::create(&old).unwrap();
        file.set_modified(std::time::SystemTime::now() - RETENTION - Duration::from_secs(10))
            .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, dir.join("cache.db.malformed-link")).unwrap();
        assert_eq!(scan(root.path()).len(), 3);
        run(root.path(), false).unwrap();
        assert!(old.exists());
        let lock = std::fs::File::create(dir.join("history-v6-4.db.lock")).unwrap();
        lock.lock_exclusive().unwrap();
        run(root.path(), true).unwrap();
        assert!(dir.join("history-v6-4.db").exists());
        assert!(!old.exists());
        drop(lock);
        run(root.path(), true).unwrap();
        assert!(!dir.join("history-v6-4.db").exists());
        assert!(dir.join("history-v6-6.db").exists());
        assert!(dir.join("history-v99-1.db").exists());
        assert!(dir.join("cache.db.malformed-recent").exists());
        assert!(dir.join("history-v6-4.db.lock").exists());
    }
}
