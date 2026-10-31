//! BUG-2169: recognise a contained seat's private clone from plain files,
//! without running git in it. A seat clone's `.git/config`, `hooks/` and
//! `info/` are seat-writable, so host git there would run seat-planted
//! configuration outside the sandbox. `aida-cli-lib`'s `seat_clone` module is
//! the only code that runs git in one; host git builders such as
//! [`crate::git_ops::host_git_command`] use [`seat_clone_root`] to refuse.
//!
//! Every read here is FIFO-safe: a seat can plant a FIFO or a symlink where a
//! marker or alternates file is expected, so a path is opened without
//! following a symlink or blocking, and read only when the opened handle is a
//! regular file, never past a size cap.
//! trace:BUG-2169 | ai:claude

use std::path::{Path, PathBuf};

/// The host-owned marker the seat-clone creator writes, relative to the
/// clone root.
// trace:BUG-2169 | ai:claude
pub const MARKER_RELATIVE: &str = ".aida/seat-clone.toml";

/// The largest marker or alternates file read; anything bigger is not ours.
const MAX_READ: u64 = 64 * 1024;

/// `path`'s contents when it is a regular file (never through a symlink,
/// never a FIFO or device) no larger than 64 KiB; `None` otherwise.
// trace:BUG-2169 | ai:claude
pub fn read_regular_file(path: &Path) -> Option<String> {
    use std::io::Read;
    // One open, then checks on that handle: a seat can swap a FIFO or a
    // symlink to /dev/zero in between a stat by name and a reopen. O_NOFOLLOW
    // refuses a final symlink, O_NONBLOCK keeps a FIFO open from blocking,
    // and the read stops one byte past the cap whatever fstat said.
    // trace:TASK-2053 | ai:claude
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let file = options.open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_READ {
        return None;
    }
    let mut body = Vec::new();
    file.take(MAX_READ + 1).read_to_end(&mut body).ok()?;
    if body.len() as u64 > MAX_READ {
        return None;
    }
    String::from_utf8(body).ok()
}

/// Whether `path` itself (not a symlink target) is a directory.
fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

/// The directory-name prefix the seat-clone creator gives every clone it
/// places. The seat cannot rename its own root (the sandbox binds it), so the
/// name still identifies a clone whose marker the seat deleted.
// trace:BUG-2169 | ai:claude
pub const CLONE_DIR_PREFIX: &str = "aida-seat-";

/// Main checkouts this process has resolved on the host side. Their
/// `.aida/sessions` leases are the host's own record of every seat clone, so
/// a clone is recognised even after its seat renames or rewires everything
/// inside it.
static HOST_MAINS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Record `main` (a checkout with a real `.git` directory that is not itself a
/// seat clone) as a host main whose clone leases [`seat_clone_root`] honours.
// trace:BUG-2169 | ai:claude
pub fn note_host_main(main: &Path) {
    let Ok(main) = main.canonicalize() else {
        return;
    };
    if !is_real_dir(&main.join(".git")) || clone_by_name_or_marker(&main).is_some() {
        return;
    }
    if let Ok(mut mains) = HOST_MAINS.lock() {
        if !mains.contains(&main) {
            mains.push(main);
        }
    }
}

/// The recorded host mains plus the git root of this process's cwd.
fn host_mains() -> Vec<PathBuf> {
    let mut mains = HOST_MAINS.lock().map(|m| m.clone()).unwrap_or_default();
    if let Ok(cwd) = std::env::current_dir().and_then(|c| c.canonicalize()) {
        if let Some(root) = cwd
            .ancestors()
            .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())
        {
            if is_real_dir(&root.join(".git"))
                && clone_by_name_or_marker(root).is_none()
                && !mains.iter().any(|m| m == root)
            {
                mains.push(root.to_path_buf());
            }
        }
    }
    mains
}

/// Whether `main` is a main checkout THIS host process resolved itself:
/// one recorded by [`note_host_main`] (host `git worktree list` on a path
/// that is not a seat clone) or the git root of this process's cwd. Never
/// derived from a seat clone's marker, which the seat can rewrite.
// trace:BUG-2169 | ai:claude
pub fn is_host_main(main: &Path) -> bool {
    let Ok(main) = main.canonicalize() else {
        return false;
    };
    host_mains().iter().any(|m| *m == main)
}

