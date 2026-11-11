//! Review discovery, dispatch, head-change supersede, and the report-only
//! ship-readiness verdict with its once-only advisor wake.
// trace:TASK-1945 trace:TASK-1993 | ai:codex
use crate::gate::review_publication::{self, admission};
use anyhow::{Context, Result};
use clap::Args;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[path = "review_watch_forge.rs"]
mod forge;
// Every process signal lives here, behind pidfd identity checks.
// trace:TASK-1945 | ai:claude
#[path = "review_watch_signal.rs"]
mod signal;
// Ship readiness: report only. trace:TASK-1945 | ai:claude
#[path = "review_watch_ready.rs"]
mod ready;
// The project's required checks also gate the live-binary promote.
// trace:STORY-1689 | ai:claude
pub(crate) use ready::required_checks;
// The author's findings mail and the round-cap wake. trace:TASK-1945 | ai:claude
#[path = "review_watch_notice.rs"]
mod notice;
// The watcher's one branch write: updating a stale head whose author is
// gone, else routing it to the advisor. trace:BUG-2426 | ai:claude
#[path = "review_watch_update.rs"]
mod update;
// trace:STORY-2571 | ai:codex
#[path = "review_watch_service.rs"]
pub(crate) mod service;
#[path = "advisor_loop_train.rs"]
mod train;
#[allow(dead_code)] // Shared engine also contains review-watch-only entry points.
#[path = "review_watch_train_service.rs"]
pub(crate) mod train_service;

#[derive(Args, Debug, Clone)]
pub struct ReviewWatchArgs {
    /// Discover and dispatch once, then leave admitted reviews running.
    #[arg(long)]
    pub once: bool,
    /// Seconds between discovery passes (default 10).
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=3600), hide_default_value = true)]
    pub interval: u64,
    /// Maximum watcher-launched reviews across instances: 1–8 (config default, otherwise 2).
    // trace:TASK-3472 | ai:codex
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=8))]
    pub max_concurrent: Option<u8>,
    /// Vendor: claude, codex, or agy; omit for ordinary queue-work selection.
    #[arg(long, value_parser = ["claude", "codex", "agy"], hide_possible_values = true)]
    pub vendor: Option<String>,
    /// Operator recovery: forget every recorded attempt for one change
    /// (`PR-N` or `MR-N`) so its current head can be reviewed again, after
    /// confirming no reviewer for it is still running. Refused while any
    /// recorded process for it is alive.
    #[arg(long, value_name = "KEY", conflicts_with = "once")]
    pub release: Option<String>,
    #[arg(long, hide = true)]
    launch_reservation: Option<String>,
}

const LIMIT: u64 = 2 * 1024 * 1024;
const STATE: &str = "state.json";
const WATCH_HEAD: &str = "AIDA_REVIEW_WATCH_HEAD";
const MAX_ENTRIES: usize = 2000;
// Tests shorten the claim retry window so a contention test stays fast.
const CLAIM_DEADLINE: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(300)
} else {
    std::time::Duration::from_secs(5)
};
const DIAGNOSTIC_LIMIT: usize = 512;
// Blocking rounds retained per change: the two-round cap.
const RETAINED_ROUNDS: usize = 2;
// How long a superseded review has to exit after its terminate request
// before it is killed. Tests shorten it. trace:TASK-1945 | ai:claude
const SUPERSEDE_GRACE: std::time::Duration = if cfg!(test) {
    std::time::Duration::from_millis(300)
} else {
    std::time::Duration::from_secs(20)
};
// How often one head's readiness is re-evaluated: each evaluation costs
// several forge reads. Tests evaluate every pass. trace:TASK-1945 | ai:claude
const READY_INTERVAL: chrono::Duration = if cfg!(test) {
    chrono::Duration::zero()
} else {
    chrono::Duration::seconds(120)
};
// How long a review process recorded in another PID namespace (one this
// observer cannot probe) keeps its slot: well past the one-hour bound
// `aida dispatch` puts on a review run, so expiry is evidence it is gone.
// trace:BUG-2425 | ai:claude
const FOREIGN_IDENTITY_TTL: chrono::Duration = chrono::Duration::hours(4);
// Wakes kept per change: current-head ones only, so two at most.
const MAX_WAKES: usize = 8;

/// Marks a failure of the whole watcher state (missing, corrupt, unsafe), the
/// only error that stops a continuous watcher; per-change errors are logged.
// trace:TASK-1993 | ai:claude
#[derive(Debug)]
struct GlobalState;
impl std::fmt::Display for GlobalState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("review-watch state is unusable; inspect .aida/review-watch/state.json")
    }
}

fn bounded_diagnostic(text: &str) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .collect();
    if out.len() > DIAGNOSTIC_LIMIT {
        let mut cut = DIAGNOSTIC_LIMIT - 3;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("...");
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    // Absent fields are omitted only so a legacy record round-trips
    // unchanged. New captures always stamp both, so state this binary writes
    // does NOT parse under a pre-BUG-2425 binary after `aida dev rollback`
    // (deny_unknown_fields); that break is accepted (single-user substrate,
    // clear the review-watch state by hand if rolling back).
    // trace:BUG-2425 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid_namespace: Option<u64>,
    // When the identity was recorded: the backstop for a process this
    // observer cannot probe. trace:BUG-2425 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded_at: Option<chrono::DateTime<chrono::Utc>>,
    pid: u32,
    start: String,
    boot: String,
}
/// Process identity equality: the same kernel process, regardless of when it
/// was observed. `recorded_at` is observation metadata (the foreign-namespace
/// backstop clock) and must not make two reads of one process differ.
// trace:BUG-2425 | ai:claude
impl PartialEq for Identity {
    fn eq(&self, other: &Self) -> bool {
        // trace:TASK-3365 | ai:codex
        match (self.process_identity(), other.process_identity()) {
            (Some(left), Some(right)) => left == right,
            // Preserve exact comparison for legacy round trips, without
            // treating incomplete evidence as a live kernel identity.
            _ => {
                self.pid_namespace == other.pid_namespace
                    && self.pid == other.pid
                    && self.start == other.start
                    && self.boot == other.boot
            }
        }
    }
}
impl Eq for Identity {}
impl Identity {
    fn current() -> Result<Self> {
        // Fail closed: receipts need a verified local procfs coordinate view.
        // Restricted/masked procfs must not mint a partial caller identity.
        // trace:TASK-3365 | ai:codex
        Self::current_from(aida_core::proc_identity::ProcIdentity::capture(
            std::process::id(),
        ))
    }
    fn current_from(
        capture: std::result::Result<aida_core::proc_identity::ProcIdentity, String>,
    ) -> Result<Self> {
        capture.map(Self::from_process_identity).map_err(|why| anyhow::anyhow!(
            "cannot record review-watch caller identity: {why}; retry from a host with a complete, local /proc view; existing reservations remain held"
        ))
    }
    fn child(child: &Child) -> Result<Self> {
        aida_core::proc_identity::ProcIdentity::capture_child(child)
            .map(Self::from_process_identity)
            .map_err(anyhow::Error::msg)
    }
    #[cfg(test)]
    fn of(pid: u32) -> Result<Self> {
        let identity =
            aida_core::proc_identity::ProcIdentity::capture(pid).map_err(anyhow::Error::msg)?;
        Ok(Self::from_process_identity(identity))
    }
    fn from_process_identity(identity: aida_core::proc_identity::ProcIdentity) -> Self {
        Self {
            pid_namespace: Some(identity.pidns_inode),
            recorded_at: Some(chrono::Utc::now()),
            pid: identity.pid,
            start: format!("linux-starttime:{}", identity.starttime),
            boot: identity.boot_id,
        }
    }
    // Keep the strict persisted wire format readable by rollback binaries.
    // Missing legacy evidence is unknown; never recapture a recorded PID.
    // trace:TASK-3365 | ai:codex
    fn process_identity(&self) -> Option<aida_core::proc_identity::ProcIdentity> {
        let ticks = self
            .start
            .strip_prefix("linux-starttime:")?
            .parse::<u64>()
            .ok()?;
        aida_core::proc_identity::ProcIdentity::from_record(
            &aida_core::process_probe::ProcessIdentity {
                pid: self.pid,
                start: format!("linux:{}:{ticks}", self.boot),
            },
            self.pid_namespace,
        )
    }
    fn verdict(&self) -> aida_core::proc_identity::Verdict {
        use aida_core::proc_identity::{observe, UnknownReason, Verdict};
        self.process_identity().map_or(
            Verdict::ForeignUnknown(UnknownReason::MissingIdentity),
            |identity| observe(&identity, true),
        )
    }
    /// Capacity policy is separate from kernel process observation.
    // trace:TASK-3365 | ai:codex
    fn alive(&self) -> bool {
        self.alive_with_verdict(self.verdict())
    }
    // trace:TASK-3367 | ai:codex
    fn alive_with_verdict(&self, verdict: aida_core::proc_identity::Verdict) -> bool {
        use aida_core::proc_identity::Verdict;
        match verdict {
            Verdict::Alive => true,
            Verdict::Dead(_) => false,
            // Every unknown observation uses the reservation backstop, including
            // read failures in an otherwise observable local PID namespace.
            Verdict::ForeignUnknown(_) => {
                self.within_foreign_backstop(chrono::Utc::now(), boot_id().ok().as_deref())
            }
        }
    }
}
impl Identity {
    /// This observer shares the PID namespace the identity was recorded in.
    // trace:BUG-2425 | ai:claude
    fn observable(&self) -> bool {
        aida_core::pid_observer::PidObserver::current().can_observe(self.pid_namespace)
    }
    /// A process in another PID namespace cannot be probed from here, and
    /// that absence proves nothing — but independent evidence still ends it:
    /// a reboot (the boot id changed), or its record outliving
    /// [`FOREIGN_IDENTITY_TTL`]. A legacy record with no `recorded_at` keeps
    /// its reservation until a reboot or an observer in its namespace.
    // trace:BUG-2425 | ai:claude
    fn within_foreign_backstop(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        boot: Option<&str>,
    ) -> bool {
        if boot.is_some_and(|boot| boot != self.boot) {
            return false;
        }
        self.recorded_at
            .is_none_or(|at| now - at <= FOREIGN_IDENTITY_TTL)
    }
}
fn boot_id() -> Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .into())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    // Peer ownership is cached until the head changes or operator recovery.
    RemoteHeld,
    Reserved,
    Running,
    Completed,
    Failed,
    // The dispatcher observed queue work exit unsuccessfully. Terminal: it
    // frees capacity and lets a later head dispatch, but never relaunches
    // this head. trace:TASK-1993 | ai:claude
    Exited,
    // A newer head made this review obsolete: its processes are being
    // stopped. Still active, so it keeps its capacity slot, and no new
    // descendant may be recorded. trace:TASK-1945 | ai:claude
    Superseding,
    // Every process of the superseded review is confirmed gone. Terminal.
    Superseded,
}

