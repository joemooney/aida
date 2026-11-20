//! STORY-2254: private, host-owned runtime receipts for detached Codex
//! launches, and the project-common scope admission they implement.
//!
//! One receipt per launch attempt, under
//! `$AIDA_HOME/.aida/launch-attempts/<project-key>/` (a 0700 directory owned
//! by this user, outside every model-writable project root), as 0600 regular
//! files replaced atomically, beside a permanent `admission.lock` sidecar.
//! A receipt holds identifiers, resource identities, the public grant
//! selector, phase, PID/start pairs, the log path and terminal evidence:
//! never a grant key, a registry/handle token, a prompt or an environment.
//!
//! Receipts are recovery EVIDENCE, never authority: nothing here mints, binds
//! or widens a grant. A corrupt, foreign, wrong-version, symlinked or
//! unreadable receipt is UNKNOWN, and unknown never frees a scope.
// trace:STORY-2254 | ai:claude

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The receipt schema version this build reads and writes.
pub(crate) const RECEIPT_VERSION: u32 = 1;
/// A receipt larger than this is not one of ours.
const RECEIPT_MAX_BYTES: u64 = 64 * 1024;
/// How long a launch waits for the admission lock before it reports the
/// state as uncertain rather than guessing.
pub(crate) const ADMISSION_LOCK_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);
const LOCK_FILE: &str = "admission.lock";
const ATTEMPTS_DIR: &str = "attempts";
/// Retired unknown receipts are kept here as evidence, outside admission.
const RETIRED_DIR: &str = "retired";
/// Terminal receipts older than this are pruned at the next admission.
pub(crate) const TERMINAL_RETENTION: chrono::Duration = chrono::Duration::days(14);
/// At most this many terminal receipts are kept, newest first.
pub(crate) const TERMINAL_KEEP_MAX: usize = 64;

/// A process named by pid AND kernel start identity (no PID reuse).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProcId {
    pub pid: u32,
    pub start: String,
}

impl ProcId {
    /// This process, when its start identity reads.
    pub(crate) fn current() -> Option<Self> {
        let pid = std::process::id();
        Some(Self {
            pid,
            start: aida_core::liveness::process_start_identity(pid)?,
        })
    }
    /// Whether the process is alive with exactly this start identity.
    pub(crate) fn is_live(&self) -> bool {
        aida_core::liveness::process_start_identity(self.pid).as_deref()
            == Some(self.start.as_str())
    }
    /// Fail-closed liveness: `Gone` only when `/proc/<pid>` is absent or now
    /// names a different start identity (the pid was reused). A recorded
    /// identity without a start, or a present process whose start cannot be
    /// read, is `Unknown`, never gone.
    // trace:STORY-2254 | ai:claude
    pub(crate) fn state(&self) -> ProcState {
        if self.start.is_empty() || self.pid == 0 {
            return ProcState::Unknown(format!("pid {} has no recorded start identity", self.pid));
        }
        match std::fs::symlink_metadata(format!("/proc/{}", self.pid)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ProcState::Gone,
            Err(e) => return ProcState::Unknown(format!("/proc/{} is unreadable: {e}", self.pid)),
            Ok(_) => {}
        }
        match aida_core::liveness::process_start_identity(self.pid) {
            Some(start) if start == self.start => ProcState::Live,
            Some(_) => ProcState::Gone,
            None => {
                if std::fs::symlink_metadata(format!("/proc/{}", self.pid)).is_err() {
                    ProcState::Gone
                } else {
                    ProcState::Unknown(format!("pid {}'s start identity is unreadable", self.pid))
                }
            }
        }
    }
}

/// Fail-closed process state for recovery decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProcState {
    Live,
    Gone,
    Unknown(String),
}

/// What a fresh scan of every process found for one attempt.
// trace:STORY-2254 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Occupancy {
    /// Every process was inspected and none belongs to the attempt.
    Clear,
    /// A live process is in the worktree, the attempt's session/group, or
    /// names the attempt on its command line.
    Occupied(String),
    /// Some process of this user could not be inspected, or `/proc` itself
    /// could not be read: nothing may be concluded.
    Unknown(String),
}