/// The trees of `main`'s `clone` session leases, read from the host-side
/// `<main>/.aida/sessions/*.toml` (never from a clone). FIFO-safe.
// trace:BUG-2169 | ai:claude
pub fn clone_lease_roots(main: &Path) -> Vec<PathBuf> {
    let sessions = main.join(".aida").join("sessions");
    let Ok(entries) = std::fs::read_dir(&sessions) else {
        return Vec::new();
    };
    // BUG-2192: this runs on every host git call, and `.aida/sessions` holds
    // hundreds of files. Each file's parsed root is memoised per process
    // under its stamp (inode, size, times; see `host_git_cache`), so an
    // unchanged file is not re-read; a new, changed or recently written file
    // is read as before.
    // trace:BUG-2192 | ai:claude
    static MEMO: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, LeaseMemo>>> =
        std::sync::Mutex::new(None);
    let mut memo = MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = memo
        .get_or_insert_with(Default::default)
        .remove(&sessions)
        .unwrap_or_default();
    let mut current = LeaseMemo::new();
    let mut roots = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if !path.extension().is_some_and(|x| x == "toml") {
            continue;
        }
        let stamp = crate::host_git_cache::stamp(&path);
        let root = match (&stamp, previous.get(&path)) {
            (Some(now), Some((then, root))) if now == then => root.clone(),
            _ => read_regular_file(&path).and_then(|body| lease_clone_root(&body)),
        };
        if let Some(stamp) = stamp {
            current.insert(path, (stamp, root.clone()));
        }
        roots.extend(root);
    }
    memo.get_or_insert_with(Default::default)
        .insert(sessions, current);
    roots
        .into_iter()
        .map(|root| root.canonicalize().unwrap_or(root))
        .collect()
}

/// One sessions directory's memo: each lease file's stamp and parsed root.
type LeaseMemo =
    std::collections::HashMap<PathBuf, (crate::host_git_cache::Stamp, Option<PathBuf>)>;

/// The tree a lease body names when it is a `clone` lease with an absolute
/// `worktree_path` (not yet canonicalized).
// trace:BUG-2169 | ai:claude
fn lease_clone_root(body: &str) -> Option<PathBuf> {
    // BUG-2192: most of `.aida/sessions` is not a clone lease. A TOML value
    // that parses to "clone" needs the literal text or an escape (a
    // backslash), so a body with neither can skip the parse without changing
    // what is found.
    // trace:BUG-2192 | ai:claude
    if !(body.contains("clone") || body.contains('\\')) {
        return None;
    }
    let lease = toml::from_str::<toml::Value>(body).ok()?;
    if lease.get("tree_kind").and_then(|v| v.as_str()) != Some("clone") {
        return None;
    }
    let root = PathBuf::from(lease.get("worktree_path")?.as_str()?);
    (root.is_absolute() && root.parent().is_some()).then_some(root)
}

/// The main checkout a writer inside the seat clone at `clone_root` binds its
/// mail source to: the marker's `main`, trusted only when the clone's `.git`
/// is a real directory, its alternates borrow that main's objects, and a
/// `clone` lease in that main names this root (the agreement the CLI's
/// `seat_clone::main_for` requires). `None` when any of it disagrees.
// trace:STORY-1843 | ai:claude
pub fn clone_writer_main(clone_root: &Path) -> Option<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Marker {
        main: PathBuf,
    }
    let clone_root = clone_root.canonicalize().ok()?;
    if !has_real_git_dir(&clone_root) {
        return None;
    }
    let marker: Marker =
        toml::from_str(&read_regular_file(&clone_root.join(MARKER_RELATIVE))?).ok()?;
    let main = marker.main.canonicalize().ok()?;
    if !is_real_dir(&main.join(".git")) || main.starts_with(&clone_root) {
        return None;
    }
    let alternates = read_regular_file(&clone_root.join(".git/objects/info/alternates"))?;
    let first = alternates.lines().map(str::trim).find(|l| !l.is_empty())?;
    if Path::new(first).canonicalize().ok()?
        != main.join(".git").join("objects").canonicalize().ok()?
    {
        return None;
    }
    clone_lease_roots(&main)
        .iter()
        .any(|root| root.canonicalize().is_ok_and(|r| r == clone_root))
        .then_some(main)
}

