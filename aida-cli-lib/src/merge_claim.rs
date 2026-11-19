//! One merge hand per PR (STORY-2905 S1t, signed sketch decision 2). Before
//! any AIDA path merges a PR, it claims the PR on this host; a PR another
//! hand holds (a ship, a drain, a land run, or a train that boarded it) is
//! refused rather than merged twice. This closes the race in which the
//! auto-train and a review-gated lander both took the same PR.
//!
//! Claims are host-local (advisor call (b)): only the primary host merges,
//! and `--match-head-commit` on every forge merge is the cross-host net.
//! They are written and read under an exclusive lock on
//! `<main>/.aida/coordination.lock`, the lock domain STORY-2902's
//! head-mutation boundary will share. A claim whose holder is positively
//! dead (its process is gone, or its pid now names another process) may be
//! taken over; one whose holder cannot be judged refuses.
// trace:STORY-2905 | ai:claude

use std::path::{Path, PathBuf};

/// Which merge path holds a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hand {
    Ship,
    Drain,
    Burndown,
    Release,
    Train,
}

impl Hand {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ship => "ship",
            Self::Drain => "drain",
            Self::Burndown => "burndown",
            Self::Release => "release",
            Self::Train => "train",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct Record {
    pr: u64,
    hand: String,
    pid: u32,
    /// The holder's kernel start identity: with `pid`, who holds it.
    start: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    at: String,
}

/// A held claim; released when dropped (only while it is still ours).
#[derive(Debug)]
pub(crate) struct ClaimGuard {
    root: PathBuf,
    path: PathBuf,
    pid: u32,
    start: String,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        let Ok(_lock) = lock(&self.root) else {
            return;
        };
        if let Ok(Some(record)) = read(&self.path) {
            if record.pid == self.pid && record.start == self.start {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn main_root(root: &Path) -> PathBuf {
    crate::worktree_gc::main_worktree_root_from(root)
}

fn claims_dir(root: &Path) -> PathBuf {
    main_root(root).join(".aida").join("land").join("claims")
}

fn claim_file(root: &Path, pr: u64) -> PathBuf {
    claims_dir(root).join(format!("PR-{pr}.json"))
}

/// The coordination lock, held exclusively for every claim read-and-write.
struct Lock(std::fs::File);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn lock(root: &Path) -> std::io::Result<Lock> {
    use fs2::FileExt;
    let path = main_root(root).join(".aida").join("coordination.lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)?;
    file.lock_exclusive()?;
    Ok(Lock(file))
}

fn read(path: &Path) -> std::io::Result<Option<Record>> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(std::io::Error::other(format!(
                "{} is not a regular file",
                path.display()
            )))
        }
        Ok(_) => {}
    }
    let body = std::fs::read_to_string(path)?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|e| std::io::Error::other(format!("{} is malformed: {e}", path.display())))
}

/// Whether a claim's holder is alive: `Some(true)` the same process still
/// runs, `Some(false)` it is positively gone (no such process, or its pid
/// names a process with another start identity), `None` cannot be told.
fn holder_alive(record: &Record, identity: &dyn Fn(u32) -> Option<String>) -> Option<bool> {
    match identity(record.pid) {
        Some(now) => Some(now == record.start),
        None if cfg!(target_os = "linux") && Path::new("/proc/self").exists() => {
            Some(Path::new(&format!("/proc/{}", record.pid)).exists())
        }
        None => None,
    }
}

/// PURE: whether `existing` stops a new claim by `pid`/`start`, and why.
fn blocked_by(
    existing: Option<&Record>,
    hand: Hand,
    pid: u32,
    start: &str,
    identity: &dyn Fn(u32) -> Option<String>,
) -> Option<String> {
    let record = existing?;
    // Re-entrant only for the same hand in the same process: one process
    // running a train and a ship of the same PR is still two hands.
    if record.pid == pid && record.start == start && record.hand == hand.as_str() {
        return None;
    }
    match holder_alive(record, identity) {
        Some(false) => None,
        Some(true) => Some(format!(
            "it is held by the {} (pid {}) since {}",
            record.hand, record.pid, record.at
        )),
        None => Some(format!(
            "it is claimed by the {} (pid {}), whose liveness cannot be checked on this host",
            record.hand, record.pid
        )),
    }
}

/// Claim PR `pr` for `hand` on this host, or say why not: another live
/// hand holds it, a train has it pinned (for any hand but the train), or
/// the claim store cannot be read. The claim lasts as long as the guard.
pub(crate) fn claim(
    root: &Path,
    pr: u64,
    head: Option<&str>,
    hand: Hand,
) -> Result<ClaimGuard, String> {
    let pid = std::process::id();
    let start = aida_core::liveness::process_start_identity(pid)
        .ok_or("this process's start identity cannot be read, so it cannot hold a claim")?;
    let refuse = |why: String| format!("PR-{pr} is not merged here: {why}");
    let _lock = lock(root).map_err(|e| refuse(format!("the claim lock cannot be taken ({e})")))?;
    if hand != Hand::Train {
        let pins = crate::pr_train::pr_head_pins(&main_root(root), pr)
            .map_err(|e| refuse(format!("its train pins cannot be read ({e:#})")))?;
        if !pins.is_empty() {
            return Err(refuse(format!("it is boarded on {}", pins.join(", "))));
        }
    }
    let path = claim_file(root, pr);
    let existing = read(&path).map_err(|e| refuse(format!("its claim cannot be read ({e})")))?;
    if let Some(why) = blocked_by(
        existing.as_ref(),
        hand,
        pid,
        &start,
        &aida_core::liveness::process_start_identity,
    ) {
        return Err(refuse(why));
    }
    let record = Record {
        pr,
        hand: hand.as_str().to_string(),
        pid,
        start: start.clone(),
        head: head.map(str::to_string),
        at: chrono::Utc::now().to_rfc3339(),
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(claims_dir(root))?;
        let tmp = path.with_extension(format!("tmp.{pid}"));
        std::fs::write(&tmp, serde_json::to_vec(&record)?)?;
        std::fs::rename(&tmp, &path)
    };
    write().map_err(|e| refuse(format!("its claim cannot be written ({e})")))?;
    Ok(ClaimGuard {
        root: root.to_path_buf(),
        path,
        pid,
        start,
    })
}

#[cfg(test)]
#[path = "tests/story_2905_merge_claim_tests.rs"]
mod tests;