/// The durable supersede receipt: written before any signal, and each signal
/// is claimed here (under the per-change lock) before it is sent, so two
/// watchers never signal twice.
// trace:TASK-1945 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Supersede {
    by: String,
    requested_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    term_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kill_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    // Pin distributed mode through child launch, even if configuration vanishes.
    // trace:STORY-2492 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    admission_hub: Option<admission::Hub>,
    // Exact pre-spawn lease; terminal process proof cannot release a replacement.
    // trace:BUG-2498 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review_lease: Option<String>,
    head: String,
    token: String,
    vendor: Option<String>,
    phase: Phase,
    identity: Option<Identity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<String>,
    // The spawned dispatcher, recorded by the parent so any watcher can see
    // a reservation whose dispatcher exited before claiming it.
    // trace:TASK-1993 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dispatcher: Option<Identity>,
    // Set just before queue work is spawned; `worker` then records that child.
    // Queue work outlives a killed dispatcher, so a claimed attempt is freed
    // only once both processes are provably gone. trace:TASK-1993 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    spawning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worker: Option<Identity>,
    // Queue work in turn marks `reviewer_spawning` and records the reviewer
    // process that actually carries the review; it can outlive both.
    // trace:TASK-1993 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reviewer_spawning: bool,
    // One automatic retry for setup exits; a persistent refusal stays visible.
    // trace:BUG-2450 | ai:codex
    #[serde(default)]
    setup_retry: bool,
    // trace:STORY-2401 | ai:codex
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    retryable_fetch: bool,
    // Persist across attempt replacement; three dispatches maximum per head.
    // trace:STORY-2401 | ai:codex
    #[serde(default)]
    fetch_failures: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reviewer: Option<Identity>,
    // The dispatcher's process group (it leads it), recorded by the watcher
    // that spawned it. Absent in older state: no group sweep then.
    // trace:TASK-1945 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pgid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supersede: Option<Supersede>,
}
impl Attempt {
    /// Recorded processes, leaf first: reviewer, queue work, dispatcher.
    // trace:TASK-1945 | ai:claude
    fn recorded(&self) -> Vec<&Identity> {
        let mut out: Vec<&Identity> = Vec::new();
        for process in [
            &self.reviewer,
            &self.worker,
            &self.identity,
            &self.dispatcher,
        ]
        .into_iter()
        .flatten()
        {
            if !out.contains(&process) {
                out.push(process);
            }
        }
        out
    }
    /// Queue work and the reviewer it launched are both provably gone, or
    /// were never started. A process marked spawning but never recorded
    /// counts as possibly alive.
    // trace:TASK-1993 | ai:claude
    fn descendants_gone(&self) -> bool {
        let gone = |spawning: bool, process: &Option<Identity>| {
            !spawning || process.as_ref().is_some_and(|p| !p.alive())
        };
        gone(self.spawning, &self.worker) && gone(self.reviewer_spawning, &self.reviewer)
    }
    fn unrecorded_descendant(&self) -> bool {
        (self.spawning && self.worker.is_none())
            || (self.reviewer_spawning && self.reviewer.is_none())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Notice {
    message: aida_core::mailbox::Message,
    sent: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    attempts: Vec<Attempt>,
    // Never written: retained rounds live in history/<key>.json. Still read
    // (and validated) so an older state file loads, and dropped on the next
    // write. trace:TASK-1955 | ai:claude (TASK-1-414)
    #[serde(default, skip_serializing)]
    historical_keys: BTreeSet<crate::gate::review_verdict::RecordingKey>,
    notices: BTreeMap<String, Notice>,
    // Advisor ready wakes for the current head, keyed `<head>:<kind>`.
    // trace:TASK-1945 | ai:claude
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    wakes: BTreeMap<String, Notice>,
    // Author findings mails for the current head, keyed
    // `<head>:<verdict digest>`. trace:TASK-1945 | ai:claude
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    findings: BTreeMap<String, Notice>,
    // The advisor's round-cap wake: once per change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cap: Option<Notice>,
    // The last readiness report, for display only: never authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    readiness: Option<Reported>,
    // The branch update the watcher requested for a stale overlapping head.
    // trace:BUG-2426 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    update: Option<update::BranchUpdate>,
    // Why the current head is parked (skipped and not reviewed), since when:
    // what `aida awaiting` lists. Display only. trace:BUG-2426 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    skipped: Option<Reported>,
}
impl Entry {
    fn is_empty(&self) -> bool {
        self.attempts.is_empty()
            && self.notices.is_empty()
            && self.wakes.is_empty()
            && self.findings.is_empty()
            && self.cap.is_none()
            && self.readiness.is_none()
            && self.update.is_none()
            && self.skipped.is_none()
    }
}
/// One readiness report: the head it judged, the verdict line, and when.
// trace:TASK-1945 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Reported {
    head: String,
    line: String,
    checked_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    repository: String,
    // Diagnostic: limit of the last watcher to select under the lock. Each
    // watcher applies its own configured cap; this is not a shared policy.
    // trace:TASK-3735 | ai:codex
    max_concurrent: u8,
    entries: BTreeMap<String, Entry>,
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid takes no pointers and has no preconditions.
    unsafe { libc::geteuid() }
}

/// Pin runtime directories, then open files relative to the pinned handle.
/// In particular a FIFO or symlink can never make a state read block or escape.
struct Directory {
    handle: File,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)?;
        let meta = handle.metadata()?;
        anyhow::ensure!(
            meta.is_dir() && meta.uid() == effective_uid() && meta.mode() & 0o022 == 0,
            "unsafe review-watch directory {}",
            path.display()
        );
        Ok(Self { handle })
    }
    // The shared .aida root and canonical history use the repository's ordinary
    // group-writable directory policy. Watch-owned subdirectories stay strict.
    // trace:TASK-1993 | ai:codex
    fn shared(path: &Path) -> Result<Self> {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        Self::existing(path)?.context("shared review runtime directory disappeared")
    }
    fn existing(path: &Path) -> Result<Option<Self>> {
        let handle = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let meta = handle.metadata()?;
        anyhow::ensure!(
            meta.is_dir() && meta.uid() == effective_uid() && meta.mode() & 0o002 == 0,
            "unsafe review history directory {}",
            path.display()
        );
        Ok(Some(Self { handle }))
    }
    fn path(&self) -> PathBuf {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/proc/self/fd/{}", self.handle.as_raw_fd()))
    }
    fn file(&self, name: &str, create: bool) -> Result<File> {
        self.open_file(name, create, false)
    }
    fn open_file(&self, name: &str, create: bool, history: bool) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(create)
            .create(create)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path().join(name))
            .with_context(|| format!("open review-watch runtime file {name}"))?;
        let meta = file.metadata()?;
        anyhow::ensure!(
            meta.is_file()
                && meta.nlink() == 1
                && if history {
                    matches!(meta.mode() & 0o777, 0o600 | 0o640 | 0o644 | 0o660 | 0o664)
                } else {
                    meta.mode() & 0o777 == 0o600
                }
                && meta.uid() == effective_uid(),
            "unsafe review-watch file {name}"
        );
        Ok(file)
    }
    fn read(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.read_file(name, false)
    }
    fn read_file(&self, name: &str, history: bool) -> Result<Option<Vec<u8>>> {
        let file = match self.open_file(name, false, history) {
            Ok(file) => file,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(LIMIT + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= LIMIT,
            "review-watch file exceeds size limit"
        );
        Ok(Some(bytes))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<()> {
        anyhow::ensure!(
            bytes.len() as u64 <= LIMIT,
            "review-watch state exceeds size limit"
        );
        let mut temp = tempfile::NamedTempFile::new_in(self.path())?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(self.path().join(name)).map_err(|e| e.error)?;
        self.handle.sync_all()?;
        Ok(())
    }
    fn lock(&self, name: &str) -> Result<File> {
        let file = self.file(name, true)?;
        // A forked child shares this lock's open file description until it
        // execs, so a just-released lock can read busy for a moment. Retry
        // briefly; a live holder still yields WouldBlock to the caller.
        // trace:TASK-1993 | ai:claude
        let mut attempts = 0;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && attempts < 50 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(e) => return Err(e).with_context(|| format!("review-watch lock busy: {name}")),
            }
        }
    }
}

struct Store {
    runtime: Directory,
    dir: Directory,
    repository: String,
    // The project root: `runtime`/`dir` paths are `/proc/self/fd` handles.
    // trace:TASK-1955 | ai:claude (TASK-2109)
    root: PathBuf,
}
impl Store {
    fn open(root: &Path, repository: &str) -> Result<Self> {
        let runtime = Directory::shared(&root.join(".aida"))?;
        let dir = Directory::open(&runtime.path().join("review-watch"))?;
        Ok(Self {
            runtime,
            dir,
            repository: repository.into(),
            root: root.to_path_buf(),
        })
    }
    fn load(&self) -> Result<State> {
        self.load_unchecked().context(GlobalState)
    }
    fn load_unchecked(&self) -> Result<State> {
        let state = match self.dir.read(STATE)? {
            Some(bytes) => serde_json::from_slice::<State>(&bytes)
                .context("invalid review-watch state; inspect .aida/review-watch/state.json")?,
            None => {
                anyhow::ensure!(self.dir.read("initialized")?.is_none(), "review-watch state is missing after initialization; inspect existing reviews before recovery");
                State {
                    version: 1,
                    repository: self.repository.clone(),
                    max_concurrent: 2,
                    entries: BTreeMap::new(),
                }
            }
        };
        validate_state(&state, &self.repository)?;
        Ok(state)
    }
    fn transaction<T>(&self, key: &str, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        parse_key(key)?;
        // Every writer uses this ordering. No forge query, child wait or mail
        // delivery occurs while either lock is held.
        let _global = self.dir.lock("admission.lock")?;
        let _change = self.dir.lock(&format!("{key}.lock"))?;
        let mut state = self.load()?;
        let initialized = self.dir.read("initialized")?.is_some();
        let before = serde_json::to_vec(&state)?;
        let result = f(&mut state)?;
        // A change seen only to be skipped leaves no durable entry, so state
        // tracks reviewed/notified changes rather than every PR ever polled.
        // trace:TASK-1993 | ai:claude
        state.entries.retain(|_, e| !e.is_empty());
        validate_state(&state, &self.repository)?;
        if before != serde_json::to_vec(&state)? || !initialized {
            self.dir.write(STATE, &serde_json::to_vec_pretty(&state)?)?;
        }
        if !initialized {
            self.dir.file("initialized", true)?.sync_all()?;
            self.dir.handle.sync_all()?;
        }
        Ok(result)
    }
    /// Drop state for changes no longer open and non-draft. Discovery is
    /// complete or refused, so an absent key is closed, merged or draft. Live
    /// or ambiguous attempts are kept: their capacity is never released here.
    /// A re-opened change re-reads canonical history; a repeated notice reuses
    /// its deterministic mailbox ID and is not duplicated.
    // trace:TASK-1993 | ai:claude
    fn prune(&self, open: &BTreeSet<String>) -> Result<usize> {
        // Every writer takes the global lock first, so it alone excludes them.
        let _global = self.dir.lock("admission.lock")?;
        let mut state = self.load()?;
        let bytes = serde_json::to_vec(&state)?;
        self.reconcile_lost_dispatchers(&mut state);
        let before = state.entries.len();
        state
            .entries
            .retain(|key, entry| open.contains(key) || entry.attempts.iter().any(active));
        let removed = before - state.entries.len();
        if bytes != serde_json::to_vec(&state)? {
            validate_state(&state, &self.repository)?;
            self.dir.write(STATE, &serde_json::to_vec_pretty(&state)?)?;
        }
        self.prune_logs(&state);
        Ok(removed)
    }
    /// Remove dispatcher logs no recorded attempt names any more (its change
    /// closed, or a later head replaced the attempt). The per-change lock
    /// files stay: locks are permanent by design. Callers hold the global
    /// lock, so no dispatcher is being admitted meanwhile.
    // trace:TASK-1955 | ai:claude (TASK-2064)
    fn prune_logs(&self, state: &State) {
        let Ok(Some(logs)) = Directory::existing(&self.dir.path().join("logs")) else {
            return;
        };
        let live: BTreeSet<String> = state
            .entries
            .iter()
            .flat_map(|(key, e)| {
                e.attempts
                    .iter()
                    .map(move |a| format!("{key}-{}.log", a.token))
            })
            .collect();
        let Ok(entries) = std::fs::read_dir(logs.path()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".log") && !live.contains(&name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    /// `aida review watch --release KEY`: forget every attempt recorded for
    /// KEY once the operator confirms none is running. Refused while any
    /// recorded dispatcher, queue-work or reviewer process is alive.
    // trace:TASK-1955 | ai:claude (TASK-1-412)
    fn release(&self, key: &str) -> Result<usize> {
        self.transaction(key, |state| {
            let Some(entry) = state.entries.get_mut(key) else {
                return Ok(0);
            };
            let alive = entry.attempts.iter().any(|a| {
                [&a.identity, &a.dispatcher, &a.worker, &a.reviewer]
                    .into_iter()
                    .flatten()
                    .any(Identity::alive)
            });
            anyhow::ensure!(
                !alive,
                "a recorded review process for {key} is still running; let it finish first"
            );
            let released = entry.attempts.len();
            entry.attempts.clear();
            Ok(released)
        })
    }
    /// An attempt whose dispatcher is provably gone (its exact kernel start
    /// identity no longer runs) can never record its own outcome: record it
    /// exited and free its slot. That covers a reservation never claimed and
    /// a claimed review whose terminal receipt was lost (a crash, or a lock
    /// that stayed busy past the deadline). Callers hold the global lock.
    // trace:TASK-1993 | ai:claude
    fn reconcile_lost_dispatchers(&self, state: &mut State) {
        for (key, entry) in state.entries.iter_mut() {
            for attempt in entry.attempts.iter_mut() {
                let (process, when) = match (&attempt.phase, &attempt.identity, &attempt.dispatcher)
                {
                    (Phase::Reserved, None, Some(dispatcher)) => {
                        (dispatcher, "before claiming its launch receipt")
                    }
                    // Queue work may outlive its dispatcher: both must be gone,
                    // and an unrecorded child stays ambiguous.
                    // trace:TASK-1993 | ai:claude
                    (Phase::Running, Some(identity), _) if attempt.descendants_gone() => {
                        (identity, "after claiming, without a terminal receipt")
                    }
                    _ => continue,
                };
                if process.alive() {
                    continue;
                }
                let tail = log_tail(self, &format!("{key}-{}.log", attempt.token));
                let diagnostic = bounded_diagnostic(&format!(
                    "dispatcher {} exited {when}: {}",
                    process.pid,
                    tail.as_deref().unwrap_or("no log output")
                ));
                // A lost supervisor still owes its captured lease completion.
                // Confirm the whole attempt before publishing that evidence.
                // trace:TASK-3777 | ai:codex
                let diagnostic = if attempt.phase == Phase::Running {
                    match confirm(attempt).map_err(anyhow::Error::msg)
                        .and_then(|()| finish_confirmed_review(&self.root, attempt)) {
                        Ok(()) => diagnostic,
                        Err(error) => bounded_diagnostic(&format!(
                            "{diagnostic}; review completion receipt refused; retained checkout requires recovery: {error:#}"
                        )),
                    }
                } else {
                    diagnostic
                };
                attempt.phase = Phase::Exited;
                attempt.diagnostic = Some(diagnostic);
            }
        }
    }
    /// Blocking review identities this watcher has observed, kept per change
    /// outside state.json so pruning (a draft toggle, close and reopen) can
    /// never forget a consumed round.
    // trace:TASK-1993 | ai:claude
    fn retained(&self, key: &str) -> Result<BTreeSet<crate::gate::review_verdict::RecordingKey>> {
        let dir = Directory::open(&self.dir.path().join("history"))?;
        let Some(bytes) = dir.read(&format!("{key}.json"))? else {
            return Ok(BTreeSet::new());
        };
        let keys: BTreeSet<crate::gate::review_verdict::RecordingKey> =
            serde_json::from_slice(&bytes).context("invalid retained review history")?;
        anyhow::ensure!(
            keys.len() <= 16 && keys.iter().all(valid_recording_key),
            "invalid retained review history"
        );
        Ok(keys)
    }
    fn retain(
        &self,
        key: &str,
        keys: impl Iterator<Item = crate::gate::review_verdict::RecordingKey>,
    ) -> Result<()> {
        let mut retained = self.retained(key)?;
        let before = retained.len();
        // Two blocking rounds reach the cap; more are never needed, and an
        // unbounded write would fail the <=16 read on the next pass.
        // trace:TASK-1955 | ai:claude (TASK-2064)
        for key in keys {
            if retained.len() >= RETAINED_ROUNDS {
                break;
            }
            retained.insert(key);
        }
        if retained.len() != before {
            let dir = Directory::open(&self.dir.path().join("history"))?;
            dir.write(&format!("{key}.json"), &serde_json::to_vec(&retained)?)?;
        }
        Ok(())
    }
    fn history(&self, key: &str) -> Result<History> {
        let mut history = History {
            keys: self.retained(key)?,
            ..History::default()
        };
        // trace:STORY-2492 | ai:codex
        for (body, publication) in review_publication::watcher_history(&self.root, key)? {
            history.add(body.as_bytes(), &publication)?;
            let synced = crate::gate::review_verdict::parse_recorded_verdict(&body)
                .and_then(|v| v.reviewed_sha)
                .context("missing synced review head")?;
            if history.keys.iter().any(|k| {
                k.0.as_deref() == Some(&synced)
                    && !history
                        .files
                        .get(k)
                        .is_some_and(|f| f.contains(&publication))
            }) {
                review_publication::report(format!(
                    "synced review overrides retained local watcher history for {key} at {synced}"
                ));
            }
            history.keys.retain(|k| {
                k.0.as_deref() != Some(synced.as_str())
                    || history
                        .files
                        .get(k)
                        .is_some_and(|f| f.contains(&publication))
            });
            history.synced_heads.insert(synced);
        }
        let Some(dir) = Directory::existing(&self.runtime.path().join("review-verdicts"))? else {
            return Ok(history);
        };
        // GitLab's ordinary standalone collector historically used PR-N too.
        let aliases = if let Some(number) = key.strip_prefix("MR-") {
            vec![key.to_string(), format!("PR-{number}")]
        } else {
            vec![key.to_string()]
        };
        for alias in aliases {
            if let Some(bytes) = dir.read_file(&format!("{alias}.json"), true)? {
                history.add(&bytes, &format!("{alias}.json"))?;
            }
            if history.blocking() >= 2 {
                return Ok(history);
            }
            if let Some(archive) = Directory::existing(&dir.path().join(&alias))? {
                let mut names = Vec::new();
                for (index, entry) in std::fs::read_dir(archive.path())?.enumerate() {
                    anyhow::ensure!(index < 1000, "review archive exceeds admission scan limit");
                    let name = entry?
                        .file_name()
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("non-UTF8 review archive entry"))?;
                    anyhow::ensure!(
                        names.len() < 1000,
                        "review archive exceeds admission scan limit"
                    );
                    if name.ends_with(".json") {
                        names.push(name);
                    }
                }
                names.sort();
                for name in names {
                    let bytes = archive
                        .read_file(&name, true)?
                        .context("review history disappeared during admission")?;
                    history.add(&bytes, &format!("{alias}/{name}"))?;
                    if history.blocking() >= 2 {
                        return Ok(history);
                    }
                }
            }
        }
        Ok(history)
    }
    /// Who a stale-head notice goes to: the AIDA identity the change author's
    /// forge login names (stored spelling), else the advisor. Mailbox
    /// delivery matches the AIDA identity exactly, so a bare forge login
    /// (`joemooney` vs `joe`) reached no inbox and the stale PR stalled.
    /// An aliased login resolves to its canonical identity first: delivery
    /// does not resolve aliases, so mail to the alias spelling is a phantom.
    // trace:TASK-1955 | ai:claude (TASK-1-410) (TASK-2118)
    fn notice_recipient(&self, author: &str) -> String {
        let store = crate::detect_distributed_store_from(&self.root)
            .unwrap_or_else(|| self.root.join(".aida-store"));
        let canonical = aida_core::alias::AliasRegistry::load(&store).resolve(author);
        crate::known_mailbox_recipients(&store)
            .get(&canonical.to_lowercase())
            .cloned()
            .unwrap_or_else(|| "advisor".to_string())
    }
    fn round_for(&self, key: &str, attempt: &Attempt) -> Result<usize> {
        self.transaction(key, |state| {
            let entry = state
                .entries
                .get_mut(key)
                .context("review reservation disappeared")?;
            anyhow::ensure!(
                entry
                    .attempts
                    .iter()
                    .any(|a| a.token == attempt.token && a.head == attempt.head && admitted(a)),
                "review reservation changed before queue work"
            );
            let history = self.history(key)?;
            anyhow::ensure!(
                !history.contains(&attempt.head),
                "head already has a recorded review; route further work to the advisor"
            );
            let used = history.blocking();
            anyhow::ensure!(
                used < 2,
                "two-round review cap reached outside watcher; route further work to the advisor"
            );
            self.retain(key, history.blocking_keys())?;
            Ok(used + 1)
        })
    }
    /// Record that queue work is being spawned (`None`) or the spawned child's
    /// identity, for the dispatcher that holds this Running receipt.
    // trace:TASK-1993 | ai:claude
    fn record_worker(&self, key: &str, token: &str, worker: Option<Identity>) -> Result<()> {
        let me = Identity::current()?;
        self.transaction(key, |state| {
            let attempt = state
                .entries
                .get_mut(key)
                .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
                .context("unknown review-watch reservation")?;
            anyhow::ensure!(
                attempt.phase == Phase::Running && attempt.identity.as_ref() == Some(&me),
                "review-watch receipt changed before queue work"
            );
            attempt.spawning = true;
            if worker.is_some() {
                attempt.worker = worker;
            }
            Ok(())
        })
    }
    /// Called from queue work (the recorded worker) for the active attempt at
    /// `head`: mark the reviewer spawning (`None`) or record its identity.
    // trace:TASK-1993 | ai:claude
    fn record_reviewer(&self, key: &str, head: &str, reviewer: Option<Identity>) -> Result<()> {
        let me = Identity::current()?;
        self.transaction(key, |state| {
            let attempt = state
                .entries
                .get_mut(key)
                .and_then(|e| {
                    e.attempts
                        .iter_mut()
                        .find(|a| a.head == head && a.phase == Phase::Running)
                })
                .context("missing watcher reservation at reviewer spawn")?;
            anyhow::ensure!(
                attempt.spawning && attempt.worker.as_ref().is_none_or(|w| w == &me),
                "review-watch reservation belongs to another queue-work process"
            );
            // The dispatcher may not have recorded this process yet.
            attempt.worker.get_or_insert(me);
            // Capture before spawn while this worker owns the reservation.
            // Missing/ambiguous legacy evidence never authorizes later release.
            // trace:BUG-2498 | ai:codex
            if !attempt.reviewer_spawning {
                let leases: Vec<_> = crate::list_leases(&self.root)
                    .into_iter()
                    .filter(|l| {
                        l.scope.eq_ignore_ascii_case(key)
                            && l.pr_head_sha.as_deref() == Some(head)
                            && crate::pr_lease_takeover::reviewer_target(l).is_some()
                    })
                    .collect();
                if let [lease] = leases.as_slice() {
                    attempt.review_lease = Some(toml::to_string(lease)?);
                }
            }
            attempt.reviewer_spawning = true;
            if reviewer.is_some() {
                attempt.reviewer = reviewer;
            }
            Ok(())
        })
    }
    fn transition(&self, key: &str, token: &str, phase: Phase) -> Result<Attempt> {
        self.transition_with(key, token, phase, None)
    }
    fn transition_with(
        &self,
        key: &str,
        token: &str,
        phase: Phase,
        diagnostic: Option<String>,
    ) -> Result<Attempt> {
        let me = Identity::current()?;
        self.transaction(key, |state| {
            let attempt = state
                .entries
                .get_mut(key)
                .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
                .context("unknown review-watch reservation; no review started")?;
            // A superseded dispatcher's own outcome never frees the slot: only
            // confirmation that every process is gone does. Keep the reason.
            // trace:TASK-1945 | ai:claude
            if attempt.phase == Phase::Superseding
                && phase != Phase::Running
                && attempt.identity.as_ref() == Some(&me)
            {
                if let Some(d) = diagnostic {
                    attempt.diagnostic = Some(bounded_diagnostic(&d));
                }
                return Ok(attempt.clone());
            }
            if phase == Phase::Running {
                anyhow::ensure!(
                    attempt.phase == Phase::Reserved && attempt.identity.is_none(),
                    "review-watch reservation already claimed; no duplicate review started"
                );
                attempt.identity = Some(me);
            } else {
                anyhow::ensure!(
                    attempt.phase == Phase::Running && attempt.identity.as_ref() == Some(&me),
                    "review-watch completion identity changed"
                );
            }
            // A queue setup failure before reviewer spawn is not a review
            // attempt. Retry this head at most once for a transient lease/setup blocker.
            // An ambiguous or still-live child retains the terminal refusal.
            // trace:BUG-2450 | ai:codex
            let setup_failed = phase == Phase::Exited
                && (!attempt.setup_retry || attempt.retryable_fetch)
                && !attempt.reviewer_spawning
                && attempt.descendants_gone();
            attempt.phase = if setup_failed { Phase::Failed } else { phase };
            // Terminal history needs no full lease snapshot; keep bounded state
            // proportional to active reviews, rather than every open change.
            // trace:BUG-2498 | ai:codex
            if matches!(
                attempt.phase,
                Phase::Completed | Phase::Exited | Phase::Failed
            ) {
                attempt.review_lease = None;
            }
            // This diagnostic also identifies the setup conversion on the next
            // tick; ordinary Failed transitions remain freely retryable.
            // trace:TASK-3112 | ai:codex
            attempt.diagnostic = diagnostic.map(|d| bounded_diagnostic(&d)).or_else(|| {
                setup_failed.then(|| "queue setup exited without a diagnostic".to_string())
            });
            Ok(attempt.clone())
        })
    }
}

// Durable rounds outside this watcher count too. Deduplicate current/archive
// copies using the verdict writer's recording identity, never a separate policy.
// trace:TASK-1993 | ai:codex
#[derive(Default)]
struct History {
    // Current synced evidence takes precedence over local cache/archives.
    synced_heads: BTreeSet<String>,
    keys: BTreeSet<crate::gate::review_verdict::RecordingKey>,
    // Sources for findings mail: local paths relative to `.aida/review-verdicts/`,
    // or explicit `.aida-store/` publication paths. trace:TASK-1945 | ai:claude
    files: BTreeMap<crate::gate::review_verdict::RecordingKey, Vec<String>>,
}
impl History {
    fn add(&mut self, bytes: &[u8], file: &str) -> Result<()> {
        let mut object = serde_json::from_slice::<serde_json::Value>(bytes)?
            .as_object()
            .cloned()
            .context("review history is not an object")?;
        let rounds = match object.remove("rounds") {
            None => Vec::new(),
            Some(serde_json::Value::Array(rounds)) => rounds,
            Some(_) => anyhow::bail!("invalid retained review rounds"),
        };
        for value in std::iter::once(serde_json::Value::Object(object)).chain(rounds) {
            let object = value
                .as_object()
                .context("invalid historical review round")?;
            anyhow::ensure!(
                object
                    .get("unarchived_rounds")
                    .is_none_or(|v| v.as_u64() == Some(0)),
                "unidentifiable historical rounds require advisor reconciliation"
            );
            let key = crate::gate::review_verdict::recording_key(object);
            anyhow::ensure!(
                valid_recording_key(&key),
                "unidentifiable historical review requires advisor reconciliation"
            );
            if !file.starts_with(".aida-store/")
                && key
                    .0
                    .as_ref()
                    .is_some_and(|h| self.synced_heads.contains(h))
            {
                if self
                    .keys
                    .iter()
                    .any(|synced| synced.0 == key.0 && synced.2 != key.2)
                {
                    review_publication::report(format!(
                        "synced review overrides contradictory local watcher history in {file}"
                    ));
                }
                continue;
            }
            let files = self.files.entry(key.clone()).or_default();
            if !files.iter().any(|f| f == file) {
                files.push(file.to_string());
            }
            self.keys.insert(key);
        }
        Ok(())
    }
    fn contains(&self, head: &str) -> bool {
        self.keys.iter().any(|key| {
            key.0
                .as_deref()
                .is_some_and(|sha| crate::gate::review_verdict::same_reviewed_sha(sha, head))
        })
    }
    // The ordinary round (queue_cmd::review_round_from_comments) counts the
    // durable findings blocks, which the hand-off writes exactly for
    // verdicts that block done. Approvals never consume a round, so a head
    // approved and then merged with main still gets its re-review at round 1.
    // trace:TASK-1993 | ai:claude
    fn blocking_keys(
        &self,
    ) -> impl Iterator<Item = crate::gate::review_verdict::RecordingKey> + '_ {
        self.keys
            .iter()
            .filter(|key| {
                crate::gate::review_verdict::VerdictKind::parse(&key.2).requires_changes()
            })
            .cloned()
    }
    fn blocking(&self) -> usize {
        self.blocking_keys().count()
    }
}

