//! Typed dispatch requests (`aida dispatch`).
//!
//! A sandboxed advisor seat queues closed, validated requests (`flow`,
//! `review`, `take`, `impl`, `train`) under `<main_worktree>/.aida/dispatch/`.
//! Request write time enforces an active Mandate covering [`Scope::Dispatch`]
//! (plus [`Scope::ShipHeld`] for shipping `flow` and `train`), [`HumanGates`],
//! the `operator-only` / `ExecutionMode::Decide` always-human floor, and a
//! process/session-key-bound `advisor` seat grant ([`grant_check`]).
//!
//! Per the signed SKETCH v1 and ADVISOR SIGNOFF:
//! - No self-binding grant guard: no code path ever appends `proc:` tokens to
//!   an advisor grant outside `aida role enter` / `--bind-pid`.
//! - The cron/schedule `dispatch tick` executes only `take` and `impl`;
//!   `review`, `flow`, and `train` require `aida dispatch serve` started by the
//!   human from an advisor terminal after `eval "$(aida role enter advisor ...)"`,
//!   and `aida dispatch list` surfaces them as `waiting for serve` while
//!   `Pending`.
// trace:STORY-1917 | ai:agy
// mandate-gate

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[path = "dispatch_args.rs"]
pub(crate) mod dispatch_args;
pub(crate) use dispatch_args::*;

#[path = "dispatch_flow.rs"]
pub(crate) mod dispatch_flow;

use crate::gate::seat_authority::{self, GrantCheck, GrantRejection};
use crate::mandate::{self, Mandate, MandateState, Scope};
use crate::mandate_drive::{ActionRow, Outcome};
use crate::mandate_gates::{GateOp, HumanGates};

pub(crate) const DISPATCH_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_BRIEF_BYTES: usize = 64 * 1024;
pub(crate) const MAX_TRAIN_PRS: usize = 16;
pub(crate) const MAX_PR_NUMBER: u64 = 999_999;

const DISPATCH_DIR: &str = "dispatch";
const REQUESTS_SUBDIR: &str = "requests";
const BRIEFS_SUBDIR: &str = "briefs";
const RESULTS_SUBDIR: &str = "results";
const LOGS_SUBDIR: &str = "logs";
const TICK_LOCK_FILE: &str = "tick.lock";
const TICK_LOCK_V2_FILE: &str = "v2.lock";
const TICK_LOCK_VERSION_FILE: &str = "version";
const TICK_LOCK_VERSION_BODY: &[u8] = b"2\n";
const TICK_LOCK_STAGING_DIR: &str = ".tick.lock.v2-staging";
const RECOVERY_FILE: &str = "recovery.json";

#[cfg(test)]
pub(crate) static TEST_PROC_PID_ALLOWLIST: std::sync::Mutex<Option<Vec<u32>>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_POST_EXCHANGE_HOOK: std::sync::Mutex<
    Option<Box<dyn FnOnce() + Send + 'static>>,
> = std::sync::Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_PRE_FLOCK_V2_HOOK: std::sync::Mutex<
    Option<Box<dyn FnOnce() + Send + 'static>>,
> = std::sync::Mutex::new(None);

pub(crate) const RECOVERY_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_RECOVERY_FILE_BYTES: usize = 256 * 1024;
pub(crate) const MAX_DISPATCH_INVENTORY_ENTRIES: usize = 4_096;

/// On-disk request record (`requests/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchRequest {
    pub schema_version: u32,
    pub id: String,
    pub requested_at: DateTime<Utc>,
    pub principal: String,
    pub requester_seat: String,
    pub requester_grant_id: String,
    pub matched_binding: Vec<String>,
    pub mandate_id: String,
    pub dedupe_key: String,
    pub target_key: String,
    pub action: DispatchVerb,
}

/// Closed verbs persisted in [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum DispatchVerb {
    #[serde(alias = "pr-flow")]
    Flow {
        pr: u64,
        #[serde(default)]
        no_ship: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Review {
        pr: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "takeover")]
    Take {
        pr: u64,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    #[serde(alias = "lane")]
    Impl {
        name: String,
        branch: String,
        spec: String,
        brief_relpath: String,
        brief_sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor: Option<DispatchVendor>,
    },
    Train {
        prs: Vec<u64>,
    },
}

impl DispatchVerb {
    pub(crate) fn verb_str(&self) -> &'static str {
        match self {
            Self::Flow { .. } => "flow",
            Self::Review { .. } => "review",
            Self::Take { .. } => "take",
            Self::Impl { .. } => "impl",
            Self::Train { .. } => "train",
        }
    }

    /// True when this verb requires the advisor seat grant at execution time
    /// and therefore runs only under `aida dispatch serve`, never under the
    /// cron/schedule `dispatch tick`.
    // trace:STORY-1917 | ai:agy
    pub(crate) fn requires_serve(&self) -> bool {
        matches!(
            self,
            Self::Flow { .. } | Self::Review { .. } | Self::Train { .. }
        )
    }

    /// True when this verb ships held PRs (`flow` without `--no-ship`, or `train`).
    pub(crate) fn ships_pr(&self) -> bool {
        matches!(self, Self::Flow { no_ship: false, .. } | Self::Train { .. })
    }

    /// True when this verb delegates a reviewer seat (`review`, or `flow`).
    pub(crate) fn delegates_reviewer(&self) -> bool {
        matches!(self, Self::Flow { .. } | Self::Review { .. })
    }

    pub(crate) fn target_key(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("pr:{pr}")
            }
            Self::Impl { name, spec, .. } => {
                format!("lane:{}:spec:{}", name, spec.to_ascii_uppercase())
            }
            Self::Train { .. } => "train".to_string(),
        }
    }

    pub(crate) fn dedupe_key(&self) -> String {
        match self {
            Self::Flow { pr, no_ship, .. } => format!("flow:pr:{pr}:noship:{no_ship}"),
            // Request queueing runs offline/store-less inside a sandboxed
            // advisor seat where forge/network queries for the current PR head
            // OID may be unavailable, so review deduplicates on active
            // (`Pending`/`Running`) requests for `pr:<pr>`; once a review
            // finishes (`Done`/`Failed`/`Refused`/`Cancelled`), a new review
            // request for a newer head queues immediately.
            Self::Review { pr, .. } => format!("review:pr:{pr}"),
            Self::Take {
                pr,
                branch,
                spec,
                brief_sha256,
                ..
            } => format!(
                "take:pr:{pr}:branch:{branch}:spec:{}:brief:{brief_sha256}",
                spec.to_ascii_uppercase()
            ),
            Self::Impl {
                name, branch, spec, ..
            } => format!(
                "impl:name:{name}:branch:{branch}:spec:{}",
                spec.to_ascii_uppercase()
            ),
            Self::Train { prs } => {
                let mut sorted = prs.clone();
                sorted.sort_unstable();
                sorted.dedup();
                let csv = sorted
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!("train:prs:{csv}")
            }
        }
    }

    pub(crate) fn subject_label(&self) -> String {
        match self {
            Self::Flow { pr, .. } | Self::Review { pr, .. } | Self::Take { pr, .. } => {
                format!("PR #{pr}")
            }
            Self::Impl { name, spec, .. } => format!("{name} ({})", spec.to_ascii_uppercase()),
            Self::Train { prs } => {
                let list = prs
                    .iter()
                    .map(|n| format!("#{n}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("PRs {list}")
            }
        }
    }
}

/// Execution status of a dispatch request.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DispatchStatus {
    Pending,
    Running,
    Done,
    Refused,
    Failed,
    Cancelled,
}

impl DispatchStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Refused => "refused",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }

    pub(crate) fn matches_filter(self, filter: DispatchStatusFilter) -> bool {
        matches!(
            (self, filter),
            (Self::Pending, DispatchStatusFilter::Pending)
                | (Self::Running, DispatchStatusFilter::Running)
                | (Self::Done, DispatchStatusFilter::Done)
                | (Self::Refused, DispatchStatusFilter::Refused)
                | (Self::Failed, DispatchStatusFilter::Failed)
                | (Self::Cancelled, DispatchStatusFilter::Cancelled)
        )
    }
}

/// On-disk result record (`results/<id>.json`, mode 0600).
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DispatchResult {
    pub id: String,
    pub verb: String,
    pub target: String,
    pub dedupe_key: String,
    pub status: DispatchStatus,
    pub requested_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub requester_grant_prefix: String,
    pub mandate_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<DateTime<Utc>>,
}

// ── Paths & Private File I/O ────────────────────────────────────────────────

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

fn is_vanished_proc_error(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
        || matches!(err.raw_os_error(), Some(libc::ENOENT | libc::ESRCH))
}