/// The nearest ancestor of `start` (itself included) named `aida-seat-*` or
/// holding a marker entry of any type.
fn clone_by_name_or_marker(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| {
            let named = dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(CLONE_DIR_PREFIX));
            named || std::fs::symlink_metadata(dir.join(MARKER_RELATIVE)).is_ok()
        })
        .map(Path::to_path_buf)
}

/// The seat clone `start` is inside, if any. A path counts when ANY ancestor
/// of its canonical form, `start` included:
///
/// - is named `aida-seat-*`;
/// - holds a marker entry of any type; or
/// - is the tree of a `clone` lease in a host main's `.aida/sessions`.
///
/// Whether or what `.git` exists there is never consulted, so a seat that
/// deletes, symlinks or rewires `.git`, lays its root out as a gitdir, nests
/// a `sub/.git`, renames its root or deletes its marker is still a clone.
// trace:BUG-2169 | ai:claude
pub fn seat_clone_root(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    if let Some(root) = clone_by_name_or_marker(&start) {
        return Some(root);
    }
    host_mains()
        .iter()
        .flat_map(|main| clone_lease_roots(main))
        .find(|root| start.starts_with(root))
}

/// Whether `clone_root`'s `.git` is a real directory, as the creator made
/// it. A symlink or `gitdir:` file means the seat rewired it.
// trace:BUG-2169 | ai:claude
pub fn has_real_git_dir(clone_root: &Path) -> bool {
    is_real_dir(&clone_root.join(".git"))
}

/// Whether THIS process is proven contained inside the seat clone at
/// `clone_root` AND that clone is its own. The proof rests only on facts a
/// seat cannot forge (BUG-HOME-038 ruling, round 2):
///
/// - read-only main: a write probe under the `.git` of the main the clone
///   names fails with `EROFS` ([`main_git_read_only`]). `EACCES` and `EPERM`
///   prove nothing (a seat can `chmod` a directory it owns, so a forged main
///   in a writable place would pass), and the host sees the real main
///   writable, so it is never "contained";
/// - host-written record: that same main holds a `clone` session lease
///   whose recorded tree canonicalises to this clone's root AND is the same
///   directory (device and inode) as the root this check began with. The
///   lease is found by the clone's PATH ([`clone_lease_roots`]), never by a
///   session id from the environment, which the seat controls. An arbitrary
///   read-only path (a squashfs under `/snap`) holds no such record;
/// - own clone: the clone's `.git` is writable to this process. The seat
///   sandbox binds the whole filesystem read-only and only the seat's own
///   tree read-write, so a sibling seat's clone (readable, and recorded by
///   its own lease) answers the write probe with `EROFS` and is refused, as
///   it is whatever session id the seat claims.
///
/// The marker and alternates ([`clone_writer_main`]) only say where to look;
/// neither is trusted on its own.
// trace:BUG-HOME-038 trace:TASK-B6B1278890774EF993D2EBC327862470-139 | ai:claude
pub fn contained_in_clone(clone_root: &Path) -> bool {
    let Some(identity) = clone_root
        .canonicalize()
        .ok()
        .and_then(|root| dir_identity(&root))
    else {
        return false;
    };
    contained_as(clone_root, identity, &mut git_dir_probe)
}

/// A directory's `(device, inode)`, read without following a final symlink.
/// `None` off Unix, or when `path` is not a real directory.
// trace:BUG-HOME-038 | ai:claude
pub fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// [`contained_in_clone`] for the clone directory `identity` captured when the
/// check began, with `probe` answering the write-probe errno of a `.git`
/// directory (the seam the unit tests inject results through).
// trace:BUG-HOME-038 trace:TASK-B6B1278890774EF993D2EBC327862470-139 | ai:claude
fn contained_as(
    clone_root: &Path,
    identity: (u64, u64),
    probe: &mut dyn FnMut(&Path) -> Option<i32>,
) -> bool {
    let Some(main) = clone_writer_main(clone_root) else {
        return false;
    };
    let Ok(root) = clone_root.canonicalize() else {
        return false;
    };
    let recorded = clone_lease_roots(&main)
        .into_iter()
        .any(|leased| leased == root && dir_identity(&leased) == Some(identity));
    recorded && is_erofs(probe(&main.join(".git"))) && probe(&root.join(".git")).is_none()
}