// trace:TASK-1993 | ai:codex
fn valid_recording_key(key: &crate::gate::review_verdict::RecordingKey) -> bool {
    key.0.as_deref().is_some_and(|sha| {
        (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
    }) && !key.1.is_empty()
        && crate::gate::review_verdict::VerdictKind::parse(&key.2)
            != crate::gate::review_verdict::VerdictKind::Unknown
}

impl State {
    /// (change, token) of every superseding attempt.
    // trace:TASK-1945 | ai:claude
    fn superseding(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .flat_map(|(key, e)| {
                e.attempts
                    .iter()
                    .filter(|a| a.phase == Phase::Superseding)
                    .map(move |a| (key.clone(), a.token.clone()))
            })
            .collect()
    }
}

fn validate_state(state: &State, repository: &str) -> Result<()> {
    anyhow::ensure!(
        state.version == 1
            && state.repository == repository
            && (1..=8).contains(&state.max_concurrent)
            // Pruning keeps this to open changes plus live attempts; the
            // forge discovery bound is 1000 rows. trace:TASK-1993 | ai:claude
            && state.entries.len() <= MAX_ENTRIES,
        "unsupported or mismatched review-watch state"
    );
    for (key, entry) in &state.entries {
        parse_key(key)?;
        anyhow::ensure!(
            entry.attempts.len() <= 2
                && entry.notices.len() <= 256
                && entry.historical_keys.len() <= 2
                && entry.historical_keys.iter().all(valid_recording_key),
            "invalid review-watch attempt/notice count"
        );
        let mut heads = BTreeSet::new();
        for attempt in &entry.attempts {
            anyhow::ensure!(
                valid_sha(&attempt.head)
                    && heads.insert(&attempt.head)
                    && uuid::Uuid::parse_str(&attempt.token).is_ok()
                    && attempt
                        .vendor
                        .as_deref()
                        .is_none_or(|v| matches!(v, "claude" | "codex" | "agy")),
                "invalid review-watch launch receipt"
            );
            anyhow::ensure!(
                match attempt.phase {
                    Phase::Reserved => attempt.identity.is_none(),
                    Phase::Running | Phase::Completed => attempt
                        .identity
                        .as_ref()
                        .is_some_and(|i| i.pid > 0 && !i.start.is_empty() && !i.boot.is_empty()),
                    // An unclaimed reservation whose dispatcher died exits
                    // without a receipt identity. trace:TASK-1993 | ai:claude
                    Phase::RemoteHeld | Phase::Failed | Phase::Exited => true,
                    // Either an unclaimed or a claimed review can be
                    // superseded; the receipt is what matters.
                    // trace:TASK-1945 | ai:claude
                    Phase::Superseding | Phase::Superseded =>
                        attempt.supersede.as_ref().is_some_and(|s| valid_sha(&s.by)
                            && s.by != attempt.head
                            && s.diagnostic
                                .as_ref()
                                .is_none_or(|d| d.len() <= DIAGNOSTIC_LIMIT)),
                },
                "invalid review-watch process receipt"
            );
            anyhow::ensure!(
                attempt
                    .diagnostic
                    .as_ref()
                    .is_none_or(|d| d.len() <= DIAGNOSTIC_LIMIT)
                    && attempt
                        .dispatcher
                        .as_ref()
                        .is_none_or(|i| i.pid > 0 && !i.start.is_empty() && !i.boot.is_empty()),
                "invalid review-watch diagnostic or dispatcher"
            );
        }
        anyhow::ensure!(
            entry.notices.keys().all(|h| valid_sha(h)),
            "invalid review-watch notice head"
        );
        // trace:TASK-1945 | ai:claude
        anyhow::ensure!(
            entry.wakes.len() <= MAX_WAKES
                && entry
                    .wakes
                    .keys()
                    .all(|k| k.split_once(':').is_some_and(|(head, rest)| {
                        // `<kind>:<approval digest>`; a bare `<kind>` was
                        // recorded before the key named the approval.
                        let (kind, digest) = rest.split_once(':').unwrap_or((rest, ""));
                        valid_sha(head)
                            && matches!(kind, ready::WAKE_READY | ready::WAKE_AFTER_UPDATE)
                            && (digest.is_empty() || notice::valid_digest(digest))
                    }))
                && entry
                    .readiness
                    .as_ref()
                    .is_none_or(|r| { valid_sha(&r.head) && r.line.len() <= DIAGNOSTIC_LIMIT }),
            "invalid review-watch readiness record"
        );
        // trace:TASK-1945 | ai:claude
        anyhow::ensure!(
            entry.findings.len() <= MAX_WAKES
                && entry.findings.keys().all(|k| k.split_once(':').is_some_and(
                    |(head, digest)| { valid_sha(head) && notice::valid_digest(digest) }
                )),
            "invalid review-watch findings receipt"
        );
        // trace:BUG-2426 | ai:claude
        anyhow::ensure!(
            entry
                .update
                .as_ref()
                .is_none_or(update::BranchUpdate::valid)
                && entry
                    .skipped
                    .as_ref()
                    .is_none_or(|r| valid_sha(&r.head) && r.line.len() <= DIAGNOSTIC_LIMIT),
            "invalid review-watch branch update or skip record"
        );
    }
    Ok(())
}

#[derive(Debug)]
enum Action {
    Dispatch(Attempt),
    Notify(Notice),
    Skip(String),
}

fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}
fn parse_key(key: &str) -> Result<u64> {
    let number = key
        .strip_prefix("PR-")
        .or_else(|| key.strip_prefix("MR-"))
        .context("invalid review-watch change key")?;
    let id = number
        .parse::<u64>()
        .context("invalid review-watch change number")?;
    anyhow::ensure!(
        id > 0 && id.to_string() == number,
        "invalid review-watch change number"
    );
    Ok(id)
}
/// Holds a capacity slot: admitted, or superseded but not yet confirmed gone.
fn active(attempt: &Attempt) -> bool {
    admitted(attempt) || attempt.phase == Phase::Superseding
}
/// May still start or carry the review of its own head.
// trace:TASK-1945 | ai:claude
fn admitted(attempt: &Attempt) -> bool {
    matches!(attempt.phase, Phase::Reserved | Phase::Running)
}

