//! BUG-2169: recognise a contained seat's private clone from plain files,
//! without running git in it. A seat clone's `.git/config`, `hooks/` and
//! `info/` are seat-writable, so host git there would run seat-planted
//! configuration outside the sandbox. `aida-cli-lib`'s `seat_clone` module is
//! the only code that runs git in one; host git builders such as
//! [`crate::git_ops::host_git_command`] use [`seat_clone_root`] to refuse.
//!
//! Every read here is FIFO-safe: a seat can plant a FIFO or a symlink where a
//! marker or alternates file is expected, so a path is read only when
//! `symlink_metadata` says it is a regular file.
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
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_READ {
        return None;
    }
    std::fs::read_to_string(path).ok()
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
    let Ok(entries) = std::fs::read_dir(main.join(".aida").join("sessions")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| read_regular_file(&p))
        .filter_map(|body| toml::from_str::<toml::Value>(&body).ok())
        .filter(|lease| lease.get("tree_kind").and_then(|v| v.as_str()) == Some("clone"))
        .filter_map(|lease| lease.get("worktree_path")?.as_str().map(PathBuf::from))
        .filter(|root| root.is_absolute() && root.parent().is_some())
        .map(|root| root.canonicalize().unwrap_or(root))
        .collect()
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
