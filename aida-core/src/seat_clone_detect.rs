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