fn select(
    store: &Store,
    change: &forge::Change,
    vendor: Option<String>,
    max: u8,
) -> Result<Action> {
    select_with_coverage(store, change, vendor, max, || {
        dispatch_carried_approval(store, change)
    })
}

// trace:STORY-2694 | ai:codex
fn select_with_coverage(
    store: &Store,
    change: &forge::Change,
    vendor: Option<String>,
    max: u8,
    coverage: impl FnOnce() -> Option<String>,
) -> Result<Action> {
    let key = change.key();
    // Resolve remote proof evidence before taking the shared runtime lock.
    // Suppression is review coverage only: readiness still checks CI at H2.
    let carried = coverage();
    let overlap = change
        .overlap
        .context("base-branch overlap evidence is unknown")?;
    store.transaction(&key, |state| {
        // trace:STORY-2571 | ai:codex
        service::check_controller_locked(&store.root, &store.dir)?;
        store.reconcile_lost_dispatchers(state);
        let total = state.entries.values().flat_map(|e| &e.attempts).filter(|a| active(a)).count();
        // Apply this watcher's limit under the admission lock. Existing reviews
        // count against a changed cap without needing to drain first.
        // trace:BUG-2496 | ai:codex
        state.max_concurrent = max;
        let repository = state.repository.clone();
        let holders = capacity_holders(state);
        let entry = state.entries.entry(key.clone()).or_default();
        // A notice is about one head; once the change moved on it is moot,
        // and keeping one per stale head grew to the 256 cap and then failed
        // every transaction for the change. trace:TASK-1955 | ai:claude (TASK-2064)
        entry.notices.retain(|head, _| *head == change.head);
        // trace:BUG-2426 | ai:claude
        if entry.update.as_ref().is_some_and(|u| u.head != change.head) {
            entry.update = None;
        }
        let history = store.history(&key)?;
        store.retain(&key, history.blocking_keys())?;
        if overlap {
            let recipient = store.notice_recipient(&change.author);
            let notice = entry.notices.entry(change.head.clone()).or_insert_with(|| Notice {
                message: stale_message(&repository, &key, change, &recipient), sent: false,
            });
            // A sent receipt is final: never re-validated or re-sent. A pending
            // notice keeps the wording it was recorded with (same deterministic
            // ID), so a retry re-delivers that exact message idempotently; one
            // recorded before notices named the base still matches its
            // template. trace:TASK-1993 | ai:claude
            if notice.sent { return Ok(Action::Skip(update::STALE_NOTIFIED.into())); }
            let recorded = serde_json::to_value(&notice.message)?;
            // A notice recorded before recipients were mapped went to the
            // forge login itself. trace:TASK-1955 | ai:claude (TASK-1-410)
            let matches = |base: Option<&str>, to: &str| -> Result<bool> {
                let mut expected = stale_message_for(&repository, &key, change, base, to);
                expected.timestamp = notice.message.timestamp;
                Ok(serde_json::to_value(&expected)? == recorded)
            };
            // A roster change between recording and sending moves the mapped
            // recipient; the recorded one stays valid (the deterministic ID,
            // body and subject still pin the notice). trace:TASK-1955 | ai:claude (TASK-2109)
            let recorded_to = match &notice.message.to {
                aida_core::mailbox::Recipient::Agent(to) => to.clone(),
                aida_core::mailbox::Recipient::Broadcast => String::new(),
            };
            let mut known = false;
            for to in [recipient.as_str(), change.author.as_str(), recorded_to.as_str()] {
                known = known || matches(Some(&change.base), to)? || matches(None, to)?;
            }
            anyhow::ensure!(known, "notification identity changed; pending message retained");
            return Ok(Action::Notify(notice.clone()));
        }
        let fetch_failures = entry.attempts.iter()
            .filter(|a| a.head == change.head)
            .map(|a| a.fetch_failures.saturating_add(u8::from(a.retryable_fetch)))
            .max().unwrap_or(0);
        if fetch_failures >= 3 {
            return Ok(Action::Skip(format!("fetch unavailable on three dispatches for this head; repair credentials/remote and retry with `aida review watch --release {key}`, or route to the advisor")));
        }
        if let Some(attempt) = entry.attempts.iter().find(|a| a.head == change.head && !a.retryable_fetch && (a.phase == Phase::Exited || (a.phase == Phase::Failed && a.setup_retry && a.diagnostic.is_some()))) {
            // trace:TASK-1993 | ai:claude
            return Ok(Action::Skip(format!("review of this head exited without completing ({}); not relaunched: push a new head, route it to the advisor, or retry with `aida review watch --release {key}`", attempt.diagnostic.as_deref().unwrap_or("no diagnostic"))));
        }
        // A superseded review of this head never finished: it may be reviewed
        // again. trace:TASK-1945 | ai:claude
        if let Some(attempt) = entry.attempts.iter().find(|a| a.head == change.head && !matches!(a.phase, Phase::Failed | Phase::Superseded)) {
            return Ok(Action::Skip(format!("head already reserved ({:?}); inspect .aida/review-watch/state.json and `aida session leases --all` for unfinished attempts", attempt.phase)));
        }
        if entry.attempts.iter().any(active) {
            // trace:TASK-1945 | ai:claude
            return Ok(Action::Skip(format!("a review of an earlier head still holds this change ({holders}); it is superseded once the change is seen at a new head")));
        }
        if let Some(reason) = &carried { return Ok(Action::Skip(reason.clone())); }
        if history.contains(&change.head) { return Ok(Action::Skip("head already has a recorded review".into())); }
        if history.blocking() >= 2 { return Ok(Action::Skip("two-round review cap reached; route further work to the advisor".into())); }
        if total >= usize::from(max) { return Ok(Action::Skip(format!("global reviewer capacity ({max}) is held by {holders}; a running review frees its slot when it exits, and an ambiguous one (dispatcher lost) stays reserved until an operator confirms it ended: check its log in .aida/review-watch/logs/ and `aida session leases --all`"))); }
        // Only Exited->Failed setup conversions carry a diagnostic. Preflight
        // and spawn failures do not consume a retry; preserve an inherited one.
        // trace:TASK-3112 | ai:codex
        let setup_retry = entry.attempts.iter().any(|a| a.head == change.head && a.phase == Phase::Failed && (a.setup_retry || (!a.retryable_fetch && a.diagnostic.is_some())));
        let attempt = Attempt { review_lease: None, admission_hub: None, setup_retry, retryable_fetch: false, fetch_failures, head: change.head.clone(), token: uuid::Uuid::new_v4().to_string(), vendor, phase: Phase::Reserved, identity: None, diagnostic: None, dispatcher: None, spawning: false, worker: None, reviewer_spawning: false, reviewer: None, pgid: None, supersede: None };
        // Nothing is active here, so earlier terminal attempts are history only:
        // durable verdicts carry the rounds. trace:TASK-1993 | ai:claude
        entry.attempts.clear();
        entry.attempts.push(attempt.clone());
        Ok(Action::Dispatch(attempt))
    })
}

/// The cheap, lock-held admission pre-check run before the forge probe (two
/// fetches, a merge-base and two diffs): `Some(reason)` when this head can
/// neither be dispatched nor needs a stale-head notice — its notice was sent,
/// a review of it is reserved, running or completed, it already has a
/// recorded review, or the change is at the round cap. A merge-base notice
/// asks for a merge *before review*, so a head already reviewed gets none;
/// one whose review exited unfinished is still probed.
// trace:TASK-1955 | ai:claude (TASK-2064, TASK-1-411)
fn probe_unneeded(store: &Store, change: &forge::Change) -> Result<Option<String>> {
    probe_unneeded_with_coverage(store, change, || dispatch_carried_approval(store, change))
}

// trace:STORY-2694 | ai:codex
fn probe_unneeded_with_coverage(
    store: &Store,
    change: &forge::Change,
    coverage: impl FnOnce() -> Option<String>,
) -> Result<Option<String>> {
    let key = change.key();
    let head = change.head.as_str();
    let recorded = store.transaction(&key, |state| {
        let Some(entry) = state.entries.get(&key) else {
            return Ok(None);
        };
        if entry.notices.get(head).is_some_and(|n| n.sent) {
            return Ok(Some((update::STALE_NOTIFIED.to_string(), false)));
        }
        Ok(entry
            .attempts
            .iter()
            .find(|a| {
                a.head == head
                    && matches!(
                        a.phase,
                        Phase::RemoteHeld | Phase::Reserved | Phase::Running | Phase::Completed
                    )
            })
            .map(|a| {
                (
                    format!("head already reserved ({:?})", a.phase),
                    a.phase == Phase::RemoteHeld,
                )
            }))
    })?;
    if let Some((reason, false)) = &recorded {
        return Ok(Some(reason.clone()));
    }
    if let Some(reason) = coverage() {
        return Ok(Some(reason));
    }
    let history = store.history(&key)?;
    if history.contains(head) {
        return Ok(Some("head already has a recorded review".into()));
    }
    if let Some((reason, true)) = recorded {
        return Ok(Some(reason));
    }
    if history.blocking() >= 2 {
        return Ok(Some(
            "two-round review cap reached; route further work to the advisor".into(),
        ));
    }
    Ok(None)
}

// trace:STORY-2694 | ai:codex
fn dispatch_carried_approval(store: &Store, change: &forge::Change) -> Option<String> {
    let proof = crate::gate::merge_hold_core::live_carried_approval(
        &[&store.root],
        &store.root,
        change.id,
        &change.head,
        Some(&change.base),
        &[],
        true,
    )
    .ok()?;
    Some(format!("APPROVED at {}, carried to {} (proof base {} at {}); CI at the current head is still required",
        proof.approved_sha, proof.new_head, proof.base_ref, proof.base_tip))
}

// trace:TASK-1993 | ai:claude
fn capacity_holders(state: &State) -> String {
    let holders: Vec<String> = state
        .entries
        .iter()
        .flat_map(|(key, entry)| {
            entry
                .attempts
                .iter()
                .filter(|a| active(a))
                .map(move |a| (key, a))
        })
        .map(|(key, a)| {
            format!(
                "{key} at {} ({:?}, token {}{})",
                &a.head[..12],
                a.phase,
                a.token,
                if a.phase == Phase::Superseding {
                    // trace:TASK-1945 | ai:claude
                    format!("; {}", hold_cause(key, a))
                } else if a.phase == Phase::Running && a.unrecorded_descendant() {
                    // trace:TASK-1993 | ai:claude
                    format!("; ambiguous: its queue-work or reviewer process was never recorded, so it is held until an operator confirms no `queue work {key}` or reviewer process for it remains and then runs `aida review watch --release {key}`")
                } else {
                    String::new()
                }
            )
        })
        .collect();
    holders.join(", ")
}

/// Why a superseding attempt still holds its slot.
// trace:TASK-1945 | ai:claude
fn hold_cause(key: &str, attempt: &Attempt) -> String {
    let receipt = attempt.supersede.as_ref();
    format!(
        "superseded for head {}, held until its processes are confirmed gone: {}; if this persists, stop them and run `aida review watch --release {key}`",
        receipt.map_or("unknown", |s| s.by.get(..12).unwrap_or(&s.by)),
        receipt
            .and_then(|s| s.diagnostic.as_deref())
            .unwrap_or("waiting for them to exit")
    )
}

/// Each change whose slot a superseded review still holds, with the cause,
/// read without creating or changing any watcher state (`aida ps`).
// trace:TASK-1945 | ai:claude
pub(crate) fn held_slots(root: &Path) -> Vec<(String, String)> {
    let Some(state) = read_only_state(root) else {
        return Vec::new();
    };
    state
        .entries
        .iter()
        .flat_map(|(key, entry)| {
            entry
                .attempts
                .iter()
                .filter(|a| a.phase == Phase::Superseding)
                .map(move |a| (key.clone(), hold_cause(key, a)))
        })
        .collect()
}

/// The watcher state of the project `root` belongs to, read without creating
/// or changing anything; `None` when there is none or it is unreadable.
// trace:TASK-1945 | ai:claude
fn read_only_state(root: &Path) -> Option<State> {
    let root = forge::main_root(root).unwrap_or_else(|_| root.to_path_buf());
    let runtime = Directory::existing(&root.join(".aida")).ok()??;
    let dir = Directory::existing(&runtime.path().join("review-watch")).ok()??;
    serde_json::from_slice::<State>(&dir.read(STATE).ok()??).ok()
}

