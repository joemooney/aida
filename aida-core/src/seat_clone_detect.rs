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

/// The root of the seat clone `start` sits in: the nearest ancestor holding
/// a `.git` entry, when that `.git` is a real directory and EITHER the
/// directory is named `aida-seat-*` OR a marker entry of any type exists. A
/// marker the seat replaced with a FIFO or symlink still marks a clone (and
/// then fails verification); a deleted marker is caught by the name. `None`
/// for a linked worktree, a main checkout or any other repository.
// trace:BUG-2169 | ai:claude
pub fn seat_clone_root(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let root = start
        .ancestors()
        .find(|dir| std::fs::symlink_metadata(dir.join(".git")).is_ok())?;
    if !is_real_dir(&root.join(".git")) {
        return None;
    }
    let named = root
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with(CLONE_DIR_PREFIX));
    let marked = std::fs::symlink_metadata(root.join(MARKER_RELATIVE)).is_ok();
    (named || marked).then(|| root.to_path_buf())
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