/// A containment probe injected by [`with_containment_probe`].
type FakeProbe = std::rc::Rc<dyn Fn(&Path) -> Option<i32>>;

thread_local! {
    /// The probe [`with_containment_probe`] injects on this thread.
    static FAKE_PROBE: std::cell::RefCell<Option<FakeProbe>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with every containment write probe on this thread answered by
/// `errno_for` (given the probed `.git` directory; `None`: the write
/// succeeded), uncached. For tests in this and dependent crates, so no real
/// read-only mount is needed; restored afterwards, even on panic.
// trace:BUG-HOME-038 trace:TASK-B6B1278890774EF993D2EBC327862470-139 | ai:claude
#[doc(hidden)]
pub fn with_containment_probe<R>(
    errno_for: impl Fn(&Path) -> Option<i32> + 'static,
    f: impl FnOnce() -> R,
) -> R {
    struct Restore(Option<FakeProbe>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            FAKE_PROBE.with(|p| *p.borrow_mut() = previous);
        }
    }
    let injected: FakeProbe = std::rc::Rc::new(errno_for);
    let _restore = Restore(FAKE_PROBE.with(|p| p.borrow_mut().replace(injected)));
    f()
}

/// The write-probe errno of `git_dir` ([`git_dir_write_probe`]). Probed once
/// per process per directory ([`cached_probe`]): every host git call in a
/// seat clone asks, and each probe would otherwise create and unlink a file.
// trace:BUG-HOME-038 | ai:claude
fn git_dir_probe(git_dir: &Path) -> Option<i32> {
    if let Some(fake) = FAKE_PROBE.with(|p| p.borrow().clone()) {
        return fake(git_dir);
    }
    static PROBED: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, Option<i32>>>,
    > = std::sync::OnceLock::new();
    cached_probe(
        PROBED.get_or_init(Default::default),
        git_dir,
        &mut git_dir_write_probe,
    )
}

/// `cache`'s answer for `key`, computed with `probe` only the first time.
fn cached_probe<T: Copy>(
    cache: &std::sync::Mutex<std::collections::HashMap<PathBuf, T>>,
    key: &Path,
    probe: &mut dyn FnMut(&Path) -> T,
) -> T {
    let mut probed = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *probed
        .entry(key.to_path_buf())
        .or_insert_with(|| probe(key))
}

/// Whether a probe's errno is `EROFS`, the only failure that proves a
/// read-only mount.
fn is_erofs(errno: Option<i32>) -> bool {
    #[cfg(unix)]
    {
        errno == Some(libc::EROFS)
    }
    #[cfg(not(unix))]
    {
        let _ = errno;
        false
    }
}

/// The errno of creating a scratch file directly under `git_dir`; `None`
/// when the create succeeded (the probe is removed at once) or off Unix,
/// where nothing is proven.
// trace:BUG-HOME-038 | ai:claude
pub fn git_dir_write_probe(git_dir: &Path) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let probe = git_dir.join(format!(
            "aida-containment-probe-{}-{nanos}",
            std::process::id()
        ));
        // O_WRONLY | O_CREAT | O_EXCL, never through a symlink.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&probe)
        {
            Ok(_) => {
                let _ = std::fs::remove_file(&probe);
                None
            }
            // An error without an errno is still a failure, never EROFS.
            Err(err) => Some(err.raw_os_error().unwrap_or(-1)),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = git_dir;
        None
    }
}

/// The refusal text for host git aimed at a seat clone.
// trace:BUG-2169 | ai:claude
pub fn host_git_refusal(clone_root: &Path) -> String {
    format!(
        "refusing to run git in {}: it is a contained seat's private clone, and its git \
         configuration and hooks are writable by the seat",
        clone_root.display()
    )
}

#[cfg(test)]
mod bug_2192_lease_scan_tests {
    use super::clone_lease_roots;

