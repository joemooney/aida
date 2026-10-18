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

use std::collections::HashSet;
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

/// Open descriptors in `/proc/self/fd` (excluding `excluded`) whose `readlink`
/// target matches `path` (including when the kernel suffixes `" (deleted)"`),
/// paired with `stat("/proc/self/fd/N")`'s `(dev, ino)`. Reading metadata from
/// `/proc/self/fd/N` inspects the open file without opening a new handle, so
/// SQLite's POSIX advisory locks are never dropped.
// trace:TASK-1739 | ai:codex
#[cfg(target_os = "linux")]
fn matching_fds(path: &Path, excluded: &HashSet<u32>) -> Vec<(u32, (u64, u64))> {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return Vec::new();
    };
    let canonical = std::fs::canonicalize(path).ok();
    let parent_canonical = path
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| path.file_name().map(|n| p.join(n)));
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let Ok(fd_num) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if excluded.contains(&fd_num) {
            continue;
        }
        let Ok(link) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let link_str = link.to_string_lossy();
        let clean = Path::new(link_str.strip_suffix(" (deleted)").unwrap_or(&link_str));
        let same_target = clean == path
            || canonical.as_deref() == Some(clean)
            || parent_canonical.as_deref() == Some(clean);
        if !same_target {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(entry.path()) {
            matches.push((fd_num, (meta.dev(), meta.ino())));
        }
    }
    matches
}

/// `(dev, ino)` of SQLite's open descriptor for `path` from `/proc/self/fd`,
/// or `None` on non-Linux or when no descriptor is open for `path`.
// trace:TASK-1739 | ai:codex
fn fd_identity(path: &Path, excluded: &HashSet<u32>) -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    {
        let fds = matching_fds(path, excluded);
        let now = identity(path);
        if let Some((_, mismatch)) = fds.iter().copied().find(|(_, id)| Some(*id) != now) {
            return Some(mismatch);
        }
        fds.first().map(|(_, id)| *id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, excluded);
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
    pinned: bool,
    excluded_fds: HashSet<u32>,
}

impl CacheBinding {
    /// Descriptor numbers in `/proc/self/fd` already open on `cache_path` and
    /// its sidecars before a new connection is opened, so the new handle's
    /// binding inspects only its own connection's descriptors.
    // trace:TASK-1739 | ai:codex
    pub(crate) fn open_fds_for(cache_path: &Path) -> HashSet<u32> {
        #[cfg(target_os = "linux")]
        {
            let empty = HashSet::new();
            CacheFile::ALL
                .iter()
                .flat_map(|file| matching_fds(&file.path_for(cache_path), &empty))
                .map(|(fd, _)| fd)
                .collect()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = cache_path;
            HashSet::new()
        }
    }

    /// Record what each cache file's open descriptor (or path on non-Linux)
    /// names right after the connection opened it.
    pub(crate) fn capture(cache_path: &Path) -> Self {
        Self::capture_with_excluded(cache_path, HashSet::new())
    }

    // trace:TASK-1739 | ai:codex
    pub(crate) fn capture_with_excluded(cache_path: &Path, excluded_fds: HashSet<u32>) -> Self {
        Self {
            files: CacheFile::ALL
                .iter()
                .map(|file| {
                    let path = file.path_for(cache_path);
                    BoundFile {
                        bound: fd_identity(&path, &excluded_fds).or_else(|| identity(&path)),
                        path,
                    }
                })
                .collect(),
            next_check: Instant::now() + BINDING_CHECK_INTERVAL,
            pinned: false,
            excluded_fds,
        }
    }

    /// A handle pinned to a single read transaction (`Cache::read_snapshot`):
    /// never runs the identity check or swaps out its connection.
    // trace:TASK-1739 | ai:codex
    pub(crate) fn pinned() -> Self {
        Self {
            files: Vec::new(),
            next_check: Instant::now() + BINDING_CHECK_INTERVAL,
            pinned: true,
            excluded_fds: HashSet::new(),
        }
    }