/// Supersede the change's active attempt at an obsolete head. `Some(cause)`
/// while that attempt still holds the change: no head of it is admitted
/// until every recorded process is confirmed gone. `reread` is one
/// authoritative forge read of the head, taken with no lock held before the
/// intent is written. Signals are claimed under the per-change lock and sent
/// outside it; the watcher never blocks on an exit.
// trace:TASK-1945 | ai:claude
fn supersede(
    store: &Store,
    change: &forge::Change,
    reread: impl FnOnce() -> Result<String>,
) -> Result<Option<String>> {
    let key = change.key();
    let head = change.head.as_str();
    // A supersede in progress is driven to the end even if the change has
    // moved back to that attempt's head.
    let found = store.transaction(&key, |state| {
        Ok(state.entries.get(&key).and_then(|e| {
            e.attempts
                .iter()
                .find(|a| active(a) && (a.head != head || a.phase == Phase::Superseding))
                .map(|a| (a.token.clone(), a.phase.clone()))
        }))
    })?;
    let Some((token, phase)) = found else {
        return Ok(None);
    };
    if phase != Phase::Superseding {
        match reread() {
            Ok(current) if current == head => {}
            Ok(_) => {
                return Ok(Some(
                    "head moved again before the earlier review could be superseded; retrying next pass".into(),
                ))
            }
            Err(error) => {
                return Ok(Some(format!(
                    "head re-read refused; the earlier review is not superseded: {error:#}"
                )))
            }
        }
        let phase = store.transaction(&key, |state| {
            let Some(a) = attempt_mut(state, &key, &token) else {
                return Ok(None);
            };
            if admitted(a) && a.head != head {
                a.phase = Phase::Superseding;
                a.supersede = Some(Supersede {
                    by: head.to_string(),
                    requested_at: chrono::Utc::now(),
                    term_at: None,
                    kill_at: None,
                    diagnostic: None,
                });
            }
            Ok(Some(a.phase.clone()))
        })?;
        if phase != Some(Phase::Superseding) {
            // It ended on its own meanwhile.
            return Ok(None);
        }
        eprintln!("{key}: superseding the review of an earlier head for {head}");
    }
    advance(store, &key, &token)
}

/// One step of a superseding attempt: confirm it gone, or claim and send its
/// next signal. `Some(cause)` while it still holds its slot.
// trace:TASK-1945 | ai:claude
fn advance(store: &Store, key: &str, token: &str) -> Result<Option<String>> {
    let key = key.to_string();
    // Confirm, or claim the next signal; never signal under the lock.
    let claim = store.transaction(&key, |state| {
        let Some(a) = attempt_mut(state, &key, token).filter(|a| a.phase == Phase::Superseding)
        else {
            return Ok(None);
        };
        let cause = match confirm(a) {
            Ok(()) => {
                // Process exit frees watcher capacity even when best-effort
                // ownership evidence cannot be recorded. Pickup still requires
                // a valid receipt before retiring the retained checkout.
                // trace:BUG-2498 | ai:codex
                if let Err(error) = finish_confirmed_review(&store.root, a) {
                    let diagnostic = bounded_diagnostic(&format!(
                        "review completion receipt refused; retained checkout requires recovery: {error:#}"
                    ));
                    eprintln!("{key}: {diagnostic}");
                    a.diagnostic = Some(diagnostic);
                }
                a.review_lease = None;
                a.phase = Phase::Superseded;
                return Ok(None);
            }
            Err(cause) => cause,
        };
        let receipt = a
            .supersede
            .as_mut()
            .context("superseding attempt has no receipt")?;
        let now = chrono::Utc::now();
        let elapsed = |at: chrono::DateTime<chrono::Utc>| {
            (now - at).to_std().unwrap_or_default() >= SUPERSEDE_GRACE
        };
        let killed = receipt.kill_at.is_some_and(elapsed);
        let signal = match (receipt.term_at, receipt.kill_at) {
            (None, _) => {
                receipt.term_at = Some(now);
                Some(signal::Signal::Term)
            }
            (Some(term), None) if elapsed(term) => {
                receipt.kill_at = Some(now);
                Some(signal::Signal::Kill)
            }
            _ => None,
        };
        Ok(Some((signal, cause, killed, a.clone())))
    })?;
    let Some((signal, cause, killed, attempt)) = claim else {
        return Ok(None);
    };
    // Every holding pass rebuilds the cause from the processes running now,
    // so it never names one that has since exited.
    let anchor = attempt.dispatcher.as_ref().or(attempt.identity.as_ref());
    let held = signal::signal_attempt(&attempt.recorded(), attempt.pgid, anchor, signal);
    let mut diagnostic = std::iter::once(cause)
        .chain(held)
        .collect::<Vec<_>>()
        .join("; ");
    if killed {
        diagnostic.push_str(" after SIGKILL and its grace");
    }
    let diagnostic = bounded_diagnostic(&diagnostic);
    store.transaction(&key, |state| {
        if let Some(receipt) = attempt_mut(state, &key, token)
            .filter(|a| a.phase == Phase::Superseding)
            .and_then(|a| a.supersede.as_mut())
        {
            receipt.diagnostic = Some(diagnostic.clone());
        }
        Ok(())
    })?;
    let state = store.load()?;
    Ok(state
        .entries
        .get(&key)
        .and_then(|e| e.attempts.iter().find(|a| a.token == token))
        .filter(|a| a.phase == Phase::Superseding)
        .map(|a| hold_cause(&key, a)))
}

// A killed supervisor cannot collect a verdict. Its recorded pre-spawn lease
// still receives completion evidence after every process is confirmed gone.
// Tree, remote coverage and merge-hold guards remain in the pickup path.
// trace:BUG-2498 | ai:codex
fn finish_confirmed_review(root: &Path, attempt: &Attempt) -> Result<()> {
    // Callers have already confirmed all recorded processes gone.
    // trace:TASK-3777 | ai:codex
    if let Some(snapshot) = &attempt.review_lease {
        let lease: crate::SessionLease = toml::from_str(snapshot)?;
        if crate::lease_path(root, &lease.id).exists()
            && !crate::ownership::reader::review_lease_ended(root, &lease)
        {
            crate::ownership::reader::review_finished(root, &lease)?;
        }
    }
    Ok(())
}

fn attempt_mut<'a>(state: &'a mut State, key: &str, token: &str) -> Option<&'a mut Attempt> {
    state
        .entries
        .get_mut(key)
        .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
}

/// Every process of a superseded attempt is provably gone: each recorded
/// identity, every member of its recorded group, and any process that was
/// being spawned but never recorded.
// trace:TASK-1945 | ai:claude
fn confirm(attempt: &Attempt) -> std::result::Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err(signal::UNSUPPORTED.into());
    }
    if let Some(process) = attempt.recorded().into_iter().find(|p| p.alive()) {
        return Err(format!("recorded process {} is still running", process.pid));
    }
    // BUG-2425: a group led from another PID namespace cannot be scanned
    // here; with every recorded process past its backstop (checked above),
    // the group is gone with its namespace. trace:BUG-2425 | ai:claude
    let anchor = attempt.dispatcher.as_ref().or(attempt.identity.as_ref());
    if anchor.is_some_and(|a| !a.observable()) {
        return Ok(());
    }
    let unrecorded = attempt.unrecorded_descendant()
        || (attempt.identity.is_none() && attempt.dispatcher.is_none());
    match attempt.pgid {
        Some(pgid) => {
            if let Some(pid) = signal::group_members(pgid)?.first() {
                return Err(format!(
                    "process {pid} in the review's process group {pgid} is still running"
                ));
            }
        }
        None if unrecorded => {
            return Err("a review process was being spawned but never recorded, and no process group was recorded to prove it gone".into())
        }
        None => {}
    }
    Ok(())
}

fn stale_message(
    repository: &str,
    key: &str,
    change: &forge::Change,
    to: &str,
) -> aida_core::mailbox::Message {
    stale_message_for(repository, key, change, Some(&change.base), to)
}

/// The stale-head notice naming the change's own base. `None` renders the
/// original default-branch wording, accepted only for a pending notice
/// recorded before the base was named. The ID never depends on the wording.
// trace:TASK-1993 | ai:claude
fn stale_message_for(
    repository: &str,
    key: &str,
    change: &forge::Change,
    base: Option<&str>,
    to: &str,
) -> aida_core::mailbox::Message {
    use aida_core::mailbox::{Intent, Message, Recipient, SenderSource};
    let (behind, merge) = match base {
        Some(base) => (
            format!("is behind its base branch `{base}`"),
            format!("Merge `{base}`"),
        ),
        None => (
            "is behind the default branch".into(),
            "Merge the default branch".into(),
        ),
    };
    let subject = match base {
        Some(base) => format!("{key}: merge `{base}` before review"),
        None => format!("{key}: merge the default branch before review"),
    };
    let id = format!(
        "review-watch-{:x}",
        Sha256::digest(format!("{repository}\0{key}\0{}\0stale", change.head))
    );
    Message { id: id.clone(), thread_id: id, from: "review-watch".into(), to: Recipient::Agent(to.to_string()),
        timestamp: chrono::Utc::now().timestamp_millis(), in_reply_to: None, // trace:BUG-B6B1278890774EF993D2EBC327862470-032 | ai:claude
        body: format!("{key} at {} {behind} with overlapping paths. {merge} into the branch and push before review. The watcher did not update the branch.", change.head),
        subject: Some(subject), urgent: false, intent: Intent::Request,
        retracted: false, deleted: false, archived: false, from_source: SenderSource::Explicit,
        from_role: None, relayed_from: None }
}

/// A deterministic mailbox ID and no-clobber publication close the
/// delivery-before-sent-receipt crash window without overwriting reader state.
fn deliver(root: &Path, notice: &Notice) -> Result<()> {
    let runtime = Directory::shared(&root.join(".aida"))?;
    let mailbox = Directory::shared(&runtime.path().join("mailbox"))?;
    let name = format!("{}.json", notice.message.id);
    let mut temp = tempfile::NamedTempFile::new_in(mailbox.path())?;
    temp.write_all(&serde_json::to_vec_pretty(&notice.message)?)?;
    temp.as_file().sync_all()?;
    match temp.persist_noclobber(mailbox.path().join(&name)) {
        Ok(_) => {
            mailbox.handle.sync_all()?;
            Ok(())
        }
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = mailbox
                .read_file(&name, true)?
                .context("notification disappeared during retry")?;
            let old: aida_core::mailbox::Message = serde_json::from_slice(&bytes)?;
            anyhow::ensure!(
                old.id == notice.message.id
                    && old.thread_id == notice.message.thread_id
                    && old.from == notice.message.from
                    && old.to == notice.message.to
                    && old.body == notice.message.body
                    && old.subject == notice.message.subject,
                "notification ID collision; existing message retained"
            );
            Ok(())
        }
        Err(e) => Err(e.error.into()),
    }
}

fn queue_args(key: &str, vendor: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "queue".into(),
        "work".into(),
        key.into(),
        "--role".into(),
        "reviewer".into(),
        "--no-human".into(),
        "--fresh".into(),
    ];
    if let Some(vendor) = vendor {
        args.extend(["--vendor".into(), vendor.into()]);
    }
    args
}

// Clear inherited orchestration/gate context so queue work must take its
// ordinary standalone attested path in the same pinned repository.
// trace:TASK-1993 | ai:codex
fn watch_command(repo: &forge::Repository, root: &Path, program: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root).stdin(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| {
            key.starts_with("AIDA_REVIEW_")
                || key.starts_with("AIDA_FROM_PR_")
                || key.starts_with("AIDA_AGENT_GATE_")
        }) {
            command.env_remove(key);
        }
    }
    command
        .env_remove("AIDA_GRADED_REVIEW_CONTEXT")
        .env_remove("AIDA_SESSION_PURPOSE")
        .env_remove("AIDA_CARGO_NO_SLOT");
    repo.pin_command(&mut command);
    command
}

// Kept separate from executable resolution so process-level fixtures exercise
// the production receipt/preflight/queue command boundary without an LLM.
// trace:TASK-1993 | ai:codex
fn child_with_program(
    root: &Path,
    repo: &forge::Repository,
    reservation: &str,
    program: &Path,
) -> Result<()> {
    let (key, token) = reservation
        .split_once(':')
        .context("invalid launch reservation")?;
    let id = parse_key(key)?;
    let store = Store::open(root, &repo.identity)?;
    let round = std::cell::Cell::new(0);
    execute_reservation(
        &store,
        key,
        token,
        |attempt| {
            admission::require_pinned(root, &attempt.admission_hub)?;
            let checked = (|| {
                let mut current = repo.change(root, id)?;
                repo.probe(root, &mut current)?;
                anyhow::ensure!(
                    current.head == attempt.head
                        && current.key() == key
                        && current.overlap == Some(false),
                    "head changed before review dispatch"
                );
                round.set(store.round_for(key, attempt)?);
                Ok::<_, anyhow::Error>(())
            })();
            if let Err(error) = checked {
                if let Err(cancel) = admission::transition(
                    root,
                    key,
                    &attempt.head,
                    token,
                    admission::State::Cancelled,
                    format!("pre-spawn preflight failed: {error:#}"),
                ) {
                    eprintln!("{key}: hub claim retained: {cancel:#}");
                }
                return Err(error);
            }
            // The local receipt is process-bound; the hub token is consumed
            // exactly once before any queue-work/reviewer process is started.
            // trace:STORY-2492 | ai:codex
            admission::transition(
                root,
                key,
                &attempt.head,
                token,
                admission::State::Running,
                format!(
                    "dispatcher {}:{}:{} passed exact-head preflight",
                    boot_id()?,
                    std::process::id(),
                    Identity::current()?.start
                ),
            )?;
            Ok(())
        },
        |attempt| {
            let mut cmd = watch_command(repo, root, program);
            cmd.args(queue_args(key, attempt.vendor.as_deref()))
                .env(WATCH_HEAD, &attempt.head)
                .env("AIDA_REVIEW_ROUND", round.get().to_string());
            // Mark spawning first, then record the child: a crash between the
            // two leaves the attempt held as ambiguous, never freed.
            // trace:TASK-1993 | ai:claude
            retry_busy(|| store.record_worker(key, token, None))?;
            let mut child = crate::process_retry::RetryEtxtbsy::spawn_retrying_etxtbsy(&mut cmd)?;
            match Identity::child(&child) {
                Ok(worker) => {
                    if let Err(error) = retry_busy(|| store.record_worker(key, token, Some(worker.clone()))) {
                        eprintln!("{key}: could not record the queue-work child; the attempt stays held as ambiguous: {error:#}");
                    }
                }
                Err(error) => eprintln!("{key}: queue-work child identity unavailable; the attempt stays held as ambiguous: {error:#}"),
            }
            let status = child.wait()?;
            // An observed queue exit alone cannot prove an unrecorded reviewer
            // stopped. Ambiguous descendants retain the distributed claim.
            // trace:STORY-2492 | ai:codex
            finish_admission(root, &store, key, token, &attempt.head, &status.to_string())?;
            if !status.success() {
                // trace:TASK-1993 | ai:claude
                let log = format!("{key}-{token}.log");
                let reason = log_tail(&store, &log).unwrap_or_default();
                anyhow::bail!("queue work exited {status}: {reason} (full log: .aida/review-watch/logs/{log})");
            }
            Ok(())
        },
    )
}

