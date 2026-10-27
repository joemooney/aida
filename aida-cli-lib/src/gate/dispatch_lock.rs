//! Dispatch `tick.lock` authority (STORY-1889, moved verbatim from
//! `dispatch.rs`). Every claim, result transition and recovery-ledger write
//! happens under this lock, and only a direct human may activate its v2
//! directory form ([`activate_lock_as_human`]), and only while dispatch is
//! quiescent. The path helpers and pinned-directory I/O are in
//! [`crate::gate::dispatch_io`].
// mandate-gate
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude

use anyhow::{bail, Context, Result};
use std::path::Path;
use uuid::Uuid;

use crate::gate::dispatch_io::{dispatch_root, open_pinned_dispatch_subdir};

pub(crate) const TICK_LOCK_FILE: &str = "tick.lock";
const TICK_LOCK_V2_FILE: &str = "v2.lock";
const TICK_LOCK_VERSION_FILE: &str = "version";
const TICK_LOCK_VERSION_BODY: &[u8] = b"2\n";
const TICK_LOCK_STAGING_DIR: &str = ".tick.lock.v2-staging";

#[cfg(test)]
pub(crate) static TEST_PROC_PID_ALLOWLIST: std::sync::Mutex<Option<Vec<u32>>> =
    std::sync::Mutex::new(None);
/// Stands in for this process's `/proc` view (namespace probe, `/proc`
/// mount options) in [`verify_sole_open_fd_for_inode`]. Unset, a test that
/// pins [`TEST_PROC_PID_ALLOWLIST`] scans only the pids it names, so the view
/// is not consulted.
// trace:STORY-1917 | ai:claude
#[cfg(test)]
pub(crate) static TEST_PROC_VIEW: std::sync::Mutex<
    Option<(crate::agent_registry::PidNamespaceProbe, Option<String>)>,
> = std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_POST_EXCHANGE_HOOK: std::sync::Mutex<
    Option<Box<dyn FnOnce() + Send + 'static>>,
> = std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_PRE_FLOCK_V2_HOOK: std::sync::Mutex<
    Option<Box<dyn FnOnce() + Send + 'static>>,
> = std::sync::Mutex::new(None);
/// Makes the next activation rollback exchange fail, to prove the failure is
/// reported and fenced.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
#[cfg(test)]
pub(crate) static TEST_FAIL_ROLLBACK_EXCHANGE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Exclusive flock over `.aida/dispatch/tick.lock/v2.lock`, explicitly unlocked
/// on Drop before closing the fd (BUG-2160, TASK-2344 Q1).
///
/// Replacing the Slice 1/2 regular file `.aida/dispatch/tick.lock` with a
/// pinned `0700` directory `.aida/dispatch/tick.lock/` (containing `v2.lock`)
/// creates a kernel-enforced physical compatibility boundary: any pre-Slice-3
/// binary attempting to open `.aida/dispatch/tick.lock` for `read+write` across
/// any of the 5 mutation entrypoints fails immediately at the OS boundary with
/// `EISDIR` (`Is a directory`) before mutating request/result state.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug)]
pub(crate) struct DispatchLock(std::fs::File);

impl Drop for DispatchLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

pub(crate) fn acquire_dispatch_lock_for_gate(main_worktree: &Path) -> Result<DispatchLock> {
    acquire_dispatch_lock(main_worktree)
}

/// `aida dispatch activate-lock`: a direct human at an interactive TTY
/// activates the v2 lock directory, and only while dispatch is quiescent.
// trace:STORY-1889 trace:STORY-1917 trace:TASK-2344 | ai:claude
pub(crate) fn activate_lock_as_human(main_worktree: &Path) -> Result<DispatchLock> {
    crate::gate::seat_authority::require_direct_human().map_err(|_| {
        anyhow::anyhow!(
            "`aida dispatch activate-lock` needs a direct human session at an interactive TTY; run it from your terminal"
        )
    })?;
    activate_dispatch_v2_lock_quiescent(main_worktree)
}

fn is_vanished_proc_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
        || matches!(err.raw_os_error(), Some(libc::ENOENT | libc::ESRCH))
}

/// The options `/proc` is mounted with (per-mount and superblock), from the
/// last `/proc` entry in `mountinfo` (the one this process sees). `None` when
/// there is none.
// trace:STORY-1917 | ai:claude
pub(crate) fn proc_mount_options(mountinfo: &str) -> Option<String> {
    mountinfo.lines().rev().find_map(|line| {
        let (mount, fs) = line.split_once(" - ")?;
        let mut fields = mount.split_whitespace();
        let mount_point = fields.nth(4)?;
        let mount_opts = fields.next()?;
        let mut fs = fs.split_whitespace();
        let (fs_type, _source, super_opts) = (fs.next()?, fs.next()?, fs.next()?);
        (mount_point == "/proc" && fs_type == "proc").then(|| format!("{mount_opts},{super_opts}"))
    })
}

/// Why this process's `/proc` cannot show every process that might hold the
/// legacy lock, if it cannot: it runs in a PID namespace of its own (a
/// sandbox, `bwrap --unshare-pid`, a container), or `/proc` is mounted with
/// `hidepid`. Either way a scan that finds no other holder proves nothing.
// trace:STORY-1917 | ai:claude
pub(crate) fn proc_view_refusal(
    probe: &crate::agent_registry::PidNamespaceProbe,
    proc_mount_options: Option<&str>,
) -> Option<String> {
    if probe.is_isolated() {
        return Some(
            "this process runs in its own PID namespace (a sandbox or container), so /proc \
             cannot show processes outside it"
                .to_string(),
        );
    }
    let Some(options) = proc_mount_options else {
        return Some("the /proc mount options could not be read".to_string());
    };
    let hidepid = options
        .split(',')
        .filter_map(|o| o.strip_prefix("hidepid="))
        .find(|v| !matches!(*v, "0" | "off"));
    hidepid.map(|v| {
        format!("/proc is mounted with hidepid={v}, so other processes' descriptors are hidden")
    })
}