/// Verify via `/proc` that `(dev, ino)` is not open on any file descriptor in
/// the current process or any other process owned by the current UID other than
/// `(std::process::id(), own_fd)`.
///
/// Any `/proc` directory entry error, unreadable `/proc/<pid>` metadata,
/// unreadable `/proc/<pid>/fd` directory for a live same-UID process (e.g.
/// `EACCES` on a non-dumpable or foreign-user-namespace process), or unreadable
/// descriptor metadata on a still-open FD is treated as **unknown coverage**
/// and fails closed (`Err`).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
fn verify_sole_open_fd_for_inode(
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

fn verify_or_init_lock_version_file(
    lock_dir: &std::fs::File,
    lock_dir_display: &Path,
    allow_init_if_missing: bool,
) -> Result<()> {
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
        if err.raw_os_error() == Some(libc::ENOENT) && allow_init_if_missing {
            return write_lock_version_file(lock_dir, lock_dir_display);
        }
        return Err(err).with_context(|| {
            format!(
                "missing or unreadable dispatch lock version marker {}",
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

/// Explicit quiescent migration from a legacy Slice 1/2 regular-file
/// `.aida/dispatch/tick.lock` to the Slice 3 directory `.aida/dispatch/tick.lock/`
/// containing `v2.lock` and `version`.
///
/// Ordinary mutation entrypoints (`acquire_dispatch_lock`) NEVER perform this
/// replacement automatically. This helper:
/// 1. Verifies `lstat` (`fstatat(..., AT_SYMLINK_NOFOLLOW)`) is a singly-linked
///    regular file owned by the current UID with mode `0600`.
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
///    an ordinary `acquire_dispatch_lock` could `mkdir` `tick.lock` or enter `v2`
///    before the second `/proc` scan completes.
/// 7. Re-runs [`verify_sole_open_fd_for_inode`] while holding BOTH `leg_file`
///    and `v2.lock`. If the second scan fails, `RENAME_EXCHANGE` atomically
///    swaps the **exact original regular-file inode `(st_dev, st_ino)`** back to
///    `tick.lock` and unlinks the staging `v2.lock` (`nlink = 0`) before
///    releasing `v2.lock`, so any concurrent `acquire_dispatch_lock` waiter sees
///    `nlink == 0` / rolled-back `tick.lock` and aborts fail-closed.
/// 8. On success, writes `tick.lock/version` (`"2\n"`), unlinks the swapped-out
///    legacy regular file at `.tick.lock.v2-staging` (verifying `nlink == 0`),
///    drops `leg_file`, and returns `DispatchLock(v2_file)`.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn activate_dispatch_v2_lock_quiescent(main_worktree: &Path) -> Result<DispatchLock> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let disp_fd = open_pinned_dispatch_subdir(main_worktree, None)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;

    let pre_st = fstatat_nofollow(&disp_fd, &c_lock_dir, &lock_dir_display)?;
    if (pre_st.st_mode & libc::S_IFMT) == libc::S_IFDIR {
        return acquire_dispatch_lock(main_worktree);
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
    if (pre_st.st_mode & 0o7777) != 0o600 {
        bail!(
            "legacy {} has mode {:04o}, expected 0600 (refused)",
            lock_dir_display.display(),
            pre_st.st_mode & 0o7777
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
            "legacy {} is actively held by another process; refusing concurrent dispatch lock migration (fail-closed)",
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
    // (or `/proc` coverage is unknown), atomically swap the exact original
    // regular-file inode `(st_dev, st_ino)` back to `tick.lock` and unlink the
    // staging `v2.lock` before releasing `v2_file`.
    if let Err(race_err) = verify_sole_open_fd_for_inode(
        leg_meta.dev(),
        leg_meta.ino(),
        leg_file.as_raw_fd(),
        &lock_dir_display,
    ) {
        let _ = unsafe {
            libc::renameat2(
                disp_fd.as_raw_fd(),
                c_staging.as_ptr(),
                disp_fd.as_raw_fd(),
                c_lock_dir.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        unsafe { libc::unlinkat(staging_dir.as_raw_fd(), c_v2.as_ptr(), 0) };
        drop(staging_dir);
        unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), libc::AT_REMOVEDIR) };
        let _ = fs2::FileExt::unlock(&v2_file);
        drop(v2_file);
        let _ = fs2::FileExt::unlock(&leg_file);
        drop(leg_file);
        return Err(race_err);
    }

    if let Err(ver_err) = write_lock_version_file(&staging_dir, &lock_dir_display) {
        let _ = unsafe {
            libc::renameat2(
                disp_fd.as_raw_fd(),
                c_staging.as_ptr(),
                disp_fd.as_raw_fd(),
                c_lock_dir.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        unsafe { libc::unlinkat(staging_dir.as_raw_fd(), c_v2.as_ptr(), 0) };
        drop(staging_dir);
        unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), libc::AT_REMOVEDIR) };
        let _ = fs2::FileExt::unlock(&v2_file);
        drop(v2_file);
        let _ = fs2::FileExt::unlock(&leg_file);
        drop(leg_file);
        return Err(ver_err);
    }

    // Unlink the swapped-out legacy regular file from `.tick.lock.v2-staging`.
    let un_rc = unsafe { libc::unlinkat(disp_fd.as_raw_fd(), c_staging.as_ptr(), 0) };
    if un_rc != 0 {
        let un_err = std::io::Error::last_os_error();
        bail!(
            "cannot unlink swapped legacy {} during quiescent migration: {un_err}",
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

fn acquire_dispatch_lock(main_worktree: &Path) -> Result<DispatchLock> {
    use fs2::FileExt;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let disp_fd = open_pinned_dispatch_subdir(main_worktree, None)?;
    let lock_dir_display = dispatch_root(main_worktree).join(TICK_LOCK_FILE);
    let c_lock_dir = std::ffi::CString::new(TICK_LOCK_FILE)?;

    // Create `tick.lock` as a pinned 0700 directory when initializing a fresh
    // `.aida/dispatch/` root. If an entry already exists at `tick.lock`, we
    // require it to be the v2 directory; a legacy regular-file `tick.lock` is
    // NEVER silently unlinked/replaced by ordinary mutations.
    let mk_rc = unsafe { libc::mkdirat(disp_fd.as_raw_fd(), c_lock_dir.as_ptr(), 0o700) };
    let created_fresh_dir = if mk_rc == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::EEXIST) {
            return Err(err)
                .with_context(|| format!("cannot create {}", lock_dir_display.display()));
        }
        false
    };

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
                    "legacy regular-file dispatch lock {} detected; automatic live lock replacement is refused to prevent split-brain with pre-Slice-3 mutators. Stop/quiesce all dispatch processes and run explicit quiescent lock activation before resuming dispatch mutations",
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

    if let Err(ver_err) = verify_or_init_lock_version_file(
        &lock_dir,
        &lock_dir_display,
        created_fresh_dir || !lock_dir_display.join(TICK_LOCK_VERSION_FILE).exists(),
    ) {
        let _ = fs2::FileExt::unlock(&file);
        return Err(ver_err);
    }

    Ok(DispatchLock(file))
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

/// Open `.aida/dispatch/logs/<id>.log` relative to the pinned `logs` directory
/// descriptor using `O_NOFOLLOW | O_NONBLOCK`, verifying it is a singly-linked
/// (`nlink == 1`) regular file owned by the current uid with mode `0600` and
/// size `<= MAX_DISPATCH_LOG_BYTES`. Never follows symlinks and never replaces
/// a symlink or hardlink.
// trace:STORY-1917 | ai:agy
fn open_pinned_log_file(
    main_worktree: &Path,
    id: &str,
    append_mode: bool,
) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;
    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}` for log file");
    }
    let logs_fd = open_pinned_dispatch_subdir(main_worktree, Some(LOGS_SUBDIR))?;
    let file_name = format!("{id}.log");
    let display_path = logs_dir(main_worktree).join(&file_name);
    let c_name = std::ffi::CString::new(file_name.as_bytes())?;
    let mut flags =
        libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
    if append_mode {
        flags |= libc::O_APPEND;
    }
    let fd = unsafe { libc::openat(logs_fd.as_raw_fd(), c_name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ELOOP) {
            bail!("{} is a symlink (refused)", display_path.display());
        }
        return Err(err).with_context(|| format!("cannot open {}", display_path.display()));
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .with_context(|| format!("cannot stat {}", display_path.display()))?;
    if !meta.is_file() {
        bail!("{} is not a regular file (refused)", display_path.display());
    }
    if meta.nlink() != 1 {
        bail!(
            "{} has {} hard links, expected 1 (refused)",
            display_path.display(),
            meta.nlink()
        );
    }
    let uid = unsafe { libc::geteuid() };
    if meta.uid() != uid {
        bail!(
            "{} is owned by uid {}, not current uid {uid}",
            display_path.display(),
            meta.uid()
        );
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o600 {
        bail!(
            "{} has mode {:04o}, expected 0600 (refused)",
            display_path.display(),
            mode
        );
    }
    if meta.len() > MAX_DISPATCH_LOG_BYTES as u64 {
        bail!(
            "{} exceeds maximum log size ({} bytes > {} bytes)",
            display_path.display(),
            meta.len(),
            MAX_DISPATCH_LOG_BYTES
        );
    }
    // Clear O_NONBLOCK now that we have verified the descriptor is a regular file.
    unsafe {
        let cur = libc::fcntl(file.as_raw_fd(), libc::F_GETFL);
        if cur >= 0 {
            let _ = libc::fcntl(file.as_raw_fd(), libc::F_SETFL, cur & !libc::O_NONBLOCK);
        }
    }
    Ok(file)
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

// ── Dual-End Validation ─────────────────────────────────────────────────────

/// Validate a PR number (`1..=999_999`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_pr(pr: u64) -> Result<u64> {
    if (1..=MAX_PR_NUMBER).contains(&pr) {
        Ok(pr)
    } else {
        bail!("invalid PR number `{pr}`: must be between 1 and {MAX_PR_NUMBER}")
    }
}

/// Validate a `train` PR list (`1..=16` items, each valid, deduplicated in order).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_prs(prs: &[u64]) -> Result<Vec<u64>> {
    if prs.is_empty() {
        bail!("train requires at least 1 PR number");
    }
    if prs.len() > MAX_TRAIN_PRS {
        bail!(
            "train accepts at most {MAX_TRAIN_PRS} PR numbers (got {})",
            prs.len()
        );
    }
    let mut out = Vec::with_capacity(prs.len());
    let mut seen = BTreeSet::new();
    for &pr in prs {
        validate_pr(pr)?;
        if seen.insert(pr) {
            out.push(pr);
        }
    }
    Ok(out)
}

/// Resolve and verify the repository's default branch strictly from local
/// `refs/remotes/origin/HEAD` without network calls, config writes, or
/// conventional-branch (`main`/`master`) fallback (TASK-2341, R2).
///
/// Fails closed if `refs/remotes/origin/HEAD` is missing, unreadable, points
/// outside `refs/remotes/origin/*`, or its target ref is dangling or does not
/// resolve to a commit object.
// trace:TASK-2341 | ai:agy
pub(crate) fn resolve_verified_local_default_branch(main_worktree: &Path) -> Result<String> {
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
        .output()
        .with_context(|| {
            format!(
                "cannot read local `refs/remotes/origin/HEAD` in {}",
                main_worktree.display()
            )
        })?;
    if !out.status.success() {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (symbolic-ref is missing or unreadable); set or refresh it locally with `git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/<default>` or `git remote set-head origin -a`"
        );
    }
    let raw = std::str::from_utf8(&out.stdout).map_err(|_| {
        anyhow::anyhow!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (non-UTF-8 symbolic-ref target)"
        )
    })?;
    let symref = raw.trim_end_matches(['\r', '\n']);
    let Some(default_branch) = symref.strip_prefix("refs/remotes/origin/") else {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (points to non-origin target `{symref}`); set it to `refs/remotes/origin/<default>`"
        );
    };
    if default_branch.is_empty()
        || default_branch == "HEAD"
        || default_branch.starts_with('/')
        || default_branch.ends_with('/')
        || default_branch.contains("..")
        || default_branch.contains("//")
        || default_branch
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (malformed default branch `{default_branch}`)"
        );
    }

    let target_commit_spec = format!("refs/remotes/origin/{default_branch}^{{commit}}");
    let rev_out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["rev-parse", "--verify", "--quiet", &target_commit_spec])
        .output()
        .with_context(|| {
            format!(
                "cannot verify `{target_commit_spec}` in {}",
                main_worktree.display()
            )
        })?;
    let commit_sha = std::str::from_utf8(&rev_out.stdout)
        .ok()
        .map(str::trim)
        .unwrap_or("");
    if !rev_out.status.success()
        || commit_sha.is_empty()
        || !commit_sha.chars().all(|c| c.is_ascii_hexdigit())
    {
        bail!(
            "cannot verify repository default branch from local `refs/remotes/origin/HEAD` (`refs/remotes/origin/{default_branch}` is missing, dangling, or does not resolve to a commit); refresh local remote refs or run `git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/<default>`"
        );
    }

    Ok(default_branch.to_string())
}

/// Validate a branch name (`^[A-Za-z0-9._/-]{1,80}$` plus git check-ref-format
/// rules, forbidding `main`, `master`, `HEAD`, and the repository's verified
/// local default branch from `refs/remotes/origin/HEAD`).
// trace:STORY-1917 trace:TASK-2341 | ai:agy
pub(crate) fn validate_branch(main_worktree: &Path, branch: &str) -> Result<String> {
    let b = branch.trim();
    if b.is_empty() || b.len() > 80 {
        bail!("invalid branch `{branch}`: length must be 1..=80 characters");
    }
    if matches!(b, "main" | "master" | "HEAD") {
        bail!("invalid branch `{b}`: cannot target `{b}` directly");
    }
    if b.starts_with('/')
        || b.ends_with('/')
        || b.starts_with('.')
        || b.ends_with('.')
        || b.starts_with('-')
    {
        bail!("invalid branch `{b}`: cannot start or end with `/`, `.`, or `-`");
    }
    if b.contains("..") || b.contains("//") || b.contains("@{") || b.ends_with(".lock") {
        bail!("invalid branch `{b}`: contains forbidden git ref sequence");
    }
    for segment in b.split('/') {
        if segment.is_empty()
            || segment.starts_with('.')
            || segment.ends_with('.')
            || segment.ends_with(".lock")
        {
            bail!("invalid branch `{b}`: invalid path component `{segment}`");
        }
    }
    if !b
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        bail!("invalid branch `{b}`: only `[A-Za-z0-9._/-]` characters are allowed");
    }
    let default_branch = resolve_verified_local_default_branch(main_worktree)?;
    if b == default_branch {
        bail!("invalid branch `{b}`: cannot target the repository default branch `{default_branch}` directly");
    }
    Ok(b.to_string())
}

/// Validate an `impl` lane / worktree name (`^[A-Za-z0-9._-]{1,60}$`, no `/`,
/// no leading `.` or `-`, no `..`).
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_lane_name(name: &str) -> Result<String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 60 {
        bail!("invalid lane name `{name}`: length must be 1..=60 characters");
    }
    if n.starts_with('.') || n.starts_with('-') || n.ends_with('.') || n.contains("..") {
        bail!("invalid lane name `{n}`: cannot start with `.`/`-` or contain `..`");
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        bail!("invalid lane name `{n}`: only `[A-Za-z0-9._-]` characters are allowed (no `/`)");
    }
    Ok(n.to_string())
}

/// Validate a spec ID syntax (`^[A-Z][A-Z0-9]*-(?:[0-9]+-)?[0-9]{1,6}$`),
/// confirm it exists in the project's store, and verify it is not
/// `operator-only` or `ExecutionMode::Decide` (in the current store or sticky
/// exclusion ledger). Also checks `include_supervised` when the active mandate
/// excludes supervised specs.
// trace:STORY-1917 | ai:agy
pub(crate) fn validate_spec_in_store(
    main_worktree: &Path,
    mandate_dir: &Path,
    raw_spec: &str,
    include_supervised: bool,
) -> Result<String> {
    let spec = raw_spec.trim().to_ascii_uppercase();
    let Some((prefix, rest)) = spec.split_once('-') else {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    };
    if prefix.is_empty()
        || !prefix.chars().next().unwrap().is_ascii_uppercase()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        bail!("invalid spec ID `{raw_spec}`: invalid prefix `{prefix}`");
    }
    let valid_number = match rest.split_once('-') {
        None => !rest.is_empty() && rest.len() <= 6 && rest.chars().all(|c| c.is_ascii_digit()),
        Some((node, seq)) => {
            !node.is_empty()
                && node.len() <= 4
                && node.chars().all(|c| c.is_ascii_digit())
                && !seq.is_empty()
                && seq.len() <= 6
                && seq.chars().all(|c| c.is_ascii_digit())
        }
    };
    if !valid_number {
        bail!("invalid spec ID `{raw_spec}`: expected `<PREFIX>-<NUMBER>`");
    }

    let ledger = mandate::read_exclusions(mandate_dir)
        .map_err(|why| anyhow::anyhow!("mandate exclusion ledger is unreadable: {why}"))?;
    if let Some(entry) = ledger.get(&spec) {
        if entry.always_human {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger and always requires a human",
                entry.class
            );
        }
        if !include_supervised {
            bail!(
                "spec `{spec}` is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                entry.class
            );
        }
    }

    let store = crate::load_store_for_lookup(main_worktree)
        .with_context(|| format!("cannot load requirement store to validate `{spec}`"))?;
    let Some(req) = store.requirements.iter().find(|r| {
        r.spec_id
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
            || r.agreed_id
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&spec))
    }) else {
        bail!("spec `{spec}` was not found in the requirement store");
    };

    // Record any exclusions seen on this requirement into the sticky ledger.
    mandate::note_exclusions(mandate_dir, &spec, std::iter::once(req));

    if let Some(class) = mandate::always_human_class(req) {
        bail!("spec `{spec}` is `{class}` and always requires a human");
    }
    for alias in [req.spec_id.as_deref(), req.agreed_id.as_deref()]
        .into_iter()
        .flatten()
    {
        let key = alias.trim().to_ascii_uppercase();
        if let Some(entry) = ledger.get(&key) {
            if entry.always_human {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger and always requires a human",
                    entry.class
                );
            }
            if !include_supervised {
                bail!(
                    "spec `{spec}` (`{key}`) is recorded as `{}` in the exclusion ledger; start the mandate with `--include-supervised` to cover it",
                    entry.class
                );
            }
        }
    }
    if !include_supervised {
        if let Some(why) = mandate::excluded_class(req) {
            bail!(
                "spec `{spec}` is in a supervised class ({why}); start the mandate with `--include-supervised` to cover it"
            );
        }
    }
    Ok(spec)
}

/// Read, validate, and freeze `--brief <path>` into `.aida/dispatch/briefs/<id>.md`
/// (mode 0600). Returns `(brief_relpath, brief_sha256)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn freeze_brief(
    main_worktree: &Path,
    request_id: &str,
    source_path: &Path,
) -> Result<(String, String)> {
    let bytes = read_bounded_regular_file(source_path, MAX_BRIEF_BYTES, false)?;
    if bytes.is_empty() {
        bail!("brief file {} is empty", source_path.display());
    }
    if bytes.contains(&0) {
        bail!("brief file {} contains NUL bytes", source_path.display());
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("brief file {} is not valid UTF-8", source_path.display()))?;
    if text.trim().is_empty() {
        bail!("brief file {} is blank", source_path.display());
    }
    let digest = sha256_hex(&bytes);
    let relpath = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    let dest = brief_path(main_worktree, request_id);
    write_private_atomic(&dest, &bytes)?;
    Ok((relpath, digest))
}

/// Execution-time re-validation of a frozen brief file on disk (`O_NOFOLLOW |
/// O_NONBLOCK`, mode 0600, owned by current uid, `<= 64 KiB`, non-empty UTF-8,
/// SHA-256 matches `expected_sha256`).
// trace:STORY-1917 | ai:agy
pub(crate) fn verify_frozen_brief(
    main_worktree: &Path,
    request_id: &str,
    brief_relpath: &str,
    expected_sha256: &str,
) -> Result<String> {
    let expected_rel = format!("{BRIEFS_SUBDIR}/{request_id}.md");
    if brief_relpath != expected_rel {
        bail!("invalid brief_relpath `{brief_relpath}` (expected `{expected_rel}`)");
    }
    if expected_sha256.len() != 64
        || !expected_sha256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("invalid brief_sha256 `{expected_sha256}`");
    }
    let path = brief_path(main_worktree, request_id);
    let bytes = read_bounded_regular_file(&path, MAX_BRIEF_BYTES, true)?;
    if bytes.is_empty() || bytes.contains(&0) {
        bail!(
            "frozen brief {} is empty or contains NUL bytes",
            path.display()
        );
    }
    let actual_sha = sha256_hex(&bytes);
    if actual_sha != expected_sha256 {
        bail!(
            "frozen brief {} SHA-256 mismatch (expected {expected_sha256}, got {actual_sha})",
            path.display()
        );
    }
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("frozen brief {} is not valid UTF-8", path.display()))?;
    Ok(text.to_string())
}

/// Execution-time re-validation of an on-disk [`DispatchRequest`].
// trace:STORY-1917 | ai:agy
pub(crate) fn revalidate_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    req: &DispatchRequest,
    include_supervised: bool,
) -> Result<()> {
    if req.schema_version != DISPATCH_SCHEMA_VERSION {
        bail!(
            "unsupported dispatch request schema_version {} (expected {})",
            req.schema_version,
            DISPATCH_SCHEMA_VERSION
        );
    }
    if Uuid::parse_str(&req.id).is_err() {
        bail!("invalid dispatch request id `{}`", req.id);
    }
    if req.requester_seat != mandate::MANDATE_SEAT {
        bail!(
            "dispatch request `{}` has requester_seat `{}`, expected `{}`",
            req.id,
            req.requester_seat,
            mandate::MANDATE_SEAT
        );
    }
    if req.matched_binding.is_empty() {
        bail!("dispatch request `{}` has an empty matched_binding", req.id);
    }
    match &req.action {
        DispatchVerb::Flow { pr, .. } | DispatchVerb::Review { pr, .. } => {
            validate_pr(*pr)?;
        }
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_pr(*pr)?;
            validate_branch(main_worktree, branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            ..
        } => {
            validate_lane_name(name)?;
            validate_branch(main_worktree, branch)?;
            validate_spec_in_store(main_worktree, mandate_dir, spec, include_supervised)?;
            verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)?;
        }
        DispatchVerb::Train { prs } => {
            validate_prs(prs)?;
        }
    }
    let expected_target = req.action.target_key();
    if req.target_key != expected_target {
        bail!(
            "dispatch request `{}` target_key `{}` does not match `{expected_target}`",
            req.id,
            req.target_key
        );
    }
    let expected_dedupe = req.action.dedupe_key();
    if req.dedupe_key != expected_dedupe {
        bail!(
            "dispatch request `{}` dedupe_key `{}` does not match `{expected_dedupe}`",
            req.id,
            req.dedupe_key
        );
    }
    Ok(())
}

/// Render the `take` or `impl` prompt wrapper from the embedded template in
/// `aida-core/templates/dispatch/`.
// trace:STORY-1917 | ai:agy
pub(crate) fn render_lane_prompt(
    main_worktree: &Path,
    worktree: &Path,
    action: &DispatchVerb,
    brief_body: &str,
    now: DateTime<Utc>,
) -> Result<String> {
    let ts = now.format("%Y-%m-%d %H:%M").to_string();
    match action {
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/take-prompt.md")
                .context("missing embedded template dispatch/take-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{PR}}", &pr.to_string())
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        DispatchVerb::Impl {
            branch,
            spec,
            vendor,
            ..
        } => {
            let tpl = aida_core::templates::TemplateLoader::with_project_root(main_worktree)
                .load("dispatch/impl-prompt.md")
                .context("missing embedded template dispatch/impl-prompt.md")?;
            let vendor_str = vendor.unwrap_or(DispatchVendor::Claude).as_str();
            Ok(tpl
                .replace("{{WORKTREE}}", &worktree.display().to_string())
                .replace("{{MAIN_WORKTREE}}", &main_worktree.display().to_string())
                .replace("{{BRANCH}}", branch)
                .replace("{{SPEC}}", spec)
                .replace("{{VENDOR}}", vendor_str)
                .replace("{{TIMESTAMP}}", &ts)
                .replace("{{BRIEF}}", brief_body.trim()))
        }
        other => bail!(
            "verb `{}` does not use a lane prompt template",
            other.verb_str()
        ),
    }
}

// ── Authority Verification at Request Write Time ────────────────────────────

/// Validated caller & mandate context for `aida dispatch request`.
#[derive(Debug, Clone)]
pub(crate) struct AuthorizedDispatchCaller {
    pub mandate: Mandate,
    pub grant_id: String,
    pub matched_binding: Vec<String>,
}

/// Check Mandate (`Scope::Dispatch` + `Scope::ShipHeld` when shipping),
/// `HumanGates` (`GateOp::Dispatch` + `GateOp::Ship`), and `advisor` seat grant
/// (bound to the calling process or its inherited session keyring).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_request_authority(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<AuthorizedDispatchCaller> {
    let (valid, _) = mandate::load_valid(mandate_dir, principal, now);
    let mandate = match mandate::state_at(&valid, now) {
        MandateState::None => {
            bail!(
                "no active mandate; start one at a human terminal with `aida mandate start --ship-held --scope dispatch` (or `--drive`)"
            )
        }
        MandateState::Expired(m) => {
            bail!(
                "mandate {} expired at {}; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id,
                m.expires_at.to_rfc3339()
            )
        }
        MandateState::Stopped(m) => {
            bail!(
                "mandate {} was stopped; start a new one with `aida mandate start --ship-held --scope dispatch`",
                m.id
            )
        }
        MandateState::Active(m) => m,
    };

    // trace:TASK-2340 | ai:agy
    if !mandate.covers(Scope::Dispatch) {
        let msg = format!(
            "active mandate {} does not cover `dispatch`; stop it and start one with `aida mandate start --ship-held --scope dispatch` (or `--drive`)",
            mandate.id
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let ships_pr = matches!(
        verb,
        DispatchRequestVerb::Flow { no_ship: false, .. } | DispatchRequestVerb::Train { .. }
    );
    if ships_pr && !mandate.covers(Scope::ShipHeld) {
        let msg = format!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            mandate.id,
            match verb {
                DispatchRequestVerb::Train { .. } => "train",
                _ => "flow",
            }
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let gates = crate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let msg = format!("dispatch is gated by human gates ({why})");
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Gated,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }
    if ships_pr {
        if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let msg = format!("shipping is gated by human gates ({why})");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Gated,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    }

    let grant = match seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(grant) => grant,
        GrantCheck::Absent => {
            let msg = "dispatch requests require a bound `advisor` seat grant (no session grant is set); enter the role before starting the advisor session with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`".to_string();
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let fix = match reason {
                GrantRejection::Unbound
                | GrantRejection::PresenterNotBound { .. }
                | GrantRejection::ForeignNamespace { .. }
                | GrantRejection::KeyringUnavailable { .. } => {
                    "A `--bind-pid` grant cannot present inside a bwrap tool call because the PID namespace hides the harness PID and no session key was inherited. Start the advisor session after running `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in the launching terminal so the kernel session keyring (`key:<sha256>`) is inherited into the sandbox."
                }
                _ => "Re-enter the advisor seat at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`.",
            };
            let seat_str = seat.as_deref().unwrap_or("unknown");
            let msg = format!("advisor seat grant {id} ({seat_str}) was rejected: {reason}. {fix}");
            emit_dispatch_event(
                main_worktree,
                &mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    };

    if crate::canonical_role_name(&grant.seat) != mandate::MANDATE_SEAT {
        let msg = format!(
            "dispatch requests require the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id, grant.seat
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    if grant.principal.trim() != mandate.principal.trim() {
        let msg = format!(
            "advisor grant principal `{}` does not match mandate principal `{}`",
            grant.principal, mandate.principal
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let delegates_reviewer = matches!(
        verb,
        DispatchRequestVerb::Flow { .. } | DispatchRequestVerb::Review { .. }
    );
    if delegates_reviewer
        && !grant
            .delegable_seats
            .iter()
            .any(|s| crate::canonical_role_name(s) == "reviewer")
    {
        let msg = format!(
            "advisor grant {} does not delegate the `reviewer` seat required by `{}`; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id,
            verb_name_of(verb)
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    let presenter = seat_authority::presenter_tokens_for(&grant);
    let matched_binding = seat_authority::matched_binding(&grant, &presenter);
    if matched_binding.is_empty() {
        let msg = format!(
            "advisor grant {} has no presenter binding matching this process; start the advisor session after `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`",
            grant.id
        );
        emit_dispatch_event(
            main_worktree,
            &mandate.id,
            verb_name_of(verb),
            &verb_subject_of(verb),
            Outcome::Refused,
            Some(msg.clone()),
        );
        bail!("{msg}");
    }

    Ok(AuthorizedDispatchCaller {
        mandate,
        grant_id: grant.id,
        matched_binding,
    })
}

fn verb_name_of(verb: &DispatchRequestVerb) -> &'static str {
    match verb {
        DispatchRequestVerb::Flow { .. } => "flow",
        DispatchRequestVerb::Review { .. } => "review",
        DispatchRequestVerb::Take { .. } => "take",
        DispatchRequestVerb::Impl { .. } => "impl",
        DispatchRequestVerb::Train { .. } => "train",
    }
}

fn verb_subject_of(verb: &DispatchRequestVerb) -> String {
    match verb {
        DispatchRequestVerb::Flow { pr, .. }
        | DispatchRequestVerb::Review { pr, .. }
        | DispatchRequestVerb::Take { pr, .. } => format!("PR #{pr}"),
        DispatchRequestVerb::Impl { name, spec, .. } => {
            format!("{name} ({})", spec.trim().to_ascii_uppercase())
        }
        DispatchRequestVerb::Train { prs } => {
            let list = prs
                .iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("PRs {list}")
        }
    }
}

fn emit_dispatch_event(
    main_worktree: &Path,
    mandate_id: &str,
    verb: &str,
    subject: &str,
    outcome: Outcome,
    why: Option<String>,
) {
    let row = ActionRow::new(&format!("dispatch:{verb}"), subject, verb, outcome, why);
    crate::mandate_drive::emit_action(main_worktree, mandate_id, "session", &row);
}

// ── Store Operations (Submit / List / Show / Cancel) ────────────────────────

/// One non-runnable (`needs-recovery`) record in `.aida/dispatch/recovery.json`
/// (TASK-2344, R4 + Q1).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryFenceEntry {
    pub id: String,
    pub reason: String,
    pub evidence_fingerprint: String,
    pub detected_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_verb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_target_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_dedupe_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_mandate_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_requested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub blocks_all_admission: bool,
}

/// Private versioned recovery index persisted at `.aida/dispatch/recovery.json`
/// (mode 0600, `#[serde(deny_unknown_fields)]`, bounded size/entry count).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryIndex {
    pub schema_version: u32,
    pub updated_at: DateTime<Utc>,
    pub entries: Vec<RecoveryFenceEntry>,
}

/// Shared bounded dispatch inventory combining valid `CompletePair` records and
/// `RecoveryRequired` fences (`fences` + optional `global_block_reason`).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
#[derive(Debug, Clone, Default)]
pub(crate) struct DispatchInventory {
    pub pairs: Vec<(DispatchRequest, DispatchResult)>,
    pub fences: Vec<RecoveryFenceEntry>,
    pub global_block_reason: Option<String>,
}

impl DispatchInventory {
    pub(crate) fn find_fence(&self, id_or_prefix: &str) -> Option<&RecoveryFenceEntry> {
        let needle = id_or_prefix.trim();
        if needle.is_empty() {
            return None;
        }
        self.fences
            .iter()
            .find(|f| f.id == needle || f.id.starts_with(needle))
    }

    /// Check whether a new request submission `(id, dedupe_key, target_key)` is
    /// blocked by a global inventory error or a `RecoveryRequired` fence.
    pub(crate) fn check_submission_fence(
        &self,
        id: &str,
        dedupe_key: &str,
        target_key: &str,
    ) -> Result<()> {
        if let Some(reason) = &self.global_block_reason {
            bail!(
                "dispatch inventory is in `needs-recovery` state and refuses new admissions: {reason}"
            );
        }
        for fence in &self.fences {
            if fence.blocks_all_admission {
                bail!(
                    "dispatch record {} is in `needs-recovery` state with an unknown target/verb ({}); resolve or remove the broken record before submitting new dispatch requests",
                    fence.id,
                    fence.reason
                );
            }
            if fence.id == id
                || fence.known_dedupe_key.as_deref() == Some(dedupe_key)
                || fence.known_target_key.as_deref() == Some(target_key)
            {
                bail!(
                    "target `{target_key}` has a dispatch record {} in `needs-recovery` state ({}); automatic replay or replacement is refused until the broken record is resolved",
                    fence.id,
                    fence.reason
                );
            }
        }
        Ok(())
    }

    /// Check whether claiming/running new work is blocked globally by an unsafe
    /// or unknown-verb `RecoveryRequired` fence, and return `(review_slots,
    /// flow_slots, train_slots)` consumed by active `Running` records PLUS
    /// known-verb `RecoveryRequired` records (without double-counting).
    pub(crate) fn active_and_fenced_cap_usage(
        &self,
        exclude_id: Option<&str>,
    ) -> Result<(usize, usize, usize)> {
        if let Some(reason) = &self.global_block_reason {
            bail!(
                "dispatch inventory is in `needs-recovery` state and refuses new execution claims: {reason}"
            );
        }
        let mut reviews = 0usize;
        let mut flows = 0usize;
        let mut trains = 0usize;
        for (req, res) in &self.pairs {
            if exclude_id == Some(req.id.as_str()) {
                continue;
            }
            if res.status == DispatchStatus::Running {
                match req.action {
                    DispatchVerb::Review { .. } => reviews += 1,
                    DispatchVerb::Flow { .. } => flows += 1,
                    DispatchVerb::Train { .. } => trains += 1,
                    DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
                }
            }
        }
        for fence in &self.fences {
            if exclude_id == Some(fence.id.as_str()) {
                continue;
            }
            if fence.blocks_all_admission {
                bail!(
                    "dispatch record {} is in `needs-recovery` state with an unknown verb/target ({}); refusing new execution claims until resolved",
                    fence.id,
                    fence.reason
                );
            }
            match fence.known_verb.as_deref() {
                Some("review") => reviews += 1,
                Some("flow") => flows += 1,
                Some("train") => trains += 1,
                Some("take") | Some("impl") => {}
                _ => {
                    bail!(
                        "dispatch record {} is in `needs-recovery` state with unknown verb ({}); refusing new execution claims until resolved",
                        fence.id,
                        fence.reason
                    );
                }
            }
        }
        Ok((reviews, flows, trains))
    }
}

enum HalfRead<T> {
    Missing,
    Valid(T),
    Corrupt(String),
}

fn read_json_half<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> HalfRead<T> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return HalfRead::Corrupt(format!("{} is a symlink (refused)", path.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HalfRead::Missing,
        Err(e) => return HalfRead::Corrupt(format!("cannot stat {}: {e}", path.display())),
    }
    match read_bounded_regular_file(path, MAX_BRIEF_BYTES, true) {
        Ok(bytes) => match serde_json::from_slice::<T>(&bytes) {
            Ok(val) => HalfRead::Valid(val),
            Err(err) => HalfRead::Corrupt(format!(
                "malformed {label} JSON ({}): {err}",
                path.display()
            )),
        },
        Err(err) => HalfRead::Corrupt(format!("{err:#}")),
    }
}

fn read_existing_recovery_index(
    main_worktree: &Path,
) -> std::result::Result<Option<RecoveryIndex>, String> {
    let path = recovery_path(main_worktree);
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!("{} is a symlink (refused)", path.display()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot stat {}: {e}", path.display())),
    }
    let bytes = read_bounded_regular_file(&path, MAX_RECOVERY_FILE_BYTES, true)
        .map_err(|e| format!("invalid recovery index {}: {e:#}", path.display()))?;
    let idx: RecoveryIndex = serde_json::from_slice(&bytes)
        .map_err(|e| format!("malformed recovery index {}: {e}", path.display()))?;
    if idx.schema_version != RECOVERY_SCHEMA_VERSION {
        return Err(format!(
            "unsupported recovery index schema_version {} in {} (expected {})",
            idx.schema_version,
            path.display(),
            RECOVERY_SCHEMA_VERSION
        ));
    }
    if idx.entries.len() > MAX_DISPATCH_INVENTORY_ENTRIES {
        return Err(format!(
            "recovery index {} exceeds maximum entry count ({})",
            path.display(),
            MAX_DISPATCH_INVENTORY_ENTRIES
        ));
    }
    Ok(Some(idx))
}

fn scan_uuid_dir_entries(
    dir_path: &Path,
    stems: &mut BTreeSet<String>,
    anomalies: &mut Vec<RecoveryFenceEntry>,
    now: DateTime<Utc>,
) -> std::result::Result<(), String> {
    match std::fs::symlink_metadata(dir_path) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            return Err(format!(
                "{} is a symlink or not a directory (refused)",
                dir_path.display()
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("cannot stat {}: {e}", dir_path.display())),
    }
    let entries = std::fs::read_dir(dir_path)
        .map_err(|e| format!("cannot read directory {}: {e}", dir_path.display()))?;
    let mut count = 0usize;
    for entry in entries {
        count += 1;
        if count > MAX_DISPATCH_INVENTORY_ENTRIES {
            return Err(format!(
                "directory {} exceeds maximum entry limit ({MAX_DISPATCH_INVENTORY_ENTRIES})",
                dir_path.display()
            ));
        }
        let entry =
            entry.map_err(|e| format!("unreadable entry in {}: {e}", dir_path.display()))?;
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            let reason = format!("non-UTF-8 entry in {}", dir_path.display());
            anomalies.push(RecoveryFenceEntry {
                id: format!(
                    "invalid-entry:{}",
                    sha256_hex(file_name.as_encoded_bytes())[..12].to_string()
                ),
                evidence_fingerprint: sha256_hex(reason.as_bytes()),
                reason,
                detected_at: now,
                known_verb: None,
                known_target_key: None,
                known_dedupe_key: None,
                known_mandate_id: None,
                known_requested_at: None,
                blocks_all_admission: true,
            });
            continue;
        };
        if name_str.starts_with('.') {
            // Temporary atomic write files (`..<name>.tmp-<uuid>`) are ignored
            // unless someone planted a symlink or subdirectory.
            if let Ok(m) = std::fs::symlink_metadata(&path) {
                if m.file_type().is_symlink() || m.is_dir() {
                    let reason = format!(
                        "unexpected hidden symlink or directory `{name_str}` in {}",
                        dir_path.display()
                    );
                    anomalies.push(RecoveryFenceEntry {
                        id: format!("invalid-entry:{name_str}"),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: None,
                        known_target_key: None,
                        known_dedupe_key: None,
                        known_mandate_id: None,
                        known_requested_at: None,
                        blocks_all_admission: true,
                    });
                }
            }
            continue;
        }
        let stem_opt = path.file_stem().and_then(|s| s.to_str());
        let ext_opt = path.extension().and_then(|s| s.to_str());
        if let (Some(stem), Some("json")) = (stem_opt, ext_opt) {
            if Uuid::parse_str(stem).is_ok() {
                stems.insert(stem.to_string());
                continue;
            }
        }
        let reason = format!(
            "unexpected non-UUID or non-JSON entry `{name_str}` in {}",
            dir_path.display()
        );
        anomalies.push(RecoveryFenceEntry {
            id: format!("invalid-entry:{name_str}"),
            evidence_fingerprint: sha256_hex(reason.as_bytes()),
            reason,
            detected_at: now,
            known_verb: None,
            known_target_key: None,
            known_dedupe_key: None,
            known_mandate_id: None,
            known_requested_at: None,
            blocks_all_admission: true,
        });
    }
    Ok(())
}

/// Inspect `.aida/dispatch/` without mutating disk or printing stderr warnings
/// (used by read-only `list`, `show`, `load_all_pairs`, and `pending_request_count`).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn inspect_dispatch_inventory_readonly(
    main_worktree: &Path,
) -> Result<DispatchInventory> {
    let now = Utc::now();
    let mut inv = DispatchInventory::default();

    let existing_recovery = match read_existing_recovery_index(main_worktree) {
        Ok(idx) => idx,
        Err(err) => {
            inv.global_block_reason = Some(err.clone());
            inv.fences.push(RecoveryFenceEntry {
                id: "recovery-index".to_string(),
                evidence_fingerprint: sha256_hex(err.as_bytes()),
                reason: err,
                detected_at: now,
                known_verb: None,
                known_target_key: None,
                known_dedupe_key: None,
                known_mandate_id: None,
                known_requested_at: None,
                blocks_all_admission: true,
            });
            None
        }
    };
    let mut prior_by_id: std::collections::BTreeMap<String, RecoveryFenceEntry> =
        std::collections::BTreeMap::new();
    if let Some(idx) = existing_recovery {
        for entry in idx.entries {
            prior_by_id.insert(entry.id.clone(), entry);
        }
    }

    let mut stems = BTreeSet::new();
    for prior_id in prior_by_id.keys() {
        if Uuid::parse_str(prior_id).is_ok() {
            stems.insert(prior_id.clone());
        }
    }
    let mut dir_anomalies = Vec::new();
    if let Err(err) = scan_uuid_dir_entries(
        &requests_dir(main_worktree),
        &mut stems,
        &mut dir_anomalies,
        now,
    ) {
        inv.global_block_reason = Some(err.clone());
        return Ok(inv);
    }
    if let Err(err) = scan_uuid_dir_entries(
        &results_dir(main_worktree),
        &mut stems,
        &mut dir_anomalies,
        now,
    ) {
        inv.global_block_reason = Some(err.clone());
        return Ok(inv);
    }
    for mut anomaly in dir_anomalies {
        if let Some(prev) = prior_by_id.get(&anomaly.id) {
            if prev.evidence_fingerprint == anomaly.evidence_fingerprint {
                anomaly.detected_at = prev.detected_at;
            }
        }
        inv.fences.push(anomaly);
    }

    for id in stems {
        let req_half: HalfRead<DispatchRequest> =
            read_json_half(&request_path(main_worktree, &id), "request");
        let res_half: HalfRead<DispatchResult> =
            read_json_half(&result_path(main_worktree, &id), "result");

        let fence_opt: Option<RecoveryFenceEntry> = match (req_half, res_half) {
            (HalfRead::Valid(req), HalfRead::Valid(res)) => {
                if req.id != id || res.id != id {
                    let reason = format!(
                        "mismatched inner id on dispatch pair {id} (request.id={}, result.id={})",
                        req.id, res.id
                    );
                    Some(RecoveryFenceEntry {
                        id: id.clone(),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: Some(req.action.verb_str().to_string()),
                        known_target_key: Some(req.target_key),
                        known_dedupe_key: Some(req.dedupe_key),
                        known_mandate_id: Some(req.mandate_id),
                        known_requested_at: Some(req.requested_at),
                        blocks_all_admission: true,
                    })
                } else if req.action.verb_str() != res.verb
                    || req.target_key != res.target
                    || req.dedupe_key != res.dedupe_key
                {
                    let reason = format!(
                        "mismatched request/result metadata for {id} (req={}/{}/{}, res={}/{}/{})",
                        req.action.verb_str(),
                        req.target_key,
                        req.dedupe_key,
                        res.verb,
                        res.target,
                        res.dedupe_key
                    );
                    Some(RecoveryFenceEntry {
                        id: id.clone(),
                        evidence_fingerprint: sha256_hex(reason.as_bytes()),
                        reason,
                        detected_at: now,
                        known_verb: None,
                        known_target_key: None,
                        known_dedupe_key: None,
                        known_mandate_id: Some(req.mandate_id),
                        known_requested_at: Some(req.requested_at),
                        blocks_all_admission: true,
                    })
                } else if let Some(prev) = prior_by_id.get(&id) {
                    // Once a record has been fenced in `recovery.json`, it stays
                    // fenced until explicit operator recovery removes the fence.
                    Some(prev.clone())
                } else {
                    inv.pairs.push((req, res));
                    None
                }
            }
            (HalfRead::Valid(req), HalfRead::Missing) => {
                let valid_id = req.id == id
                    && req.target_key == req.action.target_key()
                    && req.dedupe_key == req.action.dedupe_key();
                let reason = format!(
                    "result file results/{id}.json is missing for request {id} ({} {}); automatic replay is refused (needs-recovery)",
                    req.action.verb_str(),
                    req.target_key
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_id.then(|| req.action.verb_str().to_string()),
                    known_target_key: valid_id.then(|| req.target_key),
                    known_dedupe_key: valid_id.then(|| req.dedupe_key),
                    known_mandate_id: Some(req.mandate_id),
                    known_requested_at: Some(req.requested_at),
                    blocks_all_admission: !valid_id,
                })
            }
            (HalfRead::Missing, HalfRead::Valid(res)) => {
                let valid_verb = matches!(
                    res.verb.as_str(),
                    "flow" | "review" | "take" | "impl" | "train"
                );
                let valid_meta = res.id == id
                    && valid_verb
                    && !res.target.is_empty()
                    && !res.dedupe_key.is_empty();
                let reason = format!(
                    "request file requests/{id}.json is missing for result {id} (status: {}, target: {}); automatic cleanup or rewrite is refused (needs-recovery)",
                    res.status.as_str(),
                    res.target
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_meta.then(|| res.verb),
                    known_target_key: valid_meta.then(|| res.target),
                    known_dedupe_key: valid_meta.then(|| res.dedupe_key),
                    known_mandate_id: Some(res.mandate_id),
                    known_requested_at: Some(res.requested_at),
                    blocks_all_admission: !valid_meta,
                })
            }
            (HalfRead::Valid(req), HalfRead::Corrupt(res_err)) => {
                let valid_id = req.id == id
                    && req.target_key == req.action.target_key()
                    && req.dedupe_key == req.action.dedupe_key();
                let reason = format!(
                    "corrupt result file for request {id} ({} {}): {res_err} (needs-recovery)",
                    req.action.verb_str(),
                    req.target_key
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: valid_id.then(|| req.action.verb_str().to_string()),
                    known_target_key: valid_id.then(|| req.target_key),
                    known_dedupe_key: valid_id.then(|| req.dedupe_key),
                    known_mandate_id: Some(req.mandate_id),
                    known_requested_at: Some(req.requested_at),
                    blocks_all_admission: !valid_id,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Valid(res)) => {
                // Corrupt request means we cannot verify the canonical request
                // definition; block new admissions while still reporting the
                // result's verb/target for diagnostics.
                let reason = format!(
                    "corrupt request file for result {id} (status: {}): {req_err} (needs-recovery)",
                    res.status.as_str()
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: Some(res.verb),
                    known_target_key: Some(res.target),
                    known_dedupe_key: Some(res.dedupe_key),
                    known_mandate_id: Some(res.mandate_id),
                    known_requested_at: Some(res.requested_at),
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Missing) => {
                let reason =
                    format!("corrupt orphan request file {id}: {req_err} (needs-recovery)");
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Missing, HalfRead::Corrupt(res_err)) => {
                let reason = format!("corrupt orphan result file {id}: {res_err} (needs-recovery)");
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Corrupt(req_err), HalfRead::Corrupt(res_err)) => {
                let reason = format!(
                    "corrupt request and result files for {id}: {req_err}; {res_err} (needs-recovery)"
                );
                Some(RecoveryFenceEntry {
                    id: id.clone(),
                    evidence_fingerprint: sha256_hex(reason.as_bytes()),
                    reason,
                    detected_at: now,
                    known_verb: None,
                    known_target_key: None,
                    known_dedupe_key: None,
                    known_mandate_id: None,
                    known_requested_at: None,
                    blocks_all_admission: true,
                })
            }
            (HalfRead::Missing, HalfRead::Missing) => prior_by_id.get(&id).cloned(),
        };

        if let Some(mut fence) = fence_opt {
            if let Some(prev) = prior_by_id.get(&id) {
                if prev.evidence_fingerprint == fence.evidence_fingerprint {
                    fence.detected_at = prev.detected_at;
                }
                if fence.known_verb.is_none() {
                    fence.known_verb = prev.known_verb.clone();
                }
                if fence.known_target_key.is_none() {
                    fence.known_target_key = prev.known_target_key.clone();
                }
                if fence.known_dedupe_key.is_none() {
                    fence.known_dedupe_key = prev.known_dedupe_key.clone();
                }
                if fence.known_mandate_id.is_none() {
                    fence.known_mandate_id = prev.known_mandate_id.clone();
                }
                if fence.known_requested_at.is_none() {
                    fence.known_requested_at = prev.known_requested_at;
                }
                fence.blocks_all_admission |= prev.blocks_all_admission;
            }
            inv.fences.push(fence);
        }
    }

    inv.pairs
        .sort_by(|(a, _), (b, _)| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
    inv.fences.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(inv)
}

/// Reconcile `.aida/dispatch/` under `tick.lock`: persists `.aida/dispatch/recovery.json`
/// (mode 0600) and emits one audit event per newly detected `(id, evidence_fingerprint)`.
/// Never deletes or rewrites broken request/result files and never replays orphans.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn reconcile_inventory_locked(
    main_worktree: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchInventory> {
    let prior = read_existing_recovery_index(main_worktree).ok().flatten();
    let mut prior_fingerprints = BTreeSet::new();
    if let Some(idx) = &prior {
        for e in &idx.entries {
            prior_fingerprints.insert((e.id.clone(), e.evidence_fingerprint.clone()));
        }
    }

    let inv = inspect_dispatch_inventory_readonly(main_worktree)?;
    if inv.global_block_reason.is_none() {
        for fence in &inv.fences {
            let key = (fence.id.clone(), fence.evidence_fingerprint.clone());
            if !prior_fingerprints.contains(&key) {
                emit_dispatch_event(
                    main_worktree,
                    fence.known_mandate_id.as_deref().unwrap_or(""),
                    fence.known_verb.as_deref().unwrap_or("recovery"),
                    fence.known_target_key.as_deref().unwrap_or(&fence.id),
                    Outcome::Refused,
                    Some(fence.reason.clone()),
                );
            }
        }
        let rec_path = recovery_path(main_worktree);
        if inv.fences.is_empty() {
            if rec_path.exists() {
                let _ = std::fs::remove_file(&rec_path);
            }
        } else {
            let idx = RecoveryIndex {
                schema_version: RECOVERY_SCHEMA_VERSION,
                updated_at: now,
                entries: inv.fences.clone(),
            };
            write_private_atomic(&rec_path, &serde_json::to_vec_pretty(&idx)?)?;
        }
    }
    Ok(inv)
}

/// True when an active Mandate record on disk covers [`Scope::Dispatch`]
/// (used by the `mandate.dispatch_active` schedule predicate, R5).
// trace:STORY-1917 | ai:agy
pub(crate) fn dispatch_record_active(now: DateTime<Utc>) -> bool {
    let Ok(dir) = mandate::mandate_dir() else {
        return false;
    };
    let principal = crate::current_user_id(None);
    let (valid, _) = mandate::load_valid(&dir, &principal, now);
    matches!(
        mandate::state_at(&valid, now),
        MandateState::Active(m) if m.covers(Scope::Dispatch)
    )
}

/// Count of validated `Pending` dispatch requests in `.aida/dispatch/`
/// (used by the `dispatch.pending` schedule predicate, R5).
// trace:STORY-1917 | ai:agy
pub(crate) fn pending_request_count(main_worktree: &Path) -> i64 {
    let Ok(inv) = inspect_dispatch_inventory_readonly(main_worktree) else {
        return 0;
    };
    inv.pairs
        .iter()
        .filter(|(_, res)| res.status == DispatchStatus::Pending)
        .count() as i64
}

/// Clean up only the exact unpublished files created by the current
/// `submit_request` invocation while still holding `tick.lock` BEFORE
/// `requests/<id>.json` is published (TASK-2344, R4).
fn cleanup_unpublished_submission_locked(
    main_worktree: &Path,
    request_id: &str,
    wrote_brief: bool,
    wrote_result: bool,
) {
    if request_path(main_worktree, request_id).exists() {
        return;
    }
    if wrote_result {
        let res_p = result_path(main_worktree, request_id);
        if let HalfRead::Valid(res) = read_json_half::<DispatchResult>(&res_p, "result") {
            if res.id == request_id
                && res.status == DispatchStatus::Pending
                && res.worker_pid.is_none()
                && res.claim_token.is_none()
            {
                let _ = std::fs::remove_file(&res_p);
            }
        }
    }
    if wrote_brief {
        let _ = std::fs::remove_file(brief_path(main_worktree, request_id));
    }
}

/// Outcome of `submit_request`: either a newly created request or an existing
/// active (`Pending`/`Running`) request with the same `dedupe_key`.
#[derive(Debug, Clone)]
pub(crate) struct SubmittedDispatch {
    pub request: DispatchRequest,
    pub result: DispatchResult,
    pub deduplicated: bool,
}

/// Validate and persist a dispatch request under `.aida/dispatch/` using
/// result-first publication (`briefs/<id>.md` -> `results/<id>.json` ->
/// `requests/<id>.json`) under `tick.lock` (TASK-2344, R4).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn submit_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    principal: &str,
    verb: &DispatchRequestVerb,
    now: DateTime<Utc>,
) -> Result<SubmittedDispatch> {
    let caller = check_request_authority(main_worktree, mandate_dir, principal, verb, now)?;
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;

    let request_id = Uuid::new_v4().to_string();
    let mut wrote_brief = false;
    let action_res: Result<DispatchVerb> = (|| match verb {
        DispatchRequestVerb::Flow {
            pr,
            no_ship,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Flow {
                pr,
                no_ship: *no_ship,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Review { pr, vendor } => {
            let pr = validate_pr(*pr)?;
            Ok(DispatchVerb::Review {
                pr,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Take {
            pr,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let pr = validate_pr(*pr)?;
            let branch = validate_branch(main_worktree, branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate.include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            wrote_brief = true;
            Ok(DispatchVerb::Take {
                pr,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Impl {
            name,
            branch,
            spec,
            brief,
            vendor,
        } => {
            let name = validate_lane_name(name)?;
            let branch = validate_branch(main_worktree, branch)?;
            let spec = validate_spec_in_store(
                main_worktree,
                mandate_dir,
                spec,
                caller.mandate.include_supervised,
            )?;
            let (brief_relpath, brief_sha256) = freeze_brief(main_worktree, &request_id, brief)?;
            wrote_brief = true;
            Ok(DispatchVerb::Impl {
                name,
                branch,
                spec,
                brief_relpath,
                brief_sha256,
                vendor: *vendor,
            })
        }
        DispatchRequestVerb::Train { prs } => {
            let prs = validate_prs(prs)?;
            Ok(DispatchVerb::Train { prs })
        }
    })();

    let action = match action_res {
        Ok(action) => action,
        Err(err) => {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            emit_dispatch_event(
                main_worktree,
                &caller.mandate.id,
                verb_name_of(verb),
                &verb_subject_of(verb),
                Outcome::Refused,
                Some(format!("{err:#}")),
            );
            return Err(err);
        }
    };

    let dedupe_key = action.dedupe_key();
    let target_key = action.target_key();

    // Check RecoveryRequired fences BEFORE deduplication or target admission (TASK-2344, R4).
    if let Err(err) = inv.check_submission_fence(&request_id, &dedupe_key, &target_key) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        emit_dispatch_event(
            main_worktree,
            &caller.mandate.id,
            action.verb_str(),
            &action.subject_label(),
            Outcome::Refused,
            Some(format!("{err:#}")),
        );
        return Err(err);
    }

    // Check existing active requests for idempotency or target collision.
    for (existing_req, existing_res) in inv.pairs {
        if !existing_res.status.is_active() {
            continue;
        }
        if existing_req.dedupe_key == dedupe_key {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            return Ok(SubmittedDispatch {
                request: existing_req,
                result: existing_res,
                deduplicated: true,
            });
        }
        if existing_req.target_key == target_key {
            cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
            let msg = format!(
                "target `{target_key}` already has an active dispatch request {} ({}, status: {}); cancel it first with `aida dispatch cancel {}`",
                existing_req.id,
                existing_req.action.verb_str(),
                existing_res.status.as_str(),
                existing_req.id
            );
            emit_dispatch_event(
                main_worktree,
                &caller.mandate.id,
                action.verb_str(),
                &action.subject_label(),
                Outcome::Refused,
                Some(msg.clone()),
            );
            bail!("{msg}");
        }
    }

    let req = DispatchRequest {
        schema_version: DISPATCH_SCHEMA_VERSION,
        id: request_id.clone(),
        requested_at: now,
        principal: principal.trim().to_string(),
        requester_seat: mandate::MANDATE_SEAT.to_string(),
        requester_grant_id: caller.grant_id.clone(),
        matched_binding: caller.matched_binding,
        mandate_id: caller.mandate.id.clone(),
        dedupe_key: dedupe_key.clone(),
        target_key: target_key.clone(),
        action: action.clone(),
    };

    // Verify dual-end validator agrees with what we are about to write.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        &req,
        caller.mandate.include_supervised,
    ) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        bail!("internal validation check failed: {err:#}");
    }

    let grant_prefix: String = caller.grant_id.chars().take(8).collect();
    let summary = if action.requires_serve() {
        format!(
            "queued {} {} (waiting for serve)",
            action.verb_str(),
            action.subject_label()
        )
    } else {
        format!("queued {} {}", action.verb_str(), action.subject_label())
    };
    let res = DispatchResult {
        id: request_id.clone(),
        verb: action.verb_str().to_string(),
        target: target_key,
        dedupe_key,
        status: DispatchStatus::Pending,
        requested_at: now,
        started_at: None,
        finished_at: None,
        requester_grant_prefix: grant_prefix,
        mandate_id: caller.mandate.id.clone(),
        worker_pid: None,
        worker_start: None,
        claim_token: None,
        spawned_session: None,
        exit_code: None,
        summary: summary.clone(),
        refusal_reason: None,
        log_path: format!("{LOGS_SUBDIR}/{request_id}.log"),
        notified_at: None,
    };

    let res_bytes = serde_json::to_vec_pretty(&res)?;
    let req_bytes = serde_json::to_vec_pretty(&req)?;

    #[cfg(test)]
    if TEST_FAIL_SUBMIT_STAGE.with(|c| c.get()) == 1 {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        bail!("simulated submit failure before result write");
    }

    // 1. Write `results/<id>.json` (`Pending`) FIRST so no published
    //    `requests/<id>.json` can ever exist without its companion result file.
    if let Err(err) = write_private_atomic(&result_path(main_worktree, &request_id), &res_bytes) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, false);
        return Err(err);
    }

    #[cfg(test)]
    if TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE.with(|c| c.get()) {
        // Simulate an abrupt process crash after writing `results/<id>.json`
        // before `requests/<id>.json` is written (skipping same-invocation
        // cleanup so the next invocation sees an orphan Pending result).
        bail!("simulated hard crash after result write before request write");
    }

    #[cfg(test)]
    if TEST_FAIL_SUBMIT_STAGE.with(|c| c.get()) == 2 {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, true);
        bail!("simulated submit failure after result write before request write");
    }

    // 2. Publish `requests/<id>.json`. If this write fails within the same
    //    invocation under `_lock`, roll back the just-written `results/<id>.json`
    //    and `briefs/<id>.md`.
    if let Err(err) = write_private_atomic(&request_path(main_worktree, &request_id), &req_bytes) {
        cleanup_unpublished_submission_locked(main_worktree, &request_id, wrote_brief, true);
        return Err(err);
    }

    emit_dispatch_event(
        main_worktree,
        &caller.mandate.id,
        action.verb_str(),
        &action.subject_label(),
        Outcome::Routed,
        Some(summary),
    );

    Ok(SubmittedDispatch {
        request: req,
        result: res,
        deduplicated: false,
    })
}

/// Read one `(DispatchRequest, DispatchResult)` pair by exact ID.
// trace:STORY-1917 | ai:agy
pub(crate) fn load_pair(
    main_worktree: &Path,
    id: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }
    let req_bytes =
        read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let req: DispatchRequest = serde_json::from_slice(&req_bytes)
        .with_context(|| format!("malformed request JSON for {id}"))?;
    if req.id != id {
        bail!("request file {id}.json has mismatched id `{}`", req.id);
    }
    let res_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let res: DispatchResult = serde_json::from_slice(&res_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if res.id != id {
        bail!("result file {id}.json has mismatched id `{}`", res.id);
    }
    Ok((req, res))
}

/// Resolve an exact UUID or unique prefix to `(DispatchRequest, DispatchResult)`.
// trace:STORY-1917 | ai:agy
pub(crate) fn resolve_pair(
    main_worktree: &Path,
    id_or_prefix: &str,
) -> Result<(DispatchRequest, DispatchResult)> {
    let needle = id_or_prefix.trim();
    if Uuid::parse_str(needle).is_ok() {
        return load_pair(main_worktree, needle);
    }
    let mut matches: Vec<(DispatchRequest, DispatchResult)> = load_all_pairs(main_worktree)?
        .into_iter()
        .filter(|(req, _)| req.id.starts_with(needle))
        .collect();
    match matches.len() {
        0 => bail!("no dispatch request matching `{needle}`"),
        1 => Ok(matches.remove(0)),
        _ => bail!("ambiguous dispatch request prefix `{needle}`"),
    }
}

/// Load all valid `(DispatchRequest, DispatchResult)` pairs sorted by
/// `requested_at` ascending (then `id`) without mutating disk or spamming
/// stderr warnings (TASK-2344, R4).
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn load_all_pairs(
    main_worktree: &Path,
) -> Result<Vec<(DispatchRequest, DispatchResult)>> {
    Ok(inspect_dispatch_inventory_readonly(main_worktree)?.pairs)
}

/// Cancel a `Pending` dispatch request.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn cancel_request(
    main_worktree: &Path,
    id_or_prefix: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    if let Some(fence) = inv.find_fence(id_or_prefix) {
        bail!(
            "cannot cancel dispatch request {}: record is in `needs-recovery` state ({})",
            fence.id,
            fence.reason
        );
    }
    let (req, mut res) = resolve_pair(main_worktree, id_or_prefix)?;
    if res.status != DispatchStatus::Pending {
        bail!(
            "cannot cancel dispatch request {}: status is `{}` (only `pending` requests can be cancelled)",
            req.id,
            res.status.as_str()
        );
    }
    res.status = DispatchStatus::Cancelled;
    res.finished_at = Some(now);
    res.summary = format!(
        "cancelled {} {}",
        req.action.verb_str(),
        req.action.subject_label()
    );
    write_private_atomic(
        &result_path(main_worktree, &req.id),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id,
        req.action.verb_str(),
        &req.action.subject_label(),
        Outcome::Skipped,
        Some("cancelled".to_string()),
    );
    Ok(res)
}

/// Human-facing status label for `aida dispatch list`: per ADVISOR SIGNOFF,
/// `Pending` requests for `review`, `flow`, and `train` display as
/// `"pending (waiting for serve)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn display_status(req: &DispatchRequest, res: &DispatchResult) -> String {
    if res.status == DispatchStatus::Pending && req.action.requires_serve() {
        "pending (waiting for serve)".to_string()
    } else {
        res.status.as_str().to_string()
    }
}

pub(crate) fn format_list_text(pairs: &[(DispatchRequest, DispatchResult)]) -> String {
    format_inventory_list_text(pairs, &[])
}

pub(crate) fn format_inventory_list_text(
    pairs: &[(DispatchRequest, DispatchResult)],
    fences: &[RecoveryFenceEntry],
) -> String {
    if pairs.is_empty() && fences.is_empty() {
        return "No dispatch requests.\n".to_string();
    }
    let mut out = String::new();
    for (req, res) in pairs {
        let short_id: String = req.id.chars().take(8).collect();
        let status = display_status(req, res);
        let ts = req.requested_at.format("%Y-%m-%d %H:%M:%S");
        out.push_str(&format!(
            "{short_id}  {:<28}  {:<6}  {:<18}  {}  {}\n",
            status,
            req.action.verb_str(),
            req.target_key,
            ts,
            res.summary
        ));
    }
    for fence in fences {
        let short_id: String = fence.id.chars().take(8).collect();
        let verb = fence.known_verb.as_deref().unwrap_or("unknown");
        let target = fence.known_target_key.as_deref().unwrap_or("unknown");
        let ts = fence
            .known_requested_at
            .unwrap_or(fence.detected_at)
            .format("%Y-%m-%d %H:%M:%S");
        out.push_str(&format!(
            "{short_id}  {:<28}  {:<6}  {:<18}  {}  {}\n",
            "needs-recovery", verb, target, ts, fence.reason
        ));
    }
    out
}

pub(crate) fn list_json(pairs: &[(DispatchRequest, DispatchResult)]) -> serde_json::Value {
    inventory_list_json(pairs, &[])
}

pub(crate) fn inventory_list_json(
    pairs: &[(DispatchRequest, DispatchResult)],
    fences: &[RecoveryFenceEntry],
) -> serde_json::Value {
    let mut rows: Vec<serde_json::Value> = pairs
        .iter()
        .map(|(req, res)| {
            serde_json::json!({
                "id": req.id,
                "verb": req.action.verb_str(),
                "target": req.target_key,
                "dedupe_key": req.dedupe_key,
                "status": res.status.as_str(),
                "display_status": display_status(req, res),
                "waiting_for_serve": res.status == DispatchStatus::Pending && req.action.requires_serve(),
                "requested_at": req.requested_at.to_rfc3339(),
                "started_at": res.started_at.map(|t| t.to_rfc3339()),
                "finished_at": res.finished_at.map(|t| t.to_rfc3339()),
                "mandate_id": req.mandate_id,
                "requester_grant_prefix": res.requester_grant_prefix,
                "summary": res.summary,
                "refusal_reason": res.refusal_reason,
                "request": req,
                "result": res,
            })
        })
        .collect();
    for fence in fences {
        rows.push(serde_json::json!({
            "id": fence.id,
            "verb": fence.known_verb.as_deref().unwrap_or("unknown"),
            "target": fence.known_target_key.as_deref().unwrap_or("unknown"),
            "dedupe_key": fence.known_dedupe_key,
            "status": "needs-recovery",
            "display_status": "needs-recovery",
            "waiting_for_serve": false,
            "requested_at": fence.known_requested_at.unwrap_or(fence.detected_at).to_rfc3339(),
            "started_at": serde_json::Value::Null,
            "finished_at": serde_json::Value::Null,
            "mandate_id": fence.known_mandate_id,
            "requester_grant_prefix": serde_json::Value::Null,
            "summary": fence.reason,
            "refusal_reason": fence.reason,
            "recovery": fence,
        }));
    }
    serde_json::Value::Array(rows)
}

// ── CLI Entrypoint ──────────────────────────────────────────────────────────

pub(crate) fn handle_dispatch(action: &DispatchAction) -> Result<()> {
    let main_worktree = crate::find_main_worktree_root()
        .or_else(|_| std::env::current_dir())
        .context("cannot locate main worktree root")?;
    let mandate_dir = mandate::mandate_dir()?;
    let now = Utc::now();

    match action {
        DispatchAction::Request { verb, json } => {
            let principal = crate::current_user_id(None);
            let submitted = submit_request(&main_worktree, &mandate_dir, &principal, verb, now)?;
            if *json {
                let payload = serde_json::json!({
                    "deduplicated": submitted.deduplicated,
                    "waiting_for_serve": submitted.result.status == DispatchStatus::Pending
                        && submitted.request.action.requires_serve(),
                    "display_status": display_status(&submitted.request, &submitted.result),
                    "request": submitted.request,
                    "result": submitted.result,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(());
            }
            if submitted.deduplicated {
                println!(
                    "already active: {} ({})",
                    submitted.request.id,
                    display_status(&submitted.request, &submitted.result)
                );
            } else {
                println!(
                    "queued: {} ({} {} — {})",
                    submitted.request.id,
                    submitted.request.action.verb_str(),
                    submitted.request.action.subject_label(),
                    display_status(&submitted.request, &submitted.result)
                );
            }
            Ok(())
        }
        DispatchAction::List { status, json } => {
            let inv = inspect_dispatch_inventory_readonly(&main_worktree)?;
            let mut pairs = inv.pairs;
            let fences = if status.is_none() {
                inv.fences
            } else {
                Vec::new()
            };
            if let Some(filter) = status {
                pairs.retain(|(_, res)| res.status.matches_filter(*filter));
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&inventory_list_json(&pairs, &fences))?
                );
            } else {
                print!("{}", format_inventory_list_text(&pairs, &fences));
            }
            Ok(())
        }
        DispatchAction::Show { id, json } => {
            let inv = inspect_dispatch_inventory_readonly(&main_worktree)?;
            if let Some(fence) = inv.find_fence(id) {
                if *json {
                    let payload = serde_json::json!({
                        "display_status": "needs-recovery",
                        "waiting_for_serve": false,
                        "recovery": fence,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    println!("id:             {}", fence.id);
                    println!(
                        "verb:           {}",
                        fence.known_verb.as_deref().unwrap_or("unknown")
                    );
                    println!(
                        "target:         {}",
                        fence.known_target_key.as_deref().unwrap_or("unknown")
                    );
                    println!("status:         needs-recovery");
                    println!("detected_at:    {}", fence.detected_at.to_rfc3339());
                    println!("summary:        {}", fence.reason);
                }
                return Ok(());
            }
            let (req, res) = resolve_pair(&main_worktree, id)?;
            if *json {
                let payload = serde_json::json!({
                    "display_status": display_status(&req, &res),
                    "waiting_for_serve": res.status == DispatchStatus::Pending && req.action.requires_serve(),
                    "request": req,
                    "result": res,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("id:             {}", req.id);
                println!("verb:           {}", req.action.verb_str());
                println!("target:         {}", req.target_key);
                println!("dedupe_key:     {}", req.dedupe_key);
                println!("status:         {}", display_status(&req, &res));
                println!("requested_at:   {}", req.requested_at.to_rfc3339());
                if let Some(started) = res.started_at {
                    println!("started_at:     {}", started.to_rfc3339());
                }
                if let Some(finished) = res.finished_at {
                    println!("finished_at:    {}", finished.to_rfc3339());
                }
                println!("mandate_id:     {}", req.mandate_id);
                println!("grant_prefix:   {}", res.requester_grant_prefix);
                println!("summary:        {}", res.summary);
                if let Some(reason) = &res.refusal_reason {
                    println!("refusal_reason: {reason}");
                }
                println!("log_path:       {}", res.log_path);
            }
            Ok(())
        }
        DispatchAction::Cancel { id } => {
            let res = cancel_request(&main_worktree, id, now)?;
            println!("cancelled: {} ({})", res.id, res.summary);
            Ok(())
        }
        DispatchAction::Tick => {
            let summary = run_dispatch_tick(&main_worktree, &mandate_dir, now)?;
            if summary.spawned > 0 || summary.reaped > 0 {
                println!(
                    "dispatch tick: spawned={}, reaped={}, waiting_for_serve={}",
                    summary.spawned, summary.reaped, summary.waiting_for_serve
                );
            }
            Ok(())
        }
        DispatchAction::Serve { once, poll_ms } => {
            run_dispatch_serve(&main_worktree, &mandate_dir, *once, *poll_ms)
        }
        DispatchAction::Run { id, via_serve } => {
            let res = execute_one_request(&main_worktree, &mandate_dir, id, *via_serve, now)?;
            println!("{}: {} ({})", res.status.as_str(), res.id, res.summary);
            Ok(())
        }
    }
}

// ── Slice 2: Executor Core, Serve, and Review / Take / Impl Runners ─────────

pub(crate) const MAX_CONCURRENT_REVIEWS: usize = 2;
pub(crate) const MAX_CONCURRENT_FLOWS: usize = 4;
pub(crate) const MAX_CONCURRENT_TRAINS: usize = 1;
pub(crate) const MAX_DISPATCH_LOG_BYTES: usize = 1024 * 1024;
pub(crate) const STALE_GIT_CONFIG_LOCK_AGE_SECS: u64 = 120;

/// Summary of one `dispatch tick` or `dispatch serve` claim/reap pass.
// trace:STORY-1917 | ai:agy
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DispatchPassSummary {
    pub reaped: usize,
    pub spawned: usize,
    pub claimed_ids: Vec<String>,
    pub waiting_for_serve: usize,
}

/// Safe stale `.git/config.lock` cleanup: unlinks `<main_worktree>/.git/config.lock`
/// ONLY when `symlink_metadata` confirms a regular file (never a symlink),
/// `len() == 0`, mode `0o444`, and `mtime >= 120s` in the past relative to `now`.
/// Returns `Ok(true)` when a stale lock was removed, `Ok(false)` otherwise.
// trace:STORY-1917 | ai:agy
pub(crate) fn clean_stale_git_config_lock(
    main_worktree: &Path,
    now: std::time::SystemTime,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let lock_path = main_worktree.join(".git").join("config.lock");
    let meta = match std::fs::symlink_metadata(&lock_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("cannot stat {}", lock_path.display())),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Ok(false);
    }
    if meta.len() != 0 {
        return Ok(false);
    }
    if (meta.mode() & 0o777) != 0o444 {
        return Ok(false);
    }
    let Ok(mtime) = meta.modified() else {
        return Ok(false);
    };
    let Ok(age) = now.duration_since(mtime) else {
        return Ok(false);
    };
    if age < std::time::Duration::from_secs(STALE_GIT_CONFIG_LOCK_AGE_SECS) {
        return Ok(false);
    }
    match std::fs::remove_file(&lock_path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("cannot unlink {}", lock_path.display())),
    }
}

/// Send one best-effort mailbox notification to `advisor` when a request
/// reaches a terminal state (`Done`, `Refused`, or `Failed`), and return the
/// notification timestamp if delivery succeeded.
// trace:STORY-1917 | ai:agy
pub(crate) fn notify_advisor_outcome(
    main_worktree: &Path,
    req: &DispatchRequest,
    res: &DispatchResult,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    use aida_core::mailbox::{Intent, Message, Recipient};
    let subject = format!(
        "dispatch {} {}: {} ({})",
        req.action.verb_str(),
        req.action.subject_label(),
        res.status.as_str(),
        &req.id[..8.min(req.id.len())]
    );
    let body = match &res.refusal_reason {
        Some(reason) => format!(
            "Dispatch request {} ({} {}) {}: {}\nreason: {}\nlog: {}",
            req.id,
            req.action.verb_str(),
            req.action.subject_label(),
            res.status.as_str(),
            res.summary,
            reason,
            res.log_path
        ),
        None => format!(
            "Dispatch request {} ({} {}) {}: {}\nlog: {}",
            req.id,
            req.action.verb_str(),
            req.action.subject_label(),
            res.status.as_str(),
            res.summary,
            res.log_path
        ),
    };
    let msg_id = Uuid::new_v4().to_string();
    let msg = Message {
        id: msg_id.clone(),
        thread_id: msg_id,
        from: "dispatch".to_string(),
        to: Recipient::Agent("advisor".to_string()),
        timestamp: now.timestamp_millis(),
        in_reply_to: None,
        body,
        subject: Some(subject),
        urgent: matches!(res.status, DispatchStatus::Refused | DispatchStatus::Failed),
        intent: Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: Some("dispatch".to_string()),
        relayed_from: None,
    };
    if crate::mailbox_store::write_message(main_worktree, &msg).is_ok() {
        crate::events::emit(
            main_worktree,
            &crate::events::Event::new(
                None,
                "",
                crate::events::EventKind::MailReceived {
                    to: "advisor".to_string(),
                },
            ),
        );
        Some(now)
    } else {
        None
    }
}

pub(crate) const DISPATCH_CLAIM_TOKEN_ENV: &str = "AIDA_DISPATCH_CLAIM_TOKEN";

/// Fenced worker claim owning an in-flight `Running` dispatch record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerClaim {
    pub(crate) worker_pid: u32,
    pub(crate) worker_start: Option<String>,
    pub(crate) claim_token: String,
}

/// Append bounded log text to `.aida/dispatch/logs/<id>.log` (mode 0600, capped
/// at [`MAX_DISPATCH_LOG_BYTES`]) through a pinned, no-follow regular-file
/// descriptor so symlinks and hardlinks can neither disclose outside files nor
/// redirect writes (F1).
// trace:STORY-1917 | ai:agy
pub(crate) fn append_dispatch_log(main_worktree: &Path, id: &str, text: &str) -> Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = open_pinned_log_file(main_worktree, id, false)?;
    file.seek(SeekFrom::Start(0))?;
    let mut existing = Vec::new();
    (&file)
        .take((MAX_DISPATCH_LOG_BYTES as u64) + 1)
        .read_to_end(&mut existing)
        .with_context(|| format!("cannot read log for {id}"))?;
    if existing.len() > MAX_DISPATCH_LOG_BYTES {
        bail!("log for {id} exceeds maximum size ({MAX_DISPATCH_LOG_BYTES} bytes)");
    }
    existing.extend_from_slice(text.as_bytes());
    if existing.len() > MAX_DISPATCH_LOG_BYTES {
        let drop_len = existing.len() - MAX_DISPATCH_LOG_BYTES;
        existing.drain(..drop_len);
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&existing)?;
    file.sync_all()?;
    Ok(())
}

/// Verify that `aida dispatch serve` is running with a valid, bound `advisor`
/// seat grant (`GrantCheck::Valid` with `seat == "advisor"`).
///
/// Per ADVISOR SIGNOFF: `serve` never self-binds or mutates
/// `grant.binding`; it inherits the advisor terminal's `proc:`/`sid:`/`key:`
/// binding the ordinary way. If the grant is absent, rejected, expired, or
/// revoked, `serve` refuses to start (or stops mid-run and leaves unclaimed
/// requests `Pending`).
// trace:STORY-1917 | ai:agy
pub(crate) fn check_serve_grant(
    main_worktree: &Path,
    now: DateTime<Utc>,
) -> Result<seat_authority::SeatGrant> {
    let grant = match seat_authority::grant_check_at(main_worktree, now) {
        GrantCheck::Valid(g) => g,
        GrantCheck::Absent => {
            bail!(
                "`aida dispatch serve` requires a bound `advisor` seat grant (no session grant is set); enter the role at your terminal with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` before starting `aida dispatch serve`"
            )
        }
        GrantCheck::Rejected { id, seat, reason } => {
            let seat_str = seat.as_deref().unwrap_or("unknown");
            bail!(
                "`aida dispatch serve` advisor seat grant {id} ({seat_str}) is not valid: {reason}; re-enter with `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"`"
            )
        }
    };
    if crate::canonical_role_name(&grant.seat) != mandate::MANDATE_SEAT {
        bail!(
            "`aida dispatch serve` requires the `advisor` seat grant, but current grant {} holds seat `{}`",
            grant.id,
            grant.seat
        );
    }
    let presenter = seat_authority::presenter_tokens_for(&grant);
    if seat_authority::matched_binding(&grant, &presenter).is_empty() {
        bail!(
            "`aida dispatch serve` grant {} has no presenter binding matching this process; run `eval \"$(aida role enter advisor --delegate-seat reviewer --delegate-seat implementer)\"` in this terminal first",
            grant.id
        );
    }
    Ok(grant)
}

/// Tri-state worker identity probe (`Alive` / `Dead` / `Unknown`, TASK-2344 R4).
///
/// Reaping a `Running` worker requires positive `Probe::Dead` evidence
/// (`/proc/<pid>/stat` is `NotFound`, process state is exited/zombie `Z`/`X`, or
/// field-22 `starttime` ticks differ from the recorded `linux-starttime:<ticks>`).
/// Missing `worker_pid`, missing/unrecognized `worker_start`, or `/proc` read
/// errors (`EACCES`, etc.) return `Probe::Unknown` so callers retain the claim
/// fail-closed rather than guessing.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
pub(crate) fn probe_worker_identity(
    worker_pid: Option<u32>,
    worker_start: Option<&str>,
) -> aida_core::process_probe::Probe {
    use aida_core::process_probe::Probe;

    #[cfg(test)]
    if let Some(forced) = TEST_FORCE_WORKER_PROBE.with(|c| c.borrow().clone()) {
        return forced;
    }

    let Some(pid) = worker_pid.filter(|&p| p > 0) else {
        return Probe::Unknown("worker_pid is missing or zero".to_string());
    };
    let Some(raw_start) = worker_start.map(str::trim).filter(|s| !s.is_empty()) else {
        return Probe::Unknown(format!("worker_start is missing for pid {pid}"));
    };
    let Some(rec_ticks_str) = raw_start.strip_prefix("linux-starttime:") else {
        return Probe::Unknown(format!(
            "worker_start `{raw_start}` for pid {pid} does not use `linux-starttime:<ticks>`"
        ));
    };
    let Ok(recorded_ticks) = rec_ticks_str.parse::<u64>() else {
        return Probe::Unknown(format!(
            "worker_start `{raw_start}` for pid {pid} has invalid start ticks"
        ));
    };

    let stat_path = format!("/proc/{pid}/stat");
    let stat_str = match std::fs::read_to_string(&stat_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Probe::Dead(format!("process {pid} no longer exists"));
        }
        Err(e) => {
            return Probe::Unknown(format!("cannot read {stat_path}: {e}"));
        }
    };
    let Some(rparen) = stat_str.rfind(')') else {
        return Probe::Unknown(format!("malformed {stat_path} (missing `)`)"));
    };
    let mut fields = stat_str[rparen + 1..].split_whitespace();
    let Some(state_tok) = fields.next() else {
        return Probe::Unknown(format!("malformed {stat_path} (missing state field)"));
    };
    let state_char = state_tok.chars().next().unwrap_or('?');
    // Field 3 was consumed by `fields.next()`; field 22 (`starttime`) is 18 fields later (`nth(18)`).
    let Some(ticks_tok) = fields.nth(22 - 4) else {
        return Probe::Unknown(format!(
            "malformed {stat_path} (missing starttime field 22)"
        ));
    };
    let Ok(live_ticks) = ticks_tok.parse::<u64>() else {
        return Probe::Unknown(format!(
            "malformed {stat_path} (non-numeric starttime `{ticks_tok}`)"
        ));
    };
    if live_ticks != recorded_ticks {
        return Probe::Dead(format!(
            "pid {pid} start ticks changed ({recorded_ticks} -> {live_ticks}); PID was recycled"
        ));
    }
    if matches!(state_char, 'Z' | 'X' | 'x') {
        return Probe::Dead(format!(
            "process {pid} is in exited/zombie state `{state_char}`"
        ));
    }
    Probe::Alive
}

/// Reap any `CompletePair` `Running` request whose recorded `(worker_pid,
/// worker_start)` has positive `Probe::Dead` evidence without having recorded
/// completion. `Probe::Alive` and `Probe::Unknown` records (and all
/// `RecoveryRequired` records) are retained untouched. Caller must hold
/// `acquire_dispatch_lock`.
// trace:STORY-1917 trace:TASK-2344 | ai:agy
fn reap_dead_workers_locked(main_worktree: &Path, now: DateTime<Utc>) -> Result<usize> {
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    let mut reaped = 0usize;
    for (req, mut res) in inv.pairs {
        if res.status != DispatchStatus::Running {
            continue;
        }
        let dead_reason = match probe_worker_identity(res.worker_pid, res.worker_start.as_deref()) {
            aida_core::process_probe::Probe::Alive
            | aida_core::process_probe::Probe::Unknown(_) => continue,
            aida_core::process_probe::Probe::Dead(why) => why,
        };
        let reason = format!(
            "worker (pid {:?}) exited unexpectedly without recording completion ({dead_reason})",
            res.worker_pid
        );
        res.status = DispatchStatus::Failed;
        res.finished_at = Some(now);
        res.summary = format!(
            "failed {} {}: {reason}",
            req.action.verb_str(),
            req.action.subject_label()
        );
        res.refusal_reason = Some(reason.clone());
        res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
        write_private_atomic(
            &result_path(main_worktree, &req.id),
            &serde_json::to_vec_pretty(&res)?,
        )?;
        emit_dispatch_event(
            main_worktree,
            &req.mandate_id,
            req.action.verb_str(),
            &req.action.subject_label(),
            Outcome::Skipped,
            Some(reason),
        );
        reaped += 1;
    }
    Ok(reaped)
}

/// Spawn a detached `aida dispatch run <id> [--via-serve]` worker process,
/// redirecting stdout/stderr to the pinned `.aida/dispatch/logs/<id>.log`
/// (mode 0600, `O_NOFOLLOW`) and passing `claim_token` via
/// [`DISPATCH_CLAIM_TOKEN_ENV`].
// trace:STORY-1917 | ai:agy
pub(crate) fn spawn_detached_worker(
    main_worktree: &Path,
    id: &str,
    via_serve: bool,
    claim_token: &str,
) -> Result<(std::process::Child, u32, Option<String>)> {
    let aida_bin = resolve_aida_bin()?;
    let log_out = open_pinned_log_file(main_worktree, id, true)?;
    let log_err = log_out.try_clone()?;

    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree)
        .args(["dispatch", "run", id])
        .env(DISPATCH_CLAIM_TOKEN_ENV, claim_token);
    if via_serve {
        cmd.arg("--via-serve");
    } else {
        // Cron/schedule tick workers never inherit ambient session grants/roles.
        for key in crate::mandate_drive::SESSION_ENV {
            cmd.env_remove(key);
        }
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_out))
        .stderr(std::process::Stdio::from(log_err));

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `aida dispatch run {id}`"))?;
    let pid = child.id();
    let start = crate::process_probe::process_start_identity(pid);
    Ok((child, pid, start))
}

fn publish_spawned_worker_claim(
    main_worktree: &Path,
    id: &str,
    res: &DispatchResult,
) -> Result<()> {
    #[cfg(test)]
    if TEST_FAIL_POST_SPAWN_PUBLICATION.with(|c| c.get()) {
        bail!("simulated post-spawn worker PID publication failure");
    }
    write_private_atomic(
        &result_path(main_worktree, id),
        &serde_json::to_vec_pretty(res)?,
    )
}

/// Claim eligible `Pending` requests under `tick.lock` (moving `Pending ->
/// Running` and persisting `claim_token` BEFORE spawning so no unrecorded child
/// can race ahead of `Running` publication), enforcing concurrency caps
/// (2 reviews, 4 flows, 1 train).
// trace:STORY-1917 | ai:agy
pub(crate) fn claim_and_dispatch_pass(
    main_worktree: &Path,
    via_serve: bool,
    spawn_detached: bool,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));
    let _lock = acquire_dispatch_lock(main_worktree)?;
    let reaped = reap_dead_workers_locked(main_worktree, now)?;
    let inv = reconcile_inventory_locked(main_worktree, now)?;
    let (mut running_reviews, mut running_flows, mut running_trains) =
        inv.active_and_fenced_cap_usage(None)?;
    let pairs = inv.pairs;

    let mut summary = DispatchPassSummary {
        reaped,
        ..Default::default()
    };

    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);

    for (req, mut res) in pairs {
        if res.status != DispatchStatus::Pending {
            continue;
        }
        if req.action.requires_serve() && !via_serve {
            summary.waiting_for_serve += 1;
            continue;
        }
        match req.action {
            DispatchVerb::Review { .. } => {
                if running_reviews >= MAX_CONCURRENT_REVIEWS {
                    continue;
                }
                running_reviews += 1;
            }
            DispatchVerb::Flow { .. } => {
                if running_flows >= MAX_CONCURRENT_FLOWS {
                    continue;
                }
                running_flows += 1;
            }
            DispatchVerb::Train { .. } => {
                if running_trains >= MAX_CONCURRENT_TRAINS {
                    continue;
                }
                running_trains += 1;
            }
            DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
        }

        // Persist the Running claim and fresh claim_token BEFORE spawning (F2).
        // When `spawn_detached` is true, `worker_pid` starts as `None` until the
        // spawned child's `(pid, start)` is durably published under `tick.lock`;
        // if post-spawn publication fails, the child is immediately killed and
        // reaped under `tick.lock` and the claim is rolled back to `Failed`.
        let claim_token = Uuid::new_v4().to_string();
        res.status = DispatchStatus::Running;
        res.started_at = Some(now);
        res.worker_pid = if spawn_detached { None } else { Some(self_pid) };
        res.worker_start = if spawn_detached {
            None
        } else {
            self_start.clone()
        };
        res.claim_token = Some(claim_token.clone());
        res.summary = format!(
            "running {} {}",
            req.action.verb_str(),
            req.action.subject_label()
        );
        write_private_atomic(
            &result_path(main_worktree, &req.id),
            &serde_json::to_vec_pretty(&res)?,
        )?;

        if spawn_detached {
            match spawn_detached_worker(main_worktree, &req.id, via_serve, &claim_token) {
                Ok((mut child, pid, start)) => {
                    res.worker_pid = Some(pid);
                    res.worker_start = start;
                    res.summary = format!(
                        "running {} {} (pid {pid})",
                        req.action.verb_str(),
                        req.action.subject_label()
                    );
                    if let Err(pub_err) = publish_spawned_worker_claim(main_worktree, &req.id, &res)
                    {
                        // Parent publication failed: kill and reap the spawned
                        // child while still holding `tick.lock` so no unowned
                        // child can ever execute work, then fail the claim.
                        let _ = child.kill();
                        let _ = child.wait();
                        match req.action {
                            DispatchVerb::Review { .. } => {
                                running_reviews = running_reviews.saturating_sub(1);
                            }
                            DispatchVerb::Flow { .. } => {
                                running_flows = running_flows.saturating_sub(1);
                            }
                            DispatchVerb::Train { .. } => {
                                running_trains = running_trains.saturating_sub(1);
                            }
                            DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
                        }
                        let reason = format!(
                            "failed to publish dispatch worker claim for pid {pid}: {pub_err:#}"
                        );
                        res.status = DispatchStatus::Failed;
                        res.finished_at = Some(now);
                        res.worker_pid = None;
                        res.worker_start = None;
                        res.claim_token = None;
                        res.summary = reason.clone();
                        res.refusal_reason = Some(reason);
                        res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
                        let _ = write_private_atomic(
                            &result_path(main_worktree, &req.id),
                            &serde_json::to_vec_pretty(&res)?,
                        );
                        continue;
                    }
                }
                Err(err) => {
                    match req.action {
                        DispatchVerb::Review { .. } => {
                            running_reviews = running_reviews.saturating_sub(1);
                        }
                        DispatchVerb::Flow { .. } => {
                            running_flows = running_flows.saturating_sub(1);
                        }
                        DispatchVerb::Train { .. } => {
                            running_trains = running_trains.saturating_sub(1);
                        }
                        DispatchVerb::Take { .. } | DispatchVerb::Impl { .. } => {}
                    }
                    let reason = format!("failed to spawn dispatch worker: {err:#}");
                    res.status = DispatchStatus::Failed;
                    res.finished_at = Some(now);
                    res.worker_pid = None;
                    res.worker_start = None;
                    res.claim_token = None;
                    res.summary = reason.clone();
                    res.refusal_reason = Some(reason);
                    res.notified_at = notify_advisor_outcome(main_worktree, &req, &res, now);
                    write_private_atomic(
                        &result_path(main_worktree, &req.id),
                        &serde_json::to_vec_pretty(&res)?,
                    )?;
                    continue;
                }
            }
        }

        summary.spawned += 1;
        summary.claimed_ids.push(req.id);
    }

    Ok(summary)
}

/// Run one substrate `aida dispatch tick` pass: executes only `take` and `impl`
/// requests (leaves `review`, `flow`, and `train` `Pending` as `waiting for serve`).
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick(
    main_worktree: &Path,
    _mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    claim_and_dispatch_pass(main_worktree, false, true, now)
}

/// Synchronous test/in-process tick helper: claims eligible `take`/`impl`
/// requests (`via_serve = false`) and runs them in-process, leaving `review`,
/// `flow`, and `train` `Pending`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_tick_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let summary = claim_and_dispatch_pass(main_worktree, false, false, now)?;
    for id in &summary.claimed_ids {
        let token = load_pair(main_worktree, id)
            .ok()
            .and_then(|(_, res)| res.claim_token);
        let _ = execute_one_request_with_token(
            main_worktree,
            mandate_dir,
            id,
            false,
            token.as_deref(),
            now,
        );
    }
    Ok(summary)
}

/// Synchronous `serve --once` helper: verifies the caller's bound `advisor`
/// grant first (refusing to start without one), then claims and executes
/// eligible requests.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve_once_sync(
    main_worktree: &Path,
    mandate_dir: &Path,
    now: DateTime<Utc>,
) -> Result<DispatchPassSummary> {
    let _grant = check_serve_grant(main_worktree, now)?;
    let summary = claim_and_dispatch_pass(main_worktree, true, false, now)?;
    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);
    for id in &summary.claimed_ids {
        let token = load_pair(main_worktree, id)
            .ok()
            .and_then(|(_, res)| res.claim_token);
        // Re-check the advisor grant before each request so mid-pass expiry
        // stops immediately and reverts unclaimed requests back to Pending.
        if check_serve_grant(main_worktree, Utc::now()).is_err() {
            if let Ok(_lock) = acquire_dispatch_lock(main_worktree) {
                if let Ok((_req, mut res)) = load_pair(main_worktree, id) {
                    if res.status == DispatchStatus::Running
                        && res.worker_pid == Some(self_pid)
                        && res.worker_start == self_start
                        && res.claim_token == token
                    {
                        res.status = DispatchStatus::Pending;
                        res.started_at = None;
                        res.worker_pid = None;
                        res.worker_start = None;
                        res.claim_token = None;
                        let _ = write_private_atomic(
                            &result_path(main_worktree, id),
                            &serde_json::to_vec_pretty(&res)?,
                        );
                    }
                }
            }
            continue;
        }
        let _ = execute_one_request_with_token(
            main_worktree,
            mandate_dir,
            id,
            true,
            token.as_deref(),
            now,
        );
    }
    Ok(summary)
}

/// Long-running `aida dispatch serve` loop started from a human advisor
/// terminal after `eval "$(aida role enter advisor ...)"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_dispatch_serve(
    main_worktree: &Path,
    _mandate_dir: &Path,
    once: bool,
    poll_ms: u64,
) -> Result<()> {
    let now = Utc::now();
    let grant = check_serve_grant(main_worktree, now)?;
    println!(
        "dispatch serve: bound advisor grant {} active until {}",
        &grant.id[..8.min(grant.id.len())],
        grant.expires_at.to_rfc3339()
    );
    loop {
        let tick_now = Utc::now();
        if let Err(err) = check_serve_grant(main_worktree, tick_now) {
            eprintln!("dispatch serve stopping (pending requests left pending): {err:#}");
            return Ok(());
        }
        let summary = claim_and_dispatch_pass(main_worktree, true, true, tick_now)?;
        if summary.spawned > 0 || summary.reaped > 0 {
            println!(
                "dispatch serve: spawned={}, reaped={}",
                summary.spawned, summary.reaped
            );
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(poll_ms.clamp(100, 60_000)));
    }
}

/// Execute a single dispatch request (`aida dispatch run <id> [--via-serve]`).
///
/// Enforces:
/// 1. Atomic claim/ownership check under `tick.lock` (F2): a `Running` request
///    requires this process's `(worker_pid, worker_start, claim_token)` to
///    match the persisted claim; a `Pending` request checks concurrency caps
///    and persists a fresh claim before running.
/// 2. Dual-end re-validation of `requests/<id>.json` and any frozen brief file
///    (`O_NOFOLLOW | O_NONBLOCK`, mode 0600, current uid, SHA-256 match).
/// 3. Hard Boundary B rule (ADVISOR SIGNOFF): if `req.action.requires_serve()`
///    (`review`, `flow`, `train`) and `!via_serve`, refuses immediately without
///    touching any grant. When `via_serve` is true, verifies `check_serve_grant`
///    (inherited binding; never self-binds the grant).
/// 4. Exact-ID Mandate re-check (`mandate::recheck(mandate_dir, &req.mandate_id, ...)` — F3),
///    `HumanGates` (`GateOp::Dispatch` / `GateOp::Ship`), and requester grant
///    re-check on disk (`seat_authority::grant_authorized_at`).
// trace:STORY-1917 | ai:agy
pub(crate) fn execute_one_request(
    main_worktree: &Path,
    mandate_dir: &Path,
    id: &str,
    via_serve: bool,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let env_token = std::env::var(DISPATCH_CLAIM_TOKEN_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());
    execute_one_request_with_token(
        main_worktree,
        mandate_dir,
        id,
        via_serve,
        env_token.as_deref(),
        now,
    )
}

pub(crate) fn execute_one_request_with_token(
    main_worktree: &Path,
    mandate_dir: &Path,
    id: &str,
    via_serve: bool,
    presented_token: Option<&str>,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _ = clean_stale_git_config_lock(main_worktree, std::time::SystemTime::from(now));

    if Uuid::parse_str(id).is_err() {
        bail!("invalid dispatch request id `{id}`");
    }

    let self_pid = std::process::id();
    let self_start = crate::process_probe::process_start_identity(self_pid);

    // Acquire `tick.lock` to verify or establish exclusive worker ownership (F2)
    // BEFORE reading/validating the request or recording any refusal.
    let (mut res, claim, raw_req_res) = {
        let _lock = acquire_dispatch_lock(main_worktree)?;
        let inv = reconcile_inventory_locked(main_worktree, now)?;
        if let Some(fence) = inv.find_fence(id) {
            bail!(
                "dispatch request {id} is in `needs-recovery` state and cannot be executed: {}",
                fence.reason
            );
        }

        let res_bytes =
            read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
        let mut res: DispatchResult = serde_json::from_slice(&res_bytes)
            .with_context(|| format!("malformed result JSON for {id}"))?;
        if res.id != id {
            bail!("result file {id}.json has mismatched id `{}`", res.id);
        }
        if matches!(
            res.status,
            DispatchStatus::Done
                | DispatchStatus::Refused
                | DispatchStatus::Failed
                | DispatchStatus::Cancelled
        ) {
            bail!(
                "dispatch request {id} is already in terminal state `{}` (refusing execution)",
                res.status.as_str()
            );
        }

        let raw_req_res =
            read_bounded_regular_file(&request_path(main_worktree, id), MAX_BRIEF_BYTES, true)
                .and_then(|bytes| {
                    serde_json::from_slice::<DispatchRequest>(&bytes)
                        .with_context(|| format!("malformed request JSON for {id}"))
                });

        let claim = match res.status {
            DispatchStatus::Running => {
                let Some(expected_token) = res.claim_token.clone() else {
                    bail!(
                        "dispatch request {id} is already running without a claim token (refusing unowned execution)"
                    );
                };
                if presented_token != Some(expected_token.as_str()) {
                    bail!(
                        "dispatch request {id} is already running under worker pid {:?} (claim token mismatch; refusing to overwrite live worker)",
                        res.worker_pid
                    );
                }
                let Some(recorded_pid) = res.worker_pid else {
                    // Parent crashed or failed after pre-spawn claim write before
                    // publishing the child PID: fail closed so an unowned child can
                    // never perform work or leave a replaceable live claim.
                    let reason =
                        "worker claim publication did not complete (worker_pid is None); refusing unowned execution"
                            .to_string();
                    res.status = DispatchStatus::Failed;
                    res.finished_at = Some(now);
                    res.worker_pid = None;
                    res.worker_start = None;
                    res.claim_token = None;
                    res.summary = reason.clone();
                    res.refusal_reason = Some(reason.clone());
                    let _ = write_private_atomic(
                        &result_path(main_worktree, id),
                        &serde_json::to_vec_pretty(&res)?,
                    );
                    bail!("dispatch request {id} has an incomplete Running claim without worker_pid: {reason}");
                };
                let pid_matches = recorded_pid == self_pid && res.worker_start == self_start;
                if !pid_matches {
                    if matches!(
                        probe_worker_identity(Some(recorded_pid), res.worker_start.as_deref()),
                        aida_core::process_probe::Probe::Dead(_)
                    ) {
                        let _ = reap_dead_workers_locked(main_worktree, now);
                    }
                    bail!(
                        "dispatch request {id} is already running under worker pid {recorded_pid} (start {:?}), not this process {self_pid} (start {:?}); refusing duplicate or replacement execution",
                        res.worker_start,
                        self_start
                    );
                }
                WorkerClaim {
                    worker_pid: self_pid,
                    worker_start: self_start.clone(),
                    claim_token: expected_token,
                }
            }
            DispatchStatus::Pending => {
                let _ = reap_dead_workers_locked(main_worktree, now);
                let inv_after_reap = reconcile_inventory_locked(main_worktree, now)?;
                let (running_reviews, running_flows, running_trains) =
                    inv_after_reap.active_and_fenced_cap_usage(Some(id))?;
                // Enforce concurrency caps on direct `aida dispatch run <id>` as well.
                if let Ok(ref req) = raw_req_res {
                    match req.action {
                        DispatchVerb::Review { .. }
                            if running_reviews >= MAX_CONCURRENT_REVIEWS =>
                        {
                            bail!(
                                "cannot run dispatch request {id}: review concurrency cap ({MAX_CONCURRENT_REVIEWS}) reached"
                            );
                        }
                        DispatchVerb::Flow { .. } if running_flows >= MAX_CONCURRENT_FLOWS => {
                            bail!(
                                "cannot run dispatch request {id}: flow concurrency cap ({MAX_CONCURRENT_FLOWS}) reached"
                            );
                        }
                        DispatchVerb::Train { .. } if running_trains >= MAX_CONCURRENT_TRAINS => {
                            bail!(
                                "cannot run dispatch request {id}: train concurrency cap ({MAX_CONCURRENT_TRAINS}) reached"
                            );
                        }
                        _ => {}
                    }
                }
                let claim_token = Uuid::new_v4().to_string();
                res.status = DispatchStatus::Running;
                res.started_at = Some(now);
                res.worker_pid = Some(self_pid);
                res.worker_start = self_start.clone();
                res.claim_token = Some(claim_token.clone());
                write_private_atomic(
                    &result_path(main_worktree, id),
                    &serde_json::to_vec_pretty(&res)?,
                )?;
                WorkerClaim {
                    worker_pid: self_pid,
                    worker_start: self_start.clone(),
                    claim_token,
                }
            }
            _ => unreachable!(),
        };

        (res, claim, raw_req_res)
    };

    let req: DispatchRequest = match raw_req_res {
        Ok(req) if req.id == id => req,
        Ok(req) => {
            let reason = format!(
                "tampered request JSON for {id}: inner id `{}` does not match `{id}`",
                req.id
            );
            return record_tampered_refusal(main_worktree, id, &claim, &mut res, &reason, now);
        }
        Err(err) => {
            let reason = format!("request validation failed on disk: {err:#}");
            return record_tampered_refusal(main_worktree, id, &claim, &mut res, &reason, now);
        }
    };

    // 1. Boundary B check: `review`, `flow`, and `train` may ONLY execute under
    // `aida dispatch serve` (never under `dispatch tick`).
    if req.action.requires_serve() && !via_serve {
        let reason = format!(
            "verb `{}` requires an advisor seat grant and can only run under `aida dispatch serve` (refused by `dispatch tick`)",
            req.action.verb_str()
        );
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    if via_serve {
        if let Err(err) = check_serve_grant(main_worktree, now) {
            let reason = format!("serve advisor grant check failed at execution time: {err:#}");
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 2. Re-validate the exact recorded mandate ID on disk (`mandate::recheck`, F3).
    //    A replacement mandate B must never revive requests stopped or expired under A.
    let active_mandate = match mandate::recheck(mandate_dir, &req.mandate_id, &req.principal, now) {
        Ok(m) => m,
        Err(mandate::MandateRefusal::Expired { id: mid }) => {
            let reason = format!("mandate {mid} expired before execution");
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
        Err(mandate::MandateRefusal::Stopped { id: mid }) => {
            let reason = format!("mandate {mid} was stopped or superseded before execution");
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
        Err(other) => {
            let reason = format!(
                "mandate {} recheck refused before execution: {other:?}",
                req.mandate_id
            );
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    };

    if !active_mandate.covers(Scope::Dispatch) {
        let reason = format!(
            "active mandate {} no longer covers `dispatch`",
            active_mandate.id
        );
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.ships_pr() && !active_mandate.covers(Scope::ShipHeld) {
        let reason = format!(
            "active mandate {} does not cover `ship-held`, which `{}` requires",
            active_mandate.id,
            req.action.verb_str()
        );
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    // 3. Re-validate HumanGates at execution time.
    let gates = crate::mandate_gates::load_human_gates(main_worktree).map_err(|e| format!("{e:#}"));
    if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Dispatch) {
        let reason = format!("dispatch is gated by human gates at execution time ({why})");
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Gated,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.ships_pr() {
        if let Some(why) = crate::mandate_gates::gate_reason(&gates, GateOp::Ship) {
            let reason = format!("shipping is gated by human gates at execution time ({why})");
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Gated,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 4. Re-validate the requester's advisor grant on disk (`grant_authorized_at`),
    //    and treat any `revoked_at` stamp as immediate revocation regardless of
    //    clock skew.
    let grant_unrevoked = seat_authority::read_grant(&req.requester_grant_id)
        .map(|g| g.revoked_at.is_none())
        .unwrap_or(false);
    if !grant_unrevoked
        || !seat_authority::grant_authorized_at(
            main_worktree,
            &req.requester_grant_id,
            mandate::MANDATE_SEAT,
            now,
            &req.matched_binding,
        )
    {
        let reason = format!(
            "requester advisor grant {} is no longer valid (expired, revoked, or binding mismatch)",
            req.requester_grant_id
        );
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }
    if req.action.delegates_reviewer() {
        let delegable_ok = seat_authority::read_grant(&req.requester_grant_id)
            .map(|g| {
                g.delegable_seats
                    .iter()
                    .any(|s| crate::canonical_role_name(s) == "reviewer")
            })
            .unwrap_or(false);
        if !delegable_ok {
            let reason = format!(
                "requester advisor grant {} does not delegate `reviewer`",
                req.requester_grant_id
            );
            return finalize_request(
                main_worktree,
                &req,
                &claim,
                res,
                DispatchStatus::Refused,
                Outcome::Refused,
                None,
                None,
                reason.clone(),
                Some(reason),
                now,
            );
        }
    }

    // 5. Dual-end validation of request fields + frozen brief SHA-256 + spec store checks.
    if let Err(err) = revalidate_request(
        main_worktree,
        mandate_dir,
        &req,
        active_mandate.include_supervised,
    ) {
        let reason = format!("execution-time validation refused request: {err:#}");
        return finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        );
    }

    // 6. Execute the verb's runner.
    let runner_outcome = match &req.action {
        DispatchVerb::Review { pr, vendor } => run_review_verb(main_worktree, &req, *pr, *vendor),
        DispatchVerb::Take {
            pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_take_verb(
            main_worktree,
            &req,
            *pr,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Impl {
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            vendor,
        } => run_impl_verb(
            main_worktree,
            &req,
            name,
            branch,
            spec,
            brief_relpath,
            brief_sha256,
            *vendor,
            now,
        ),
        DispatchVerb::Flow {
            pr,
            no_ship,
            vendor,
        } => dispatch_flow::run_flow_verb(
            main_worktree,
            mandate_dir,
            &req,
            &claim,
            *pr,
            *no_ship,
            *vendor,
            now,
        ),
        DispatchVerb::Train { prs } => {
            dispatch_flow::run_train_verb(main_worktree, mandate_dir, &req, &claim, prs, now)
        }
    };

    match runner_outcome {
        RunnerOutcome::Done {
            summary,
            spawned_session,
            exit_code,
        } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Done,
            Outcome::Done,
            spawned_session,
            exit_code,
            summary,
            None,
            now,
        ),
        RunnerOutcome::Refused { reason } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Refused,
            Outcome::Refused,
            None,
            None,
            reason.clone(),
            Some(reason),
            now,
        ),
        RunnerOutcome::Failed { summary, exit_code } => finalize_request(
            main_worktree,
            &req,
            &claim,
            res,
            DispatchStatus::Failed,
            Outcome::Skipped,
            None,
            exit_code,
            summary.clone(),
            Some(summary),
            now,
        ),
    }
}

#[derive(Debug)]
pub(crate) enum RunnerOutcome {
    Done {
        summary: String,
        spawned_session: Option<String>,
        exit_code: Option<i32>,
    },
    Refused {
        reason: String,
    },
    Failed {
        summary: String,
        exit_code: Option<i32>,
    },
}

/// Verify under `tick.lock` that the on-disk result for `id` is still `Running`,
/// not fenced in `.aida/dispatch/recovery.json`, and owned by `claim` before
/// allowing a terminal state write (F2, TASK-2344 R4).
pub(crate) fn verify_claim_locked(
    main_worktree: &Path,
    id: &str,
    claim: &WorkerClaim,
) -> Result<()> {
    let inv = reconcile_inventory_locked(main_worktree, Utc::now())?;
    if let Some(fence) = inv.find_fence(id) {
        bail!(
            "fenced result transition refused for {id}: record is in `needs-recovery` state ({})",
            fence.reason
        );
    }
    let on_disk_bytes =
        read_bounded_regular_file(&result_path(main_worktree, id), MAX_BRIEF_BYTES, true)?;
    let on_disk: DispatchResult = serde_json::from_slice(&on_disk_bytes)
        .with_context(|| format!("malformed result JSON for {id}"))?;
    if on_disk.status != DispatchStatus::Running
        || on_disk.worker_pid != Some(claim.worker_pid)
        || on_disk.worker_start != claim.worker_start
        || on_disk.claim_token.as_deref() != Some(claim.claim_token.as_str())
    {
        bail!(
            "fenced result transition refused for {id}: on-disk claim {:?}/{:?} (status {}) does not match worker claim {}/{:?}",
            on_disk.worker_pid,
            on_disk.claim_token,
            on_disk.status.as_str(),
            claim.worker_pid,
            claim.claim_token
        );
    }
    Ok(())
}

pub(crate) fn record_tampered_refusal(
    main_worktree: &Path,
    id: &str,
    claim: &WorkerClaim,
    res: &mut DispatchResult,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    verify_claim_locked(main_worktree, id, claim)?;
    let _ = append_dispatch_log(main_worktree, id, &format!("refused: {reason}\n"));
    res.status = DispatchStatus::Refused;
    res.finished_at = Some(now);
    res.summary = reason.to_string();
    res.refusal_reason = Some(reason.to_string());
    write_private_atomic(
        &result_path(main_worktree, id),
        &serde_json::to_vec_pretty(res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &res.mandate_id,
        &res.verb,
        &res.target,
        Outcome::Refused,
        Some(reason.to_string()),
    );
    Ok(res.clone())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finalize_request(
    main_worktree: &Path,
    req: &DispatchRequest,
    claim: &WorkerClaim,
    mut res: DispatchResult,
    status: DispatchStatus,
    event_outcome: Outcome,
    spawned_session: Option<String>,
    exit_code: Option<i32>,
    summary: String,
    refusal_reason: Option<String>,
    now: DateTime<Utc>,
) -> Result<DispatchResult> {
    let _lock = acquire_dispatch_lock(main_worktree)?;
    verify_claim_locked(main_worktree, &req.id, claim)?;
    let _ = append_dispatch_log(
        main_worktree,
        &req.id,
        &format!("{}: {}\n", status.as_str(), summary),
    );
    res.status = status;
    res.finished_at = Some(now);
    res.spawned_session = spawned_session;
    res.exit_code = exit_code;
    res.summary = summary.clone();
    res.refusal_reason = refusal_reason.clone();
    res.notified_at = notify_advisor_outcome(main_worktree, req, &res, now);
    write_private_atomic(
        &result_path(main_worktree, &req.id),
        &serde_json::to_vec_pretty(&res)?,
    )?;
    emit_dispatch_event(
        main_worktree,
        &req.mandate_id,
        req.action.verb_str(),
        &req.action.subject_label(),
        event_outcome,
        refusal_reason.or(Some(summary)),
    );
    Ok(res)
}

/// Resolve the sibling `<project>-worktrees` root directory next to `main_worktree`.
// trace:STORY-1917 | ai:agy
pub(crate) fn worktrees_root(main_worktree: &Path) -> PathBuf {
    let parent = main_worktree.parent().unwrap_or(main_worktree);
    let repo_name = main_worktree
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("aida");
    parent.join(format!("{repo_name}-worktrees"))
}

fn worktree_current_branch(wt: &Path) -> Option<String> {
    if !wt.exists() {
        return None;
    }
    let out = aida_core::git_ops::host_git_command(wt)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b.is_empty()).then_some(b)
}

/// Scan `/proc/<pid>/cwd` for any live non-zombie process (excluding the calling
/// process) whose current working directory is equal to or inside `wt_canon`.
fn find_live_proc_with_cwd_in(wt_canon: &Path) -> Option<u32> {
    let self_pid = std::process::id();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name_str.parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let Ok(raw_cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        let raw_str = raw_cwd.to_string_lossy();
        let cleaned = Path::new(raw_str.strip_suffix(" (deleted)").unwrap_or(&raw_str));
        let proc_canon = cleaned
            .canonicalize()
            .unwrap_or_else(|_| cleaned.to_path_buf());
        if proc_canon == wt_canon || proc_canon.starts_with(wt_canon) {
            if crate::process_probe::pid_is_alive(pid) {
                return Some(pid);
            }
        }
    }
    None
}

/// Check if any live or unknown occupant owns `worktree`, `branch`, or `scope`
/// across session leases (`.aida/sessions/`), the agent process registry
/// (`.aida/agents/`), or live OS processes running inside the target worktree
/// (F5). Returns a descriptive refusal reason when occupied or when registry/lease
/// state is unreadable/unknown.
// trace:STORY-1917 | ai:agy
pub(crate) fn check_live_worktree_or_branch_occupant(
    main_worktree: &Path,
    worktree: &Path,
    branch: Option<&str>,
    scope: Option<&str>,
) -> Option<String> {
    let live_sessions = crate::process_probe::probe_live_claude_sessions();
    let now = Utc::now();
    let main_canon = main_worktree
        .canonicalize()
        .unwrap_or_else(|_| main_worktree.to_path_buf());
    let target_canon = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());

    // 1. Check `.aida/sessions/` leases (treating unreadable/corrupt lease files
    //    as unknown occupants so cleanup/branch reset fails closed).
    let leases_dir = crate::leases::leases_dir(main_worktree);
    match std::fs::read_dir(&leases_dir) {
        Ok(entries) => {
            for entry in entries {
                let Ok(entry) = entry else {
                    return Some(format!(
                        "unknown occupant: unreadable entry in {}",
                        leases_dir.display()
                    ));
                };
                let p = entry.path();
                if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(&p) else {
                    return Some(format!(
                        "unknown occupant: cannot read session lease {}",
                        p.display()
                    ));
                };
                if toml::from_str::<crate::SessionLease>(&body).is_err() {
                    return Some(format!(
                        "unknown occupant: malformed session lease {}",
                        p.display()
                    ));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Some(format!(
                "unknown occupant: cannot read {}: {e}",
                leases_dir.display()
            ));
        }
    }

    for lease in crate::leases::list_leases(main_worktree) {
        let state = crate::leases::lease_state_for(&lease, &live_sessions, now);
        let active_pid_live = lease.active_pid.is_some_and(|pid| {
            crate::process_probe::process_identity_is_alive(
                pid,
                lease.active_pid_start_time.as_deref(),
            )
        });
        let is_live = matches!(state, crate::LeaseState::Live)
            || active_pid_live
            || crate::agent_registry::live_agents_covering_cwd(main_worktree, &lease.worktree_path)
                > 0;
        if !is_live {
            continue;
        }
        let lease_canon = lease
            .worktree_path
            .canonicalize()
            .unwrap_or_else(|_| lease.worktree_path.clone());
        if !lease.worktree_path.as_os_str().is_empty() && lease_canon == target_canon {
            return Some(format!(
                "worktree {} is currently occupied by live session lease {} (scope `{}`, role `{}`)",
                worktree.display(),
                lease.id,
                lease.scope,
                lease.role.as_deref().unwrap_or("unspecified")
            ));
        }
        if let Some(b) = branch {
            if !lease.branch.is_empty() && lease.branch == b {
                return Some(format!(
                    "branch `{b}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
        if let Some(s) = scope {
            if lease.scope.eq_ignore_ascii_case(s) {
                return Some(format!(
                    "scope `{s}` is currently occupied by live session lease {} in {}",
                    lease.id,
                    lease.worktree_path.display()
                ));
            }
        }
    }

    // 2. Check `.aida/agents/*.toml` independently of leases (F5): any live or
    //    unobservable/unknown agent (or unreadable/corrupt registry file)
    //    retains its worktree, branch, and scope.
    let observer = crate::agent_registry::PidObserver::current();
    let mut agent_dirs = vec![crate::agent_registry::agents_dir(main_worktree)];
    let wt_agents_dir = worktree.join(".aida").join("agents");
    if wt_agents_dir.exists() {
        let main_agents_canon = agent_dirs[0]
            .canonicalize()
            .unwrap_or_else(|_| agent_dirs[0].clone());
        let wt_agents_canon = wt_agents_dir
            .canonicalize()
            .unwrap_or_else(|_| wt_agents_dir.clone());
        if wt_agents_canon != main_agents_canon {
            agent_dirs.push(wt_agents_dir);
        }
    }
    let wt_dir_name = worktree.file_name().and_then(|n| n.to_str());
    for ag_dir in agent_dirs {
        let entries = match std::fs::read_dir(&ag_dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Some(format!(
                    "unknown occupant: cannot read agent registry {}: {e}",
                    ag_dir.display()
                ));
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                return Some(format!(
                    "unknown occupant: unreadable entry in {}",
                    ag_dir.display()
                ));
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                return Some(format!(
                    "unknown occupant: cannot read agent registry record {}",
                    path.display()
                ));
            };
            let Ok(record) = toml::from_str::<crate::agent_registry::AgentRegistryEntry>(&body)
            else {
                return Some(format!(
                    "unknown occupant: malformed agent registry record {}",
                    path.display()
                ));
            };
            if record.ended_at.is_some() {
                continue;
            }
            let observable = observer.can_observe(record.pid_namespace);
            let live_or_unknown = if !observable {
                true
            } else {
                crate::process_probe::process_identity_is_alive(
                    record.pid,
                    record.pid_start.as_deref(),
                )
            };
            if !live_or_unknown {
                continue;
            }
            let state_word = if observable {
                "live"
            } else {
                "unknown-liveness"
            };
            let label = record.name.as_deref().unwrap_or(&record.id);
            let rec_wt = record
                .worktree_path
                .canonicalize()
                .unwrap_or_else(|_| record.worktree_path.clone());
            let wt_matches = !record.worktree_path.as_os_str().is_empty()
                && (rec_wt == target_canon
                    || (rec_wt != main_canon
                        && target_canon != main_canon
                        && (target_canon.starts_with(&rec_wt)
                            || rec_wt.starts_with(&target_canon))));
            if wt_matches {
                return Some(format!(
                    "worktree {} is currently occupied by {state_word} registered agent `{label}` (pid {})",
                    worktree.display(),
                    record.pid
                ));
            }
            if let Some(wt_name) = wt_dir_name {
                if record
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wt_name))
                {
                    return Some(format!(
                        "lane `{wt_name}` is currently occupied by {state_word} registered agent `{label}` (pid {})",
                        record.pid
                    ));
                }
            }
            if let Some(s) = scope {
                if record
                    .current_spec
                    .as_deref()
                    .is_some_and(|cs| cs.eq_ignore_ascii_case(s))
                    || record
                        .name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(s))
                {
                    return Some(format!(
                        "scope `{s}` is currently occupied by {state_word} registered agent `{label}` (pid {}) in {}",
                        record.pid,
                        record.worktree_path.display()
                    ));
                }
            }
            if let Some(b) = branch {
                if !record.worktree_path.as_os_str().is_empty() && rec_wt != main_canon {
                    if worktree_current_branch(&record.worktree_path).as_deref() == Some(b) {
                        return Some(format!(
                            "branch `{b}` is currently occupied by {state_word} registered agent `{label}` (pid {}) in {}",
                            record.pid,
                            record.worktree_path.display()
                        ));
                    }
                }
            }
        }
    }

    // 3. Direct process / cwd liveness probe for `worktree` and any linked
    //    worktree currently holding `branch` (independent of both leases and
    //    registry entries — catches raw vendor processes launched by take/impl).
    let mut probe_wts = Vec::new();
    if target_canon != main_canon && worktree.exists() {
        probe_wts.push((worktree.to_path_buf(), target_canon.clone()));
    }
    if let Some(b) = branch {
        if let Some(b_wt) = find_worktree_for_branch(main_worktree, b) {
            let b_canon = b_wt.canonicalize().unwrap_or_else(|_| b_wt.clone());
            if b_canon != main_canon && !probe_wts.iter().any(|(_, c)| *c == b_canon) {
                probe_wts.push((b_wt, b_canon));
            }
        }
    }
    for (wt_display, wt_canon) in &probe_wts {
        for sess in &live_sessions {
            let sess_canon = sess.cwd.canonicalize().unwrap_or_else(|_| sess.cwd.clone());
            if sess_canon == *wt_canon || sess_canon.starts_with(wt_canon) {
                return Some(format!(
                    "worktree {} is currently occupied by live claude process (pid {})",
                    wt_display.display(),
                    sess.pid
                ));
            }
        }
        if let Some(pid) = find_live_proc_with_cwd_in(wt_canon) {
            return Some(format!(
                "worktree {} is currently occupied by live process (pid {pid})",
                wt_display.display()
            ));
        }
    }

    None
}

/// Preserve any merge-hold marker `<rev_wt>/.aida/merge-holds/PR-<pr>` into
/// `<main_worktree>/.aida/merge-holds/PR-<pr>` using atomic no-clobber
/// publication and post-transfer verification (F4). Any read, write, symlink,
/// or verification error aborts worktree cleanup so a hold is never lost.
// trace:STORY-1917 | ai:agy
fn preserve_reviewer_worktree_hold(
    main_worktree: &Path,
    rev_wt: &Path,
    pr_scope: &str,
) -> Result<bool> {
    let wt_aida = rev_wt.join(".aida");
    match std::fs::symlink_metadata(&wt_aida) {
        Ok(m) if m.file_type().is_symlink() => {
            let main_aida_canon = main_worktree
                .join(".aida")
                .canonicalize()
                .unwrap_or_else(|_| main_worktree.join(".aida"));
            let wt_aida_canon = wt_aida.canonicalize().unwrap_or_else(|_| wt_aida.clone());
            if wt_aida_canon != main_aida_canon {
                bail!(
                    "refusing reviewer worktree cleanup: {} is a symlink outside main worktree .aida",
                    wt_aida.display()
                );
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot stat {}", wt_aida.display()));
        }
    }

    let wt_hold_dir = wt_aida.join("merge-holds");
    match std::fs::symlink_metadata(&wt_hold_dir) {
        Ok(m) if m.file_type().is_symlink() || !m.is_dir() => {
            bail!(
                "refusing reviewer worktree cleanup: {} is a symlink or not a directory",
                wt_hold_dir.display()
            );
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot stat {}", wt_hold_dir.display()));
        }
    }

    let wt_hold = wt_hold_dir.join(pr_scope);
    match std::fs::symlink_metadata(&wt_hold) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("cannot stat reviewer merge-hold {}", wt_hold.display()));
        }
    }

    // Source hold exists in `rev_wt`: read and validate it with `O_NOFOLLOW`.
    // If unreadable, empty, or a symlink, abort cleanup immediately!
    let wt_bytes = read_bounded_regular_file(&wt_hold, MAX_BRIEF_BYTES, false)
        .with_context(|| format!("cannot read reviewer merge-hold {}", wt_hold.display()))?;
    if wt_bytes.is_empty() {
        bail!(
            "refusing reviewer worktree cleanup: merge-hold {} is empty",
            wt_hold.display()
        );
    }

    let main_hold_dir = main_worktree.join(".aida").join("merge-holds");
    std::fs::create_dir_all(&main_hold_dir)
        .with_context(|| format!("cannot create {}", main_hold_dir.display()))?;
    let main_dir_meta = std::fs::symlink_metadata(&main_hold_dir)
        .with_context(|| format!("cannot stat {}", main_hold_dir.display()))?;
    if main_dir_meta.file_type().is_symlink() || !main_dir_meta.is_dir() {
        bail!(
            "refusing reviewer worktree cleanup: {} is a symlink or not a directory",
            main_hold_dir.display()
        );
    }

    let main_hold = main_hold_dir.join(pr_scope);
    let tmp_hold = main_hold_dir.join(format!(".{pr_scope}.tmp-{}", Uuid::new_v4()));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = opts.open(&tmp_hold).with_context(|| {
            format!(
                "cannot stage merge-hold transfer in {}",
                main_hold_dir.display()
            )
        })?;
        if let Err(e) = f.write_all(&wt_bytes).and_then(|()| f.sync_all()) {
            drop(f);
            let _ = std::fs::remove_file(&tmp_hold);
            return Err(e).with_context(|| {
                format!("failed writing staged merge-hold {}", tmp_hold.display())
            });
        }
    }

    match crate::gate::review_verdict::publish_verdict_no_replace(&tmp_hold, &main_hold) {
        Ok(()) => {
            let landed = read_bounded_regular_file(&main_hold, MAX_BRIEF_BYTES, false)
                .with_context(|| {
                    format!(
                        "cannot verify transferred merge-hold at {}",
                        main_hold.display()
                    )
                })?;
            if landed != wt_bytes {
                bail!(
                    "transferred merge-hold {} content did not match source {}",
                    main_hold.display(),
                    wt_hold.display()
                );
            }
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&tmp_hold);
            // Destination hold already exists (pre-existing or concurrently placed);
            // verify it is a valid non-empty regular file before allowing cleanup.
            let existing = read_bounded_regular_file(&main_hold, MAX_BRIEF_BYTES, false)
                .with_context(|| {
                    format!(
                        "existing destination merge-hold {} is invalid or unreadable",
                        main_hold.display()
                    )
                })?;
            if existing.is_empty() {
                bail!(
                    "existing destination merge-hold {} is empty; refusing reviewer worktree cleanup",
                    main_hold.display()
                );
            }
            Ok(true)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_hold);
            Err(e).with_context(|| {
                format!(
                    "failed atomic no-clobber transfer of merge-hold to {}",
                    main_hold.display()
                )
            })
        }
    }
}

/// Prepare a dead reviewer worktree (`aida-pr-<pr>`) before launching a fresh
/// review:
/// 1. Refuses if a live or unknown occupant (lease, registered agent, or live
///    process) currently occupies `rev_wt`, `pr-<pr>`, or `PR-<pr>` (F5).
/// 2. Atomically preserves any `<rev_wt>/.aida/merge-holds/PR-<pr>` marker into
///    `<main_worktree>/.aida/merge-holds/PR-<pr>` with verified no-clobber
///    transfer, aborting cleanup on any preservation failure (F4).
/// 3. Removes `rev_wt` via `git worktree remove --force`.
// trace:STORY-1917 | ai:agy
pub(crate) fn cleanup_dead_reviewer_worktree(main_worktree: &Path, pr: u64) -> Result<()> {
    let pr_scope = format!("PR-{pr}");
    let pr_branch = format!("pr-{pr}");
    let candidates = [
        worktrees_root(main_worktree).join(format!("aida-pr-{pr}")),
        main_worktree
            .parent()
            .unwrap_or(main_worktree)
            .join(format!("aida-pr-{pr}")),
    ];
    for rev_wt in candidates {
        if let Some(busy) = check_live_worktree_or_branch_occupant(
            main_worktree,
            &rev_wt,
            Some(&pr_branch),
            Some(&pr_scope),
        ) {
            bail!("refusing to clean up reviewer worktree: {busy}");
        }
        if !rev_wt.exists() && std::fs::symlink_metadata(&rev_wt).is_err() {
            continue;
        }
        preserve_reviewer_worktree_hold(main_worktree, &rev_wt, &pr_scope)?;
        let out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "remove", "--force"])
            .arg(&rev_wt)
            .output()
            .with_context(|| {
                format!("failed to run git worktree remove on {}", rev_wt.display())
            })?;
        if !out.status.success() && rev_wt.exists() {
            let _ = aida_core::git_ops::host_git_command(main_worktree)
                .args(["worktree", "prune"])
                .output();
        }
    }
    // Best-effort cleanup of a leftover `pr-<pr>` branch after removing any dead
    // reviewer worktree, matching `review-one.sh`.
    let _ = aida_core::git_ops::host_git_command(main_worktree)
        .args(["branch", "-D", &pr_branch])
        .output();
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static TEST_AIDA_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_AGENT_BIN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    pub(crate) static TEST_FAIL_POST_SPAWN_PUBLICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static TEST_FAIL_SUBMIT_STAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    pub(crate) static TEST_SIMULATE_CRASH_AFTER_RESULT_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static TEST_FORCE_WORKER_PROBE: std::cell::RefCell<Option<aida_core::process_probe::Probe>> = const { std::cell::RefCell::new(None) };
}

fn resolve_aida_bin() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(p) = TEST_AIDA_BIN.with(|c| c.borrow().clone()) {
        return Ok(p);
    }
    crate::aida_bin::process()
        .map(|r| r.path)
        .or_else(|_| Ok(crate::aida_exe_path().to_path_buf()))
}

fn resolve_vendor_bin(vendor: DispatchVendor) -> PathBuf {
    #[cfg(test)]
    if let Some(p) = TEST_AGENT_BIN.with(|c| c.borrow().clone()) {
        return p;
    }
    let name = match vendor {
        DispatchVendor::Claude => "claude",
        DispatchVendor::Codex => "codex",
        DispatchVendor::Agy => "agy",
    };
    crate::find_executable_on_path(name).unwrap_or_else(|| PathBuf::from(name))
}

/// Run the `review` verb (`aida queue work PR-<pr> --role reviewer --no-human [--vendor <v>]`).
/// Runs only under `aida dispatch serve` so it inherits the human advisor's
/// bound seat grant directly.
// trace:STORY-1917 | ai:agy
pub(crate) fn run_review_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    pr: u64,
    vendor: Option<DispatchVendor>,
) -> RunnerOutcome {
    if let Err(err) = cleanup_dead_reviewer_worktree(main_worktree, pr) {
        return RunnerOutcome::Refused {
            reason: format!("{err:#}"),
        };
    }

    let aida_bin = match resolve_aida_bin() {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Failed {
                summary: format!("cannot locate aida binary: {err:#}"),
                exit_code: None,
            }
        }
    };

    let pr_arg = format!("PR-{pr}");
    let mut cmd = std::process::Command::new(aida_bin);
    cmd.current_dir(main_worktree).args([
        "queue",
        "work",
        &pr_arg,
        "--role",
        "reviewer",
        "--no-human",
    ]);
    if let Some(v) = vendor {
        cmd.args(["--vendor", v.as_str()]);
    }
    cmd.env_remove("AIDA_SESSION_PURPOSE")
        .env_remove(DISPATCH_CLAIM_TOKEN_ENV);

    match cmd.output() {
        Ok(out) => {
            let combined = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let _ = append_dispatch_log(main_worktree, &req.id, &combined);
            let code = out.status.code();
            if out.status.success() {
                RunnerOutcome::Done {
                    summary: format!("completed review of PR #{pr}"),
                    spawned_session: None,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!("review of PR #{pr} exited with {:?}", code),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn `aida queue work PR-{pr}`: {err:#}"),
            exit_code: None,
        },
    }
}

/// If `refs/heads/<branch>` is already checked out in a linked worktree, return
/// that worktree's path so `take` can reuse it rather than failing `git worktree add`.
// trace:STORY-1917 | ai:agy
fn find_worktree_for_branch(main_worktree: &Path, branch: &str) -> Option<PathBuf> {
    let out = aida_core::git_ops::host_git_command(main_worktree)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let want_branch = format!("branch refs/heads/{branch}");
    let main_canon = main_worktree
        .canonicalize()
        .unwrap_or_else(|_| main_worktree.to_path_buf());
    let mut current_wt: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            current_wt = Some(PathBuf::from(rest.trim()));
        } else if line.trim() == want_branch {
            if let Some(wt) = current_wt.take() {
                let wt_canon = wt.canonicalize().unwrap_or_else(|_| wt.clone());
                if wt_canon != main_canon {
                    return Some(wt);
                }
            }
        } else if line.trim().is_empty() {
            current_wt = None;
        }
    }
    None
}

fn git_ref_exists(repo: &Path, git_ref: &str) -> bool {
    aida_core::git_ops::host_git_command(repo)
        .args(["rev-parse", "--verify", "--quiet", git_ref])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Prepare or reuse a worktree at `wt_path` on `branch` tracking `start_ref`
/// (e.g. `origin/<branch>` for `take`, or `origin/main` for `impl`) without
/// ever force-pushing. Refuses if `wt_path` or `branch` is currently occupied
/// by a live or unknown process/agent/lease (F5).
// trace:STORY-1917 | ai:agy
pub(crate) fn prepare_lane_worktree(
    main_worktree: &Path,
    wt_path: &Path,
    branch: &str,
    fetch_ref: &str,
    start_ref: &str,
) -> Result<()> {
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, wt_path, Some(branch), None)
    {
        bail!("refusing to modify occupied worktree or branch: {busy}");
    }
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Best-effort fetch of the base/branch from origin if `origin` remote is configured.
    let has_origin = aida_core::git_ops::host_git_command(main_worktree)
        .args(["remote", "get-url", "origin"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_origin {
        let fetch_out = aida_core::git_ops::host_remote_git_command(main_worktree, "origin")
            .args(["fetch", "origin", fetch_ref])
            .output()
            .with_context(|| format!("failed to run git fetch origin {fetch_ref}"))?;
        if !fetch_out.status.success() {
            bail!(
                "git fetch origin {fetch_ref} failed: {}",
                String::from_utf8_lossy(&fetch_out.stderr).trim()
            );
        }
    }

    let resolved_start = if git_ref_exists(main_worktree, start_ref) {
        start_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{fetch_ref}")) {
        fetch_ref.to_string()
    } else if git_ref_exists(main_worktree, &format!("refs/heads/{branch}")) {
        branch.to_string()
    } else {
        "HEAD".to_string()
    };

    if wt_path.exists() {
        let out = aida_core::git_ops::host_git_command(wt_path)
            .args(["checkout", "-B", branch, &resolved_start])
            .output()
            .with_context(|| format!("failed to checkout {branch} in {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git checkout -B {branch} {resolved_start} in {} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    } else {
        let out = aida_core::git_ops::host_git_command(main_worktree)
            .args(["worktree", "add", "-B", branch])
            .arg(wt_path)
            .arg(&resolved_start)
            .output()
            .with_context(|| format!("failed to add worktree {}", wt_path.display()))?;
        if !out.status.success() {
            bail!(
                "git worktree add -B {branch} {} {resolved_start} failed: {}",
                wt_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }

    // Ensure `.aida` inside the worktree links back to `main_worktree/.aida`
    // so the implementer sees the project config, sessions, and mailbox.
    let wt_aida = wt_path.join(".aida");
    if !wt_aida.exists() && std::fs::symlink_metadata(&wt_aida).is_err() {
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(main_worktree.join(".aida"), &wt_aida);
    }
    Ok(())
}

/// Build the scrubbed `Command` that launches an implementer worker in `wt_path`
/// with the session grant and purpose environment variables cleared and
/// `AIDA_SESSION_ROLE="implementer"`.
// trace:STORY-1917 | ai:agy
pub(crate) fn build_implementer_launch_command(
    agent_bin: &Path,
    main_worktree: &Path,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    name: &str,
    vendor: DispatchVendor,
) -> std::process::Command {
    let mut cmd = std::process::Command::new(agent_bin);
    cmd.current_dir(wt_path);
    match vendor {
        DispatchVendor::Claude => {
            cmd.args(["--bg", "-n", name, prompt_text]);
        }
        DispatchVendor::Codex => {
            cmd.args(["exec", "--full-auto", "-C"])
                .arg(wt_path)
                .arg(prompt_text);
        }
        DispatchVendor::Agy => {
            cmd.args(["--prompt", prompt_text]);
        }
    }
    // Critical Boundary B requirement: implementer lanes never inherit or hold
    // any session grant, purpose, or dispatch worker claim token.
    cmd.env(seat_authority::GRANT_ENV, "")
        .env(seat_authority::GRANT_FILE_ENV, "")
        .env("AIDA_SESSION_PURPOSE", "")
        .env_remove(DISPATCH_CLAIM_TOKEN_ENV)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_SESSION_SCOPE", spec)
        .env("AIDA_PROJECT_ROOT", main_worktree)
        .env("AIDA_AGENT_NAME", name)
        .env("AIDA_USER", name);
    cmd
}

#[allow(clippy::too_many_arguments)]
fn run_take_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    pr: u64,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body = match verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)
    {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("frozen brief verification failed: {err:#}"),
            }
        }
    };

    let wt_name = format!("take-{pr}");
    let default_wt = worktrees_root(main_worktree).join(&wt_name);
    // Reuse an existing worktree that already has `branch` checked out when
    // `take-<pr>` does not exist yet (matching `take.sh`).
    let wt_path = if !default_wt.exists() {
        find_worktree_for_branch(main_worktree, branch).unwrap_or(default_wt)
    } else {
        default_wt
    };
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch takeover on PR #{pr}: {busy}"),
        };
    }

    let start_ref = format!("origin/{branch}");
    if let Err(err) = prepare_lane_worktree(main_worktree, &wt_path, branch, branch, &start_ref) {
        return RunnerOutcome::Failed {
            summary: format!("takeover worktree setup failed for PR #{pr}: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action, &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render takeover prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        &wt_name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_impl_verb(
    main_worktree: &Path,
    req: &DispatchRequest,
    name: &str,
    branch: &str,
    spec: &str,
    brief_relpath: &str,
    brief_sha256: &str,
    vendor: Option<DispatchVendor>,
    now: DateTime<Utc>,
) -> RunnerOutcome {
    let brief_body = match verify_frozen_brief(main_worktree, &req.id, brief_relpath, brief_sha256)
    {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("frozen brief verification failed: {err:#}"),
            }
        }
    };

    let wt_path = worktrees_root(main_worktree).join(name);
    if let Some(busy) =
        check_live_worktree_or_branch_occupant(main_worktree, &wt_path, Some(branch), Some(spec))
    {
        return RunnerOutcome::Refused {
            reason: format!("cannot launch impl lane `{name}` ({spec}): {busy}"),
        };
    }

    let default_branch = match resolve_verified_local_default_branch(main_worktree) {
        Ok(b) => b,
        Err(err) => {
            return RunnerOutcome::Refused {
                reason: format!("default branch verification failed for `{name}`: {err:#}"),
            };
        }
    };
    let default_remote_ref = format!("origin/{default_branch}");
    if let Err(err) = prepare_lane_worktree(
        main_worktree,
        &wt_path,
        branch,
        &default_branch,
        &default_remote_ref,
    ) {
        return RunnerOutcome::Failed {
            summary: format!("impl worktree setup failed for `{name}`: {err:#}"),
            exit_code: None,
        };
    }

    let prompt_text =
        match render_lane_prompt(main_worktree, &wt_path, &req.action, &brief_body, now) {
            Ok(p) => p,
            Err(err) => {
                return RunnerOutcome::Failed {
                    summary: format!("failed to render impl prompt: {err:#}"),
                    exit_code: None,
                }
            }
        };
    let rendered_prompt_path = briefs_dir(main_worktree).join(format!("{}.prompt.md", req.id));
    if let Err(err) = write_private_atomic(&rendered_prompt_path, prompt_text.as_bytes()) {
        return RunnerOutcome::Failed {
            summary: format!("failed to write rendered prompt: {err:#}"),
            exit_code: None,
        };
    }

    launch_implementer_lane(
        main_worktree,
        req,
        &wt_path,
        spec,
        &prompt_text,
        name,
        vendor.unwrap_or(DispatchVendor::Claude),
    )
}

fn parse_spawned_bg_session_id(stdout: &str) -> Option<String> {
    if let Some(id) = crate::agent_launch::parse_bg_session_id(stdout) {
        return Some(id);
    }
    for line in stdout.lines() {
        if let Some((_, rest)) = line.split_once("claude logs ") {
            let token = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return Some(token.to_string());
            }
        }
    }
    None
}

fn launch_implementer_lane(
    main_worktree: &Path,
    req: &DispatchRequest,
    wt_path: &Path,
    spec: &str,
    prompt_text: &str,
    lane_name: &str,
    vendor: DispatchVendor,
) -> RunnerOutcome {
    let agent_bin = resolve_vendor_bin(vendor);

    let mut cmd = build_implementer_launch_command(
        &agent_bin,
        main_worktree,
        wt_path,
        spec,
        prompt_text,
        lane_name,
        vendor,
    );

    match cmd.output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let combined = format!("{stdout}{stderr}");
            let _ = append_dispatch_log(main_worktree, &req.id, &combined);
            let code = out.status.code();
            if out.status.success() {
                let bg_id = parse_spawned_bg_session_id(&stdout);
                RunnerOutcome::Done {
                    summary: format!(
                        "launched {} {} in {}",
                        req.action.verb_str(),
                        req.action.subject_label(),
                        wt_path.display()
                    ),
                    spawned_session: bg_id,
                    exit_code: code,
                }
            } else {
                RunnerOutcome::Failed {
                    summary: format!(
                        "{} {} launcher exited with {:?}: {}",
                        req.action.verb_str(),
                        req.action.subject_label(),
                        code,
                        stderr.lines().next().unwrap_or("launch failed").trim()
                    ),
                    exit_code: code,
                }
            }
        }
        Err(err) => RunnerOutcome::Failed {
            summary: format!("failed to spawn implementer launcher: {err:#}"),
            exit_code: None,
        },
    }
}

#[cfg(test)]
#[path = "tests/story_1917_dispatch_request_tests.rs"]
mod story_1917_dispatch_request_tests;

#[cfg(test)]
#[path = "tests/story_1917_dispatch_exec_tests.rs"]
mod story_1917_dispatch_exec_tests;

#[cfg(test)]
#[path = "tests/story_1917_dispatch_flow_schedule_tests.rs"]
mod story_1917_dispatch_flow_schedule_tests;

/// Queue and inspect typed dispatch requests (`flow`, `review`, `take`,
/// `impl`, `train`) from a sandboxed advisor seat for unsandboxed execution.
// trace:STORY-1917 | ai:agy
#[derive(clap::Parser, Debug)]
#[clap(name = "dispatch")]
pub(crate) struct DispatchArgs {
    #[clap(subcommand)]
    pub action: crate::cli::DispatchAction,
}

fn run_dispatch(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> anyhow::Result<()> {
    let args = <DispatchArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_dispatch(&args.action)
}

// trace:STORY-1917 | ai:agy
pub(crate) static DISPATCH: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "dispatch",
        help_after: "fleet",
        command: <DispatchArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_dispatch),
    };
