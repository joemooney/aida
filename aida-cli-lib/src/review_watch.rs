//! Review discovery, dispatch, head-change supersede, and the report-only
//! ship-readiness verdict with its once-only advisor wake.
// trace:TASK-1945 trace:TASK-1993 | ai:codex
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
// The author's findings mail and the round-cap wake. trace:TASK-1945 | ai:claude
#[path = "review_watch_notice.rs"]
mod notice;

#[derive(Args, Debug)]
pub struct ReviewWatchArgs {
    /// Discover and dispatch once, then leave admitted reviews running.
    #[arg(long)]
    pub once: bool,
    /// Seconds between discovery passes (default 10).
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=3600), hide_default_value = true)]
    pub interval: u64,
    /// Maximum watcher-launched reviews across instances: one or two (default two).
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=2), hide_default_value = true)]
    pub max_concurrent: u8,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    pid: u32,
    start: String,
    boot: String,
}
impl Identity {
    fn current() -> Result<Self> {
        Self::of(std::process::id())
    }
    fn of(pid: u32) -> Result<Self> {
        Ok(Self {
            pid,
            start: aida_core::liveness::process_start_identity_exact(pid)
                .context("cannot bind review launch to its kernel process identity")?,
            boot: boot_id()?,
        })
    }
    /// The same kernel process is still running (not reused, not a zombie).
    // trace:TASK-1993 | ai:claude
    fn alive(&self) -> bool {
        let zombie = std::fs::read_to_string(format!("/proc/{}/stat", self.pid))
            .ok()
            .and_then(|stat| {
                let rest = &stat[stat.rfind(')')? + 1..];
                rest.split_whitespace().next().map(|state| state == "Z")
            })
            .unwrap_or(true);
        !zombie
            && boot_id().is_ok_and(|boot| boot == self.boot)
            && aida_core::liveness::process_start_identity_exact(self.pid).as_deref()
                == Some(self.start.as_str())
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
    historical_keys: BTreeSet<crate::review_verdict::RecordingKey>,
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
}
impl Entry {
    fn is_empty(&self) -> bool {
        self.attempts.is_empty()
            && self.notices.is_empty()
            && self.wakes.is_empty()
            && self.findings.is_empty()
            && self.cap.is_none()
            && self.readiness.is_none()
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
                attempt.phase = Phase::Exited;
                attempt.diagnostic = Some(diagnostic);
            }
        }
    }
    /// Blocking review identities this watcher has observed, kept per change
    /// outside state.json so pruning (a draft toggle, close and reopen) can
    /// never forget a consumed round.
    // trace:TASK-1993 | ai:claude
    fn retained(&self, key: &str) -> Result<BTreeSet<crate::review_verdict::RecordingKey>> {
        let dir = Directory::open(&self.dir.path().join("history"))?;
        let Some(bytes) = dir.read(&format!("{key}.json"))? else {
            return Ok(BTreeSet::new());
        };
        let keys: BTreeSet<crate::review_verdict::RecordingKey> =
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
        keys: impl Iterator<Item = crate::review_verdict::RecordingKey>,
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
            attempt.phase = phase;
            attempt.diagnostic = diagnostic.map(|d| bounded_diagnostic(&d));
            Ok(attempt.clone())
        })
    }
}

// Durable rounds outside this watcher count too. Deduplicate current/archive
// copies using the verdict writer's recording identity, never a separate policy.
// trace:TASK-1993 | ai:codex
#[derive(Default)]
struct History {
    keys: BTreeSet<crate::review_verdict::RecordingKey>,
    // The verdict files each recording was read from, relative to
    // `.aida/review-verdicts/`, for the findings mail. trace:TASK-1945 | ai:claude
    files: BTreeMap<crate::review_verdict::RecordingKey, Vec<String>>,
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
            let key = crate::review_verdict::recording_key(object);
            anyhow::ensure!(
                valid_recording_key(&key),
                "unidentifiable historical review requires advisor reconciliation"
            );
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
                .is_some_and(|sha| crate::review_verdict::same_reviewed_sha(sha, head))
        })
    }
    // The ordinary round (queue_cmd::review_round_from_comments) counts the
    // durable findings blocks, which the hand-off writes exactly for
    // verdicts that block done. Approvals never consume a round, so a head
    // approved and then merged with main still gets its re-review at round 1.
    // trace:TASK-1993 | ai:claude
    fn blocking_keys(&self) -> impl Iterator<Item = crate::review_verdict::RecordingKey> + '_ {
        self.keys
            .iter()
            .filter(|key| crate::review_verdict::VerdictKind::parse(&key.2).blocks_done())
            .cloned()
    }
    fn blocking(&self) -> usize {
        self.blocking_keys().count()
    }
}