// A failed terminal publication is surfaced and the hub claim remains held.
// trace:STORY-2492 | ai:codex
fn finish_admission(
    root: &Path,
    store: &Store,
    key: &str,
    token: &str,
    head: &str,
    status: &str,
) -> Result<()> {
    let current = store.load()?;
    let finished = current
        .entries
        .get(key)
        .and_then(|e| e.attempts.iter().find(|a| a.token == token))
        .context("review attempt disappeared at exit")?;
    if !finished.unrecorded_descendant()
        && [&finished.worker, &finished.reviewer]
            .into_iter()
            .flatten()
            .all(|p| p.observable() && !p.alive())
    {
        admission::transition(root, key, head, token, admission::State::Terminal,
            format!("owner observed queue exit {status}; all recorded worker/reviewer identities stopped"))?;
    }
    Ok(())
}

fn execute_reservation(
    store: &Store,
    key: &str,
    token: &str,
    preflight: impl FnOnce(&Attempt) -> Result<()>,
    work: impl FnOnce(&Attempt) -> Result<()>,
) -> Result<()> {
    // No work begins before the durable, process-bound receipt. If this
    // dispatcher dies before a terminal receipt, the attempt stays Running
    // until a watcher proves the receipt's process identity is gone.
    let attempt = retry_busy(|| store.transition(key, token, Phase::Running))?;
    if let Err(error) = preflight(&attempt) {
        // No queue-work process was started, so this reservation can retry
        // without consuming a review round or claiming reviewer completion.
        retry_busy(|| store.transition(key, token, Phase::Failed))?;
        return Err(error);
    }
    if let Err(error) = work(&attempt) {
        // The exit was observed in-process, so it is not ambiguous: record it
        // terminally with its reason. Only a lost dispatcher stays Running.
        // trace:TASK-1993 | ai:claude
        retry_busy(|| store.transition_with(key, token, Phase::Exited, Some(format!("{error:#}"))))
            .with_context(|| format!("{error:#}"))?;
        return Err(error);
    }
    retry_busy(|| store.transition(key, token, Phase::Completed))?;
    Ok(())
}

/// Retry a state transition through lock contention for the claim deadline:
/// terminal receipts get the same patience as the claim, so a finished review
/// is not left Running by a briefly busy lock.
// trace:TASK-1993 | ai:claude
fn retry_busy<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let deadline = std::time::Instant::now() + CLAIM_DEADLINE;
    loop {
        match f() {
            Err(e) if is_busy(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            result => return result,
        }
    }
}

/// The last non-empty line of a dispatcher log: queue work's refusal reason.
// trace:TASK-1993 | ai:claude
fn log_tail(store: &Store, name: &str) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let logs = Directory::existing(&store.dir.path().join("logs")).ok()??;
    let mut file = logs.file(name, false).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(4096))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(bounded_diagnostic)
}

const REVIEWER_POLICY: &str = "Review-watch policy: do not run Cargo or local test/build commands. Inspect source and existing CI evidence; record the ordinary attested verdict. Never merge or update the branch.";

/// Called at the ordinary standalone-review launch boundary: outside a watch
/// launch, `None`; inside one, the prompt with the watch policy appended,
/// after revalidating head, overlap and reservation. The context restricts
/// the head and prompt; it supplies no seat or verdict authority.
// trace:TASK-1993 | ai:claude
pub(crate) fn reviewer_launch_prompt(root: &Path, id: u64, prompt: &str) -> Result<Option<String>> {
    Ok(reviewer_launch_context(root, id)?.then(|| format!("{prompt}\n\n{REVIEWER_POLICY}")))
}

pub(crate) fn reviewer_launch_context(root: &Path, id: u64) -> Result<bool> {
    let Some(head) = std::env::var_os(WATCH_HEAD) else {
        return Ok(false);
    };
    let head = head.to_str().context("invalid review-watch head context")?;
    anyhow::ensure!(valid_sha(head), "invalid review-watch head context");
    // Queue work revalidates before its verdict reset and the reviewer launch
    // asks again: the second call reuses the first's revalidated launch
    // instead of repeating the forge query, fetch and round transaction.
    // trace:TASK-1955 | ai:claude (TASK-1-415)
    let revalidated = LAUNCH.with(|launch| {
        launch.borrow().as_ref().is_some_and(|(r, _, key, h)| {
            r == root && h == head && parse_key(key).is_ok_and(|n| n == id)
        })
    });
    if revalidated {
        return Ok(true);
    }
    let repository = forge::Repository::load(root)?;
    let mut current = repository.change(root, id)?;
    if let Err(error) = repository.probe(root, &mut current) {
        if error.downcast_ref::<forge::FetchUnavailable>().is_some() {
            let store = Store::open(root, &repository.identity)?;
            let me = Identity::current()?;
            store.transaction(&current.key(), |state| {
                let attempt = state
                    .entries
                    .get_mut(&current.key())
                    .and_then(|entry| {
                        entry
                            .attempts
                            .iter_mut()
                            .find(|a| admitted(a) && a.head == head)
                    })
                    .context("fetch retry reservation disappeared")?;
                anyhow::ensure!(
                    attempt.worker.as_ref() == Some(&me) && !attempt.reviewer_spawning,
                    "fetch retry worker identity changed"
                );
                attempt.retryable_fetch = true;
                Ok(())
            })?;
        }
        return Err(error);
    }
    anyhow::ensure!(
        current.head == head && current.overlap == Some(false),
        "review-watch head changed before reviewer launch; no review started"
    );
    let store = Store::open(root, &repository.identity)?;
    let key = current.key();
    let state = store.load()?;
    let attempt = state
        .entries
        .get(&key)
        .and_then(|entry| {
            entry
                .attempts
                .iter()
                .find(|a| admitted(a) && a.head == head)
        })
        .context("missing watcher reservation at reviewer launch")?;
    store.round_for(&key, attempt)?;
    LAUNCH.with(|launch| {
        *launch.borrow_mut() = Some((
            root.to_path_buf(),
            repository.identity.clone(),
            key,
            head.to_string(),
        ))
    });
    Ok(true)
}

// The revalidated watch launch this queue-work process serves, if any. The
// launch check and the reviewer spawn run on the same thread.
// trace:TASK-1993 | ai:claude
type Launch = (PathBuf, String, String, String);
thread_local! {
    static LAUNCH: std::cell::RefCell<Option<Launch>> = const { std::cell::RefCell::new(None) };
    // TASK-2807: agent processes this thread actually spawned, so a pickup
    // launch tells "the spawn failed" (rolled back) from "an agent ran".
    // trace:TASK-2807 | ai:claude
    static SPAWNED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// TASK-2807: how many agent processes this thread has spawned through
/// [`reviewer_status_on_spawn`]. Bumped only once `spawn` returned a child.
// trace:TASK-2807 | ai:claude
pub(crate) fn agent_spawn_count() -> u64 {
    SPAWNED.with(std::cell::Cell::get)
}

fn note_agent_spawned() {
    SPAWNED.with(|n| n.set(n.get() + 1));
}

/// Run the reviewer process queue work launches. Inside a watch launch, mark
/// it spawning, spawn it, and record its identity before waiting: it carries
/// the review and can outlive queue work and the dispatcher. Outside one,
/// this is the ordinary blocking run.
// trace:TASK-1993 | ai:claude
pub(crate) fn reviewer_status(cmd: &mut Command) -> std::io::Result<std::process::ExitStatus> {
    reviewer_status_on_spawn(cmd, || {})
}

// trace:STORY-1843 | ai:codex
pub(crate) fn reviewer_status_on_spawn(
    cmd: &mut Command,
    on_spawn: impl FnOnce(),
) -> std::io::Result<std::process::ExitStatus> {
    use crate::process_retry::RetryEtxtbsy;
    let launch = LAUNCH.with(|launch| launch.borrow().clone());
    let Some((root, identity, key, head)) = launch else {
        let mut child = cmd.spawn_retrying_etxtbsy()?;
        note_agent_spawned();
        on_spawn();
        return child.wait();
    };
    let store = Store::open(&root, &identity).map_err(std::io::Error::other)?;
    retry_busy(|| store.record_reviewer(&key, &head, None)).map_err(std::io::Error::other)?;
    let mut child = cmd.spawn_retrying_etxtbsy()?;
    note_agent_spawned();
    on_spawn();
    match Identity::child(&child) {
        Ok(reviewer) => {
            if let Err(error) =
                retry_busy(|| store.record_reviewer(&key, &head, Some(reviewer.clone())))
            {
                eprintln!("{key}: could not record the reviewer process; the attempt stays held as ambiguous: {error:#}");
            }
        }
        Err(error) => eprintln!(
            "{key}: reviewer identity unavailable; the attempt stays held as ambiguous: {error:#}"
        ),
    }
    child.wait()
}

// trace:TASK-3472 | ai:codex
fn resolve_max_concurrent(root: &Path, flag: Option<u8>) -> Result<u8> {
    if let Some(max) = flag {
        anyhow::ensure!((1..=8).contains(&max), "review concurrency must be 1..=8");
        return Ok(max);
    }
    let path = root.join(".aida/config.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(2),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let config: toml::Value =
        toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let Some(value) = config
        .get("review_watch")
        .and_then(|s| s.get("max_concurrent"))
    else {
        return Ok(2);
    };
    let max = value
        .as_integer()
        .filter(|max| (1..=8).contains(max))
        .context("`[review_watch] max_concurrent` must be an integer in 1..=8")?;
    Ok(max as u8)
}

pub(crate) fn run(args: &ReviewWatchArgs) -> Result<()> {
    run_with_program(args, &std::env::current_dir()?, None).map(|_| ())
}

// The executable seam replaces only the model/CLI process in end-to-end tests;
// discovery, admission, exact-head preflight and receipts remain production code.
// trace:STORY-2492.ac540936 | ai:codex
pub(crate) fn run_with_program(
    args: &ReviewWatchArgs,
    start: &Path,
    program: Option<&Path>,
) -> Result<Vec<Child>> {
    let root = forge::main_root(start)?;
    let repo = forge::Repository::load(&root)?;
    crate::gate::seat_authority::validate_child_delegation(&root, "reviewer")?;
    if let Some(reservation) = args.launch_reservation.as_deref() {
        let program = program
            .map(Path::to_path_buf)
            .unwrap_or_else(crate::aida_exe_path);
        return child_with_program(&root, &repo, reservation, &program).map(|_| Vec::new());
    }
    let store = Store::open(&root, &repo.identity)?;
    // trace:TASK-1955 | ai:claude (TASK-1-412)
    if let Some(key) = args.release.as_deref() {
        admission::require_released(&root, key)?;
        let released = store.release(key)?;
        println!("{key}: released {released} recorded review attempt(s); its current head can be reviewed again.");
        return Ok(Vec::new());
    }
    // Resolve once per watcher, before discovery or admission.
    // trace:TASK-3472 | ai:codex
    let mut resolved = args.clone();
    resolved.max_concurrent = Some(resolve_max_concurrent(&root, args.max_concurrent)?);
    let args = &resolved;
    // trace:TASK-1955 | ai:claude (TASK-1-413)
    let swept = forge::sweep_probe_refs(&root);
    if swept > 0 {
        eprintln!("review-watch: removed {swept} leaked probe ref(s)");
    }
    let mut children = Vec::<Dispatcher>::new();
    let mut reported_trains = BTreeSet::new();
    loop {
        service::check_controller(&root)?;
        for outcome in reap_dispatchers(&store, &mut children) {
            service::controller_dispatch(&root, outcome.as_deref(), outcome.is_none())?;
        }
        let mut pass_failure = None;
        // trace:STORY-2492 | ai:codex
        if !sync_pass(&root, args.once)? {
            std::thread::sleep(std::time::Duration::from_secs(args.interval));
            continue;
        }
        let changes = match observe_pass_input(&root, "discovery", repo.discover(&root)) {
            Ok(changes) => changes,
            Err(error) if !args.once => {
                eprintln!("review-watch discovery refused: {error:#}");
                std::thread::sleep(std::time::Duration::from_secs(args.interval));
                continue;
            }
            Err(error) => return Err(error),
        };
        let open: BTreeSet<String> = changes.iter().map(forge::Change::key).collect();
        match store.prune(&open) {
            Ok(_) => {}
            Err(error) if is_busy(&error) => eprintln!("review-watch prune deferred: busy"),
            Err(error) => return Err(error),
        }
        // A change that closed or went draft mid-supersede is no longer
        // discovered; its superseding attempt still has to finish.
        // trace:TASK-1945 | ai:claude
        for (key, token) in store.load()?.superseding() {
            if open.contains(&key) {
                continue;
            }
            match advance(&store, &key, &token) {
                Ok(Some(cause)) => eprintln!("{key}: {cause}"),
                Ok(None) => {}
                Err(error) if is_busy(&error) => eprintln!("{key}: watcher admission is busy"),
                Err(error) if stops_watcher(&error) => return Err(error),
                Err(error) => eprintln!("{key}: refused: {error:#}"),
            }
        }
        // One lazy classification per pass, rather than one /proc scan per PR.
        // trace:BUG-2442 | ai:codex
        let authors = update::AuthorLiveness::default();
        watch_changes(
            &store,
            reviewable_changes(changes, &mut reported_trains),
            |change| {
                let result = watch_change(
                    &root,
                    &repo,
                    &store,
                    args,
                    &mut children,
                    &authors,
                    change,
                    program,
                );
                if let Err(error) = &result {
                    if !is_busy(error) {
                        pass_failure = Some(format!("{error:#}"));
                    }
                }
                result
            },
        )?;
        // Synchronous setup/spawn errors are dispatch failures too: an idle
        // discovery pass must not clear them before a dispatch succeeds.
        // trace:STORY-2691 | ai:codex
        service::controller_dispatch(&root, pass_failure.as_deref(), false)?;
        service::controller_pass(&root, None)?;
        if args.once {
            return Ok(children.into_iter().map(|d| d.child).collect());
        }
        std::thread::sleep(std::time::Duration::from_secs(args.interval));
    }
}

// Confirm outcomes by reaping our own children; reservations alone are not success.
// trace:STORY-2691 | ai:codex
fn reap_dispatchers(store: &Store, children: &mut Vec<Dispatcher>) -> Vec<Option<String>> {
    let mut outcomes = Vec::new();
    children.retain_mut(|d| match d.child.try_wait() {
        Ok(Some(status)) => {
            // Match the durable receipt by both change and attempt token. A
            // superseded attempt may already be terminal by the time we reap.
            // Unreadable evidence cannot excuse an unexpected child failure.
            let cancelled = store.load().ok().is_some_and(|state| {
                state.entries.get(&d.key).is_some_and(|entry| {
                    entry
                        .attempts
                        .iter()
                        .any(|attempt| attempt.token == d.token && attempt.supersede.is_some())
                })
            });
            if !cancelled {
                outcomes.push(
                    (!status.success()).then(|| format!("{}: dispatcher exited {status}", d.key)),
                );
            }
            if let Err(error) = mark_unclaimed_exit(store, &d.key, &d.token, &status.to_string()) {
                eprintln!("{}: could not record dispatcher exit: {error:#}", d.key);
            }
            false
        }
        _ => true,
    });
    outcomes
}

// Train members were reviewed at their own heads. Exclude the landing PR
// before probes, stale-author updates, reservations or reviewer spawning.
// Keep diagnostics once per change for this watcher, across polling passes.
// trace:BUG-2472 | ai:codex
fn reviewable_changes(
    changes: Vec<forge::Change>,
    reported_trains: &mut BTreeSet<String>,
) -> Vec<forge::Change> {
    changes
        .into_iter()
        .filter(|change| {
            if change
                .head_branch
                .as_deref()
                .is_some_and(|branch| branch.starts_with(crate::pr_train::BRANCH_PREFIX))
            {
                let key = change.key();
                if reported_trains.insert(key.clone()) {
                    eprintln!("{key}: train PR: not reviewed");
                }
                false
            } else {
                true
            }
        })
        .collect()
}

// A transient synchronization refusal skips only this pass. `--once` retains
// its nonzero contract; continuous mode will try again after the interval.
// trace:STORY-b6b1278890774ef993d2ebc327862470-418.ac8cff0f | ai:codex
fn observe_pass_input<T>(root: &Path, label: &str, result: Result<T>) -> Result<T> {
    if let Err(error) = &result {
        if !is_busy(error) {
            service::controller_pass(root, Some(&format!("{label}: {error:#}")))?;
        }
    }
    result
}

fn sync_pass(root: &Path, once: bool) -> Result<bool> {
    match observe_pass_input(root, "synchronization", admission::sync(root)) {
        Ok(()) => Ok(true),
        Err(error) if once => Err(error),
        Err(error) => {
            eprintln!("review-watch synchronization refused for this pass: {error:#}");
            Ok(false)
        }
    }
}
#[cfg(test)]
pub(crate) fn fixture_sync_pass(root: &Path, once: bool) -> Result<bool> {
    sync_pass(root, once)
}

/// Per-change refusals are durable diagnostics, never a reason to abandon the
/// remaining changes in this pass (including `--once`).
// trace:BUG-2471 | ai:codex
fn watch_changes(
    store: &Store,
    changes: Vec<forge::Change>,
    mut watch: impl FnMut(forge::Change) -> Result<()>,
) -> Result<()> {
    for change in changes {
        let key = change.key();
        match watch(change.clone()) {
            Ok(()) => {}
            Err(error) if is_busy(&error) => eprintln!("{key}: watcher admission is busy"),
            Err(error) if stops_watcher(&error) => {
                return Err(error.context(format!("{key}: review watch refused")))
            }
            Err(error) => {
                let reason = format!("refused: {error:#}");
                eprintln!("{key}: {reason}");
                // Diagnostic contention or per-change I/O must not stop the pass.
                // Preserve only the whole-state failures tagged by store.load().
                match retry_busy(|| record_skip(store, &change, Some(&reason))) {
                    Ok(()) => {}
                    Err(error) if is_busy(&error) => {
                        eprintln!("{key}: could not record refusal: watcher admission is busy")
                    }
                    Err(error) if stops_watcher(&error) => return Err(error),
                    Err(error) => eprintln!("{key}: could not record refusal: {error:#}"),
                }
            }
        }
    }
    Ok(())
}

/// Only whole-state failure stops the watcher; one change's refusal must not
/// stall every other change.
// trace:BUG-2471 | ai:codex
fn stops_watcher(error: &anyhow::Error) -> bool {
    error.downcast_ref::<GlobalState>().is_some()
}

struct Dispatcher {
    child: Child,
    key: String,
    token: String,
}

/// An observed dispatcher exit before it claimed its receipt (a refused grant,
/// an unusable store, a claim that timed out) can never start a review.
// trace:TASK-1993 | ai:claude
fn mark_unclaimed_exit(store: &Store, key: &str, token: &str, status: &str) -> Result<()> {
    let tail = log_tail(store, &format!("{key}-{token}.log"));
    let cancelled = store.transaction(key, |state| {
        let Some(a) = state
            .entries
            .get_mut(key)
            .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
            .filter(|a| a.identity.is_none())
        else {
            return Ok(None);
        };
        // Only the parent observing its Child exit can prove this pre-spawn
        // failure. Background PID/age reconciliation never cancels hub claims.
        // trace:STORY-2492 | ai:codex
        let can_cancel = a.admission_hub.is_some()
            && matches!(a.phase, Phase::Reserved | Phase::Superseding)
            && !a.spawning
            && !a.reviewer_spawning
            && a.worker.is_none()
            && a.reviewer.is_none();
        let diagnostic = bounded_diagnostic(&format!(
            "dispatcher exited {status} before claiming its launch receipt: {}",
            tail.as_deref().unwrap_or("no log output")
        ));
        match a.phase {
            Phase::Reserved => {
                a.phase = Phase::Exited;
                a.diagnostic = Some(diagnostic);
            }
            Phase::Superseding => {
                a.phase = Phase::Superseded;
                a.diagnostic = Some(diagnostic);
            }
            _ => {}
        }
        Ok(can_cancel.then(|| a.clone()))
    })?;
    if let Some(a) = cancelled {
        admission::require_pinned(&store.root, &a.admission_hub)?;
        admission::transition(&store.root, key, &a.head, token, admission::State::Cancelled,
            format!("owner reaped dispatcher exit {status} before any process-bound claim or worker spawn"))?;
    }
    Ok(())
}

/// The dispatcher for this reservation could not be spawned, so nothing of
/// it runs.
// trace:TASK-1993 | ai:claude
fn mark_spawn_failed(store: &Store, key: &str, token: &str) -> Result<()> {
    store.transaction(key, |state| {
        let a = attempt_mut(state, key, token).context("launch reservation disappeared")?;
        anyhow::ensure!(a.identity.is_none(), "launch reservation already claimed");
        a.phase = match a.phase {
            Phase::Reserved => Phase::Failed,
            // Superseded before the spawn failed: nothing was started.
            // trace:TASK-1945 | ai:claude (TASK-2150)
            Phase::Superseding => Phase::Superseded,
            _ => anyhow::bail!("launch reservation already claimed"),
        };
        Ok(())
    })
}

fn is_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|e| {
        e.downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
    })
}

