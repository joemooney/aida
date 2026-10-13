//! Re-bind a long-lived cache handle whose files were replaced on disk.
//!
//! A process that keeps a `Cache` open (the web server, the TUI, any
//! session-lifetime holder) holds file handles to `cache.db`, `cache.db-wal`
//! and `cache.db-shm`. When another process deletes or replaces one of those
//! files by name (corrupt-cache removal, quarantine, an older binary that
//! does not persist the WAL), the holder's handles keep pointing at the old
//! inode while everyone else uses the new one: the WAL/shm is silently split.
//! Claude Code's Linux sandbox makes this worse, because it binds each
//! allow-listed file by inode and detaches the bind when the host replaces it.
//!
//! Only a process that already held the old handles can be split, so the
//! check runs in the holder, against its own handles: the identity
//! `(dev, ino)` of each file as it was when the connection bound it (the
//! handles cannot change inode, so this is what `fstat` on them reads)
//! versus what a fresh `stat` of the path returns now. A mismatch, or a path
//! that no longer exists, means the handle points at a deleted or replaced
//! file. The cache is derived data, so the holder re-opens it rather than
//! refusing anything.
// trace:BUG-1984 | ai:claude

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often a holder re-checks its bound cache files during normal use.
pub(crate) const BINDING_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// One of the files SQLite binds for a cache database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheFile {
    Db,
    Wal,
    Shm,
}

impl CacheFile {
    const ALL: [CacheFile; 3] = [CacheFile::Db, CacheFile::Wal, CacheFile::Shm];

    /// The path of this file for the cache database at `cache_path`.
    pub(crate) fn path_for(self, cache_path: &Path) -> PathBuf {
        let suffix = match self {
            CacheFile::Db => return cache_path.to_path_buf(),
            CacheFile::Wal => "-wal",
            CacheFile::Shm => "-shm",
        };
        let mut name = cache_path.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        cache_path.with_file_name(name)
    }
}

/// `(dev, ino)` of whatever the path names now; `None` when it is absent (or
/// on platforms without inode metadata, where the check never fires).
fn identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[derive(Debug)]
struct BoundFile {
    path: PathBuf,
    bound: Option<(u64, u64)>,
}

/// The identities a cache connection bound, plus the rate limit for checking
/// them during normal use.
#[derive(Debug)]
pub(crate) struct CacheBinding {
    files: Vec<BoundFile>,
    next_check: Instant,
}

impl CacheBinding {
    /// Record what each cache file's path names right after the connection
    /// opened it.
    pub(crate) fn capture(cache_path: &Path) -> Self {
        Self {
            files: CacheFile::ALL
                .iter()
                .map(|file| {
                    let path = file.path_for(cache_path);
                    BoundFile {
                        bound: identity(&path),
                        path,
                    }
                })
                .collect(),
            next_check: Instant::now() + BINDING_CHECK_INTERVAL,
        }
    }