    /// The pre-parse filter skips only bodies that cannot hold a "clone"
    /// value: a plain clone lease and one that spells it with a TOML escape
    /// are both found; a worktree lease and an activity file are not.
    // trace:BUG-2192 | ai:claude
    #[test]
    fn lease_scan_filter_keeps_every_clone_lease() {
        let main = tempfile::TempDir::new().unwrap();
        let sessions = main.path().join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let plain = main.path().join("aida-seat-plain");
        let escaped = main.path().join("aida-seat-escaped");
        let write = |name: &str, body: String| std::fs::write(sessions.join(name), body).unwrap();
        // Quote each path through `toml` so a Windows backslash is escaped.
        let lease = |kind: &str, path: &std::path::Path| {
            let path = toml::Value::String(path.display().to_string());
            format!("tree_kind = {kind}\nworktree_path = {path}\n")
        };
        write("a.toml", lease("\"clone\"", &plain));
        write("b.toml", lease("\"\\u0063lone\"", &escaped));
        write(
            "c.toml",
            lease("\"worktree\"", &main.path().join("aida-wt")),
        );
        write("c.activity.toml", "last = 1\n".to_string());

        let mut found = clone_lease_roots(main.path());
        found.sort();
        let mut want = vec![plain, escaped];
        want.sort();
        assert_eq!(found, want);
    }

    /// The per-file memo never hides a change: a memoised worktree lease
    /// rewritten in place as a clone lease, and a new clone lease, are both
    /// found, and a removed one is dropped.
    // trace:BUG-2192 | ai:claude
    #[test]
    fn lease_scan_memo_sees_rewritten_new_and_removed_leases() {
        crate::host_git_cache::ALWAYS_SETTLED.with(|s| s.set(true));
        let main = tempfile::TempDir::new().unwrap();
        let sessions = main.path().join(".aida").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let seat = main.path().join("aida-seat-a");
        let path = toml::Value::String(seat.display().to_string());
        // Same length, so only the inode times tell the rewrite apart.
        let worktree = format!("tree_kind = \"xlone\"\nworktree_path = {path}\n");
        let clone = format!("tree_kind = \"clone\"\nworktree_path = {path}\n");
        std::fs::write(sessions.join("a.toml"), &worktree).unwrap();
        assert!(clone_lease_roots(main.path()).is_empty());
        assert!(clone_lease_roots(main.path()).is_empty(), "memo hit");

        // Past one coarse timestamp tick, so the in-place rewrite moves ctime.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::OpenOptions::new()
            .write(true)
            .open(sessions.join("a.toml"))
            .and_then(|mut f| std::io::Write::write_all(&mut f, clone.as_bytes()))
            .unwrap();
        assert_eq!(clone_lease_roots(main.path()), vec![seat.clone()]);

        let other = main.path().join("aida-seat-b");
        let other_path = toml::Value::String(other.display().to_string());
        std::fs::write(
            sessions.join("b.toml"),
            format!("tree_kind = \"clone\"\nworktree_path = {other_path}\n"),
        )
        .unwrap();
        let mut found = clone_lease_roots(main.path());
        found.sort();
        assert_eq!(found, vec![seat.clone(), other]);

        std::fs::remove_file(sessions.join("b.toml")).unwrap();
        assert_eq!(clone_lease_roots(main.path()), vec![seat]);
        crate::host_git_cache::ALWAYS_SETTLED.with(|s| s.set(false));
    }
}

#[cfg(all(test, unix))]
mod bug_home_038_containment_tests {
    use super::*;

    const SESSION: &str = "c0ffee038100";

    /// A main at `dir/name` and its seat clone at `dir/aida-seat-<name>`,
    /// laid out from plain files the way the creator leaves them: the
    /// clone's real `.git` borrows main's objects, its marker names main, and
    /// (when `leased`) main holds `SESSION`'s `clone` lease naming it.
    /// Returns (main, clone), both canonical.
    fn layout(dir: &Path, name: &str, leased: bool) -> (PathBuf, PathBuf) {
        let main = dir.join(name);
        let clone = dir.join(format!("{CLONE_DIR_PREFIX}{name}"));
        std::fs::create_dir_all(main.join(".git/objects")).unwrap();
        std::fs::create_dir_all(main.join(".aida/sessions")).unwrap();
        let (main, clone) = (main.canonicalize().unwrap(), make_clone(&clone, &main));
        if leased {
            let path = toml::Value::String(clone.display().to_string());
            std::fs::write(
                main.join(format!(".aida/sessions/{SESSION}.toml")),
                format!("tree_kind = \"clone\"\nworktree_path = {path}\n"),
            )
            .unwrap();
        }
        (main, clone)
    }