fn live_proc_view_refusal() -> Option<String> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").ok();
    proc_view_refusal(
        &crate::agent_registry::PidNamespaceProbe::read(),
        mountinfo.as_deref().and_then(proc_mount_options).as_deref(),
    )
}

/// Verify via `/proc` that `(dev, ino)` is not open on any file descriptor in
/// the current process or any other process owned by the current UID other than
/// `(std::process::id(), own_fd)`.
///
/// Any `/proc` directory entry error, unreadable `/proc/<pid>` metadata,
/// unreadable `/proc/<pid>/fd` directory for a live same-UID process (e.g.
/// `EACCES` on a non-dumpable or foreign-user-namespace process), or unreadable
/// descriptor metadata on a still-open FD is treated as **unknown coverage**
/// and fails closed (`Err`). So is a `/proc` that cannot see every process
/// ([`proc_view_refusal`]). Processes of other users are skipped: a
/// pre-Slice-3 dispatch binary runs as the operator who owns the store.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn verify_sole_open_fd_for_inode(
    dev: u64,
    ino: u64,
    own_fd: std::os::fd::RawFd,
    display_path: &Path,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let self_pid = std::process::id();
    let uid = unsafe { libc::geteuid() };
    #[cfg(test)]
    let pid_allowlist = TEST_PROC_PID_ALLOWLIST.lock().ok().and_then(|g| g.clone());

    // trace:STORY-1917 | ai:claude
    #[cfg(not(test))]
    let view_refusal = live_proc_view_refusal();
    #[cfg(test)]
    let view_refusal = match TEST_PROC_VIEW.lock().ok().and_then(|g| g.clone()) {
        Some((probe, options)) => proc_view_refusal(&probe, options.as_deref()),
        None if pid_allowlist.is_some() => None,
        None => live_proc_view_refusal(),
    };
    if let Some(why) = view_refusal {
        bail!(
            "cannot prove legacy {} is unused: {why} (unknown process coverage; fail-closed). \
             Run `aida dispatch activate-lock` from the host's own PID namespace",
            display_path.display()
        );
    }

    let proc_entries = std::fs::read_dir("/proc")
        .context("cannot read /proc to verify legacy dispatch lock quiescence")?;
    let mut saw_own_fd = false;

    for entry_res in proc_entries {
        let entry = entry_res.context(
            "error reading /proc directory entry during legacy lock quiescence check (fail-closed)",
        )?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            bail!("non-UTF-8 entry in /proc during legacy lock quiescence check (fail-closed)");
        };
        let Ok(pid) = name_str.parse::<u32>() else {
            // Non-PID entries in /proc (`self`, `thread-self`, `mounts`, `cpuinfo`, etc.).
            continue;
        };
        #[cfg(test)]
        if let Some(ref allowed) = pid_allowlist {
            if !allowed.contains(&pid) {
                continue;
            }
        }
        let proc_path = entry.path();
        if pid != self_pid {
            let proc_meta = match std::fs::symlink_metadata(&proc_path) {
                Ok(m) => m,
                Err(e) if is_vanished_proc_error(&e) && !proc_path.exists() => continue,
                Err(e) => {
                    bail!(
                        "cannot stat {} during legacy lock quiescence check: {e} (unknown process coverage; fail-closed)",
                        proc_path.display()
                    );
                }
            };
            if proc_meta.uid() != uid {
                continue;
            }
        }
        let fd_dir = proc_path.join("fd");
        let fd_entries = match std::fs::read_dir(&fd_dir) {
            Ok(rd) => rd,
            Err(e) if is_vanished_proc_error(&e) && !proc_path.exists() => continue,
            Err(e) => {
                bail!(
                    "cannot read {} for same-uid pid {pid} during legacy lock quiescence check: {e} (unknown FD coverage; fail-closed)",
                    fd_dir.display()
                );
            }
        };
        for fd_entry_res in fd_entries {
            let fd_entry = match fd_entry_res {
                Ok(fe) => fe,
                Err(e) if is_vanished_proc_error(&e) && !proc_path.exists() => break,
                Err(e) => {
                    bail!(
                        "error reading entry in {} for same-uid pid {pid}: {e} (unknown FD coverage; fail-closed)",
                        fd_dir.display()
                    );
                }
            };
            let fd_name = fd_entry.file_name();
            let Some(fd_str) = fd_name.to_str() else {
                bail!(
                    "non-UTF-8 fd entry in {} for pid {pid} (fail-closed)",
                    fd_dir.display()
                );
            };
            let Ok(fd_num) = fd_str.parse::<i32>() else {
                bail!(
                    "unexpected non-numeric fd entry `{fd_str}` in {} for pid {pid} (fail-closed)",
                    fd_dir.display()
                );
            };
            let fd_path = fd_entry.path();
            let target_meta = match std::fs::metadata(&fd_path) {
                Ok(m) => m,
                Err(e) => {
                    // Distinguish a descriptor that closed (or process that exited)
                    // between `readdir` and `stat` from an open descriptor whose
                    // target cannot be inspected.
                    match std::fs::symlink_metadata(&fd_path) {
                        Err(le) if is_vanished_proc_error(&le) => continue,
                        _ => {
                            bail!(
                                "cannot stat open descriptor {} (pid {pid} fd {fd_num}) during legacy lock quiescence check: {e} (unknown FD coverage; fail-closed)",
                                fd_path.display()
                            );
                        }
                    }
                }
            };
            if target_meta.dev() == dev && target_meta.ino() == ino {
                if pid == self_pid && fd_num == own_fd {
                    saw_own_fd = true;
                } else {
                    bail!(
                        "legacy {} (dev={dev}, ino={ino}) is still open in pid {pid} fd {fd_num} (open-before-flock or waiter race detected); stop all pre-Slice-3 dispatch processes before activating v2 lock",
                        display_path.display()
                    );
                }
            }
        }
    }

    if !saw_own_fd {
        bail!(
            "cannot confirm own descriptor (pid={self_pid}, fd={own_fd}) in /proc for legacy {} (fail-closed)",
            display_path.display()
        );
    }
    Ok(())
}