    /// True (and the next check is scheduled) when a rate-limited check is due.
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        if now < self.next_check {
            return false;
        }
        self.next_check = now + BINDING_CHECK_INTERVAL;
        true
    }

    /// Make the next `due` call fire (retry after a failed re-open; tests).
    pub(crate) fn expire(&mut self) {
        self.next_check = Instant::now();
    }

    /// Why the bound handles no longer match the paths, or `None` when they
    /// still do. A sidecar SQLite had not created yet at capture is adopted
    /// the first time it is seen.
    pub(crate) fn stale_reason(&mut self) -> Option<String> {
        let mut reasons = Vec::new();
        for file in &mut self.files {
            let now = identity(&file.path);
            let name = file
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match (file.bound, now) {
                (None, now) => file.bound = now,
                (Some(_), None) => reasons.push(format!("{name} was deleted")),
                (Some(bound), Some(now)) if bound != now => {
                    reasons.push(format!("{name} was replaced"))
                }
                _ => {}
            }
        }
        (!reasons.is_empty()).then(|| reasons.join(", "))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::db::Cache;

    /// Replace `path` the way a host-side writer does: write a new file
    /// beside it and rename it over the name, so the old inode loses its
    /// last link while the holder still has it open.
    fn rename_over(path: &Path, contents: &[u8]) {
        let mut name = path.file_name().unwrap().to_os_string();
        name.push(".replacement");
        let tmp = path.with_file_name(name);
        std::fs::write(&tmp, contents).unwrap();
        std::fs::rename(&tmp, path).unwrap();
    }

    /// A healthy cache at `dir/cache.db` whose recorded head is `marker`.
    fn cache_with_marker(dir: &Path, marker: &str) -> (PathBuf, Cache) {
        let path = dir.join("cache.db");
        let cache = Cache::open(&path).unwrap();
        cache.set_source_head_sha(marker).unwrap();
        (path, cache)
    }

    /// A closed, checkpointed cache database whose recorded head is `marker`.
    fn donor_db(marker: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), marker);
        drop(cache);
        (dir, path)
    }

    #[test]
    fn sidecar_paths_are_selected_by_name() {
        let db = Path::new("/x/.aida/cache.db");
        assert_eq!(CacheFile::Db.path_for(db), db);
        assert_eq!(
            CacheFile::Wal.path_for(db),
            Path::new("/x/.aida/cache.db-wal")
        );
        assert_eq!(
            CacheFile::Shm.path_for(db),
            Path::new("/x/.aida/cache.db-shm")
        );
    }

    #[test]
    fn an_untouched_cache_does_not_rebind() {
        let dir = tempfile::tempdir().unwrap();
        let (_path, cache) = cache_with_marker(dir.path(), "holder");
        assert_eq!(cache.rebind_if_replaced().unwrap(), None);
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("holder"));
    }

    /// The primary topology: a holder has the cache open, then another
    /// process replaces cache.db, -wal and -shm by rename. The holder's
    /// handles now point at unlinked inodes; its check notices, re-opens,
    /// and from then on it reads what the path names.
    #[test]
    fn holder_rebinds_after_all_cache_files_are_renamed_over() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let wal = CacheFile::Wal.path_for(&path);
        let shm = CacheFile::Shm.path_for(&path);
        assert!(wal.exists() && shm.exists(), "fixture binds both sidecars");

        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&wal, b"");
        rename_over(&shm, b"");
        assert_eq!(
            cache.source_head_sha().unwrap().as_deref(),
            Some("holder"),
            "before the check the holder still reads its unlinked files"
        );

        let reason = cache.rebind_if_replaced().unwrap().expect("rebinds");
        for name in ["cache.db was replaced", "cache.db-wal", "cache.db-shm"] {
            assert!(reason.contains(name), "{reason}");
        }
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("host"));
        assert_eq!(cache.rebind_if_replaced().unwrap(), None, "bound again");
    }

    /// A sidecar unlinked without a replacement is also a stale binding.
    #[test]
    fn holder_rebinds_after_its_shm_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        std::fs::remove_file(CacheFile::Shm.path_for(&path)).unwrap();
        let reason = cache.rebind_if_replaced().unwrap().expect("rebinds");
        assert_eq!(reason, "cache.db-shm was deleted");
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("holder"));
        assert!(CacheFile::Shm.path_for(&path).exists(), "re-created");
        assert_eq!(cache.rebind_if_replaced().unwrap(), None);
    }

    /// Normal cache use runs the check, rate-limited: a replace is not seen
    /// until the interval has passed, then the next read follows the path.
    #[test]
    fn normal_use_rebinds_once_the_check_interval_passes() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&CacheFile::Wal.path_for(&path), b"");
        rename_over(&CacheFile::Shm.path_for(&path), b"");

        assert_eq!(
            cache.source_head_sha().unwrap().as_deref(),
            Some("holder"),
            "within the interval the read does not re-check"
        );
        cache.expire_binding_check();
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("host"));
    }

    #[test]
    fn the_rate_limit_fires_once_per_interval() {
        let dir = tempfile::tempdir().unwrap();
        let mut binding = CacheBinding::capture(&dir.path().join("cache.db"));
        let start = Instant::now();
        assert!(!binding.due(start));
        assert!(binding.due(start + BINDING_CHECK_INTERVAL));
        assert!(!binding.due(start + BINDING_CHECK_INTERVAL));
        binding.expire();
        assert!(binding.due(Instant::now()));
    }

    /// The contained-session topology (Linux): the holder runs in its own
    /// user + mount namespace with each cache file bind-mounted onto itself,
    /// as Claude Code's sandbox does. The host then replaces the files; the
    /// kernel detaches the binds, so inside the namespace the path names the
    /// host's new inode while the holder's handles still name the old one.
    /// Skips cleanly when unprivileged user namespaces are unavailable.
    #[cfg(target_os = "linux")]
    #[test]
    fn holder_in_a_mount_namespace_rebinds_after_the_host_replaces_its_files() {
        use std::process::{Command, Stdio};

        let userns_ok = Command::new("unshare")
            .args(["-rm", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !userns_ok {
            eprintln!("skipping: unprivileged user + mount namespaces are unavailable");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let files: Vec<PathBuf> = CacheFile::ALL.iter().map(|f| f.path_for(&path)).collect();
        // Keep the sidecars around for the binds after the host handle closes.
        drop(cache);
        for file in &files {
            if !file.exists() {
                std::fs::write(file, b"").unwrap();
            }
        }

        let binds = files
            .iter()
            .map(|f| format!("mount --bind '{0}' '{0}' && ", f.display()))
            .collect::<String>();
        let exe = std::env::current_exe().unwrap();
        let script = format!(
            "{binds}exec '{}' --exact db::cache_binding::tests::namespace_holder_child \
             --ignored --nocapture --test-threads=1",
            exe.display()
        );
        let mut child = Command::new("unshare")
            .args(["-rm", "sh", "-c", &script])
            .env("AIDA_BUG_1984_HOLDER_DIR", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let ready = dir.path().join("holder-ready");
        let replaced = dir.path().join("host-replaced");
        let result = dir.path().join("holder-result");
        let wait_for = |marker: &Path, child: &mut std::process::Child| {
            let deadline = Instant::now() + Duration::from_secs(300);
            while !marker.exists() {
                if let Ok(Some(status)) = child.try_wait() {
                    panic!("namespace holder exited early: {status}");
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {marker:?}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait_for(&ready, &mut child);

        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&files[1], b"");
        rename_over(&files[2], b"");
        std::fs::write(&replaced, b"").unwrap();

        wait_for(&result, &mut child);
        let output = child.wait_with_output().unwrap();
        let report = std::fs::read_to_string(&result).unwrap();
        assert!(
            output.status.success(),
            "holder failed: {report}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(report.starts_with("rebound: "), "{report}");
        assert!(report.contains("cache.db was replaced"), "{report}");
        assert!(report.ends_with("then read: host"), "{report}");
    }

    /// The holder half of the namespace test; runs only when re-executed by
    /// it inside the namespace.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "run by holder_in_a_mount_namespace_rebinds_after_the_host_replaces_its_files"]
    fn namespace_holder_child() {
        let Some(dir) = std::env::var_os("AIDA_BUG_1984_HOLDER_DIR").map(PathBuf::from) else {
            return;
        };
        let cache = Cache::open(dir.join("cache.db")).unwrap();
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("holder"));
        std::fs::write(dir.join("holder-ready"), b"").unwrap();
        let deadline = Instant::now() + Duration::from_secs(300);
        while !dir.join("host-replaced").exists() {
            assert!(Instant::now() < deadline, "host never replaced the files");
            std::thread::sleep(Duration::from_millis(50));
        }
        let report = match cache.rebind_if_replaced() {
            Ok(Some(reason)) => format!(
                "rebound: {reason}; then read: {}",
                cache.source_head_sha().ok().flatten().unwrap_or_default()
            ),
            Ok(None) => "not rebound".to_string(),
            Err(err) => format!("error: {err:#}"),
        };
        std::fs::write(dir.join("holder-result"), &report).unwrap();
    }
}