/// What [`scan_occupancy`] looks for.
#[derive(Debug, Default)]
pub(crate) struct OccupancyProbe<'a> {
    /// Every process whose cwd is within this tree.
    pub worktree: Option<&'a Path>,
    /// Every process in this process group (the worker leads its own).
    pub group: Option<u32>,
    /// Every process in this session (the supervisor leads its own).
    pub session: Option<u32>,
    /// Every process whose command line names this attempt id.
    pub attempt: Option<&'a str>,
    /// Kernel start tick of the attempt's FIRST recorded process. A process
    /// of this user whose details are unreadable (a PAM helper, a process
    /// holding capabilities) and that started strictly before it cannot be
    /// the attempt's work; without this bound, unreadable means unknown.
    pub not_before: Option<u64>,
}

impl ProcId {
    /// The Linux start tick of a recorded identity, when it is one.
    pub(crate) fn start_tick(&self) -> Option<u64> {
        self.start.strip_prefix("linux-starttime:")?.parse().ok()
    }
}

/// A FRESH scan (never cached) of every process. Processes of another user
/// whose details are unreadable cannot be this user's worker and are
/// skipped; any process of THIS user that vanishes is skipped, but one that
/// is present and unreadable makes the result `Unknown`. A process whose
/// cwd was deleted but lay inside the tree still counts as occupying it.
// trace:STORY-2254 | ai:claude
pub(crate) fn scan_occupancy(
    probe: &OccupancyProbe<'_>,
    exclude: &std::collections::HashSet<u32>,
) -> Occupancy {
    scan_occupancy_in(Path::new("/proc"), probe, exclude)
}

pub(crate) fn scan_occupancy_in(
    proc_root: &Path,
    probe: &OccupancyProbe<'_>,
    exclude: &std::collections::HashSet<u32>,
) -> Occupancy {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid only reads this process's credentials.
    #[cfg(unix)]
    let me = unsafe { libc::geteuid() };
    scan_occupancy_in_with_owner(proc_root, probe, exclude, |meta| {
        #[cfg(unix)]
        {
            meta.uid() == me
        }
        #[cfg(not(unix))]
        {
            let _ = meta;
            true
        }
    })
}

// Keep ownership injectable so foreign-uid regressions need neither root nor
// changes to the test process's credentials. Production always uses geteuid.
// trace:BUG-2511 | ai:codex
pub(crate) fn scan_occupancy_in_with_owner(
    proc_root: &Path,
    probe: &OccupancyProbe<'_>,
    exclude: &std::collections::HashSet<u32>,
    is_ours: impl Fn(&std::fs::Metadata) -> bool,
) -> Occupancy {
    let tree = probe
        .worktree
        .filter(|t| !t.as_os_str().is_empty())
        .map(|t| t.canonicalize().unwrap_or_else(|_| t.to_path_buf()));
    let entries = match std::fs::read_dir(proc_root) {
        Ok(entries) => entries,
        Err(e) => return Occupancy::Unknown(format!("{} is unreadable: {e}", proc_root.display())),
    };
    let gone = |e: &std::io::Error| {
        e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
    };
    let mut seen = 0usize;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Occupancy::Unknown(format!("listing processes failed: {e}")),
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        seen += 1;
        if exclude.contains(&pid) {
            continue;
        }
        let base = entry.path();
        let meta = match std::fs::metadata(&base) {
            Ok(meta) => meta,
            Err(e) if gone(&e) => continue,
            Err(e) => return Occupancy::Unknown(format!("process {pid} is unreadable: {e}")),
        };
        let ours = is_ours(&meta);
        let stat = match std::fs::read_to_string(base.join("stat")) {
            Ok(stat) => stat,
            Err(e) if gone(&e) => continue,
            Err(e) if !ours => {
                let _ = e;
                continue;
            }
            Err(e) => {
                return Occupancy::Unknown(format!("process {pid}'s stat is unreadable: {e}"))
            }
        };
        // Fields after the `)` closing comm: state ppid pgrp session ...
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest.split_whitespace().collect())
            .unwrap_or_default();
        let started = fields.get(19).and_then(|f| f.parse::<u64>().ok());
        // An unreadable detail of this user's process is decisive only for a
        // process that could be the attempt's: one started before the
        // attempt's first process cannot be.
        let predates = matches!((probe.not_before, started), (Some(bound), Some(at)) if at < bound);
        let (Some(pgrp), Some(sid)) = (
            fields.get(2).and_then(|f| f.parse::<u32>().ok()),
            fields.get(3).and_then(|f| f.parse::<u32>().ok()),
        ) else {
            if ours {
                return Occupancy::Unknown(format!("process {pid}'s stat is malformed"));
            }
            continue;
        };
        if fields.first() == Some(&"Z") && probe.group != Some(pgrp) && probe.session != Some(sid) {
            // An unrelated zombie holds no cwd and runs nothing.
            continue;
        }
        if probe.group == Some(pgrp) {
            return Occupancy::Occupied(format!(
                "pid {pid} is in the worker's process group {pgrp}"
            ));
        }
        if probe.session == Some(sid) {
            return Occupancy::Occupied(format!("pid {pid} is in the supervisor's session {sid}"));
        }
        if let Some(attempt) = probe.attempt {
            match std::fs::read(base.join("cmdline")) {
                Ok(raw) => {
                    if String::from_utf8_lossy(&raw).contains(attempt) {
                        return Occupancy::Occupied(format!("pid {pid} names attempt {attempt}"));
                    }
                }
                Err(e) if gone(&e) || !ours || predates => {}
                Err(e) => {
                    return Occupancy::Unknown(format!(
                        "process {pid}'s command line is unreadable: {e}"
                    ))
                }
            }
        }
        if let Some(tree) = &tree {
            match std::fs::read_link(base.join("cwd")) {
                Ok(cwd) => {
                    let raw = cwd.to_string_lossy();
                    let cwd = PathBuf::from(raw.strip_suffix(" (deleted)").unwrap_or(&raw));
                    if cwd == *tree || cwd.starts_with(tree) {
                        return Occupancy::Occupied(format!(
                            "pid {pid} is running in {}",
                            tree.display()
                        ));
                    }
                }
                Err(e) if gone(&e) || !ours || predates => {}
                Err(e) => {
                    return Occupancy::Unknown(format!("process {pid}'s cwd is unreadable: {e}"))
                }
            }
        }
    }
    if seen == 0 {
        return Occupancy::Unknown(format!("{} lists no processes", proc_root.display()));
    }
    Occupancy::Clear
}

