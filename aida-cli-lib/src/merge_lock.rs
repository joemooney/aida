//! STORY-1171: the branch merge-serialization lock (the "merge-lease").
//!
//! A per-target-branch lockfile at the MAIN clone's `.aida/merge-locks/<branch>`
//! that a merger ACQUIRES before the check→merge→pull critical section, holds
//! through it, and releases after — so no two mergers race to the same branch.
//! This SERIALIZES mergers; it is distinct from, and complements, the per-PR
//! `merge_hold` marker + Layer-2 branch protection (which GATE one PR but do not
//! serialize two green PRs). It is also distinct from git's transient
//! `index.lock` (which correctly does NOT block targeted single-spec store
//! writes). See ADR-38.
//!
//! Scope (ADR-38 decision 1): LOCAL file, same-machine mergers only — that is the
//! race we actually observed (advisor session + product session + drain, one
//! machine). Cross-host fleet serialization is a future extension.
//!
//! Contention (decision 2): bounded-wait-then-refuse. Stale-lock recovery mirrors
//! the drain-lock (coordination.rs) / index.lock (BUG-1164): a same-host DEAD pid
//! is reclaimed immediately; unknown or refused holders require operator recovery.
// trace:STORY-1171 | ai:claude

use aida_core::proc_identity::{HolderRecord, HolderRefusal, UnknownReason, Verdict};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Bounded wait before a contending merger gives up and REFUSES (decision 2).
pub(crate) const DEFAULT_WAIT: Duration = Duration::from_secs(180);
/// Poll cadence while waiting for a held lock to free.
const POLL: Duration = Duration::from_secs(2);
/// Compatibility field for older readers: disable their wall-time TTL backstop.
/// New readers reclaim only a proven dead admitted process. Retire old binaries
/// before rollback; new records also fence their PID probes with a versioned
/// namespace spelling that old readers treat as foreign.
// trace:TASK-3364 | ai:codex
const DEFAULT_TTL_SECS: u64 = i64::MAX as u64;
/// A lease file with no complete record (empty or cut short) is a writer
/// mid-publish — `acquire` before TASK-1-421 created the file, then wrote it.
/// It reads live for this long after its mtime, and stale after.
// trace:TASK-1955 | ai:claude (TASK-1-421)
const PARTIAL_GRACE_SECS: u64 = 30;

fn locks_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("merge-locks")
}

/// Permanent per-branch mutation lock: compare/remove and publish share it.
/// Never swept as an orphan publication and never unlinked.
// trace:TASK-3364 | ai:codex
fn mutation_guard(path: &Path, wait: Duration) -> std::io::Result<std::fs::File> {
    use fs2::FileExt;
    let name = path.file_name().unwrap().to_string_lossy();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_file_name(format!(".guard-{name}")))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(
                    Duration::from_millis(10)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Err(e) => return Err(e),
        }
    }
}