/// Stat a child entry relative to `parent_fd` with `AT_SYMLINK_NOFOLLOW`.
fn fstatat_nofollow(
    parent_fd: &std::fs::File,
    child_cstr: &std::ffi::CStr,
    display_path: &Path,
) -> Result<libc::stat> {
    use std::os::fd::AsRawFd;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            parent_fd.as_raw_fd(),
            child_cstr.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot lstat {}", display_path.display()));
    }
    Ok(st)
}

fn cleanup_staging_lock_dir(disp_fd: &std::fs::File) {
    use std::os::fd::{AsRawFd, FromRawFd};
    let Ok(c_staging) = std::ffi::CString::new(TICK_LOCK_STAGING_DIR) else {
        return;
    };
    let st_fd = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_staging.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if st_fd >= 0 {
        let st_dir = unsafe { std::fs::File::from_raw_fd(st_fd) };
        if let Ok(c_v2) = std::ffi::CString::new(TICK_LOCK_V2_FILE) {
            unsafe { libc::unlinkat(st_dir.as_raw_fd(), c_v2.as_ptr(), 0) };
        }
        if let Ok(c_ver) = std::ffi::CString::new(TICK_LOCK_VERSION_FILE) {
            unsafe { libc::unlinkat(st_dir.as_raw_fd(), c_ver.as_ptr(), 0) };
        }
        drop(st_dir);
        unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), libc::AT_REMOVEDIR) };
    } else {
        unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), 0) };
    }
}

fn write_lock_version_file(lock_dir: &std::fs::File, lock_dir_display: &Path) -> Result<()> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    let c_ver = std::ffi::CString::new(TICK_LOCK_VERSION_FILE)?;
    let ver_display = lock_dir_display.join(TICK_LOCK_VERSION_FILE);
    let ver_fd = unsafe {
        libc::openat(
            lock_dir.as_raw_fd(),
            c_ver.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if ver_fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot write {}", ver_display.display()));
    }
    let mut ver_file = unsafe { std::fs::File::from_raw_fd(ver_fd) };
    ver_file
        .write_all(TICK_LOCK_VERSION_BODY)
        .with_context(|| format!("cannot write {}", ver_display.display()))?;
    ver_file
        .sync_all()
        .with_context(|| format!("cannot sync {}", ver_display.display()))?;
    Ok(())
}

/// Operator recovery steps named by every refusal that involves the legacy
/// lock file or an unfinished activation.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
const LOCK_ACTIVATION_GUIDANCE: &str = "stop every `aida dispatch serve`, `aida dispatch tick` and `aida dispatch run` process and the schedule driver, then run `aida dispatch activate-lock` from your terminal";

fn verify_lock_version_file(lock_dir: &std::fs::File, lock_dir_display: &Path) -> Result<()> {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    let c_ver = std::ffi::CString::new(TICK_LOCK_VERSION_FILE)?;
    let ver_display = lock_dir_display.join(TICK_LOCK_VERSION_FILE);
    let ver_fd = unsafe {
        libc::openat(
            lock_dir.as_raw_fd(),
            c_ver.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0,
        )
    };
    if ver_fd < 0 {
        let err = std::io::Error::last_os_error();
        // A v2 directory without its marker is an activation that stopped
        // after its exchange (a crash, or a rollback that failed). It is
        // never adopted here: only the activation command may finish it.
        // trace:STORY-1917 trace:TASK-2344 | ai:claude
        if err.raw_os_error() == Some(libc::ENOENT) {
            bail!(
                "{} has no version marker: a dispatch lock activation was interrupted or its rollback failed, so dispatch refuses to use it. To finish or recover it, {LOCK_ACTIVATION_GUIDANCE}",
                lock_dir_display.display()
            );
        }
        return Err(err).with_context(|| {
            format!(
                "unreadable dispatch lock version marker {}",
                ver_display.display()
            )
        });
    }
    let mut ver_file = unsafe { std::fs::File::from_raw_fd(ver_fd) };
    let meta = ver_file
        .metadata()
        .with_context(|| format!("cannot stat {}", ver_display.display()))?;
    if !meta.is_file() || meta.nlink() != 1 {
        bail!(
            "{} is not a singly-linked regular file (refused)",
            ver_display.display()
        );
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            ver_display.display(),
            meta.uid()
        );
    }
    if (meta.mode() & 0o7777) != 0o600 {
        bail!(
            "{} has mode {:04o}, expected 0600 (refused)",
            ver_display.display(),
            meta.mode() & 0o7777
        );
    }
    let mut body = String::new();
    ver_file
        .read_to_string(&mut body)
        .with_context(|| format!("cannot read {}", ver_display.display()))?;
    if body.trim() != "2" {
        bail!(
            "unsupported dispatch lock version {:?} in {} (expected \"2\")",
            body.trim(),
            ver_display.display()
        );
    }
    Ok(())
}