/// Who may perform destructive cleanup for an attempt. Exactly one owner;
/// the transfer from dispatcher to supervisor is a compare-update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub(crate) enum Owner {
    Dispatcher(ProcId),
    Supervisor(ProcId),
    /// Nobody: terminal, or released by an explicit recovery.
    Released,
}

/// The attempt's lifecycle. Internal schema, not a monitor contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Phase {
    /// The dispatcher owns setup; no worker has executed.
    Prepared,
    /// Ownership moved to the supervisor; the worker is still gated.
    SupervisorOwned,
    /// Grant binding, lease, authorship and registry all match the worker.
    Registered,
    /// The vendor executable was observed running at the worker PID.
    AcceptedRunning,
    /// The worker exited (or never ran past an observed terminal outcome).
    Exited,
    /// Refused before any model work could start; resources rolled back.
    RefusedBeforeExecution,
    /// State is uncertain; resources are retained for explicit recovery.
    RecoveryRequired,
}

impl Phase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::SupervisorOwned => "supervisor-owned",
            Self::Registered => "registered",
            Self::AcceptedRunning => "accepted-running",
            Self::Exited => "exited",
            Self::RefusedBeforeExecution => "refused-before-execution",
            Self::RecoveryRequired => "recovery-required",
        }
    }
    /// Whether this attempt still holds its scope.
    pub(crate) fn holds_scope(self) -> bool {
        !matches!(self, Self::Exited | Self::RefusedBeforeExecution)
    }
}

/// The worker's terminal evidence. `unknown` is distinct from any exit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalRecord {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    /// Why the outcome is not known (wait failed, process unobservable).
    pub unknown: Option<String>,
}

/// The normalized scope a detached driver owns: project + role + spec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScopeKey {
    pub role: String,
    pub spec: String,
}