    /// True (and the next check is scheduled) when a rate-limited check is due.
    pub(crate) fn due(&mut self, now: Instant) -> bool {
        if self.pinned || now < self.next_check {
            return false;
        }
        self.next_check = now + BINDING_CHECK_INTERVAL;
        true
    }

    /// Make the next `due` call fire (tests).
    pub(crate) fn expire(&mut self) {
        self.next_check = Instant::now();
    }

    /// Back off for one full check interval after a failed re-open so callers
    /// do not retry the broken open on every `conn()` access.
    // trace:TASK-1739 | ai:codex
    pub(crate) fn backoff(&mut self) {
        self.next_check = Instant::now() + BINDING_CHECK_INTERVAL;
    }

    /// Why the bound handles no longer match the paths, or `None` when they
    /// still do. A sidecar SQLite had not created yet at capture is adopted
    /// the first time it is seen, cross-checked against `/proc/self/fd` on
    /// Linux so a replacement before that first check is still caught.
    // trace:BUG-1984 trace:TASK-1739 | ai:claude
    pub(crate) fn stale_reason(&mut self) -> Option<String> {
        if self.pinned {
            return None;
        }
        let mut reasons = Vec::new();
        for file in &mut self.files {
            let now = identity(&file.path);
            let fd_now = fd_identity(&file.path, &self.excluded_fds);
            if file.bound.is_none() {
                file.bound = fd_now;
            }
            let name = file
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            match (file.bound, now) {
                (None, now) => file.bound = now,
                (Some(_), None) => reasons.push(format!("{name} was deleted")),
                (Some(bound), Some(now)) if bound != now || fd_now.is_some_and(|fd| fd != now) => {
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

    /// A1 (TASK-1739): if a host replacement happens inside the open bind
    /// window (after SQLite opens its fds, before `CacheBinding::capture`
    /// records `stat(path)`), or before a sidecar is first adopted at a later
    /// check, `/proc/self/fd` still carries SQLite's open inode and detects
    /// the split.
    // trace:TASK-1739 | ai:codex
    #[cfg(target_os = "linux")]
    #[test]
    fn holder_rebinds_when_replaced_inside_bind_window_or_before_sidecar_adoption() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let wal = CacheFile::Wal.path_for(&path);
        let shm = CacheFile::Shm.path_for(&path);

        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&wal, b"");
        rename_over(&shm, b"");

        // Simulate `CacheBinding::capture` running AFTER the host already
        // replaced the files during `open_at`, and also clear the sidecars'
        // bound state to exercise lazy sidecar adoption after replacement.
        let mut recaptured = CacheBinding::capture(&path);
        for file in &mut recaptured.files[1..] {
            file.bound = None;
        }
        let reason = recaptured
            .stale_reason()
            .expect("detects split via /proc/self/fd");
        for name in [
            "cache.db was replaced",
            "cache.db-wal was replaced",
            "cache.db-shm was replaced",
        ] {
            assert!(reason.contains(name), "{reason}");
        }

        let rebound = cache.rebind_if_replaced().unwrap().expect("rebinds");
        assert!(rebound.contains("cache.db was replaced"), "{rebound}");
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("host"));
    }

    /// Item 2 (TASK-1739): `rebind_if_replaced` disables checkpoint-on-close
    /// (`SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE`) before closing the stale connection,
    /// so closing the last handle to the old database never folds its WAL into
    /// the old database inode or unlinks the replacement WAL by name.
    // trace:TASK-1739 | ai:codex
    #[test]
    fn rebind_does_not_checkpoint_stale_wal_into_old_db_on_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.db");
        let cache = Cache::open(&path).unwrap();
        cache.set_source_head_sha("initial").unwrap();
        drop(cache);

        let cache = Cache::open(&path).unwrap();
        let old_db_link = dir.path().join("old-inode.db");
        std::fs::hard_link(&path, &old_db_link).unwrap();

        let marker = "uncheckpointed-stale-wal-marker-1739";
        cache.set_source_head_sha(marker).unwrap();
        assert!(
            !std::fs::read(&old_db_link)
                .unwrap()
                .windows(marker.len())
                .any(|w| w == marker.as_bytes()),
            "marker must sit in the WAL before close, not in the main db pages"
        );

        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&CacheFile::Wal.path_for(&path), b"replacement-wal");
        rename_over(&CacheFile::Shm.path_for(&path), b"");