/// Branch → single path segment (feature branches contain `/`).
fn branch_slug(branch: &str) -> String {
    branch
        .chars()
        .map(|c| {
            if c == '/' || c == std::path::MAIN_SEPARATOR {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// The lockfile path for one target branch. Public so callers/status can log it.
pub(crate) fn lock_path(project_root: &Path, branch: &str) -> PathBuf {
    locks_dir(project_root).join(branch_slug(branch))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The recorded holder of a merge-lease — enough for stale-recovery + `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Holder {
    /// Cooperative admission and generation; absent legacy records remain unknown.
    // trace:TASK-3364 | ai:codex
    pub process: Option<HolderRecord>,
    pub host: String,
    pub pid: u32,
    /// Kernel start time of `pid` when it took the lease, so a recycled pid
    /// does not read as the live holder. `None` for a lockfile written before
    /// TASK-1750; incomplete legacy identity remains unknown.
    // trace:TASK-1750 | ai:claude
    pub pid_start: Option<String>,
    /// PID namespace inode of the holder (`/proc/self/ns/pid`), so a pid
    /// written from inside a sandbox is never probed from outside it, nor the
    /// reverse. `None` for a lockfile written before BUG-2417: its holder is
    /// then unknown and never judged dead by pid.
    // trace:BUG-2417 | ai:claude
    pub pid_ns: Option<String>,
    /// Boot id of the holder; missing legacy identity remains unknown.
    // trace:BUG-2417 | ai:claude
    pub boot: Option<String>,
    pub pr: Option<u64>,
    pub note: String,
    pub acquired_at: u64,
    pub ttl_secs: u64,
}

fn serialize(h: &Holder) -> String {
    format!(
        "process={}\nhost={}\npid={}\npid_start={}\npid_ns={}\nboot={}\npr={}\nnote={}\nacquired_at={}\nttl_secs={}\n",
        h.process.as_ref().map(|p| serde_json::to_string(p).expect("holder JSON")).unwrap_or_default(),
        h.host,
        h.pid,
        h.pid_start.as_deref().unwrap_or_default(),
        h.pid_ns.as_deref().unwrap_or_default(),
        h.boot.as_deref().unwrap_or_default(),
        h.pr.map(|p| p.to_string()).unwrap_or_default(),
        h.note.replace('\n', " "),
        h.acquired_at,
        h.ttl_secs,
    )
}

/// Lenient parse — an unreadable/garbled field falls back to a safe default so a
/// corrupt lockfile remains unknown and requires explicit operator recovery.
fn parse(body: &str) -> Holder {
    let mut h = Holder {
        process: None,
        host: String::new(),
        pid: 0,
        pid_start: None,
        pid_ns: None,
        boot: None,
        pr: None,
        note: String::new(),
        acquired_at: 0,
        ttl_secs: DEFAULT_TTL_SECS,
    };
    for line in body.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            match k.trim() {
                "process" => h.process = serde_json::from_str(v).ok(),
                "host" => h.host = v.to_string(),
                "pid" => h.pid = v.parse().unwrap_or(0),
                "pid_start" => h.pid_start = Some(v.to_string()).filter(|v| !v.is_empty()),
                "pid_ns" => h.pid_ns = Some(v.to_string()).filter(|v| !v.is_empty()),
                "boot" => h.boot = Some(v.to_string()).filter(|v| !v.is_empty()),
                "pr" => h.pr = v.parse().ok(),
                "note" => h.note = v.to_string(),
                "acquired_at" => h.acquired_at = v.parse().unwrap_or(0),
                "ttl_secs" => h.ttl_secs = v.parse().unwrap_or(DEFAULT_TTL_SECS),
                _ => {}
            }
        }
    }
    h
}

/// Observe only a cooperatively admitted generation. Hostnames and legacy PID
/// fields cannot establish node provenance or upgrade a recycled process.
// trace:TASK-3364 | ai:codex
fn holder_verdict(h: &Holder) -> Result<Verdict, HolderRefusal> {
    let Some(record) = &h.process else {
        return Ok(Verdict::ForeignUnknown(UnknownReason::MissingIdentity));
    };
    let id = record.validate()?;
    if h.pid != id.pid
        || h.pid_start.as_deref() != Some(format!("linux-starttime:{}", id.starttime).as_str())
        || h.pid_ns.as_deref().and_then(|ns| {
            ns.strip_prefix("proc-identity-v1:")
                .unwrap_or(ns)
                .parse()
                .ok()
        }) != Some(id.pidns_inode)
        || h.boot.as_deref() != Some(id.boot_id.as_str())
    {
        return Err(HolderRefusal::IdentityChanged);
    }
    record.observe()
}

/// Preserve the same admitted holder generation in the ownership shadow store.
// trace:TASK-3364 | ai:codex
pub(crate) fn holder_witness(
    h: &Holder,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::ownership::Witness> {
    holder_verdict(h).ok()?;
    let record = h.process.as_ref()?;
    let id = record.validate().ok()?;
    Some(crate::ownership::Witness {
        kind: crate::ownership::WitnessKind::Process {
            host: h.host.clone(),
            id: id.legacy(),
            pid_ns: Some(id.pidns_inode),
        },
        admission: Some(record.clone()),
        recorded_at: now,
    })
}

/// A record is complete once its last field, `ttl_secs`, is present.
fn is_complete_record(body: &str) -> bool {
    body.lines().any(|l| l.starts_with("ttl_secs="))
}

/// `Some(reason)` if the lease file at `path` is reclaimable. An incomplete
/// record is live inside [`PARTIAL_GRACE_SECS`] of its mtime: reading it as
/// `acquired_at=0` would judge a just-created lease TTL-stale and steal it.
// trace:TASK-1955 | ai:claude (TASK-1-421)
fn file_stale_reason(path: &Path, our_host: &str) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    if is_complete_record(&body) {
        return stale_reason(&parse(&body), our_host);
    }
    let age = mtime_distance_secs(path)?;
    (age > PARTIAL_GRACE_SECS).then(|| {
        format!("incomplete lease record aged {age}s past its {PARTIAL_GRACE_SECS}s grace")
    })
}

/// How far `path`'s mtime is from now, either way: a future mtime (clock skew,
/// a copied file) is as far off as a past one, so it cannot read live forever.
// trace:TASK-1955 | ai:claude (TASK-1-421)
fn mtime_distance_secs(path: &Path) -> Option<u64> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let now = SystemTime::now();
    let d = now
        .duration_since(mtime)
        .or_else(|_| mtime.duration_since(now))
        .ok()?;
    Some(d.as_secs())
}

/// Remove the temp records for `path` (this module's and release.sh's
/// `.<name>.*`) orphaned by a writer killed before its link: any aged past
/// [`PARTIAL_GRACE_SECS`]. A live writer links within moments.
// trace:TASK-1955 | ai:claude (TASK-1-422)
fn sweep_orphan_temps(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let prefix = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        if file.starts_with(&prefix)
            && mtime_distance_secs(&e.path()).is_some_and(|age| age > PARTIAL_GRACE_SECS)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Test seam: act as a filesystem without hard links.
    static NO_HARD_LINKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn link_record(tmp: &Path, path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if NO_HARD_LINKS.with(std::cell::Cell::get) {
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    std::fs::hard_link(tmp, path)
}

/// Publish `record` at `path` with `O_EXCL` where the filesystem has no hard
/// links. A reader may briefly see it partial, which [`file_stale_reason`]
/// reads as live inside its grace; a failed write removes the file.
// trace:TASK-1955 | ai:claude (TASK-1-421)
fn publish_exclusive(path: &Path, record: &str) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(record.as_bytes()).inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

/// The temp file a lease record is written to before it is linked into place.
/// Dot-prefixed, so `status` skips it and it never names a branch.
// trace:TASK-1955 | ai:claude (TASK-1-422)
fn temp_path(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()))
}

/// Expiry is not process death. Complete legacy, unknown and refused records
/// require operator recovery; even a wall-clock jump never steals a live merger.
// trace:TASK-3364 | ai:codex
fn stale_reason(h: &Holder, _our_host: &str) -> Option<String> {
    match holder_verdict(h) {
        Ok(Verdict::Dead(reason)) => Some(format!("holder process ended: {reason:?}")),
        _ => None,
    }
}

/// RAII guard: dropping it removes the lockfile IF we hold it. Release early
/// with `drop(lease)`.
#[must_use = "the merge-lease releases when dropped; hold it across the critical section"]
#[derive(Debug)]
pub(crate) struct MergeLease {
    path: PathBuf,
    held: bool,
    /// Exact publication token, also used to fence drop against replacement.
    record: String,
    /// The main worktree, when this is a train's lease: its ownership record
    /// is released with it (dual-write).
    // trace:STORY-2325 | ai:claude
    train_root: Option<PathBuf>,
}

impl MergeLease {
    // trace:BUG-b6b1278890774ef993d2ebc327862470-179 | ai:codex
    /// Validate the live guard as well as the on-disk holder before a train attempt.
    pub(crate) fn covers(&self, root: &Path, base: &str, pr: u64) -> bool {
        if !self.held || self.path != lock_path(root, base) {
            return false;
        }
        std::fs::read_to_string(&self.path)
            .ok()
            .is_some_and(|body| {
                let h = parse(&body);
                body == self.record
                    && is_complete_record(&body)
                    && h.pr == Some(pr)
                    && h.note == crate::pr_train::TRAIN_LEASE_NOTE
                    && h.pid == std::process::id()
                    && matches!(holder_verdict(&h), Ok(Verdict::Alive))
            })
    }

    /// The lock file this lease holds; ship hands it to its cleanup worker so
    /// the worker can record that the lease is gone before it starts.
    // trace:BUG-2384 | ai:claude
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    // trace:TASK-1581 | ai:antigravity — the explicit `release(self)` had no
    // caller; `drop(lease)` is the same operation.
    fn do_release(&mut self) {
        if self.held {
            let Ok(_guard) = mutation_guard(&self.path, Duration::from_secs(2)) else {
                return;
            };
            if std::fs::read_to_string(&self.path).ok().as_deref() != Some(&self.record) {
                self.held = false;
                return;
            }
            if std::fs::remove_file(&self.path).is_err() {
                return;
            }
            self.held = false;
            // trace:STORY-2325 | ai:claude
            if let Some(root) = self.train_root.take() {
                crate::ownership::dual::train_lease_released(&root);
            }
        }
    }
}

impl Drop for MergeLease {
    fn drop(&mut self) {
        self.do_release();
    }
}

/// Acquire the merge-lease for `branch`, waiting up to `wait` on contention
/// (decision 2). Reclaims a proven dead admitted holder.
/// Returns `WouldBlock` if a live holder outlasts the wait — the caller REFUSES.
pub(crate) fn acquire(
    project_root: &Path,
    branch: &str,
    pr: Option<u64>,
    note: &str,
    wait: Duration,
) -> std::io::Result<MergeLease> {
    let dir = locks_dir(project_root);
    std::fs::create_dir_all(&dir)?;
    let path = lock_path(project_root, branch);
    let our_host = crate::coordination::hostname();
    let deadline = Instant::now() + wait;
    sweep_orphan_temps(&path);
    // Capture self once, before publishing; invalid/subreaper holders refuse.
    // trace:TASK-3364 | ai:codex
    let process = HolderRecord::capture(std::process::id());
    let id = process
        .validate()
        .map_err(|why| std::io::Error::other(format!("merge-lease holder refused: {why:?}")))?
        .clone();
    loop {
        let guard = mutation_guard(&path, deadline.saturating_duration_since(Instant::now()))?;
        let h = Holder {
            process: Some(process.clone()),
            host: our_host.clone(),
            pid: std::process::id(),
            pid_start: Some(format!("linux-starttime:{}", id.starttime)),
            pid_ns: Some(format!("proc-identity-v1:{}", id.pidns_inode)),
            boot: Some(id.boot_id.clone()),
            pr,
            note: note.to_string(),
            acquired_at: now_secs(),
            ttl_secs: DEFAULT_TTL_SECS,
        };
        // Write the full record to a temp file, then hard-link it into place:
        // link(2) fails if the lease exists, so the claim stays atomic and no
        // reader (release.sh included) sees an empty or partial lease.
        // trace:TASK-1955 | ai:claude (TASK-1-421)
        // A filesystem without hard links falls back to O_EXCL.
        let record = serialize(&h);
        let tmp = temp_path(&path);
        let linked = std::fs::File::create(&tmp)
            .and_then(|mut f| f.write_all(record.as_bytes()))
            .and_then(|()| match link_record(&tmp, &path) {
                Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                    publish_exclusive(&path, &record)
                }
                linked => linked,
            });
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => {
                // Dual-write: the lock file stays authoritative. // trace:STORY-2325 | ai:claude
                let train = pr.filter(|_| note == crate::pr_train::TRAIN_LEASE_NOTE);
                if let Some(train) = train {
                    crate::ownership::dual::train_lease_taken(project_root, train, &h);
                }
                return Ok(MergeLease {
                    path,
                    held: true,
                    record,
                    train_root: train.map(|_| project_root.to_path_buf()),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let reclaimable = file_stale_reason(&path, &our_host);
                if reclaimable.is_some() {
                    // Steal the stale lock and retry immediately.
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                    continue;
                }
                if Instant::now() >= deadline {
                    let evidence = std::fs::read_to_string(&path)
                        .map(|body| format!("{:?}", holder_verdict(&parse(&body))))
                        .unwrap_or_else(|e| format!("unreadable holder: {e}"));
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        format!(
                            "merge-lease for '{branch}' is retained ({evidence}); waited {}s; inspect with `aida merge-lock`",
                            wait.as_secs()
                        ),
                    ));
                }
                drop(guard);
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Active merge-leases: exclude proven dead holders, retain unknown/refused ones.
// trace:BUG-2220 | ai:claude
pub(crate) fn live_status(project_root: &Path) -> Vec<(String, Holder)> {
    let our_host = crate::coordination::hostname();
    status(project_root)
        .into_iter()
        .filter(|(_, h)| stale_reason(h, &our_host).is_none())
        .collect()
}

/// Read-only alarm observation. Unknown records never establish no running train.
// trace:STORY-2476 | ai:codex
pub(crate) fn observed_train_running(root: &Path) -> Option<bool> {
    let entries = match std::fs::read_dir(locks_dir(root)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(false),
        Err(_) => return None,
    };
    let mut unknown = false;
    for entry in entries {
        let Ok(entry) = entry else { return None };
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(entry.path()) else {
            return None;
        };
        let h = parse(&body);
        if !is_complete_record(&body) || h.pid == 0 || h.note.is_empty() {
            unknown = true;
            continue;
        }
        if h.note != crate::pr_train::TRAIN_LEASE_NOTE {
            continue;
        }
        match holder_verdict(&h) {
            Ok(Verdict::Alive) => return Some(true),
            Ok(Verdict::Dead(_)) => {}
            Ok(Verdict::ForeignUnknown(_)) | Err(_) => unknown = true,
        }
    }
    if unknown {
        None
    } else {
        Some(false)
    }
}

/// Every active merge-lease as `(branch, holder)`, ascending by branch — the
/// `aida merge-lock status` surface.
pub(crate) fn status(project_root: &Path) -> Vec<(String, Holder)> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(locks_dir(project_root)) {
        for e in entries.flatten() {
            // Skip in-flight temp records. trace:TASK-1955 | ai:claude (TASK-1-422)
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if let Ok(body) = std::fs::read_to_string(e.path()) {
                out.push((e.file_name().to_string_lossy().to_string(), parse(&body)));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:TASK-3364 | ai:codex
    fn current_holder() -> Holder {
        let record = HolderRecord::capture(std::process::id());
        let id = record.validate().expect("test process must admit holder");
        Holder {
            process: Some(record.clone()),
            host: crate::coordination::hostname(),
            pid: id.pid,
            pid_start: Some(format!("linux-starttime:{}", id.starttime)),
            pid_ns: Some(format!("proc-identity-v1:{}", id.pidns_inode)),
            boot: Some(id.boot_id.clone()),
            pr: Some(42),
            note: crate::pr_train::TRAIN_LEASE_NOTE.into(),
            acquired_at: 0,
            ttl_secs: 1,
        }
    }
    fn change_identity(h: &mut Holder, key: &str, value: serde_json::Value) {
        let mut raw = serde_json::to_value(h.process.as_ref().unwrap()).unwrap();
        raw["identity"][key] = value;
        h.process = Some(serde_json::from_value(raw).unwrap());
        let id = h.process.as_ref().unwrap().identity().unwrap();
        h.pid = id.pid;
        h.pid_start = Some(format!("linux-starttime:{}", id.starttime));
        h.pid_ns = Some(id.pidns_inode.to_string());
        h.boot = Some(id.boot_id.clone());
    }
    #[test]
    fn identity_matrix_retains_unknown_refusal_and_live_despite_clock_age() {
        let live = current_holder();
        assert_eq!(parse(&serialize(&live)), live);
        assert_ne!(
            live.pid_ns,
            aida_core::local_pid_ns(),
            "old readers must not probe a versioned record"
        );
        let mut compat = live.clone();
        compat.ttl_secs = DEFAULT_TTL_SECS;
        assert_eq!(compat.ttl_secs, i64::MAX as u64);
        assert_eq!(holder_verdict(&live), Ok(Verdict::Alive));
        assert_eq!(
            holder_witness(&live, chrono::Utc::now()).unwrap().admission,
            live.process
        );
        let mut reboot = live.clone();
        change_identity(
            &mut reboot,
            "boot_id",
            "11111111-1111-4111-8111-111111111111".into(),
        );
        assert!(matches!(
            holder_verdict(&reboot),
            Ok(Verdict::Dead(aida_core::proc_identity::DeadReason::Reboot))
        ));
        assert_eq!(
            aida_core::CacheLockInfo {
                pid: live.pid,
                boot_id: live.boot.clone(),
                pid_ns: live.pid_ns.clone(),
                ..Default::default()
            }
            .visible_pid(),
            None,
            "legacy Rust readers must refuse numeric lookup"
        );
        for acquired_at in [0, now_secs(), u64::MAX] {
            let mut h = live.clone();
            h.acquired_at = acquired_at;
            assert_eq!(stale_reason(&h, &h.host), None);
        }
        let mut reused = live.clone();
        change_identity(&mut reused, "starttime", 0.into());
        assert!(matches!(holder_verdict(&reused), Ok(Verdict::Dead(_))));
        assert!(stale_reason(&reused, &reused.host).is_some());
        let mut foreign = live.clone();
        change_identity(&mut foreign, "pidns_inode", 1.into());
        assert_eq!(
            holder_verdict(&foreign),
            Ok(Verdict::ForeignUnknown(UnknownReason::NamespaceMismatch))
        );
        assert!(stale_reason(&foreign, &foreign.host).is_none());
        let mut other_node = live.clone();
        let mut raw = serde_json::to_value(other_node.process.unwrap()).unwrap();
        raw["node_id"] = "another-node".into();
        other_node.process = Some(serde_json::from_value(raw).unwrap());
        assert_eq!(
            holder_verdict(&other_node),
            Ok(Verdict::ForeignUnknown(UnknownReason::OtherNode))
        );
        assert!(stale_reason(&other_node, &other_node.host).is_none());
        for refusal in [
            HolderRefusal::Subreaper,
            HolderRefusal::InitOrManager,
            HolderRefusal::EvidenceUnavailable("unknown".into()),
        ] {
            let mut h = live.clone();
            h.process = Some(HolderRecord::refused(refusal.clone()));
            assert_eq!(holder_verdict(&h), Err(refusal));
            assert!(stale_reason(&h, &h.host).is_none());
        }
        let mut legacy = live.clone();
        legacy.process = None;
        assert!(matches!(
            holder_verdict(&legacy),
            Ok(Verdict::ForeignUnknown(_))
        ));
        assert!(stale_reason(&legacy, &legacy.host).is_none());
        assert!(parse("host=h\npid=42\nttl_secs=1\n").process.is_none());
        let mut mismatch = live;
        mismatch.pid_start = Some("linux-starttime:0".into());
        assert_eq!(
            holder_verdict(&mismatch),
            Err(HolderRefusal::IdentityChanged)
        );
    }
    #[test]
    fn proven_dead_child_is_reclaimed() {
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30");
        let pending = aida_core::proc_identity::prepare_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let record = pending.finish(&mut child);
        let mut h = current_holder();
        h.process = Some(record);
        let id = h.process.as_ref().unwrap().validate().unwrap();
        h.pid = id.pid;
        h.pid_start = Some(format!("linux-starttime:{}", id.starttime));
        h.pid_ns = Some(id.pidns_inode.to_string());
        h.boot = Some(id.boot_id.clone());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(matches!(holder_verdict(&h), Ok(Verdict::Dead(_))));
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(locks_dir(dir.path())).unwrap();
        std::fs::write(lock_path(dir.path(), "main"), serialize(&h)).unwrap();
        let _lease = acquire(dir.path(), "main", None, "replacement", Duration::ZERO).unwrap();
    }
    #[test]
    fn cooperative_subreaper_is_retained_as_refusal() {
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("30");
        // SAFETY: the child-only hook uses async-signal-safe prctl.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let pending = aida_core::proc_identity::prepare_command(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let record = pending.finish(&mut child);
        child.kill().unwrap();
        child.wait().unwrap();
        let mut h = current_holder();
        h.process = Some(record);
        assert_eq!(holder_verdict(&h), Err(HolderRefusal::Subreaper));
        assert!(stale_reason(&h, &h.host).is_none());
    }
    #[test]
    fn mutation_guard_respects_zero_wait() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(locks_dir(dir.path())).unwrap();
        let path = lock_path(dir.path(), "main");
        let _held = mutation_guard(&path, Duration::ZERO).unwrap();
        let err = acquire(dir.path(), "main", None, "blocked", Duration::ZERO).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(!path.exists());
    }
    #[test]
    fn covers_and_drop_are_generation_fenced() {
        let dir = tempfile::tempdir().unwrap();
        let lease = acquire(
            dir.path(),
            "main",
            Some(42),
            crate::pr_train::TRAIN_LEASE_NOTE,
            Duration::ZERO,
        )
        .unwrap();
        assert!(lease.covers(dir.path(), "main", 42));
        let replacement = serialize(&current_holder());
        std::fs::write(lease.path(), &replacement).unwrap();
        assert!(!lease.covers(dir.path(), "main", 42));
        drop(lease);
        assert_eq!(
            std::fs::read_to_string(lock_path(dir.path(), "main")).unwrap(),
            replacement
        );
    }
    #[test]
    fn train_observation_preserves_unknown_and_refusal() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(observed_train_running(dir.path()), Some(false));
        std::fs::create_dir_all(locks_dir(dir.path())).unwrap();
        let path = lock_path(dir.path(), "main");
        let mut h = current_holder();
        std::fs::write(&path, serialize(&h)).unwrap();
        assert_eq!(observed_train_running(dir.path()), Some(true));
        assert_eq!(live_status(dir.path()).len(), 1);
        change_identity(&mut h, "starttime", 0.into());
        std::fs::write(&path, serialize(&h)).unwrap();
        assert_eq!(observed_train_running(dir.path()), Some(false));
        assert!(live_status(dir.path()).is_empty());
        h.process = None;
        std::fs::write(&path, serialize(&h)).unwrap();
        assert_eq!(observed_train_running(dir.path()), None);
        assert_eq!(live_status(dir.path()).len(), 1);
        assert_eq!(
            acquire(dir.path(), "main", None, "blocked", Duration::ZERO)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn acquire_serializes_and_bounded_wait_refuses() {
        // trace:STORY-1171.ac991dc3 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let held = acquire(root, "main", Some(1), "first", DEFAULT_WAIT).unwrap();
        // A second acquire with a tiny wait must REFUSE (WouldBlock) while held.
        let start = Instant::now();
        let err = acquire(root, "main", Some(2), "second", Duration::from_millis(300)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "must actually wait the bound"
        );
        // Release → the branch is free again.
        drop(held);
        let _ = acquire(root, "main", Some(3), "third", Duration::from_millis(300)).unwrap();
    }
    #[test]
    fn drop_releases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        {
            let _g = acquire(root, "main", None, "x", DEFAULT_WAIT).unwrap();
            assert!(lock_path(root, "main").exists());
        }
        assert!(
            !lock_path(root, "main").exists(),
            "drop must remove the lockfile"
        );
    }
    #[test]
    fn task_1_421_empty_fresh_lease_is_live_until_its_grace_passes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        let path = lock_path(root, "main");
        std::fs::write(&path, "").unwrap();
        let err = acquire(root, "main", Some(2), "second", Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "", "never stolen");

        std::fs::write(&path, "host=h\npid=1\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(PARTIAL_GRACE_SECS + 5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let _g = acquire(root, "main", Some(3), "steal", Duration::from_millis(100)).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("note=steal"));
    }
    #[test]
    fn task_1_422_acquire_publishes_complete_and_status_skips_temp_records() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _g = acquire(root, "main", Some(1), "held", DEFAULT_WAIT).unwrap();
        let names: Vec<String> = std::fs::read_dir(locks_dir(root))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with(".guard-"))
            .collect();
        assert_eq!(names, vec!["main".to_string()]);
        assert!(is_complete_record(
            &std::fs::read_to_string(lock_path(root, "main")).unwrap()
        ));
        std::fs::write(locks_dir(root).join(".main.abc123"), "host=x\n").unwrap();
        let st = status(root);
        assert_eq!(st.len(), 1, "{st:?}");
        assert_eq!(st[0].0, "main");
    }
    #[test]
    fn task_1_421_incomplete_lease_with_a_future_mtime_is_stale_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(locks_dir(root)).unwrap();
        let path = lock_path(root, "main");
        std::fs::write(&path, "").unwrap();
        let future = SystemTime::now() + Duration::from_secs(PARTIAL_GRACE_SECS + 60);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(future)
            .unwrap();
        let _g = acquire(root, "main", Some(2), "steal", Duration::from_millis(100)).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("note=steal"));
    }
    #[test]
    fn task_1_422_acquire_sweeps_orphaned_temp_records() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let locks = locks_dir(root);
        std::fs::create_dir_all(&locks).unwrap();
        let old = SystemTime::now() - Duration::from_secs(PARTIAL_GRACE_SECS + 5);
        for name in [".main.4242.17.tmp", ".main.Ab12Cd"] {
            std::fs::write(locks.join(name), "host=x\n").unwrap();
            std::fs::File::options()
                .write(true)
                .open(locks.join(name))
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        std::fs::write(locks.join(".main.99.1.tmp"), "host=x\n").unwrap();
        std::fs::write(locks.join(".other.1.1.tmp"), "host=x\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(locks.join(".other.1.1.tmp"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let _g = acquire(root, "main", Some(1), "held", DEFAULT_WAIT).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&locks)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with(".guard-"))
            .collect();
        names.sort();
        assert_eq!(names, [".main.99.1.tmp", ".other.1.1.tmp", "main"]);
    }
    #[test]
    fn task_1_421_acquire_falls_back_to_o_excl_without_hard_links() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        NO_HARD_LINKS.with(|c| c.set(true));
        let held = acquire(root, "main", Some(1), "held", DEFAULT_WAIT);
        let second = acquire(root, "main", Some(2), "second", Duration::from_millis(100));
        NO_HARD_LINKS.with(|c| c.set(false));
        let _g = held.unwrap();
        assert_eq!(second.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        assert!(is_complete_record(
            &std::fs::read_to_string(lock_path(root, "main")).unwrap()
        ));
        let names: Vec<String> = std::fs::read_dir(locks_dir(root))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with(".guard-"))
            .collect();
        assert_eq!(names, ["main"], "no temp record is left behind");
    }
    #[test]
    fn status_lists_active_leases() {
        // trace:STORY-1171.ac117c5a | ai:codex
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let _a = acquire(root, "main", Some(1), "on main", DEFAULT_WAIT).unwrap();
        let _b = acquire(root, "release/2", Some(2), "on release", DEFAULT_WAIT).unwrap();
        let st = status(root);
        let branches: Vec<_> = st.iter().map(|(b, _)| b.clone()).collect();
        assert_eq!(branches, vec!["main".to_string(), "release_2".to_string()]);
        assert_eq!(st[0].1.pr, Some(1));
    }
}