    /// The clone side of [`layout`] at `clone`, pointing at `main`.
    fn make_clone(clone: &Path, main: &Path) -> PathBuf {
        std::fs::create_dir_all(clone.join(".git/objects/info")).unwrap();
        std::fs::create_dir_all(clone.join(".aida")).unwrap();
        std::fs::write(
            clone.join(".git/objects/info/alternates"),
            format!("{}\n", main.join(".git/objects").display()),
        )
        .unwrap();
        let main_value = toml::Value::String(main.display().to_string());
        std::fs::write(
            clone.join(MARKER_RELATIVE),
            format!("main = {main_value}\n"),
        )
        .unwrap();
        clone.canonicalize().unwrap()
    }

    /// The containment check with the probe answering `errno` for main's
    /// `.git` and success (the seat's own writable tree) for the clone's.
    fn contained_with(clone: &Path, identity: (u64, u64), errno: Option<i32>) -> bool {
        contained_probed(clone, identity, errno, None)
    }

    /// The containment check with the probe answering `main_errno` for any
    /// `.git` outside `clone` and `clone_errno` for the clone's.
    fn contained_probed(
        clone: &Path,
        identity: (u64, u64),
        main_errno: Option<i32>,
        clone_errno: Option<i32>,
    ) -> bool {
        let root = clone.canonicalize().unwrap();
        contained_as(clone, identity, &mut |git_dir| {
            if git_dir.starts_with(&root) {
                clone_errno
            } else {
                main_errno
            }
        })
    }