        cache.rebind_if_replaced().unwrap().expect("rebinds");
        assert!(
            !std::fs::read(&old_db_link)
                .unwrap()
                .windows(marker.len())
                .any(|w| w == marker.as_bytes()),
            "stale connection must not checkpoint its WAL into the old db inode on close"
        );
        assert!(
            CacheFile::Wal.path_for(&path).exists(),
            "closing the stale connection must not unlink the replacement WAL by name"
        );
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("host"));
    }

    /// Item 3 (TASK-1739): a failed reopen backs off for one check interval
    /// instead of expiring the timer and retrying on every `conn()` call.
    // trace:TASK-1739 | ai:codex
    #[test]
    fn failed_reopen_backs_off_instead_of_retrying_on_every_use() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        cache.expire_binding_check();
        assert!(cache.rebind_if_replaced().is_err());

        let now = Instant::now();
        assert!(
            !cache.binding.lock().unwrap().due(now),
            "failed reopen must back off instead of firing on the very next use"
        );
        assert!(
            cache
                .binding
                .lock()
                .unwrap()
                .due(now + BINDING_CHECK_INTERVAL),
            "re-checks once the backoff interval has elapsed"
        );
    }

    /// Item 4 (TASK-1739): `read_snapshot` handles are pinned to a single read
    /// transaction and must skip the identity check even if they outlive the
    /// 60s check interval.
    // trace:TASK-1739 | ai:codex
    #[test]
    fn read_snapshot_skips_the_identity_check_even_after_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let snap = cache.read_snapshot().unwrap();
        assert_eq!(snap.source_head_sha().unwrap().as_deref(), Some("holder"));

        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&CacheFile::Wal.path_for(&path), b"");
        rename_over(&CacheFile::Shm.path_for(&path), b"");

        snap.expire_binding_check();
        assert_eq!(snap.rebind_if_replaced().unwrap(), None);
        assert_eq!(
            snap.source_head_sha().unwrap().as_deref(),
            Some("holder"),
            "snapshot handle stays pinned to its transaction's files"
        );

        assert!(cache.rebind_if_replaced().unwrap().is_some());
        assert_eq!(cache.source_head_sha().unwrap().as_deref(), Some("host"));
        assert_eq!(
            cache.rebind_if_replaced().unwrap(),
            None,
            "fresh handle ignores the snapshot handle's still-open old descriptors"
        );
    }

    /// Item 5 (TASK-1739): while a TUI owns the terminal, the rebind note is
    /// routed into `cache_read_note()` instead of writing to raw stderr.
    // trace:TASK-1739 | ai:codex
    #[test]
    fn rebind_note_routes_to_cache_read_note_while_tui_owns_terminal() {
        use crate::db::cache_refresh::{cache_read_note, set_tui_owns_terminal, CacheReadScope};

        let _scope = CacheReadScope::new();
        let prev = set_tui_owns_terminal(true);
        let dir = tempfile::tempdir().unwrap();
        let (path, cache) = cache_with_marker(dir.path(), "holder");
        let (_donor_dir, donor) = donor_db("host");
        std::fs::rename(&donor, &path).unwrap();
        rename_over(&CacheFile::Wal.path_for(&path), b"");
        rename_over(&CacheFile::Shm.path_for(&path), b"");

        let reason = cache.rebind_if_replaced().unwrap();
        set_tui_owns_terminal(prev);

        assert!(reason.is_some());
        let note = cache_read_note().expect("rebind note recorded for TUI status line");
        assert!(note.contains("changed on disk"), "{note}");
        assert!(note.contains("re-opened it"), "{note}");
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