// trace:TASK-1993 | ai:codex
fn valid_recording_key(key: &crate::review_verdict::RecordingKey) -> bool {
    key.0.as_deref().is_some_and(|sha| {
        (7..=64).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())
    }) && !key.1.is_empty()
        && crate::review_verdict::VerdictKind::parse(&key.2)
            != crate::review_verdict::VerdictKind::Unknown
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
            && (1..=2).contains(&state.max_concurrent)
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
                    Phase::Failed | Phase::Exited => true,
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
    let key = change.key();
    let overlap = change
        .overlap
        .context("base-branch overlap evidence is unknown")?;
    store.transaction(&key, |state| {
        store.reconcile_lost_dispatchers(state);
        let total = state.entries.values().flat_map(|e| &e.attempts).filter(|a| active(a)).count();
        if total == 0 { state.max_concurrent = max; }
        anyhow::ensure!(state.max_concurrent == max, "another watcher uses a different concurrency limit; use --max-concurrent {}", state.max_concurrent);
        let repository = state.repository.clone();
        let holders = capacity_holders(state);
        let entry = state.entries.entry(key.clone()).or_default();
        // A notice is about one head; once the change moved on it is moot,
        // and keeping one per stale head grew to the 256 cap and then failed
        // every transaction for the change. trace:TASK-1955 | ai:claude (TASK-2064)
        entry.notices.retain(|head, _| *head == change.head);
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
            if notice.sent { return Ok(Action::Skip("stale overlapping head already notified".into())); }
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
        if let Some(attempt) = entry.attempts.iter().find(|a| a.head == change.head && a.phase == Phase::Exited) {
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
        if history.contains(&change.head) { return Ok(Action::Skip("head already has a recorded review".into())); }
        if history.blocking() >= 2 { return Ok(Action::Skip("two-round review cap reached; route further work to the advisor".into())); }
        if total >= usize::from(max) { return Ok(Action::Skip(format!("global reviewer capacity ({max}) is held by {holders}; a running review frees its slot when it exits, and an ambiguous one (dispatcher lost) stays reserved until an operator confirms it ended: check its log in .aida/review-watch/logs/ and `aida session leases --all`"))); }
        let attempt = Attempt { head: change.head.clone(), token: uuid::Uuid::new_v4().to_string(), vendor, phase: Phase::Reserved, identity: None, diagnostic: None, dispatcher: None, spawning: false, worker: None, reviewer_spawning: false, reviewer: None, pgid: None, supersede: None };
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
    let key = change.key();
    let head = change.head.as_str();
    let recorded = store.transaction(&key, |state| {
        let Some(entry) = state.entries.get(&key) else {
            return Ok(None);
        };
        if entry.notices.get(head).is_some_and(|n| n.sent) {
            return Ok(Some("stale overlapping head already notified".to_string()));
        }
        Ok(entry
            .attempts
            .iter()
            .find(|a| {
                a.head == head
                    && matches!(a.phase, Phase::Reserved | Phase::Running | Phase::Completed)
            })
            .map(|a| format!("head already reserved ({:?})", a.phase)))
    })?;
    if recorded.is_some() {
        return Ok(recorded);
    }
    let history = store.history(&key)?;
    if history.contains(head) {
        return Ok(Some("head already has a recorded review".into()));
    }
    if history.blocking() >= 2 {
        return Ok(Some(
            "two-round review cap reached; route further work to the advisor".into(),
        ));
    }
    Ok(None)
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
        timestamp: chrono::Utc::now().timestamp(), in_reply_to: None,
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

fn child(root: &Path, repo: &forge::Repository, reservation: &str) -> Result<()> {
    child_with_program(root, repo, reservation, &crate::aida_exe_path())
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
            let mut current = repo.change(root, id)?;
            repo.probe(root, &mut current)?;
            anyhow::ensure!(
                current.head == attempt.head
                    && current.key() == key
                    && current.overlap == Some(false),
                "head changed before review dispatch"
            );
            round.set(store.round_for(key, attempt)?);
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
            match Identity::of(child.id()) {
                Ok(worker) => {
                    if let Err(error) = retry_busy(|| store.record_worker(key, token, Some(worker.clone()))) {
                        eprintln!("{key}: could not record the queue-work child; the attempt stays held as ambiguous: {error:#}");
                    }
                }
                Err(error) => eprintln!("{key}: queue-work child identity unavailable; the attempt stays held as ambiguous: {error:#}"),
            }
            let status = child.wait()?;
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
    repository.probe(root, &mut current)?;
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
}

/// Run the reviewer process queue work launches. Inside a watch launch, mark
/// it spawning, spawn it, and record its identity before waiting: it carries
/// the review and can outlive queue work and the dispatcher. Outside one,
/// this is the ordinary blocking run.
// trace:TASK-1993 | ai:claude
pub(crate) fn reviewer_status(cmd: &mut Command) -> std::io::Result<std::process::ExitStatus> {
    use crate::process_retry::RetryEtxtbsy;
    let launch = LAUNCH.with(|launch| launch.borrow().clone());
    let Some((root, identity, key, head)) = launch else {
        return cmd.status_retrying_etxtbsy();
    };
    let store = Store::open(&root, &identity).map_err(std::io::Error::other)?;
    retry_busy(|| store.record_reviewer(&key, &head, None)).map_err(std::io::Error::other)?;
    let mut child = cmd.spawn_retrying_etxtbsy()?;
    match Identity::of(child.id()) {
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

pub(crate) fn run(args: &ReviewWatchArgs) -> Result<()> {
    let root = forge::main_root(&std::env::current_dir()?)?;
    let repo = forge::Repository::load(&root)?;
    crate::seat_authority::validate_child_delegation(&root, "reviewer")?;
    if let Some(reservation) = args.launch_reservation.as_deref() {
        return child(&root, &repo, reservation);
    }
    let store = Store::open(&root, &repo.identity)?;
    // trace:TASK-1955 | ai:claude (TASK-1-412)
    if let Some(key) = args.release.as_deref() {
        let released = store.release(key)?;
        println!("{key}: released {released} recorded review attempt(s); its current head can be reviewed again.");
        return Ok(());
    }
    // trace:TASK-1955 | ai:claude (TASK-1-413)
    let swept = forge::sweep_probe_refs(&root);
    if swept > 0 {
        eprintln!("review-watch: removed {swept} leaked probe ref(s)");
    }
    let mut children = Vec::<Dispatcher>::new();
    loop {
        children.retain_mut(|d| match d.child.try_wait() {
            Ok(Some(status)) => {
                if let Err(error) =
                    mark_unclaimed_exit(&store, &d.key, &d.token, &status.to_string())
                {
                    eprintln!("{}: could not record dispatcher exit: {error:#}", d.key);
                }
                false
            }
            _ => true,
        });
        let changes = match repo.discover(&root) {
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
                Err(error) if stops_watcher(&error, false) => return Err(error),
                Err(error) => eprintln!("{key}: refused: {error:#}"),
            }
        }
        for change in changes {
            let key = change.key();
            match watch_change(&root, &repo, &store, args, &mut children, change) {
                Ok(()) => {}
                Err(error) if is_busy(&error) => eprintln!("{key}: watcher admission is busy"),
                Err(error) if stops_watcher(&error, args.once) => {
                    return Err(error.context(format!("{key}: review watch refused")))
                }
                Err(error) => eprintln!("{key}: refused: {error:#}"),
            }
        }
        if args.once {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(args.interval));
    }
}

/// Only whole-state failure stops a continuous watcher; one change's refusal
/// must not stall every other change. `--once` reports any refusal.
// trace:TASK-1993 | ai:claude
fn stops_watcher(error: &anyhow::Error, once: bool) -> bool {
    once || error.downcast_ref::<GlobalState>().is_some()
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
    store.transaction(key, |state| {
        if let Some(a) = state
            .entries
            .get_mut(key)
            .and_then(|e| e.attempts.iter_mut().find(|a| a.token == token))
            .filter(|a| a.identity.is_none())
        {
            let diagnostic = bounded_diagnostic(&format!(
                "dispatcher exited {status} before claiming its launch receipt: {}",
                tail.as_deref().unwrap_or("no log output")
            ));
            match a.phase {
                Phase::Reserved => {
                    a.phase = Phase::Exited;
                    a.diagnostic = Some(diagnostic);
                }
                // Superseded before it claimed: an unclaimed dispatcher can
                // start nothing, and this one is reaped, so nothing runs.
                // trace:TASK-1945 | ai:claude (TASK-2150)
                Phase::Superseding => {
                    a.phase = Phase::Superseded;
                    a.diagnostic = Some(diagnostic);
                }
                _ => {}
            }
        }
        Ok(())
    })
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
    mut change: forge::Change,
) -> Result<()> {
    let key = change.key();
    // An obsolete head is superseded before anything else, whether or not
    // the new head is stale or overlapping. trace:TASK-1945 | ai:claude
    if let Some(reason) = supersede(store, &change, || Ok(repo.change(root, change.id)?.head))? {
        eprintln!("{key}: {reason}");
        return Ok(());
    }
    // A reviewed head is reported on, and needs no base probe to be skipped.
    // trace:TASK-1945 | ai:claude
    let live = ready::Live { root, repo };
    // trace:TASK-1955 | ai:claude (TASK-2064, TASK-1-411)
    if let Some(reason) = probe_unneeded(store, &change)? {
        eprintln!("{key}: {reason}");
        return report(root, store, &change, &live);
    }
    repo.probe(root, &mut change)
        .context("base evidence refused")?;
    match select(store, &change, args.vendor.clone(), args.max_concurrent)? {
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
        Action::Dispatch(attempt) => {
            use std::os::unix::process::CommandExt;
            let spawned = (|| {
                let logs = Directory::open(&store.dir.path().join("logs"))?;
                let log = logs.file(&format!("{key}-{}.log", attempt.token), true)?;
                let mut command = watch_command(repo, root, &crate::aida_exe_path());
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
                    let exited = child.try_wait().ok().flatten();
                    let pid = child.id();
                    let identity = Identity::of(pid).ok();
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
                    mark_spawn_failed(store, &key, &attempt.token)?;
                    return Err(error).context("could not spawn review dispatcher");
                }
            }
        }
    }
    Ok(())
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