/// `true` when `tick.lock` (already known to be a directory) carries its
/// `version` marker, `false` when the marker is absent.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
fn lock_dir_has_version(disp_fd: &std::fs::File, lock_dir_display: &Path) -> Result<bool> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;
    let raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if raw < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot open {}", lock_dir_display.display()));
    }
    let dir = unsafe { std::fs::File::from_raw_fd(raw) };
    let c_ver = std::ffi::CString::new(TICK_LOCK_VERSION_FILE)?;
    let ver_display = lock_dir_display.join(TICK_LOCK_VERSION_FILE);
    match fstatat_nofollow(&dir, &c_ver, &ver_display) {
        Ok(_) => Ok(true),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .and_then(|io| io.raw_os_error())
                == Some(libc::ENOENT) =>
        {
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Create `.aida/dispatch/tick.lock/` for a dispatch root that has none. The
/// directory is assembled under a private name with its `v2.lock` and
/// `version` already in place, then renamed into position with
/// `RENAME_NOREPLACE`, so no process can ever observe a v2 directory that
/// lacks its marker. Losing the race to another creator is not an error.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
fn create_lock_dir_if_absent(disp_fd: &std::fs::File, main_worktree: &Path) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::fstatat(
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() != Some(libc::ENOENT) {
        return Err(err).with_context(|| format!("cannot lstat {}", lock_dir_display.display()));
    }

    let init_name = format!(
        ".tick.lock.init-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    );
    let init_display = dispatch_root(main_worktree).join(&init_name);
    let c_init = std::ffi::CString::new(init_name)?;
    if unsafe { libc::mkdirat(disp_fd.as_raw_fd(), c_init.as_ptr(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot create {}", init_display.display()));
    }
    let c_v2 = std::ffi::CString::new(TICK_LOCK_V2_FILE)?;
    let c_ver = std::ffi::CString::new(TICK_LOCK_VERSION_FILE)?;
    let discard = |dir: Option<&std::fs::File>| {
        if let Some(dir) = dir {
            unsafe { libc::unlinkat(dir.as_raw_fd(), c_v2.as_ptr(), 0) };
            unsafe { libc::unlinkat(dir.as_raw_fd(), c_ver.as_ptr(), 0) };
        }
        unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_init.as_ptr(), libc::AT_REMOVEDIR) };
    };
    let dir_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_init.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if dir_raw < 0 {
        let err = std::io::Error::last_os_error();
        discard(None);
        return Err(err).with_context(|| format!("cannot open {}", init_display.display()));
    }
    let init_dir = unsafe { std::fs::File::from_raw_fd(dir_raw) };
    let v2_raw = unsafe {
        libc::openat(
            init_dir.as_raw_fd(),
            c_v2.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if v2_raw < 0 {
        let err = std::io::Error::last_os_error();
        discard(Some(&init_dir));
        return Err(err).with_context(|| {
            format!(
                "cannot create {}",
                init_display.join(TICK_LOCK_V2_FILE).display()
            )
        });
    }
    drop(unsafe { std::fs::File::from_raw_fd(v2_raw) });
    if let Err(e) = write_lock_version_file(&init_dir, &init_display) {
        discard(Some(&init_dir));
        return Err(e);
    }
    let rc = unsafe {
        libc::renameat2(
            disp_fd.as_raw_fd(),
            c_init.as_ptr(),
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        discard(Some(&init_dir));
        if err.raw_os_error() == Some(libc::EEXIST) {
            return Ok(());
        }
        return Err(err).with_context(|| {
            format!(
                "cannot move {} into place at {}",
                init_display.display(),
                lock_dir_display.display()
            )
        });
    }
    Ok(())
}

/// Undo an activation's `RENAME_EXCHANGE`: put the legacy regular-file inode
/// back at `tick.lock`, then remove the staging `v2.lock` and directory. A
/// failed exchange-back is reported, never ignored. It leaves the v2
/// directory at `tick.lock` without its `version` marker, which every v2
/// mutator refuses (and every pre-Slice-3 binary fails on with `EISDIR`), so
/// nothing can lock either inode until `aida dispatch activate-lock` resumes.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
fn rollback_activation_exchange(
    disp_fd: &std::fs::File,
    staging_dir: std::fs::File,
    main_worktree: &Path,
) -> Result<()> {
    use std::os::fd::AsRawFd;
    let c_staging = std::ffi::CString::new(TICK_LOCK_STAGING_DIR)?;
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;
    let c_v2 = std::ffi::CString::new(TICK_LOCK_V2_FILE)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let staging_display = dispatch_root(main_worktree).join(TICK_LOCK_STAGING_DIR);

    #[cfg(test)]
    let injected = TEST_FAIL_ROLLBACK_EXCHANGE.swap(false, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(test))]
    let injected = false;
    let rc = if injected {
        -1
    } else {
        unsafe {
            libc::renameat2(
                disp_fd.as_raw_fd(),
                c_staging.as_ptr(),
                disp_fd.as_raw_fd(),
                c_lock_dir.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        }
    };
    if rc != 0 {
        let err = if injected {
            std::io::Error::from_raw_os_error(libc::EIO)
        } else {
            std::io::Error::last_os_error()
        };
        bail!(
            "rolling back the dispatch lock activation failed ({err}). {} is left as the v2 directory without its version marker, so every dispatch command refuses and pre-Slice-3 binaries fail with EISDIR; the legacy lock file is at {}. To finish or recover it, {LOCK_ACTIVATION_GUIDANCE}",
            lock_dir_display.display(),
            staging_display.display()
        );
    }
    let mut problems = Vec::new();
    if unsafe { libc::unlinkat(staging_dir.as_raw_fd(), c_v2.as_ptr(), 0) } != 0 {
        problems.push(format!(
            "cannot unlink {}: {}",
            staging_display.join(TICK_LOCK_V2_FILE).display(),
            std::io::Error::last_os_error()
        ));
    }
    drop(staging_dir);
    if unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        problems.push(format!(
            "cannot remove {}: {}",
            staging_display.display(),
            std::io::Error::last_os_error()
        ));
    }
    if !problems.is_empty() {
        bail!(
            "the dispatch lock activation was rolled back and {} is the legacy lock file again, but cleanup failed ({}); the next `aida dispatch activate-lock` removes the leftovers",
            lock_dir_display.display(),
            problems.join("; ")
        );
    }
    Ok(())
}

/// Finish an activation that stopped after its `RENAME_EXCHANGE`: `tick.lock`
/// is the v2 directory but has no `version` marker, and the legacy inode is
/// at the staging name. Quiescence is proven again on that inode (no flock,
/// no other open descriptor, full `/proc` coverage) before the marker is
/// written and the legacy inode is unlinked. Any other shape is refused with
/// manual recovery guidance, never guessed at.
// trace:STORY-1917 trace:TASK-2344 | ai:claude
fn resume_interrupted_activation(
    disp_fd: &std::fs::File,
    main_worktree: &Path,
) -> Result<DispatchLock> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let staging_display = dispatch_root(main_worktree).join(TICK_LOCK_STAGING_DIR);
    let c_staging = std::ffi::CString::new(TICK_LOCK_STAGING_DIR)?;
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;
    let uid = unsafe { libc::geteuid() };

    let manual = format!(
        "confirm no dispatch process runs, remove {} by hand, and run `aida dispatch activate-lock` again",
        lock_dir_display.display()
    );
    let st = match fstatat_nofollow(disp_fd, &c_staging, &staging_display) {
        Ok(st) => st,
        Err(_) => bail!(
            "{} has no version marker and no legacy lock file remains at {} to prove quiescence against, so the activation cannot be resumed; {manual}",
            lock_dir_display.display(),
            staging_display.display()
        ),
    };
    if (st.st_mode & libc::S_IFMT) != libc::S_IFREG || st.st_nlink != 1 || st.st_uid != uid {
        bail!(
            "{} has no version marker and {} is not your singly-linked legacy lock file, so the activation cannot be resumed; {manual}",
            lock_dir_display.display(),
            staging_display.display()
        );
    }
    let leg_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_staging.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0,
        )
    };
    if leg_raw < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot open {}", staging_display.display()));
    }
    let leg_file = unsafe { std::fs::File::from_raw_fd(leg_raw) };
    let leg_meta = leg_file.metadata()?;
    if !leg_meta.is_file() || leg_meta.dev() != st.st_dev || leg_meta.ino() != st.st_ino {
        bail!(
            "{} changed identity during open (refused)",
            staging_display.display()
        );
    }
    if unsafe { libc::flock(leg_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!(
            "the legacy lock file {} is still locked by another process; {LOCK_ACTIVATION_GUIDANCE}",
            staging_display.display()
        );
    }
    verify_sole_open_fd_for_inode(
        leg_meta.dev(),
        leg_meta.ino(),
        leg_file.as_raw_fd(),
        &staging_display,
    )?;

    let dir_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if dir_raw < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot open {}", lock_dir_display.display()));
    }
    let lock_dir = unsafe { std::fs::File::from_raw_fd(dir_raw) };
    let dir_meta = lock_dir.metadata()?;
    if !dir_meta.is_dir() || dir_meta.uid() != uid || (dir_meta.mode() & 0o7777) != 0o700 {
        bail!(
            "{} is not your 0700 v2 lock directory (refused); {manual}",
            lock_dir_display.display()
        );
    }
    let c_v2 = std::ffi::CString::new(TICK_LOCK_V2_FILE)?;
    let v2_display = lock_dir_display.join(TICK_LOCK_V2_FILE);
    let v2_raw = unsafe {
        libc::openat(
            lock_dir.as_raw_fd(),
            c_v2.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0,
        )
    };
    if v2_raw < 0 {
        let err = std::io::Error::last_os_error();
        bail!(
            "cannot open {} to resume the activation ({err}); {manual}",
            v2_display.display()
        );
    }
    let v2_file = unsafe { std::fs::File::from_raw_fd(v2_raw) };
    let v2_meta = v2_file.metadata()?;
    if !v2_meta.is_file()
        || v2_meta.nlink() != 1
        || v2_meta.uid() != uid
        || (v2_meta.mode() & 0o7777) != 0o600
    {
        bail!(
            "{} is not your singly-linked 0600 lock file (refused); {manual}",
            v2_display.display()
        );
    }
    if unsafe { libc::flock(v2_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!(
            "{} is held by another process; {LOCK_ACTIVATION_GUIDANCE}",
            v2_display.display()
        );
    }
    if lock_dir_has_version(disp_fd, &lock_dir_display)? {
        bail!(
            "another `aida dispatch activate-lock` finished while this one was resuming; run it again to confirm"
        );
    }
    let now_st = fstatat_nofollow(disp_fd, &c_staging, &staging_display)?;
    if now_st.st_dev != leg_meta.dev() || now_st.st_ino != leg_meta.ino() {
        bail!(
            "{} changed identity while resuming the activation (refused)",
            staging_display.display()
        );
    }
    write_lock_version_file(&lock_dir, &lock_dir_display)?;
    if unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), 0) } != 0 {
        let err = std::io::Error::last_os_error();
        bail!(
            "the dispatch lock is active, but the legacy lock file {} could not be removed ({err}); remove it by hand",
            staging_display.display()
        );
    }
    let _ = fs2::FileExt::unlock(&leg_file);
    drop(leg_file);
    Ok(DispatchLock(v2_file))
}

