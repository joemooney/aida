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
// mandate-gate

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
    /// Who holds it: the shared process-identity record.
    holder: aida_core::proc_identity::HolderRecord,
    /// The authority it was taken under, for the audit: `grant:<id>` (the
    /// validated seat grant), `operator` (at a terminal), or `none`.
    authority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    at: String,
}

/// A held claim; released when dropped (only while it is still ours).
#[derive(Debug)]
pub(crate) struct ClaimGuard {
    root: PathBuf,
    path: PathBuf,
    holder: aida_core::proc_identity::ProcIdentity,
    hand: Hand,
    /// Whether this guard took the claim (a re-entrant claim of a PR this
    /// hand already holds in this process releases nothing).
    releases: bool,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if !self.releases {
            return;
        }
        let Ok(_lock) = lock(&self.root) else {
            return;
        };
        if let Ok(Some(record)) = read(&self.path) {
            if record.holder.identity() == Some(&self.holder) && record.hand == self.hand.as_str() {
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

/// Whether a claim's holder is alive, from the shared process-identity
/// observation: `Some(true)` it still runs, `Some(false)` it is positively
/// gone (exited, or its pid reused), `None` it cannot be judged here.
fn holder_alive(record: &Record) -> Option<bool> {
    use aida_core::proc_identity::Verdict;
    match record.holder.observe() {
        Ok(Verdict::Alive) => Some(true),
        Ok(Verdict::Dead(_)) => Some(false),
        _ => None,
    }
}

/// PURE: whether `existing` stops `ours` claiming the PR as `hand`, and why.
/// `alive` judges the existing holder.
fn blocked_by(
    existing: Option<&Record>,
    hand: Hand,
    ours: &aida_core::proc_identity::ProcIdentity,
    alive: &dyn Fn(&Record) -> Option<bool>,
) -> Option<String> {
    let record = existing?;
    // Re-entrant only for the same hand in the same process: one process
    // running a train and a ship of the same PR is still two hands.
    if record.holder.identity() == Some(ours) && record.hand == hand.as_str() {
        return None;
    }
    let pid = record
        .holder
        .identity()
        .map_or_else(|| "unknown".to_string(), |id| id.pid.to_string());
    match alive(record) {
        Some(false) => None,
        Some(true) => Some(format!(
            "it is held by the {} (pid {pid}) since {}",
            record.hand, record.at
        )),
        None => Some(format!(
            "it is claimed by the {} (pid {pid}), whose liveness cannot be checked on this host",
            record.hand
        )),
    }
}

/// Why a PR boarded on a train is not merged by another hand, with the
/// recovery: wait for the train, or release a train nothing drives.
// trace:STORY-2905 | ai:claude
fn boarded_refusal(pins: &[(String, String)]) -> String {
    let trains: Vec<&str> = pins.iter().map(|(train, _)| train.as_str()).collect();
    let release: Vec<String> = pins
        .iter()
        .map(|(_, journal)| format!("`aida pr train --abandon {journal}`"))
        .collect();
    format!(
        "it is boarded on {}; wait for that train to finish (`aida pr train --status` shows \
         it), or, when no process drives it, release it with {} and merge again",
        trains.join(", "),
        release.join(" and ")
    )
}

/// The authority this process acts under, as a claim records it.
fn authority(root: &Path) -> String {
    if crate::gate::role_authority::has_integrity_floor_authority() {
        return "operator".to_string();
    }
    crate::gate::seat_authority::current_grant(root)
        .map_or_else(|| "none".to_string(), |g| format!("grant:{}", g.id))
}

/// Which trains have boarded PR `pr`, as `(train, journal file)`. The train
/// journal belongs to the command layer, so the caller outside the gate
/// supplies the reader (`crate::pr_train::pr_head_pin_journals`); the gate
/// takes no dependency on it (promotion 23 review F3).
// trace:STORY-2905 | ai:claude
pub(crate) type TrainPins<'a> = &'a dyn Fn(&Path, u64) -> anyhow::Result<Vec<(String, String)>>;

/// Claim PR `pr` for `hand` on this host, or say why not: another live
/// hand holds it, a train has it pinned (for any hand but the train, read
/// through `pins`; a non-train hand with no reader refuses), or the claim
/// store cannot be read. The claim lasts as long as the guard.
pub(crate) fn claim(
    root: &Path,
    pr: u64,
    head: Option<&str>,
    hand: Hand,
    pins: Option<TrainPins<'_>>,
) -> Result<ClaimGuard, String> {
    let pid = std::process::id();
    let refuse = |why: String| format!("PR-{pr} is not merged here: {why}");
    let holder = aida_core::proc_identity::HolderRecord::capture(pid);
    let ours = holder
        .validate()
        .map_err(|e| refuse(format!("this process cannot hold a claim ({e:?})")))?
        .clone();
    let _lock = lock(root).map_err(|e| refuse(format!("the claim lock cannot be taken ({e})")))?;
    if hand != Hand::Train {
        let read = pins.ok_or_else(|| refuse("its train pins cannot be read".to_string()))?;
        let pins = read(&main_root(root), pr)
            .map_err(|e| refuse(format!("its train pins cannot be read ({e:#})")))?;
        if !pins.is_empty() {
            return Err(refuse(boarded_refusal(&pins)));
        }
    }
    let path = claim_file(root, pr);
    let existing = read(&path).map_err(|e| refuse(format!("its claim cannot be read ({e})")))?;
    if let Some(why) = blocked_by(existing.as_ref(), hand, &ours, &holder_alive) {
        return Err(refuse(why));
    }
    // Already ours for this hand: the outer claim keeps (and releases) it.
    if existing
        .as_ref()
        .is_some_and(|r| r.holder.identity() == Some(&ours) && r.hand == hand.as_str())
    {
        return Ok(ClaimGuard {
            root: root.to_path_buf(),
            path,
            holder: ours,
            hand,
            releases: false,
        });
    }
    let record = Record {
        pr,
        hand: hand.as_str().to_string(),
        holder,
        authority: authority(root),
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
        holder: ours,
        hand,
        releases: true,
    })
}

/// The train's boarding (STORY-2905): claim each of `members` (by PR and
/// head) for the train; those another hand holds are left off, each with
/// why. Returns the guards (held while the train runs), the members that
/// boarded, and the refused ones.
// trace:STORY-2905 | ai:claude
#[allow(clippy::type_complexity)]
pub(crate) fn board<T>(
    root: &Path,
    members: Vec<T>,
    key: impl Fn(&T) -> (u64, String),
) -> (Vec<ClaimGuard>, Vec<T>, Vec<(u64, String)>) {
    let mut guards = Vec::new();
    let mut boarded = Vec::new();
    let mut refused = Vec::new();
    for member in members {
        let (pr, head) = key(&member);
        match claim(root, pr, Some(&head), Hand::Train, None) {
            Ok(guard) => {
                guards.push(guard);
                boarded.push(member);
            }
            Err(why) => refused.push((pr, why)),
        }
    }
    (guards, boarded, refused)
}

/// Journal recovery (STORY-2905 round 1): every car of a retained train is
/// claimed for the train again before the recovery mutates or lands
/// anything; any car another hand holds, or whose claim cannot be read or
/// judged, refuses the whole recovery.
// trace:STORY-2905 | ai:claude
pub(crate) fn claim_all(root: &Path, prs: &[u64]) -> Result<Vec<ClaimGuard>, String> {
    prs.iter()
        .map(|pr| claim(root, *pr, None, Hand::Train, None))
        .collect()
}

#[cfg(test)]
#[path = "tests/story_2905_merge_claim_tests.rs"]
mod tests;