// trace:TASK-1993 | ai:claude
fn watch_change(
    root: &Path,
    repo: &forge::Repository,
    store: &Store,
    args: &ReviewWatchArgs,
    children: &mut Vec<Dispatcher>,
    authors: &update::AuthorLiveness,
    mut change: forge::Change,
    program: Option<&Path>,
) -> Result<()> {
    service::check_controller(root)?;
    service::validate_controller_delegation(root)?;
    let key = change.key();
    // An obsolete head is superseded before anything else, whether or not
    // the new head is stale or overlapping. trace:TASK-1945 | ai:claude
    if let Some(reason) = supersede(store, &change, || Ok(repo.change(root, change.id)?.head))? {
        eprintln!("{key}: {reason}");
        return Ok(());
    }
    // trace:STORY-2705 | ai:codex
    if let Some(reason) = crate::review_implementer_gate::deferred(root, change.id, change.gitlab)?
    {
        eprintln!("{key}: {reason}");
        record_skip(store, &change, Some(&reason))?;
        return Ok(());
    }
    // trace:STORY-2640 | ai:codex — reserve before the seat can open its PR.
    if let Some(branch) = change.head_branch.as_deref() {
        if crate::lane_fast_gate::blocks_review(root, branch)? {
            let reason = "host fast gate or its single repair round is pending";
            eprintln!("{key}: {reason}");
            // Replace a former live-lane diagnostic after that lease ends.
            // trace:STORY-2705 | ai:codex
            record_skip(store, &change, Some(reason))?;
            return Ok(());
        }
    }
    // A reviewed head is reported on, and needs no base probe to be skipped.
    // trace:TASK-1945 | ai:claude
    let live = ready::Live { root, repo };
    // A notified stale head whose author is gone gets its branch updated, or
    // is routed to the advisor, instead of waiting forever.
    // trace:BUG-2426 | ai:claude
    if let Some(reason) = update::follow_up(
        root,
        store,
        &change,
        &|c| authors.author_live(root, c),
        &|c| update::forge_update(root, repo, c),
    )? {
        eprintln!("{key}: {reason}");
        record_skip(store, &change, Some(&reason))?;
        return report(root, store, &change, &live);
    }
    // trace:TASK-1955 | ai:claude (TASK-2064, TASK-1-411)
    if let Some(reason) = probe_unneeded(store, &change)? {
        eprintln!("{key}: {reason}");
        record_skip(store, &change, Some(&reason))?;
        return report(root, store, &change, &live);
    }
    repo.probe(root, &mut change)
        .context("base evidence refused")?;
    crate::gate::seat_authority::validate_child_delegation(root, "reviewer")?;
    let action = select(
        store,
        &change,
        args.vendor.clone(),
        args.max_concurrent
            .context("unresolved review concurrency")?,
    )?;
    record_skip(
        store,
        &change,
        match &action {
            Action::Skip(reason) => Some(reason.as_str()),
            _ => None,
        },
    )?;
    match action {
        Action::Skip(reason) => {
            eprintln!("{key}: {reason}");
            report(root, store, &change, &live)?;
        }
        Action::Notify(notice) => {
            deliver(root, &notice)?;
            store.transaction(&key, |state| {
                let pending = state
                    .entries
                    .get_mut(&key)
                    .and_then(|e| e.notices.get_mut(&change.head))
                    .context("notification receipt disappeared")?;
                anyhow::ensure!(
                    pending.message.id == notice.message.id,
                    "notification receipt changed"
                );
                pending.sent = true;
                Ok(())
            })?;
            eprintln!(
                "{key}: merge-base notice delivered to {}",
                match &notice.message.to {
                    aida_core::mailbox::Recipient::Agent(to) => to.as_str(),
                    aida_core::mailbox::Recipient::Broadcast => "everyone",
                }
            );
            report(root, store, &change, &live)?;
        }
        Action::Dispatch(mut attempt) => {
            // Reserve at the sole authority before spawning even the dispatcher.
            // A refused/uncertain claim never authorizes a child.
            // trace:STORY-2492 | ai:codex
            if !reserve_dispatch(root, store, &key, &mut attempt)? {
                eprintln!("{key}: head/change already reserved or reviewed at admission hub");
                return Ok(());
            }
            use std::os::unix::process::CommandExt;
            let spawned = (|| {
                let logs = Directory::open(&store.dir.path().join("logs"))?;
                let log = logs.file(&format!("{key}-{}.log", attempt.token), true)?;
                let program = program
                    .map(Path::to_path_buf)
                    .unwrap_or_else(crate::aida_exe_path);
                let mut command = watch_command(repo, root, &program);
                command
                    .args([
                        "review",
                        "watch",
                        "--launch-reservation",
                        &format!("{key}:{}", attempt.token),
                    ])
                    .stdout(Stdio::from(log.try_clone()?))
                    .stderr(Stdio::from(log))
                    .process_group(0);
                Ok::<_, anyhow::Error>(crate::process_retry::RetryEtxtbsy::spawn_retrying_etxtbsy(
                    &mut command,
                )?)
            })();
            match spawned {
                Ok(mut child) => {
                    // trace:TASK-1993 | ai:claude
                    // Capture before try_wait can reap a fast-exiting dispatcher.
                    // trace:TASK-3365 | ai:codex
                    let identity = Identity::child(&child).ok();
                    let exited = child.try_wait().ok().flatten();
                    let pid = child.id();
                    let token = attempt.token.clone();
                    children.push(Dispatcher {
                        child,
                        key: key.clone(),
                        token: token.clone(),
                    });
                    match exited {
                        Some(status) => {
                            mark_unclaimed_exit(store, &key, &token, &status.to_string())?
                        }
                        None => store.transaction(&key, |state| {
                            let Some(a) = state
                                .entries
                                .get_mut(&key)
                                .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
                            else {
                                return Ok(());
                            };
                            // A reservation superseded before this record
                            // still needs its dispatcher pinned.
                            // trace:TASK-1945 | ai:claude
                            if matches!(a.phase, Phase::Reserved | Phase::Superseding)
                                && a.identity.is_none()
                            {
                                a.dispatcher = identity;
                            }
                            // The dispatcher may have claimed first; the group
                            // it leads is still the one spawned here.
                            if active(a)
                                && a.pgid.is_none()
                                && a.identity.as_ref().is_none_or(|i| i.pid == pid)
                            {
                                a.pgid = Some(pid);
                            }
                            Ok(())
                        })?,
                    }
                    eprintln!("{key}: review admitted for {}", change.head);
                }
                Err(error) => {
                    cancel_spawn_failure(root, store, &key, &attempt, &error.to_string())?;
                    return Err(error).context("could not spawn review dispatcher");
                }
            }
        }
    }
    Ok(())
}

// A hub failure must not prevent recording a proven local pre-spawn failure.
// trace:STORY-2492 | ai:codex
fn cancel_spawn_failure(
    root: &Path,
    store: &Store,
    key: &str,
    attempt: &Attempt,
    error: &str,
) -> Result<()> {
    // Free local capacity first, even if the cancellation loses connectivity.
    retry_busy(|| mark_spawn_failed(store, key, &attempt.token))?;
    if let Err(cancel) = admission::transition(
        root,
        key,
        &attempt.head,
        &attempt.token,
        admission::State::Cancelled,
        format!("dispatcher spawn failed before creating any process: {error}"),
    ) {
        eprintln!("{key}: hub claim retained; operator recovery required: {cancel:#}");
    }
    Ok(())
}

// One boundary for local process/capacity receipts and distributed admission.
// trace:STORY-2492 | ai:codex
// trace:TASK-b6b1278890774ef993d2ebc327862470-871 | ai:codex
fn reserve_dispatch(root: &Path, store: &Store, key: &str, attempt: &mut Attempt) -> Result<bool> {
    let result = (|| {
        attempt.admission_hub = admission::Hub::load(root)?;
        if let Some(hub) = &attempt.admission_hub {
            hub.validate_repository(&store.repository)?;
        }
        store.transaction(key, |state| {
            let local = attempt_mut(state, key, &attempt.token)
                .context("reservation disappeared before hub admission")?;
            local.admission_hub = attempt.admission_hub.clone();
            Ok(())
        })?;
        admission::reserve(root, key, &attempt.head, &attempt.token)
    })();
    match result {
        Ok(true) => Ok(true),
        Ok(false) => {
            store.transaction(key, |state| {
                let local =
                    attempt_mut(state, key, &attempt.token).context("reservation disappeared")?;
                local.phase = Phase::RemoteHeld;
                Ok(())
            })?;
            Ok(false)
        }
        Err(error) => {
            mark_spawn_failed(store, key, &attempt.token).with_context(|| {
                format!("admission failed: {error:#}; local cleanup also failed")
            })?;
            Err(error)
        }
    }
}