/// Explicit quiescent migration from a legacy Slice 1/2 regular-file
/// `.aida/dispatch/tick.lock` to the Slice 3 directory `.aida/dispatch/tick.lock/`
/// containing `v2.lock` and `version`. Run by `aida dispatch activate-lock`.
///
/// Ordinary mutation entrypoints (`acquire_dispatch_lock`) NEVER perform this
/// replacement automatically. This helper:
/// 1. Verifies `lstat` (`fstatat(..., AT_SYMLINK_NOFOLLOW)`) is a singly-linked
///    regular file owned by the current UID with no special or world-write
///    bits. Slice 1/2 created it with the umask's default mode (typically
///    0644 or 0664), and the inode is discarded once activation commits.
/// 2. Opens the file with `O_NOFOLLOW | O_NONBLOCK` and verifies exact
///    pathname-to-handle `(st_dev, st_ino)` identity.
/// 3. Acquires a non-blocking exclusive `flock` (`LOCK_EX | LOCK_NB`), refusing
///    if any legacy mutator holds the lock, and re-verifies pathname-to-handle
///    identity after locking.
/// 4. Scans `/proc` via [`verify_sole_open_fd_for_inode`] to refuse if any
///    other process (or descriptor) has opened the legacy inode before `flock`
///    or if any same-UID process has unknown `/proc/<pid>/fd` coverage.
/// 5. Creates a staging directory `.aida/dispatch/.tick.lock.v2-staging` (`0700`)
///    containing `v2.lock` (`0600`) and acquires `flock(LOCK_EX | LOCK_NB)` on
///    `v2.lock` BEFORE replacing `.aida/dispatch/tick.lock`.
/// 6. Atomically exchanges `.tick.lock.v2-staging` and `tick.lock` via
///    `renameat2(..., RENAME_EXCHANGE)`, eliminating any `ENOENT` window where
///    an ordinary `acquire_dispatch_lock` could create `tick.lock` or enter `v2`
///    before the second `/proc` scan completes.
/// 7. Re-runs [`verify_sole_open_fd_for_inode`] while holding BOTH `leg_file`
///    and `v2.lock`. If the second scan fails, [`rollback_activation_exchange`]
///    swaps the **exact original regular-file inode `(st_dev, st_ino)`** back to
///    `tick.lock` and unlinks the staging `v2.lock` before releasing it, so any
///    concurrent `acquire_dispatch_lock` waiter aborts fail-closed. A failed
///    rollback is reported and leaves the marker-less directory fenced.
/// 8. On success, writes `tick.lock/version` (`"2\n"`), unlinks the swapped-out
///    legacy regular file at `.tick.lock.v2-staging` (verifying `nlink == 0`),
///    drops `leg_file`, and returns `DispatchLock(v2_file)`.
///
/// A `tick.lock` directory without its marker is an activation that stopped
/// after step 6; it is finished by [`resume_interrupted_activation`].
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn activate_dispatch_v2_lock_quiescent(main_worktree: &Path) -> Result<DispatchLock> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let disp_fd = open_pinned_dispatch_subdir(main_worktree, None)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;

    let pre_st = match fstatat_nofollow(&disp_fd, &c_lock_dir, &lock_dir_display) {
        Ok(st) => st,
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .and_then(|io| io.raw_os_error())
                == Some(libc::ENOENT) =>
        {
            // No lock at all: nothing legacy to migrate from.
            return acquire_dispatch_lock(main_worktree);
        }
        Err(e) => return Err(e),
    };
    if (pre_st.st_mode & libc::S_IFMT) == libc::S_IFDIR {
        // trace:STORY-1917 trace:TASK-2344 | ai:claude
        if lock_dir_has_version(&disp_fd, &lock_dir_display)? {
            return acquire_dispatch_lock(main_worktree);
        }
        return resume_interrupted_activation(&disp_fd, main_worktree);
    }
    if (pre_st.st_mode & libc::S_IFMT) != libc::S_IFREG || pre_st.st_nlink != 1 {
        bail!(
            "legacy {} is not a singly-linked regular file (refused)",
            lock_dir_display.display()
        );
    }
    let uid = unsafe { libc::geteuid() };
    if pre_st.st_uid != uid {
        bail!(
            "legacy {} is owned by uid {}, not current uid {uid}",
            lock_dir_display.display(),
            pre_st.st_uid
        );
    }
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    if (pre_st.st_mode & 0o7002) != 0 {
        bail!(
            "legacy {} has mode {:04o} (special or world-writable bits set); refused. Run `chmod 0600 {}` once no dispatch process runs, then retry",
            lock_dir_display.display(),
            pre_st.st_mode & 0o7777,
            lock_dir_display.display()
        );
    }

    let leg_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0,
        )
    };
    if leg_raw < 0 {
        let open_err = std::io::Error::last_os_error();
        bail!(
            "cannot open legacy {} for quiescent migration: {open_err}",
            lock_dir_display.display()
        );
    }
    let leg_file = unsafe { std::fs::File::from_raw_fd(leg_raw) };
    let leg_meta = leg_file
        .metadata()
        .with_context(|| format!("cannot fstat legacy {}", lock_dir_display.display()))?;
    if !leg_meta.is_file()
        || leg_meta.nlink() != 1
        || leg_meta.dev() != pre_st.st_dev
        || leg_meta.ino() != pre_st.st_ino
    {
        bail!(
            "legacy {} changed identity during open (pathname-to-handle mismatch; refused)",
            lock_dir_display.display()
        );
    }

    let flock_rc = unsafe { libc::flock(leg_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if flock_rc != 0 {
        bail!(
            "legacy {} is actively held by another process; refusing concurrent dispatch lock migration (fail-closed). {LOCK_ACTIVATION_GUIDANCE}",
            lock_dir_display.display()
        );
    }

    let post_lock_st = fstatat_nofollow(&disp_fd, &c_lock_dir, &lock_dir_display)?;
    if post_lock_st.st_dev != leg_meta.dev()
        || post_lock_st.st_ino != leg_meta.ino()
        || post_lock_st.st_nlink != 1
    {
        bail!(
            "legacy {} pathname-to-handle identity changed before migration (refused)",
            lock_dir_display.display()
        );
    }

    verify_sole_open_fd_for_inode(
        leg_meta.dev(),
        leg_meta.ino(),
        leg_file.as_raw_fd(),
        &lock_dir_display,
    )?;

    // Prepare pre-locked staging directory `.aida/dispatch/.tick.lock.v2-staging`.
    cleanup_staging_lock_dir(&disp_fd);
    let staging_display = dispatch_root(main_worktree).join(TICK_LOCK_STAGING_DIR);
    let c_staging = std::ffi::CString::new(TICK_LOCK_STAGING_DIR)?;
    let mk_st = unsafe { libc::mkdirat(disp_fd.as_raw_fd(), c_staging.as_ptr(), 0o700) };
    if mk_st != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("cannot create staging dir {}", staging_display.display()));
    }
    let st_dir_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_staging.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    if st_dir_raw < 0 {
        let err = std::io::Error::last_os_error();
        cleanup_staging_lock_dir(&disp_fd);
        return Err(err)
            .with_context(|| format!("cannot open staging dir {}", staging_display.display()));
    }
    let staging_dir = unsafe { std::fs::File::from_raw_fd(st_dir_raw) };

    let c_v2 = std::ffi::CString::new(TICK_LOCK_V2_FILE)?;
    let v2_raw = unsafe {
        libc::openat(
            staging_dir.as_raw_fd(),
            c_v2.as_ptr(),
            libc::O_RDWR
                | libc::O_CREAT
                | libc::O_EXCL
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK
                | libc::O_CLOEXEC,
            0o600,
        )
    };
    if v2_raw < 0 {
        let err = std::io::Error::last_os_error();
        drop(staging_dir);
        cleanup_staging_lock_dir(&disp_fd);
        return Err(err).with_context(|| {
            format!(
                "cannot create staging lock {}",
                staging_display.join(TICK_LOCK_V2_FILE).display()
            )
        });
    }
    let v2_file = unsafe { std::fs::File::from_raw_fd(v2_raw) };
    let v2_flock_rc = unsafe { libc::flock(v2_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if v2_flock_rc != 0 {
        let err = std::io::Error::last_os_error();
        drop(v2_file);
        drop(staging_dir);
        cleanup_staging_lock_dir(&disp_fd);
        return Err(err).context("cannot lock staging v2.lock before exchange");
    }

    // Atomically exchange `.tick.lock.v2-staging` (directory with `v2.lock`
    // already locked by us) and `tick.lock` (legacy regular file). There is no
    // `ENOENT` window where `tick.lock` is missing.
    let ex_rc = unsafe {
        libc::renameat2(
            disp_fd.as_raw_fd(),
            c_staging.as_ptr(),
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };
    if ex_rc != 0 {
        let err = std::io::Error::last_os_error();
        let _ = fs2::FileExt::unlock(&v2_file);
        drop(v2_file);
        drop(staging_dir);
        cleanup_staging_lock_dir(&disp_fd);
        return Err(err).with_context(|| {
            format!(
                "cannot atomically exchange {} with staging v2 directory",
                lock_dir_display.display()
            )
        });
    }

    #[cfg(test)]
    if let Some(hook) = TEST_POST_EXCHANGE_HOOK
        .lock()
        .ok()
        .and_then(|mut g| g.take())
    {
        hook();
    }

    // Re-check `/proc` after `RENAME_EXCHANGE` while holding BOTH `leg_file`
    // and `v2_file`. Because `tick.lock` is now a directory, no new old-binary
    // `open("tick.lock", O_RDWR)` can succeed after the exchange. If a process
    // opened `leg_file` in the narrow window immediately before the exchange
    // (or `/proc` coverage is unknown), swap the exact original regular-file
    // inode back to `tick.lock` and unlink the staging `v2.lock` before
    // releasing `v2_file`; a failed rollback is reported, not swallowed.
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    let step = verify_sole_open_fd_for_inode(
        leg_meta.dev(),
        leg_meta.ino(),
        leg_file.as_raw_fd(),
        &lock_dir_display,
    )
    .and_then(|()| write_lock_version_file(&staging_dir, &lock_dir_display));
    if let Err(step_err) = step {
        let rollback = rollback_activation_exchange(&disp_fd, staging_dir, main_worktree);
        let _ = fs2::FileExt::unlock(&v2_file);
        drop(v2_file);
        let _ = fs2::FileExt::unlock(&leg_file);
        drop(leg_file);
        return Err(match rollback {
            Ok(()) => step_err,
            Err(rollback_err) => step_err.context(format!("{rollback_err:#}")),
        });
    }

    // Unlink the swapped-out legacy regular file from `.tick.lock.v2-staging`.
    let un_rc = unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), 0) };
    if un_rc != 0 {
        let un_err = std::io::Error::last_os_error();
        bail!(
            "the dispatch lock is active, but the swapped-out legacy lock file {} could not be removed ({un_err}); remove it by hand",
            staging_display.display()
        );
    }
    let unlinked_meta = leg_file
        .metadata()
        .with_context(|| format!("cannot fstat unlinked {}", lock_dir_display.display()))?;
    if unlinked_meta.nlink() != 0 {
        bail!(
            "legacy {} still has nlink={} after unlink (refused)",
            lock_dir_display.display(),
            unlinked_meta.nlink()
        );
    }

    let _ = fs2::FileExt::unlock(&leg_file);
    drop(leg_file);
    drop(staging_dir);
    Ok(DispatchLock(v2_file))
}

