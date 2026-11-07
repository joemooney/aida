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
//! is reclaimed immediately; non-Alive holders also have a boot-clock expiry.
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
/// New readers use shared process verdicts plus a separate boot deadline. Retire old binaries
/// before rollback; new records also fence their PID probes with a versioned
/// namespace spelling that old readers treat as foreign.
// trace:TASK-3364 | ai:codex
const DEFAULT_TTL_SECS: u64 = i64::MAX as u64;
const LEASE_SECS: u64 = 600;
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
    /// Cooperative admission and generation; absent legacy evidence remains unknown.
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
    pub deadline_boot: Option<String>,
    pub deadline_boottime: Option<u64>,
}

fn serialize(h: &Holder) -> String {
    format!(
        "process={}\nhost={}\npid={}\npid_start={}\npid_ns={}\nboot={}\npr={}\nnote={}\nacquired_at={}\ndeadline_boot={}\ndeadline_boottime={}\nttl_secs={}\n",
        h.process.as_ref().map(|p| serde_json::to_string(p).expect("holder JSON")).unwrap_or_default(),
        h.host,
        h.pid,
        h.pid_start.as_deref().unwrap_or_default(),
        h.pid_ns.as_deref().unwrap_or_default(),
        h.boot.as_deref().unwrap_or_default(),
        h.pr.map(|p| p.to_string()).unwrap_or_default(),
        h.note.replace('\n', " "),
        h.acquired_at,
        h.deadline_boot.as_deref().unwrap_or_default(),
        h.deadline_boottime.map(|n| n.to_string()).unwrap_or_default(),
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
        deadline_boot: None,
        deadline_boottime: None,
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
                "deadline_boot" => {
                    h.deadline_boot = Some(v.into()).filter(|v: &String| !v.is_empty())
                }
                "deadline_boottime" => h.deadline_boottime = v.parse().ok(),
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
    body_stale_reason(path, &body, our_host)
}

fn body_stale_reason(path: &Path, body: &str, our_host: &str) -> Option<String> {
    if is_complete_record(body) {
        return stale_reason(&parse(body), our_host);
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

/// Suspend-inclusive boot clock. Wall time is diagnostic only.
// trace:TASK-3364 | ai:codex
fn boot_clock() -> std::io::Result<(String, u64)> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes to a valid timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((boot.trim().into(), time.tv_sec as u64))
}

fn stale_at(h: &Holder, clock: Option<(&str, u64)>) -> Option<String> {
    match holder_verdict(h) {
        Ok(Verdict::Alive) => return None,
        Ok(Verdict::Dead(reason)) => return Some(format!("Dead({reason:?})")),
        _ => {}
    }
    let (boot, now) = clock?;
    let deadline_boot = h.deadline_boot.as_deref()?;
    let deadline = h.deadline_boottime?;
    (boot != deadline_boot || now >= deadline).then(|| "Expired: lease expired (boot clock)".into())
}

pub(crate) fn stale_reason(h: &Holder, _our_host: &str) -> Option<String> {
    let clock = boot_clock().ok();
    stale_at(h, clock.as_ref().map(|(b, n)| (b.as_str(), *n)))
}

pub(crate) fn diagnostic(h: &Holder) -> (String, String) {
    match holder_verdict(h) {
        Ok(Verdict::Alive) => ("Alive".into(), String::new()),
        Ok(Verdict::Dead(r)) => ("Dead".into(), format!("{r:?}")),
        Ok(Verdict::ForeignUnknown(r)) => ("ForeignUnknown".into(), format!("{r:?}")),
        Err(r) => ("Refused".into(), format!("{r:?}")),
    }
}

pub(crate) fn repair_hint(path: &Path) -> String {
    format!("For legacy records, run `aida merge-lock --migrate-legacy` to arm a boot deadline. For immediate repair, stop all mergers for this branch; verify on the owning node that no merge is running; then remove only {}; preserve its .guard sidecar", path.display())
}

/// Read-only helper used by shell consumers, under their mutation guard.
pub(crate) fn inspect_file(path: &Path) -> serde_json::Value {
    match std::fs::read_to_string(path) {
        Ok(body) => {
            let h = parse(&body);
            let (verdict, reason) = diagnostic(&h);
            let stale = body_stale_reason(path, &body, "");
            serde_json::json!({"holder_verdict": verdict, "reason": reason,
                "lease_state": if !is_complete_record(&body) { "Incomplete" } else if stale.as_deref().is_some_and(|s| s.starts_with("Expired")) { "Expired" } else if verdict == "Dead" { "Dead" } else { "Retained" },
                "reclaimable": stale.is_some(), "reclaim_reason": stale, "repair_hint": repair_hint(path)})
        }
        Err(e) => {
            serde_json::json!({"reclaimable": false, "reason": e.to_string(), "repair_hint": repair_hint(path)})
        }
    }
}

/// One-time deadline migration at the mutation boundary. No legacy PID is
/// recaptured: cooperative admission cannot be retroactively established.
/// Inspections remain read-only and report repair for unrebindable identities.
fn migrate_legacy_deadline(path: &Path) -> std::io::Result<bool> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    if !is_complete_record(&body) {
        return Ok(false);
    }
    let mut h = parse(&body);
    if h.deadline_boot.is_some() || h.deadline_boottime.is_some() {
        return Ok(false);
    }
    let (boot, now) = boot_clock()?;
    h.deadline_boot = Some(boot);
    h.deadline_boottime = Some(now.saturating_add(h.ttl_secs.min(LEASE_SECS)));
    h.ttl_secs = DEFAULT_TTL_SECS;
    let tmp = temp_path(path);
    std::fs::write(&tmp, serialize(&h))?;
    let result = std::fs::rename(&tmp, path);
    let _ = std::fs::remove_file(tmp);
    result.map(|()| true)
}

/// Explicit upgrade/repair action; status and train observations never mutate.
// trace:TASK-3364 | ai:codex
pub(crate) fn migrate_legacy(project_root: &Path) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(locks_dir(project_root)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut changed = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let _guard = mutation_guard(&path, Duration::from_secs(5))?;
        changed += usize::from(migrate_legacy_deadline(&path)?);
    }
    Ok(changed)
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
            let _guard = match mutation_guard(&self.path, Duration::from_secs(5)) {
                Ok(guard) => guard,
                Err(e) => {
                    eprintln!(
                        "warning: merge-lease release guard failed: {e}; {}",
                        repair_hint(&self.path)
                    );
                    return;
                }
            };
            match std::fs::read_to_string(&self.path) {
                Ok(body) if body == self.record => {
                    if let Err(e) = std::fs::remove_file(&self.path) {
                        eprintln!(
                            "warning: merge-lease release failed: {e}; {}",
                            repair_hint(&self.path)
                        );
                        return;
                    }
                }
                Ok(_) => {} // Replaced: release our shadow, never remove their lease.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!(
                        "warning: merge-lease release could not read holder: {e}; {}",
                        repair_hint(&self.path)
                    );
                    return;
                }
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
/// (decision 2). Reclaims Dead or separately Expired non-Alive holders.
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
        // trace:TASK-3364 | ai:codex
        let retained = || {
            let evidence = std::fs::read_to_string(&path)
                .map(|body| format!("{:?}", holder_verdict(&parse(&body))))
                .unwrap_or_else(|e| format!("unreadable holder: {e}"));
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "merge-lease for '{branch}' is retained ({evidence}); waited {}s; inspect with `aida merge-lock`",
                    wait.as_secs()
                ),
            )
        };
        let guard = mutation_guard(&path, deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    retained()
                } else {
                    e
                }
            })?;
        migrate_legacy_deadline(&path)?;
        let (deadline_boot, clock_now) = boot_clock()?;
        let h = Holder {
            deadline_boot: Some(deadline_boot),
            deadline_boottime: Some(clock_now.saturating_add(LEASE_SECS)),
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
                    return Err(retained());
                }
                drop(guard);
                std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Active merge-leases: exclude Dead and Expired holders; always retain Alive.
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
        if stale_reason(&h, "").is_some() {
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
            deadline_boot: None,
            deadline_boottime: None,
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
    // trace:TASK-3364 | ai:codex
    #[test]
    fn boot_deadline_expires_unknown_and_refused_but_never_alive() {
        let mut h = current_holder();
        h.deadline_boot = Some("boot-a".into());
        h.deadline_boottime = Some(100);
        for acquired_at in [0, u64::MAX] {
            h.acquired_at = acquired_at;
            for clock in [("boot-a", 99), ("boot-a", 101), ("boot-b", 1)] {
                assert!(
                    stale_at(&h, Some(clock)).is_none(),
                    "Alive wins over clock/boot"
                );
            }
        }
        for process in [None, Some(HolderRecord::refused(HolderRefusal::Subreaper))] {
            h.process = process;
            assert!(stale_at(&h, Some(("boot-a", 99))).is_none());
            assert!(stale_at(&h, None).is_none());
            for clock in [("boot-a", 100), ("boot-a", 101), ("boot-b", 1)] {
                assert!(stale_at(&h, Some(clock)).unwrap().starts_with("Expired"));
            }
        }
        let mut foreign = current_holder();
        change_identity(&mut foreign, "pidns_inode", 1.into());
        foreign.deadline_boot = h.deadline_boot;
        foreign.deadline_boottime = h.deadline_boottime;
        assert!(stale_at(&foreign, Some(("boot-a", 101)))
            .unwrap()
            .starts_with("Expired"));
    }

    #[test]
    fn legacy_migration_is_once_and_inspection_reports_repair_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(locks_dir(dir.path())).unwrap();
        let path = lock_path(dir.path(), "main");
        let mut h = current_holder();
        h.process = None;
        h.ttl_secs = LEASE_SECS;
        let legacy = serialize(&h);
        std::fs::write(&path, &legacy).unwrap();
        let view = inspect_file(&path);
        assert_eq!(view["holder_verdict"], "ForeignUnknown");
        assert_eq!(view["reason"], "MissingIdentity");
        assert!(view["repair_hint"]
            .as_str()
            .unwrap()
            .contains(path.to_str().unwrap()));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);
        assert!(acquire(dir.path(), "main", None, "blocked", Duration::ZERO).is_err());
        let migrated = std::fs::read_to_string(&path).unwrap();
        let h = parse(&migrated);
        assert!(h.process.is_none());
        assert!(h.deadline_boottime.is_some());
        migrate_legacy_deadline(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), migrated);
        let mut expired = h;
        expired.deadline_boottime = Some(0);
        std::fs::write(&path, serialize(&expired)).unwrap();
        assert_eq!(inspect_file(&path)["lease_state"], "Expired");
        assert!(live_status(dir.path()).is_empty());
        assert_eq!(observed_train_running(dir.path()), Some(false));
        let _lease = acquire(dir.path(), "main", None, "recovered", Duration::ZERO).unwrap();
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
        // trace:TASK-3364 | ai:codex
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(locks_dir(dir.path())).unwrap();
        let path = lock_path(dir.path(), "main");
        let _held = mutation_guard(&path, Duration::ZERO).unwrap();
        let err = acquire(dir.path(), "main", None, "blocked", Duration::ZERO).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert!(err.to_string().contains("unreadable holder:"));
        assert!(err
            .to_string()
            .contains("waited 0s; inspect with `aida merge-lock`"));
        assert!(!path.exists());
        let record = serialize(&current_holder());
        std::fs::write(&path, &record).unwrap();
        let err = acquire(dir.path(), "main", None, "blocked", Duration::ZERO).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(
            err.to_string(),
            "merge-lease for 'main' is retained (Ok(Alive)); waited 0s; inspect with `aida merge-lock`"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), record);
    }
    #[test]
    fn release_retries_guard_past_old_two_second_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut lease = acquire(dir.path(), "main", None, "held", Duration::ZERO).unwrap();
        let guard = mutation_guard(lease.path(), Duration::ZERO).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2100));
            drop(guard);
        });
        lease.do_release();
        release.join().unwrap();
        assert!(!lease.held);
        assert!(!lease.path().exists());
    }

    #[test]
    fn covers_and_drop_are_generation_fenced() {
        let dir = tempfile::tempdir().unwrap();
        let mut lease = acquire(
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
        lease.do_release();
        assert!(!lease.held);
        assert!(lease.train_root.is_none());
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
        assert_eq!(inspect_file(&path)["lease_state"], "Incomplete");
        assert_eq!(inspect_file(&path)["reclaimable"], false);

        std::fs::write(&path, "host=h\npid=1\n").unwrap();
        let old = SystemTime::now() - Duration::from_secs(PARTIAL_GRACE_SECS + 5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(inspect_file(&path)["lease_state"], "Incomplete");
        assert_eq!(inspect_file(&path)["reclaimable"], true);
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