/// Whether a skip reason parks the head: it is not reviewed and nothing in
/// flight will review it. A head already reviewed or under review, one
/// waiting only for a free reviewer slot, or one whose earlier head's review
/// is still being superseded, is not parked.
// trace:BUG-2426 | ai:claude
// trace:TASK-2813 | ai:claude
fn parks(reason: &str) -> bool {
    ![
        "head already has a recorded review",
        "head already reserved",
        "global reviewer capacity",
        "a review of an earlier head still holds this change",
    ]
    .iter()
    .any(|p| reason.starts_with(p))
}

/// Record (or clear) why the current head is parked, for `aida awaiting`.
/// The record keeps its first-seen time while the head stays parked, even
/// as the reason changes (notified, update requested, routed), so `since`
/// is since-parked; a steady skip writes no state.
// trace:BUG-2426 | ai:claude
// trace:TASK-2813 | ai:claude
fn record_skip(store: &Store, change: &forge::Change, reason: Option<&str>) -> Result<()> {
    let key = change.key();
    let line = reason.filter(|r| parks(r)).map(bounded_diagnostic);
    store.transaction(&key, |state| {
        let current = state.entries.get(&key).and_then(|e| e.skipped.as_ref());
        let same = match (current, line.as_deref()) {
            (Some(r), Some(line)) => r.head == change.head && r.line == line,
            (None, None) => true,
            _ => false,
        };
        if same {
            return Ok(());
        }
        let since = current
            .filter(|r| r.head == change.head)
            .map_or_else(chrono::Utc::now, |r| r.checked_at);
        let entry = state.entries.entry(key.clone()).or_default();
        entry.skipped = line.map(|line| Reported {
            head: change.head.clone(),
            line,
            checked_at: since,
        });
        Ok(())
    })
}

/// Every change whose current head the watcher is parking, as `(change,
/// head, reason, since)`, read without creating or changing any watcher
/// state (`aida awaiting`).
// trace:BUG-2426 | ai:claude
pub(crate) fn skipped_changes(
    root: &Path,
) -> Vec<(String, String, String, chrono::DateTime<chrono::Utc>)> {
    let Some(state) = read_only_state(root) else {
        return Vec::new();
    };
    state
        .entries
        .into_iter()
        .filter_map(|(key, entry)| {
            let r = entry.skipped?;
            Some((key, r.head, r.line, r.checked_at))
        })
        .collect()
}

/// Report a reviewed change's ship readiness, at most once per
/// [`READY_INTERVAL`] per head, and wake the advisor once when it is ready.
/// Report only: nothing here merges, pushes or updates a branch. A change
/// with a review still active, or with no recorded verdict, is not judged.
// trace:TASK-1945 | ai:claude
fn report(
    root: &Path,
    store: &Store,
    change: &forge::Change,
    evidence: &dyn ready::Evidence,
) -> Result<()> {
    let key = change.key();
    let state = store.load()?;
    let entry = state.entries.get(&key);
    if entry.is_some_and(|e| e.attempts.iter().any(active)) {
        return Ok(());
    }
    let recent =
        entry.is_some_and(|e| throttled(e, &change.head, chrono::Utc::now(), READY_INTERVAL));
    let history = store.history(&key);
    if let Ok(history) = &history {
        if history.keys.is_empty() {
            return Ok(());
        }
        // Not throttled: a failed delivery is retried on the next pass, and
        // never stops the readiness report.
        if let Err(error) = notice::send(root, store, change, history, evidence) {
            eprintln!("{key}: review notice not delivered; retried next pass: {error:#}");
        }
    }
    if recent {
        return Ok(());
    }
    // An unreadable verdict history is reported, never read as clean.
    let (readiness, approval) = match history {
        Ok(_) => ready::evaluate(change, evidence),
        Err(error) => (
            ready::Readiness::NotReady(format!("verdict history unreadable: {error:#}")),
            None,
        ),
    };
    let line = bounded_diagnostic(&readiness.line(change));
    eprintln!("{key} at {}: {line}", &change.head[..12]);
    let wake = store.transaction(&key, |state| {
        let repository = state.repository.clone();
        let entry = state.entries.entry(key.clone()).or_default();
        entry.readiness = Some(Reported {
            head: change.head.clone(),
            line: line.clone(),
            checked_at: chrono::Utc::now(),
        });
        let prefix = format!("{}:", change.head);
        let current = match (readiness.wake(), approval.as_ref()) {
            (Some(kind), Some(approval)) => Some((
                format!("{prefix}{kind}:{}", notice::digest(&approval.identity)),
                approval,
            )),
            _ => None,
        };
        prune_wakes(
            &mut entry.wakes,
            &prefix,
            current.as_ref().map(|(k, _)| k.as_str()),
        );
        let Some((wake_key, approval)) = current else {
            return Ok(None);
        };
        // A pending wake keeps the message it was recorded with, so a retry
        // re-delivers it under the same ID. The key names the approval, as
        // the message ID does: a new approval of the head is a new wake.
        // trace:TASK-1945 | ai:claude (TASK-2200)
        let wake = entry.wakes.entry(wake_key).or_insert_with(|| Notice {
            message: ready::wake_message(&repository, change, &readiness, approval),
            sent: false,
        });
        Ok((!wake.sent).then(|| wake.clone()))
    })?;
    let Some(wake) = wake else {
        return Ok(());
    };
    deliver(root, &wake)?;
    let event = crate::events::Event::new(
        None,
        "",
        crate::events::EventKind::ReviewWatchReady {
            pr: change.id,
            head: change.head.clone(),
            after_update_branch: matches!(
                readiness,
                ready::Readiness::ReadyAfterUpdateBranch { .. }
            ),
        },
    );
    crate::events::emit_recorded(root, &event).map_err(|e| anyhow::anyhow!(e))?;
    store.transaction(&key, |state| {
        let pending = state
            .entries
            .get_mut(&key)
            .and_then(|e| {
                e.wakes
                    .values_mut()
                    .find(|w| w.message.id == wake.message.id)
            })
            .context("ready wake receipt disappeared")?;
        pending.sent = true;
        Ok(())
    })?;
    eprintln!("{key}: ready wake delivered to the advisor");
    Ok(())
}

/// Keep only the wakes for the current head (`prefix` is `<head>:`), and of
/// those drop every unsent one but `current`: a pending wake whose readiness
/// kind or approval has since changed is never redelivered, and left in place
/// it would disable [`throttled`] for the head on every pass.
// trace:TASK-2238 | ai:claude
fn prune_wakes(wakes: &mut BTreeMap<String, Notice>, prefix: &str, current: Option<&str>) {
    wakes.retain(|k, w| k.starts_with(prefix) && (w.sent || Some(k.as_str()) == current));
}

/// Whether `head` was judged within `interval`, so this pass skips it. A
/// wake for the head still pending delivery is never throttled: its retry
/// runs on the next pass, not an interval later.
// trace:TASK-1945 | ai:claude (TASK-2204)
fn throttled(
    entry: &Entry,
    head: &str,
    now: chrono::DateTime<chrono::Utc>,
    interval: chrono::Duration,
) -> bool {
    let pending = entry
        .wakes
        .iter()
        .any(|(k, w)| !w.sent && k.split_once(':').is_some_and(|(h, _)| h == head));
    !pending
        && entry
            .readiness
            .as_ref()
            .is_some_and(|r| r.head == head && now - r.checked_at < interval)
}

/// Each change's last readiness report, `(change, "at <head>: <line>
/// (checked <when>)")`, read without creating or changing any watcher state
/// (`aida ps`).
// trace:TASK-1945 | ai:claude
pub(crate) fn readiness_reports(root: &Path) -> Vec<(String, String)> {
    let Some(state) = read_only_state(root) else {
        return Vec::new();
    };
    state
        .entries
        .iter()
        .filter_map(|(key, entry)| {
            let r = entry.readiness.as_ref()?;
            Some((
                key.clone(),
                format!(
                    "at {}: {} (checked {})",
                    &r.head[..12],
                    r.line,
                    r.checked_at.format("%Y-%m-%d %H:%M UTC")
                ),
            ))
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/task_1993_review_watch_tests.rs"]
mod tests;

// Exercise watch dispatch against the real consecutive-head pickup fixture.
// trace:BUG-2498 | ai:codex
#[cfg(test)]
pub(crate) use tests::{
    bug2498_confirm_superseded, bug2498_review_worktree_release_allows_one_watch_setup_retry,
};

// Real process fixtures: Linux only. trace:TASK-1945 | ai:claude
#[cfg(all(test, target_os = "linux"))]
#[path = "tests/task_1945_review_watch_supersede_tests.rs"]
mod supersede_tests;

#[cfg(test)]
#[path = "tests/task_1945_review_watch_ready_tests.rs"]
mod ready_tests;

#[cfg(test)]
#[path = "tests/task_1945_review_watch_notice_tests.rs"]
mod notice_tests;

#[cfg(test)]
#[path = "tests/bug_2426_review_watch_update_tests.rs"]
mod update_tests;

// Exercise the real history/admission precheck from the two-clone publication
// fixture without exposing watcher runtime internals to production callers.
// trace:STORY-2492 | ai:codex
#[cfg(test)]
pub(crate) fn synced_history_precheck(root: &Path, id: u64, head: &str) -> Result<Option<String>> {
    let repo = forge::Repository::load(root);
    let identity = repo
        .map(|r| r.identity)
        .unwrap_or_else(|_| "fixture".into());
    let store = Store::open(root, &identity)?;
    probe_unneeded(
        &store,
        &forge::Change {
            id,
            head: head.into(),
            base: "main".into(),
            author: "fixture-author".into(),
            gitlab: false,
            overlap: None,
            behind: None,
            head_branch: None,
        },
    )
}

#[cfg(test)]
pub(crate) use admission_fixture::{assert_failed_admission, fixture_admit_dispatch};

#[cfg(test)]
mod admission_fixture {
    use super::*;

    // The real selection/reserve boundary with a harmless fixture dispatcher.
    // trace:STORY-2492 | ai:codex
    #[cfg(test)]
    pub(crate) fn fixture_admit_dispatch(
        root: &Path,
        head: &str,
        program: &Path,
        marker: &Path,
    ) -> Result<bool> {
        let store = Store::open(root, "github:github.com/fixture/review")?;
        let change = forge::Change {
            id: 7,
            head: head.into(),
            base: "main".into(),
            author: "fixture-author".into(),
            gitlab: false,
            overlap: Some(false),
            behind: Some(0),
            head_branch: None,
        };
        let Action::Dispatch(mut attempt) = select(&store, &change, None, 2)? else {
            return Ok(false);
        };
        if !reserve_dispatch(root, &store, "PR-7", &mut attempt)? {
            return Ok(false);
        }
        anyhow::ensure!(
            Command::new(program).arg(marker).status()?.success(),
            "fixture dispatcher failed"
        );
        Ok(true)
    }

    // trace:TASK-3196 | ai:codex
    pub(crate) fn assert_failed_admission(root: &Path, head: &str) {
        let store = Store::open(root, "github:github.com/fixture/review").unwrap();
        let state = store.load().unwrap();
        let attempt = state.entries["PR-7"].attempts.last().unwrap();
        assert_eq!(attempt.head, head);
        assert_eq!(attempt.phase, Phase::Failed);
    }
}

// Test entry point into the same exit/cancellation functions used by children.
// trace:TASK-3196 | ai:codex
#[cfg(test)]
pub(crate) fn fixture_admission_exit(
    root: &Path,
    ambiguous: bool,
    unclaimed: bool,
    before_exit: impl FnOnce(),
) -> Result<()> {
    let store = Store::open(root, "github:github.com/fixture/review")?;
    let change = forge::Change {
        id: 7,
        head: "a".repeat(40),
        base: "main".into(),
        author: "fixture-author".into(),
        gitlab: false,
        overlap: Some(false),
        behind: Some(0),
        head_branch: None,
    };
    let Action::Dispatch(mut attempt) = select(&store, &change, None, 2)? else {
        anyhow::bail!("fixture not selected");
    };
    anyhow::ensure!(
        reserve_dispatch(root, &store, "PR-7", &mut attempt)?,
        "fixture not admitted"
    );
    if unclaimed {
        before_exit();
        return mark_unclaimed_exit(&store, "PR-7", &attempt.token, "fixture pre-claim exit");
    }
    execute_reservation(
        &store,
        "PR-7",
        &attempt.token,
        |_| {
            admission::transition(
                root,
                "PR-7",
                &attempt.head,
                &attempt.token,
                admission::State::Running,
                "fixture exact-head preflight".into(),
            )
        },
        |_| {
            let mut child = Command::new("sleep").arg("0.05").spawn()?;
            let identity = Identity::child(&child)?;
            store.record_worker("PR-7", &attempt.token, Some(identity))?;
            child.wait()?;
            if ambiguous {
                // Reproduce a lost queue-work receipt after it began reviewer
                // spawn: the dispatcher cannot attest that descendant stopped.
                store.transaction("PR-7", |state| {
                    attempt_mut(state, "PR-7", &attempt.token)
                        .unwrap()
                        .reviewer_spawning = true;
                    Ok(())
                })?;
            }
            before_exit();
            finish_admission(
                root,
                &store,
                "PR-7",
                &attempt.token,
                &attempt.head,
                "fixture observed exit",
            )
        },
    )
}

#[cfg(test)]
// trace:TASK-3196 | ai:codex
pub(crate) fn fixture_admission_spawn_failure(
    root: &Path,
    before_cancel: impl FnOnce(),
) -> Result<()> {
    let store = Store::open(root, "github:github.com/fixture/review")?;
    let change = forge::Change {
        id: 7,
        head: "a".repeat(40),
        base: "main".into(),
        author: "fixture-author".into(),
        gitlab: false,
        overlap: Some(false),
        behind: Some(0),
        head_branch: None,
    };
    let Action::Dispatch(mut attempt) = select(&store, &change, None, 2)? else {
        anyhow::bail!("fixture not selected");
    };
    anyhow::ensure!(
        reserve_dispatch(root, &store, "PR-7", &mut attempt)?,
        "fixture not admitted"
    );
    before_cancel();
    cancel_spawn_failure(root, &store, "PR-7", &attempt, "fixture executable missing")?;
    let state = store.load()?;
    anyhow::ensure!(
        state.entries["PR-7"].attempts[0].phase == Phase::Failed,
        "local capacity leaked after hub cancellation error"
    );
    Ok(())
}

// Automatic trains deliberately require an approval recorded on this exact
// head. The train's ordinary member gate still checks all publication gates.
// trace:STORY-2691 | ai:codex
pub(crate) fn advisor_train_eligible(
    root: &Path,
    pr: u64,
) -> std::result::Result<String, crate::pr_train::MemberGateError> {
    let unavailable =
        |e: anyhow::Error| crate::pr_train::MemberGateError::Unavailable(format!("{e:#}"));
    let repo = forge::Repository::load(root).map_err(unavailable)?;
    let change = repo.change(root, pr).map_err(unavailable)?;
    train::eligible(&change, &ready::Live { root, repo: &repo })?;
    Ok(change.head)
}