    /// (a) A forged main in a writable temp dir, its `.git` chmod 0555 and
    /// holding a lease for the clone: the real probe fails with EACCES, which
    /// no longer counts, so the clone is refused.
    #[test]
    fn forged_main_with_0555_git_is_refused() {
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("SKIP (root ignores directory permissions)");
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let (main, clone) = layout(dir.path(), "forged", true);
        let identity = dir_identity(&clone).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let git = main.join(".git");
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o555)).unwrap();
        let errno = git_dir_write_probe(&git);
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(errno, Some(libc::EACCES));
        assert!(!contained_with(&clone, identity, errno));
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o555)).unwrap();
        let real = contained_as(&clone, identity, &mut git_dir_write_probe);
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!real);
    }

    /// (b) A marker pointing at a read-only main (EROFS injected) that holds
    /// no record of this clone is refused.
    #[test]
    fn read_only_main_without_record_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_, clone) = layout(dir.path(), "snap", false);
        let identity = dir_identity(&clone).unwrap();
        assert!(!contained_with(&clone, identity, Some(libc::EROFS)));
    }

    /// (c) EROFS plus a matching lease record and a writable clone is
    /// allowed; a writable main (the host's view) is not, nor is a clone the
    /// process cannot write (a sibling seat's, bound read-only).
    #[test]
    fn read_only_main_with_matching_record_is_allowed() {
        let dir = tempfile::TempDir::new().unwrap();
        let (_, clone) = layout(dir.path(), "real", true);
        let identity = dir_identity(&clone).unwrap();
        assert!(contained_with(&clone, identity, Some(libc::EROFS)));
        assert!(!contained_with(&clone, identity, None));
        assert!(!contained_with(&clone, identity, Some(libc::EPERM)));
        let erofs = Some(libc::EROFS);
        assert!(!contained_probed(&clone, identity, erofs, erofs));
        assert!(!contained_probed(
            &clone,
            identity,
            erofs,
            Some(libc::EACCES)
        ));
    }

    /// TASK-139: the own-clone record is found by the clone's path, not by a
    /// session id (`contained_as` takes none), and "own" is the clone this
    /// process can write: a sibling's clone, recorded by its own lease but
    /// bound read-only, is refused.
    // trace:TASK-B6B1278890774EF993D2EBC327862470-139 | ai:claude
    #[test]
    fn own_clone_identity_ignores_the_session_env() {
        let dir = tempfile::TempDir::new().unwrap();
        let (main, own) = layout(dir.path(), "own", true);
        let sibling = make_clone(&dir.path().join(format!("{CLONE_DIR_PREFIX}sib")), &main);
        let path = toml::Value::String(sibling.display().to_string());
        std::fs::write(
            main.join(".aida/sessions/c0ffee0381ff.toml"),
            format!("tree_kind = \"clone\"\nworktree_path = {path}\n"),
        )
        .unwrap();
        let (own_id, sib_id) = (dir_identity(&own).unwrap(), dir_identity(&sibling).unwrap());
        let main_git = main.join(".git");
        // The sandbox's view from `own`: main and the sibling read-only.
        let seat_view = |git_dir: &Path| {
            (git_dir == main_git || git_dir.starts_with(&sibling)).then_some(libc::EROFS)
        };
        assert!(contained_as(&own, own_id, &mut |g| seat_view(g)));
        assert!(!contained_as(&sibling, sib_id, &mut |g| seat_view(g)));
        // The sibling's own view (its tree writable) still passes for it.
        let sibling_view = |git_dir: &Path| {
            (git_dir == main_git || git_dir.starts_with(&own)).then_some(libc::EROFS)
        };
        assert!(contained_as(&sibling, sib_id, &mut |g| sibling_view(g)));
        assert!(!contained_as(&own, own_id, &mut |g| sibling_view(g)));
    }

    /// (d) The lease's path still canonicalises to the clone root, but the
    /// directory there is no longer the one the check began with: refused.
    #[test]
    fn matching_path_with_different_inode_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let (main, clone) = layout(dir.path(), "swapped", true);
        let identity = dir_identity(&clone).unwrap();
        std::fs::rename(&clone, dir.path().join("moved-away")).unwrap();
        let clone = make_clone(&clone, &main);
        assert_ne!(dir_identity(&clone), Some(identity));
        assert!(!contained_with(&clone, identity, Some(libc::EROFS)));
        let fresh = dir_identity(&clone).unwrap();
        assert!(contained_with(&clone, fresh, Some(libc::EROFS)));
    }

    /// The probe cache runs the probe once per main and answers later calls
    /// from the cache, even when the probe would now answer differently.
    #[test]
    fn probe_cache_probes_each_main_once() {
        let cache = std::sync::Mutex::new(std::collections::HashMap::new());
        let mut calls = Vec::new();
        let (a, b) = (Path::new("/main-a"), Path::new("/main-b"));
        let mut ask = |main: &Path, answer: bool| {
            cached_probe(&cache, main, &mut |m| {
                calls.push(m.to_path_buf());
                answer
            })
        };
        assert!(ask(a, true));
        assert!(ask(a, false), "a's first answer is cached");
        assert!(!ask(b, false));
        assert!(!ask(b, true), "b's first answer is cached");
        assert!(ask(a, false));
        assert_eq!(calls, vec![a.to_path_buf(), b.to_path_buf()]);
    }

    /// The injected probe errno bypasses the cache and is restored after.
    #[test]
    fn injected_probe_is_scoped_and_uncached() {
        let git = Path::new("/no-such-main-038/.git");
        assert_eq!(
            with_containment_probe(|_| Some(libc::EROFS), || git_dir_probe(git)),
            Some(libc::EROFS)
        );
        assert_eq!(
            with_containment_probe(|_| Some(libc::EACCES), || git_dir_probe(git)),
            Some(libc::EACCES)
        );
        assert!(FAKE_PROBE.with(|p| p.borrow().is_none()));
    }
}

#[cfg(all(test, unix))]
mod task_2053_read_regular_file_tests {
    use super::read_regular_file;

    /// A FIFO with no writer, a symlink to /dev/zero and an oversized file all
    /// read as `None` without blocking or reading without bound; a small
    /// regular file reads back.
    // trace:TASK-2053 | ai:claude
    #[test]
    fn read_regular_file_refuses_fifo_symlink_and_oversize() {
        let dir = tempfile::TempDir::new().unwrap();
        let fifo = dir.path().join("config-fifo");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        assert_eq!(read_regular_file(&fifo), None);

        let zero = dir.path().join("config-zero");
        std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
        assert_eq!(read_regular_file(&zero), None);

        let big = dir.path().join("config-big");
        std::fs::write(&big, vec![b'a'; 64 * 1024 + 1]).unwrap();
        assert_eq!(read_regular_file(&big), None);

        let ok = dir.path().join("config");
        std::fs::write(&ok, "[core]\n").unwrap();
        assert_eq!(read_regular_file(&ok).as_deref(), Some("[core]\n"));
    }
}
