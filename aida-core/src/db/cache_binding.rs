//! Contained-session cache sidecar binding check (BUG-1984).
//!
//! Claude Code's Linux sandbox binds each allow-listed file as an INODE, not a
//! name. When the host deletes or replaces `cache.db`, `cache.db-wal` or
//! `cache.db-shm` while a contained session runs (corrupt-cache removal,
//! quarantine, or an older binary that does not persist the WAL), the session
//! keeps using the old inode, or the two sides end up with a split WAL/shm.
//! The cache is derived data, so nothing is lost, but the split is silent.
//!
//! Two signals expose it:
//! - the path's link count is 0: inside the sandbox the bind still shows the
//!   old inode at the path after the host unlinked it;
//! - (Linux) a file this process holds open for the path has link count 0, or
//!   a different `(dev, ino)` than a fresh `stat` of the path: the file was
//!   replaced behind an open connection.
// trace:BUG-1984 | ai:claude

use std::path::{Path, PathBuf};

/// One sidecar whose binding no longer matches its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarBindingProblem {
    pub path: PathBuf,
    pub detail: String,
}

impl std::fmt::Display for SidecarBindingProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.detail)
    }
}

/// True when this process runs inside an AIDA contained session.
pub fn running_contained() -> bool {
    std::env::var("AIDA_CONTAINED").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

/// The cache file and the SQLite sidecars bound beside it. The directory is
/// canonicalized so the paths compare equal to what `/proc/self/fd` reports
/// when `.aida` is reached through a symlink.
pub fn bound_cache_files(cache_path: &Path) -> Vec<PathBuf> {
    let cache_path = &cache_path
        .parent()
        .and_then(|dir| std::fs::canonicalize(dir).ok())
        .zip(cache_path.file_name())
        .map(|(dir, name)| dir.join(name))
        .unwrap_or_else(|| cache_path.to_path_buf());
    let name = cache_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cache.db".to_string());
    vec![
        cache_path.to_path_buf(),
        cache_path.with_file_name(format!("{name}-wal")),
        cache_path.with_file_name(format!("{name}-shm")),
    ]
}

/// Every bound cache file whose inode no longer matches its path. Empty when
/// the binding is healthy, or on platforms without inode metadata.
pub fn sidecar_binding_problems(cache_path: &Path) -> Vec<SidecarBindingProblem> {
    let mut problems = Vec::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let open = open_files_by_path();
        for path in bound_cache_files(cache_path) {
            let at_path = std::fs::metadata(&path).ok();
            if at_path.as_ref().is_some_and(|m| m.nlink() == 0) {
                problems.push(SidecarBindingProblem {
                    path: path.clone(),
                    detail: "link count is 0 (the host deleted or replaced it)".to_string(),
                });
                continue;
            }
            let held = open.iter().filter(|(p, _)| *p == path).map(|(_, m)| m);
            for held in held {
                let detail = if held.nlink() == 0 {
                    "an open handle points at a deleted file".to_string()
                } else if let Some(m) = &at_path {
                    if (m.dev(), m.ino()) == (held.dev(), held.ino()) {
                        continue;
                    }
                    format!(
                        "an open handle points at inode {} but the path now names inode {}",
                        held.ino(),
                        m.ino()
                    )
                } else {
                    "an open handle points at a file the path no longer names".to_string()
                };
                problems.push(SidecarBindingProblem {
                    path: path.clone(),
                    detail,
                });
                break;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = cache_path;
    problems
}

/// Files this process holds open, keyed by the path they were opened at
/// (with the kernel's ` (deleted)` suffix stripped). Linux only; elsewhere
/// only the path link-count signal applies.
#[cfg(target_os = "linux")]
fn open_files_by_path() -> Vec<(PathBuf, std::fs::Metadata)> {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::read_link(entry.path()).ok()?;
            let target = target.to_string_lossy();
            let path = PathBuf::from(target.strip_suffix(" (deleted)").unwrap_or(&target));
            // `metadata` follows the fd's magic link to the inode itself, so
            // a deleted file still answers.
            Some((path, std::fs::metadata(entry.path()).ok()?))
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_files_by_path() -> Vec<(PathBuf, std::fs::Metadata)> {
    Vec::new()
}

/// The error a contained-session cache open fails with.
pub fn binding_error(problems: &[SidecarBindingProblem]) -> anyhow::Error {
    let list = problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    anyhow::anyhow!(
        "the cache files bound into this contained session no longer match the host's: {list}. \
         The session would read or write a split cache. Restart the contained session to \
         re-bind them."
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn open_cache_with_wal(dir: &Path) -> (PathBuf, crate::db::Cache) {
        let path = dir.join("cache.db");
        let cache = crate::db::Cache::open(&path).unwrap();
        (path, cache)
    }

    #[test]
    fn healthy_cache_reports_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _cache) = open_cache_with_wal(dir.path());
        assert_eq!(sidecar_binding_problems(&path), Vec::new());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wal_unlinked_and_recreated_behind_an_open_cache_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _cache) = open_cache_with_wal(dir.path());
        let wal = dir.path().join("cache.db-wal");
        assert!(wal.exists(), "fixture must have a live WAL");
        std::fs::remove_file(&wal).unwrap();
        std::fs::write(&wal, b"").unwrap();
        let problems = sidecar_binding_problems(&path);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, wal);
        assert!(problems[0].detail.contains("deleted"), "{problems:?}");
        let message = binding_error(&problems).to_string();
        assert!(
            message.contains("cache.db-wal") && message.contains("Restart the contained session")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shm_unlinked_without_replacement_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _cache) = open_cache_with_wal(dir.path());
        let shm = dir.path().join("cache.db-shm");
        assert!(shm.exists(), "fixture must have a live shm");
        std::fs::remove_file(&shm).unwrap();
        let problems = sidecar_binding_problems(&path);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].path, shm);
        assert!(problems[0].detail.contains("deleted"), "{problems:?}");
    }

    /// The open itself fails in a contained session, naming the file and the
    /// recovery; the same split is ignored outside one.
    #[cfg(target_os = "linux")]
    #[test]
    fn contained_open_fails_on_a_split_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let (path, _held) = open_cache_with_wal(dir.path());
        std::fs::remove_file(dir.path().join("cache.db-shm")).unwrap();
        {
            let _env = crate::test_env::EnvVarGuard::unset("AIDA_CONTAINED");
            crate::db::Cache::open(&path).expect("uncontained open is unaffected");
        }
        let _env = crate::test_env::EnvVarGuard::set("AIDA_CONTAINED", "1");
        let err = crate::db::Cache::open(&path)
            .err()
            .expect("contained open must fail");
        let message = format!("{err:#}");
        assert!(message.contains("cache.db-shm"), "{message}");
        assert!(
            message.contains("Restart the contained session"),
            "{message}"
        );
    }

    #[test]
    fn contained_flag_reads_the_launcher_env() {
        {
            let _env = crate::test_env::EnvVarGuard::set("AIDA_CONTAINED", "1");
            assert!(running_contained());
        }
        let _env = crate::test_env::EnvVarGuard::unset("AIDA_CONTAINED");
        assert!(!running_contained());
    }
}
