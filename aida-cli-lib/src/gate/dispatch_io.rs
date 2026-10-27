//! Dispatch paths and private file I/O (STORY-1889). The gate reads request,
//! result, brief and recovery records through these primitives (brief-hash
//! comparison, bounded no-follow reads, pinned lock directories), so they sit
//! on the protected floor with the decisions that trust them. `crate::dispatch`
//! calls them from here; a ratchet keeps `gate/dispatch_*` from importing
//! authority-bearing primitives back out of unprotected modules.
// mandate-gate
// trace:STORY-1917 | ai:agy
// trace:STORY-1889 | ai:claude

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) const MAX_BRIEF_BYTES: usize = 64 * 1024;

pub(crate) const DISPATCH_DIR: &str = "dispatch";
pub(crate) const REQUESTS_SUBDIR: &str = "requests";
pub(crate) const BRIEFS_SUBDIR: &str = "briefs";
pub(crate) const RESULTS_SUBDIR: &str = "results";
pub(crate) const LOGS_SUBDIR: &str = "logs";
pub(crate) const RECOVERY_FILE: &str = "recovery.json";

pub(crate) fn dispatch_root(main_worktree: &Path) -> PathBuf {
    main_worktree.join(".aida").join(DISPATCH_DIR)
}

pub(crate) fn requests_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(REQUESTS_SUBDIR)
}

pub(crate) fn briefs_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(BRIEFS_SUBDIR)
}

pub(crate) fn results_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(RESULTS_SUBDIR)
}

pub(crate) fn logs_dir(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(LOGS_SUBDIR)
}

pub(crate) fn request_path(main_worktree: &Path, id: &str) -> PathBuf {
    requests_dir(main_worktree).join(format!("{id}.json"))
}

pub(crate) fn result_path(main_worktree: &Path, id: &str) -> PathBuf {
    results_dir(main_worktree).join(format!("{id}.json"))
}

pub(crate) fn brief_path(main_worktree: &Path, id: &str) -> PathBuf {
    briefs_dir(main_worktree).join(format!("{id}.md"))
}

pub(crate) fn recovery_path(main_worktree: &Path) -> PathBuf {
    dispatch_root(main_worktree).join(RECOVERY_FILE)
}

/// Write `body` to `path` as a 0600 regular file, atomically (temp + rename).
/// Refuses if `dir` or `path` is a symlink.
// trace:STORY-1917 | ai:agy
pub(crate) fn write_private_atomic(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().context("dispatch path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let dir_meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("cannot stat parent directory {}", dir.display()))?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        bail!(
            "{} is a symlink or not a directory (refused)",
            dir.display()
        );
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            bail!("{} is a symlink (refused)", path.display());
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot stat {}", path.display())),
    }
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("dispatch"),
        Uuid::new_v4()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts.open(&tmp)?;
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    if let Ok(m) = std::fs::symlink_metadata(path) {
        if m.file_type().is_symlink() {
            let _ = std::fs::remove_file(&tmp);
            bail!("{} is a symlink (refused)", path.display());
        }
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Ensure and open a child directory under a held parent directory descriptor
/// without following symlinks (`mkdirat` + `openat(O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)`),
/// verifying it is owned by the current uid.
// trace:STORY-1917 | ai:agy
fn open_or_create_pinned_child_dir(
    parent: &std::fs::File,
    name: &str,
    display_path: &Path,
) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    let c_name = std::ffi::CString::new(name.as_bytes())
        .with_context(|| format!("invalid directory component `{name}`"))?;
    let mk_rc = unsafe { libc::mkdirat(parent.as_raw_fd(), c_name.as_ptr(), 0o700) };
    if mk_rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EEXIST) {
            return Err(err).with_context(|| format!("cannot create {}", display_path.display()));
        }
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            c_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        if matches!(err.raw_os_error(), Some(libc::ELOOP) | Some(libc::ENOTDIR)) {
            bail!(
                "{} is a symlink or not a directory (refused)",
                display_path.display()
            );
        }
        return Err(err).with_context(|| format!("cannot open {}", display_path.display()));
    }
    let dir = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = dir
        .metadata()
        .with_context(|| format!("cannot stat {}", display_path.display()))?;
    if !meta.is_dir() {
        bail!("{} is not a directory (refused)", display_path.display());
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            display_path.display(),
            meta.uid()
        );
    }
    Ok(dir)
}

/// Pin `.aida/dispatch[/<subdir>]` relative to `main_worktree` using descriptor-relative
/// `openat(O_DIRECTORY | O_NOFOLLOW)` traversal so no symlink at `.aida`,
/// `.aida/dispatch`, or `.aida/dispatch/<subdir>` is ever followed.
// trace:STORY-1917 | ai:agy
pub(crate) fn open_pinned_dispatch_subdir(
    main_worktree: &Path,
    subdir: Option<&str>,
) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let root_fd = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(main_worktree)
        .with_context(|| format!("cannot open main worktree {}", main_worktree.display()))?;
    let aida_path = main_worktree.join(".aida");
    let aida_fd = open_or_create_pinned_child_dir(&root_fd, ".aida", &aida_path)?;
    let disp_path = aida_path.join(DISPATCH_DIR);
    let disp_fd = open_or_create_pinned_child_dir(&aida_fd, DISPATCH_DIR, &disp_path)?;
    match subdir {
        Some(sub) => {
            let sub_path = disp_path.join(sub);
            open_or_create_pinned_child_dir(&disp_fd, sub, &sub_path)
        }
        None => Ok(disp_fd),
    }
}

/// Open `path` with `O_NOFOLLOW | O_NONBLOCK`, verify it is a regular file
/// owned by the current uid with mode 0600 (when `require_0600` is true) and
/// size `<= max_bytes`, and read its bytes.
// trace:STORY-1917 | ai:agy
pub(crate) fn read_bounded_regular_file(
    path: &Path,
    max_bytes: usize,
    require_0600: bool,
) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            if e.raw_os_error() == Some(libc::ELOOP) {
                anyhow::anyhow!("{} is a symlink (refused)", path.display())
            } else {
                anyhow::anyhow!("cannot open {}: {e}", path.display())
            }
        })?;
    let meta = file
        .metadata()
        .with_context(|| format!("cannot stat {}", path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file", path.display());
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            path.display(),
            meta.uid()
        );
    }
    if require_0600 {
        if meta.nlink() != 1 {
            bail!(
                "{} has {} hard links, expected 1 (refused)",
                path.display(),
                meta.nlink()
            );
        }
        let mode = meta.mode() & 0o7777;
        if mode != 0o600 {
            bail!("{} has mode {:04o}, expected 0600", path.display(), mode);
        }
    }
    if meta.len() > max_bytes as u64 {
        bail!(
            "{} exceeds maximum size ({} bytes > {} bytes)",
            path.display(),
            meta.len(),
            max_bytes
        );
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    file.take((max_bytes as u64) + 1)
        .read_to_end(&mut buf)
        .with_context(|| format!("cannot read {}", path.display()))?;
    if buf.len() > max_bytes {
        bail!(
            "{} exceeds maximum size ({} bytes)",
            path.display(),
            max_bytes
        );
    }
    Ok(buf)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