impl ScopeKey {
    pub(crate) fn new(role: &str, spec: &str) -> Self {
        Self {
            role: aida_core::team::canonical_role(role.trim()),
            spec: spec.trim().to_ascii_uppercase(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LaunchReceipt {
    pub version: u32,
    pub attempt: String,
    pub project_root: PathBuf,
    pub scope: ScopeKey,
    pub phase: Phase,
    pub owner: Owner,
    /// Bumped by every compare-update; a writer that read an older value
    /// never clobbers a newer one.
    pub generation: u64,
    pub agent_name: String,
    pub lease_id: Option<String>,
    /// Public selector of the attempt's own child grant (never its key).
    pub grant_id: Option<String>,
    pub worktree: PathBuf,
    pub log_path: PathBuf,
    pub context_path: Option<PathBuf>,
    pub supervisor: Option<ProcId>,
    pub worker: Option<ProcId>,
    pub terminal: Option<TerminalRecord>,
    /// The last precise cause or uncertainty, for status output.
    pub reason: Option<String>,
    pub updated_at: String,
}

impl LaunchReceipt {
    pub(crate) fn new(
        project_root: &Path,
        attempt: &str,
        scope: ScopeKey,
        owner: ProcId,
        agent_name: &str,
        worktree: &Path,
        log_path: &Path,
    ) -> Self {
        Self {
            version: RECEIPT_VERSION,
            attempt: attempt.to_string(),
            project_root: project_root.to_path_buf(),
            scope,
            phase: Phase::Prepared,
            owner: Owner::Dispatcher(owner),
            generation: 0,
            agent_name: agent_name.to_string(),
            lease_id: None,
            grant_id: None,
            worktree: worktree.to_path_buf(),
            log_path: log_path.to_path_buf(),
            context_path: None,
            supervisor: None,
            worker: None,
            terminal: None,
            reason: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// A receipt as read from disk. Anything not provably ours is `Unknown`.
#[derive(Debug)]
pub(crate) enum ReceiptRead {
    Known(Box<LaunchReceipt>),
    Unknown(String),
}

/// The runtime directory for `project_root`, created 0700 when absent and
/// refused when it is not a private directory owned by this user.
pub(crate) fn runtime_dir(project_root: &Path) -> Result<PathBuf> {
    let home = crate::gate::kernel_deps::aida_home_dir()
        .context("cannot locate the AIDA home for launch receipts")?;
    let dir = home
        .join(".aida")
        .join("launch-attempts")
        .join(project_key(project_root));
    ensure_private_dir(&dir)?;
    ensure_private_dir(&dir.join(ATTEMPTS_DIR))?;
    Ok(dir)
}

/// Read-only lookup of the runtime directory: never creates it.
pub(crate) fn existing_runtime_dir(project_root: &Path) -> Option<PathBuf> {
    let home = crate::gate::kernel_deps::aida_home_dir()?;
    let dir = home
        .join(".aida")
        .join("launch-attempts")
        .join(project_key(project_root));
    check_private_dir(&dir).ok().map(|_| dir)
}

/// Stable per-project key: a digest of the canonical main checkout path.
pub(crate) fn project_key(project_root: &Path) -> String {
    use sha2::{Digest, Sha256};
    let main = crate::main_worktree_root_from(project_root);
    let canonical = main.canonicalize().unwrap_or(main);
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    format!("{:x}", digest)[..24].to_string()
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    if std::fs::symlink_metadata(dir).is_err() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(dir)?;
    }
    check_private_dir(dir)
}

fn check_private_dir(dir: &Path) -> Result<()> {
    let meta =
        std::fs::symlink_metadata(dir).with_context(|| format!("reading {}", dir.display()))?;
    anyhow::ensure!(
        meta.file_type().is_dir(),
        "launch receipt directory {} is not a plain directory",
        dir.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid only reads this process's credentials.
        let me = unsafe { libc::geteuid() };
        anyhow::ensure!(
            meta.uid() == me && meta.mode() & 0o077 == 0,
            "launch receipt directory {} must be owned by this user with mode 0700",
            dir.display()
        );
    }
    Ok(())
}

fn receipt_path(dir: &Path, attempt: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        uuid::Uuid::parse_str(attempt).is_ok(),
        "launch attempt id must be a UUID"
    );
    Ok(dir.join(ATTEMPTS_DIR).join(format!("{attempt}.json")))
}

/// Run `f` under the project's permanent admission lock, waiting at most
/// [`ADMISSION_LOCK_BUDGET`]. Callers hold NO other lock when they take it
/// (admission is first in the lock order), and take at most one resource
/// lock at a time inside it, never across IPC, exec or a model wait.
pub(crate) fn with_admission_lock<T>(dir: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    use fs2::FileExt;
    let path = dir.join(LOCK_FILE);
    // flock needs no write access: a read-only view of the same lock file
    // (a contained seat's home) still serialises with the host.
    let lock = match open_private(&path, false) {
        Ok(lock) => lock,
        Err(error) => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            options.open(&path).map_err(|_| error)?
        }
    };
    let started = std::time::Instant::now();
    loop {
        match lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(e) if aida_core::file_lock::is_lock_contended(&e) => {
                anyhow::ensure!(
                    started.elapsed() < ADMISSION_LOCK_BUDGET,
                    "launch admission lock {} stayed held for {}s; the launch state is uncertain, nothing was changed",
                    path.display(),
                    ADMISSION_LOCK_BUDGET.as_secs()
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e).with_context(|| format!("locking {}", path.display())),
        }
    }
    let out = f();
    let _ = FileExt::unlock(&lock);
    out
}

fn open_private(path: &Path, truncate: bool) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// Checked read: regular file, bounded size, our version, our project.
pub(crate) fn read_receipt(dir: &Path, project_root: &Path, attempt: &str) -> ReceiptRead {
    let path = match receipt_path(dir, attempt) {
        Ok(path) => path,
        Err(e) => return ReceiptRead::Unknown(format!("{e:#}")),
    };
    read_receipt_at(&path, project_root, &project_key(project_root))
}

/// `key` is `project_key(project_root)`, computed once by the caller: the
/// key resolves the main checkout, which a listing must not repeat per file.
fn read_receipt_at(path: &Path, project_root: &Path, key: &str) -> ReceiptRead {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) => return ReceiptRead::Unknown(format!("receipt unreadable: {e}")),
    };
    if !meta.file_type().is_file() || meta.len() > RECEIPT_MAX_BYTES {
        return ReceiptRead::Unknown("receipt is not a bounded regular file".into());
    }
    let body = match std::fs::read(path) {
        Ok(body) => body,
        Err(e) => return ReceiptRead::Unknown(format!("receipt unreadable: {e}")),
    };
    let receipt: LaunchReceipt = match serde_json::from_slice(&body) {
        Ok(receipt) => receipt,
        Err(e) => return ReceiptRead::Unknown(format!("receipt malformed: {e}")),
    };
    if receipt.version != RECEIPT_VERSION {
        return ReceiptRead::Unknown(format!(
            "receipt version {} is not {RECEIPT_VERSION}",
            receipt.version
        ));
    }
    // trace:STORY-2254 | ai:claude — an identical root has the same key.
    if receipt.project_root != project_root && project_key(&receipt.project_root) != key {
        return ReceiptRead::Unknown("receipt belongs to another project".into());
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if receipt.attempt != stem {
        return ReceiptRead::Unknown("receipt attempt id does not match its file".into());
    }
    ReceiptRead::Known(Box::new(receipt))
}

fn write_receipt(dir: &Path, receipt: &LaunchReceipt) -> Result<()> {
    let path = receipt_path(dir, &receipt.attempt)?;
    let tmp =
        dir.join(ATTEMPTS_DIR)
            .join(format!(".{}.{}.tmp", receipt.attempt, uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(receipt)?;
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Every receipt file in `dir`, read checked. Unparseable names are listed
/// as unknown too: they may belong to any scope.
pub(crate) fn list_receipts(dir: &Path, project_root: &Path) -> Vec<(String, ReceiptRead)> {
    let Ok(entries) = std::fs::read_dir(dir.join(ATTEMPTS_DIR)) else {
        return Vec::new();
    };
    let key = project_key(project_root);
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let attempt = name.strip_suffix(".json").unwrap_or(&name).to_string();
        let read = if name.ends_with(".json") && uuid::Uuid::parse_str(&attempt).is_ok() {
            read_receipt_at(&entry.path(), project_root, &key)
        } else {
            ReceiptRead::Unknown(format!("unexpected file {name}"))
        };
        out.push((attempt, read));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Why `scope` is not free: an attempt that still holds it, or an unknown
/// receipt that might. `None` when no receipt stands in the way.
pub(crate) fn scope_blocker(dir: &Path, project_root: &Path, scope: &ScopeKey) -> Option<String> {
    scope_blocker_except(dir, project_root, scope, None)
}

/// [`scope_blocker`] that ignores attempt `except` (the caller's own).
fn scope_blocker_except(
    dir: &Path,
    project_root: &Path,
    scope: &ScopeKey,
    except: Option<&str>,
) -> Option<String> {
    for (attempt, read) in list_receipts(dir, project_root) {
        if except == Some(attempt.as_str()) {
            continue;
        }
        match read {
            ReceiptRead::Known(receipt) => {
                if receipt.scope == *scope && receipt.phase.holds_scope() {
                    return Some(format!(
                        "detached launch attempt {attempt} still owns {} {} ({}); inspect `aida agent launch-status {attempt}`",
                        scope.role,
                        scope.spec,
                        receipt.phase.as_str()
                    ));
                }
            }
            ReceiptRead::Unknown(why) => {
                // trace:STORY-2254 | ai:claude — an unknown receipt blocks the
                // scope it names; one whose scope cannot be read at all blocks
                // every scope only while something may still run for it.
                let blocks = match unknown_receipt_scope(dir, &attempt) {
                    Some(owned) => owned == *scope,
                    None => !unknown_attempt_is_dead(dir, &attempt),
                };
                if blocks {
                    return Some(format!(
                        "detached launch attempt {attempt} has an unreadable receipt ({why}), so it may still own this scope; inspect `aida agent launch-status {attempt}`, and once its processes are gone retire it with `aida agent recover-launch {attempt} --retire-unknown`"
                    ));
                }
            }
        }
    }
    None
}

/// The scope an unreadable receipt names, read leniently (any JSON object
/// with a `scope.role` and `scope.spec`). Evidence only: it narrows which
/// scope the receipt blocks, it never frees one.
// trace:STORY-2254 | ai:claude
pub(crate) fn unknown_receipt_scope(dir: &Path, attempt: &str) -> Option<ScopeKey> {
    let path = receipt_path(dir, attempt).ok()?;
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.file_type().is_file() || meta.len() > RECEIPT_MAX_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let scope = value.get("scope")?;
    let role = scope.get("role")?.as_str()?;
    let spec = scope.get("spec")?.as_str()?;
    (!role.trim().is_empty() && !spec.trim().is_empty()).then(|| ScopeKey::new(role, spec))
}

/// For a receipt with no readable identities: the boot tick of the
/// attempt's private log's creation (made right after admission, before its
/// supervisor and worker existed), less a 2s margin. `None` when the birth
/// time or the boot time cannot be read.
// trace:STORY-2254 | ai:claude
fn log_birth_tick(dir: &Path, attempt: &str) -> Option<u64> {
    let born = std::fs::symlink_metadata(dir.join("logs").join(format!("{attempt}.log")))
        .ok()?
        .created()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())?;
    // SAFETY: sysconf only reads a system constant.
    let hz = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok()?;
    Some(born.checked_sub(boot)?.saturating_sub(2) * hz)
}

/// Whether a FRESH scan proves nothing runs for an unknown attempt: no
/// process names it, and none occupies the worker group, supervisor session
/// or worktree its receipt names (read leniently). Unreadable means NO.
// trace:STORY-2254 | ai:claude
pub(crate) fn unknown_attempt_is_dead(dir: &Path, attempt: &str) -> bool {
    let lenient = receipt_path(dir, attempt)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    let field_pid = |name: &str| {
        lenient
            .as_ref()
            .and_then(|v| v.get(name)?.get("pid")?.as_u64())
            .and_then(|p| u32::try_from(p).ok())
    };
    let worktree = lenient
        .as_ref()
        .and_then(|v| v.get("worktree")?.as_str().map(PathBuf::from));
    let tick = |v: Option<&serde_json::Value>| {
        v?.get("start")?
            .as_str()?
            .strip_prefix("linux-starttime:")?
            .parse::<u64>()
            .ok()
    };
    let field = |name: &str| lenient.as_ref().and_then(|v| v.get(name));
    let probe = OccupancyProbe {
        worktree: worktree.as_deref(),
        group: field_pid("worker"),
        session: field_pid("supervisor"),
        attempt: Some(attempt),
        not_before: [
            tick(field("owner")),
            tick(field("supervisor")),
            tick(field("worker")),
        ]
        .into_iter()
        .flatten()
        .min()
        .or_else(|| log_birth_tick(dir, attempt)),
    };
    let mut own = std::collections::HashSet::new();
    own.insert(std::process::id());
    scan_occupancy(&probe, &own) == Occupancy::Clear
}

/// Admit `receipt` (phase Prepared) for its scope, or refuse while any
/// attempt holds the same scope or any receipt is unknown. The check and the
/// write share one admission-lock hold, so two launches cannot both pass.
///
/// `also_free` runs inside the same lock hold, after the receipt check: the
/// caller's own check that no other route already holds the scope (a live
/// lease). Other routes write their lease under this lock too
/// ([`with_scope_entry`]), so admission and lease creation are ordered.
pub(crate) fn admit(
    dir: &Path,
    receipt: &LaunchReceipt,
    also_free: impl FnOnce() -> Result<()>,
) -> Result<()> {
    with_admission_lock(dir, || {
        prune_terminal_receipts(dir, &receipt.project_root, chrono::Utc::now());
        if let Some(blocker) = scope_blocker(dir, &receipt.project_root, &receipt.scope) {
            anyhow::bail!("refusing a second detached driver: {blocker}");
        }
        also_free()?;
        write_receipt(dir, receipt)
    })
}

/// Bounded retention: delete terminal receipts (exited or refused before
/// execution, owner released) older than [`TERMINAL_RETENTION`], and all
/// but the newest [`TERMINAL_KEEP_MAX`] of them. Never touches a receipt
/// that still holds its scope or one that is unknown. Call under the
/// admission lock.
// trace:STORY-2254 | ai:claude
pub(crate) fn prune_terminal_receipts(
    dir: &Path,
    project_root: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    let mut terminal: Vec<(chrono::DateTime<chrono::Utc>, String)> =
        list_receipts(dir, project_root)
            .into_iter()
            .filter_map(|(attempt, read)| match read {
                ReceiptRead::Known(r) if !r.phase.holds_scope() && r.owner == Owner::Released => {
                    let at = chrono::DateTime::parse_from_rfc3339(&r.updated_at)
                        .map(|t| t.with_timezone(&chrono::Utc))
                        .unwrap_or(now);
                    Some((at, attempt))
                }
                _ => None,
            })
            .collect();
    terminal.sort_by(|a, b| b.0.cmp(&a.0));
    let mut removed = 0;
    for (index, (at, attempt)) in terminal.iter().enumerate() {
        if index >= TERMINAL_KEEP_MAX || now - *at > TERMINAL_RETENTION {
            if let Ok(path) = receipt_path(dir, attempt) {
                if std::fs::remove_file(path).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}

/// Retire an UNKNOWN receipt whose attempt is provably dead: move it, as
/// evidence, out of admission into `retired/`. Refuses a readable receipt
/// (recover it instead) and any attempt a fresh scan cannot prove dead.
// trace:STORY-2254 | ai:claude
pub(crate) fn retire_unknown(dir: &Path, project_root: &Path, attempt: &str) -> Result<PathBuf> {
    with_admission_lock(dir, || {
        let why = match read_receipt(dir, project_root, attempt) {
            ReceiptRead::Known(r) => anyhow::bail!(
                "attempt {attempt}'s receipt is readable ({}); use `aida agent recover-launch {attempt}` instead",
                r.phase.as_str()
            ),
            ReceiptRead::Unknown(why) => why,
        };
        let path = receipt_path(dir, attempt)?;
        anyhow::ensure!(
            std::fs::symlink_metadata(&path).is_ok(),
            "attempt {attempt} has no receipt to retire"
        );
        anyhow::ensure!(
            unknown_attempt_is_dead(dir, attempt),
            "attempt {attempt} ({why}) cannot be proven dead: a process may still run for it, or the process table could not be fully read; nothing was changed"
        );
        let retired = dir.join(RETIRED_DIR);
        ensure_private_dir(&retired)?;
        let target = retired.join(format!("{attempt}.{}.json", chrono::Utc::now().timestamp()));
        std::fs::rename(&path, &target).with_context(|| format!("retiring {}", path.display()))?;
        Ok(target)
    })
}

/// STORY-2254 cross-route admission: run `enter` (a route's own lease
/// write for `role` + `spec`) inside the admission lock, after checking that
/// no detached attempt (active, pending or unknown) holds that scope. The
/// detached admission checks live leases under the same lock, so the two
/// can never both pass. `own_attempt` exempts the detached dispatcher's own
/// lease write for its admitted attempt.
// trace:STORY-2254 | ai:claude
pub(crate) fn with_scope_entry<T>(
    project_root: &Path,
    role: &str,
    spec: &str,
    enter: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // No runtime directory can be created (a read-only home): fall back to
    // an existing one, and with none, no detached attempt exists to honour.
    let dir = match runtime_dir(project_root) {
        Ok(dir) => dir,
        Err(_) => match existing_runtime_dir(project_root) {
            Some(dir) => dir,
            None => return enter(),
        },
    };
    let scope = ScopeKey::new(role, spec);
    with_admission_lock(&dir, || {
        if let Some(blocker) =
            scope_blocker_except(&dir, project_root, &scope, own_attempt().as_deref())
        {
            anyhow::bail!("refusing to enter `{}`: {blocker}", scope.spec);
        }
        enter()
    })
}

/// Read-only twin of [`with_scope_entry`] for routes that create nothing
/// (a resume picker): the check runs inside the admission lock. Never
/// creates the runtime directory.
// trace:STORY-2254 | ai:claude
pub(crate) fn scope_entry_blocker(project_root: &Path, role: &str, spec: &str) -> Option<String> {
    let dir = existing_runtime_dir(project_root)?;
    let scope = ScopeKey::new(role, spec);
    match with_admission_lock(&dir, || {
        Ok(scope_blocker_except(
            &dir,
            project_root,
            &scope,
            own_attempt().as_deref(),
        ))
    }) {
        Ok(blocker) => blocker,
        Err(e) => Some(format!("{e:#}")),
    }
}

thread_local! {
    static OWN_ATTEMPT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn own_attempt() -> Option<String> {
    OWN_ATTEMPT.with(|a| a.borrow().clone())
}

/// While alive, this thread's lease writes are the detached dispatcher's own
/// for `attempt` (already admitted), so they are not refused by it.
pub(crate) struct OwnAttemptScope(Option<String>);
impl OwnAttemptScope {
    pub(crate) fn enter(attempt: &str) -> Self {
        Self(OWN_ATTEMPT.with(|a| a.replace(Some(attempt.to_string()))))
    }
}
impl Drop for OwnAttemptScope {
    fn drop(&mut self) {
        OWN_ATTEMPT.with(|a| *a.borrow_mut() = self.0.take());
    }
}

/// Compare-update attempt `attempt` under the admission lock: `expect`
/// decides from the current record (by owner, phase, generation) whether the
/// caller may change it; `change` applies. The persisted result is re-read
/// and verified. Returns the updated receipt.
pub(crate) fn compare_update(
    dir: &Path,
    project_root: &Path,
    attempt: &str,
    expect: impl FnOnce(&LaunchReceipt) -> Result<()>,
    change: impl FnOnce(&mut LaunchReceipt),
) -> Result<LaunchReceipt> {
    with_admission_lock(dir, || {
        let mut receipt = match read_receipt(dir, project_root, attempt) {
            ReceiptRead::Known(receipt) => *receipt,
            ReceiptRead::Unknown(why) => {
                anyhow::bail!("launch attempt {attempt} is unknown ({why}); nothing was changed")
            }
        };
        expect(&receipt)?;
        change(&mut receipt);
        receipt.generation += 1;
        receipt.updated_at = chrono::Utc::now().to_rfc3339();
        write_receipt(dir, &receipt)?;
        match read_receipt(dir, project_root, attempt) {
            ReceiptRead::Known(now) if *now == receipt => Ok(receipt),
            _ => anyhow::bail!(
                "launch attempt {attempt}: the receipt update could not be verified; state is uncertain"
            ),
        }
    })
}

/// Read-only owner probe used by an invocation's rollback guards: does
/// `me` still own `attempt` as its dispatcher? Anything else (transferred,
/// unknown, unreadable) means NO, so the guard never destroys setup.
pub(crate) fn dispatcher_still_owns(
    dir: &Path,
    project_root: &Path,
    attempt: &str,
    me: &ProcId,
) -> bool {
    with_admission_lock(dir, || {
        Ok(matches!(
            read_receipt(dir, project_root, attempt),
            ReceiptRead::Known(r) if r.owner == Owner::Dispatcher(me.clone()) && r.phase == Phase::Prepared
        ))
    })
    .unwrap_or(false)
}

#[cfg(test)]
#[path = "tests/story_2254_codex_bg_receipt_tests.rs"]
mod story_2254_codex_bg_receipt_tests;