pub(crate) fn acquire_dispatch_lock(main_worktree: &Path) -> Result<DispatchLock> {
    use fs2::FileExt;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let disp_fd = open_pinned_dispatch_subdir(main_worktree, None)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;

    // A fresh `.aida/dispatch/` root gets its `tick.lock` directory (with
    // `version` already inside) atomically. An entry that already exists must
    // be the v2 directory; a legacy regular-file `tick.lock` is NEVER
    // silently unlinked/replaced by ordinary mutations.
    // trace:STORY-1917 trace:TASK-2344 | ai:claude
    create_lock_dir_if_absent(&disp_fd, main_worktree)?;

    let dir_fd_raw = unsafe {
        libc::openat(
            disp_fd.as_raw_fd(),
            c_lock_dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0,
        )
    };
    let (lock_dir, lock_dir_meta) = if dir_fd_raw >= 0 {
        let dir = unsafe { std::fs::File::from_raw_fd(dir_fd_raw) };
        let meta = dir
            .metadata()
            .with_context(|| format!("cannot stat {}", lock_dir_display.display()))?;
        if !meta.is_dir() {
            bail!(
                "{} is not a directory (refused)",
                lock_dir_display.display()
            );
        }
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            bail!(
                "{} is owned by uid {}, not current uid {uid}",
                lock_dir_display.display(),
                meta.uid()
            );
        }
        if (meta.mode() & 0o7777) != 0o700 {
            bail!(
                "{} has mode {:04o}, expected 0700 (refused)",
                lock_dir_display.display(),
                meta.mode() & 0o7777
            );
        }
        (dir, meta)
    } else {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ELOOP) => {
                bail!("{} is a symlink (refused)", lock_dir_display.display());
            }
            Some(libc::ENOTDIR) => {
                bail!(
                    "legacy regular-file dispatch lock {} detected (written by an aida from before the v2 lock); automatic live lock replacement is refused to prevent split-brain with older dispatch processes. To migrate it, {LOCK_ACTIVATION_GUIDANCE}",
                    lock_dir_display.display()
                );
            }
            _ => {
                return Err(err)
                    .with_context(|| format!("cannot open {}", lock_dir_display.display()));
            }
        }
    };

    let v2_display = lock_dir_display.join(TICK_LOCK_V2_FILE);
    let c_v2 = std::ffi::CString::new(TICK_LOCK_V2_FILE)?;
    let v2_fd = unsafe {
        libc::openat(
            lock_dir.as_raw_fd(),
            c_v2.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600,
        )
    };
    if v2_fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ELOOP) {
            bail!("{} is a symlink (refused)", v2_display.display());
        }
        return Err(err).with_context(|| format!("cannot open {}", v2_display.display()));
    }
    let file = unsafe { std::fs::File::from_raw_fd(v2_fd) };
    let meta = file
        .metadata()
        .with_context(|| format!("cannot stat {}", v2_display.display()))?;
    if !meta.is_file() || meta.nlink() != 1 {
        bail!(
            "{} is not a singly-linked regular file (refused)",
            v2_display.display()
        );
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            v2_display.display(),
            meta.uid()
        );
    }
    if (meta.mode() & 0o7777) != 0o600 {
        bail!(
            "{} has mode {:04o}, expected 0600 (refused)",
            v2_display.display(),
            meta.mode() & 0o7777
        );
    }
    #[cfg(test)]
    if let Some(hook) = TEST_PRE_FLOCK_V2_HOOK
        .lock()
        .ok()
        .and_then(|mut g| g.take())
    {
        hook();
    }
    file.lock_exclusive()
        .context("failed to acquire .aida/dispatch/tick.lock/v2.lock")?;

    // Post-flock verification: ensure `v2.lock` was not unlinked and `tick.lock`
    // was not rolled back to the legacy regular file while we waited for `flock`.
    let post_meta = file
        .metadata()
        .with_context(|| format!("cannot stat {} after lock", v2_display.display()))?;
    if post_meta.nlink() != 1 {
        let _ = fs2::FileExt::unlock(&file);
        bail!(
            "{} was unlinked or rolled back while waiting for dispatch lock (refused)",
            v2_display.display()
        );
    }
    let cur_dir_st = fstatat_nofollow(&disp_fd, &c_lock_dir, &lock_dir_display)?;
    if (cur_dir_st.st_mode & libc::S_IFMT) != libc::S_IFDIR
        || cur_dir_st.st_dev != lock_dir_meta.dev()
        || cur_dir_st.st_ino != lock_dir_meta.ino()
    {
        let _ = fs2::FileExt::unlock(&file);
        bail!(
            "{} directory identity changed or rolled back while waiting for dispatch lock (refused)",
            lock_dir_display.display()
        );
    }

    // The marker is never written here: a directory without it is an
    // unfinished activation, and only `aida dispatch activate-lock` may finish
    // it. trace:STORY-1917 trace:TASK-2344 | ai:claude
    if let Err(ver_err) = verify_lock_version_file(&lock_dir, &lock_dir_display) {
        let _ = fs2::FileExt::unlock(&file);
        return Err(ver_err);
    }

    Ok(DispatchLock(file))
}
