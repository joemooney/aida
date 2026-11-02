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
    let lock = open_private(&path, false)?;
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
    read_receipt_at(&path, project_root)
}

fn read_receipt_at(path: &Path, project_root: &Path) -> ReceiptRead {
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
    if project_key(&receipt.project_root) != project_key(project_root) {
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
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let attempt = name.strip_suffix(".json").unwrap_or(&name).to_string();
        let read = if name.ends_with(".json") && uuid::Uuid::parse_str(&attempt).is_ok() {
            read_receipt_at(&entry.path(), project_root)
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
    for (attempt, read) in list_receipts(dir, project_root) {
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
                return Some(format!(
                    "detached launch attempt {attempt} has an unreadable receipt ({why}), so the scope it may own is unknown; inspect `aida agent launch-status {attempt}`"
                ));
            }
        }
    }
    None
}

/// Admit `receipt` (phase Prepared) for its scope, or refuse while any
/// attempt holds the same scope or any receipt is unknown. The check and the
/// write share one admission-lock hold, so two launches cannot both pass.
pub(crate) fn admit(dir: &Path, receipt: &LaunchReceipt) -> Result<()> {
    with_admission_lock(dir, || {
        if let Some(blocker) = scope_blocker(dir, &receipt.project_root, &receipt.scope) {
            anyhow::bail!("refusing a second detached driver: {blocker}");
        }
        write_receipt(dir, receipt)
    })
}

/// Read-only: the blocker other launch routes must honour before they enter
/// `scope` (role + spec) themselves. Never creates the runtime directory.
pub(crate) fn detached_scope_blocker(
    project_root: &Path,
    role: &str,
    spec: &str,
) -> Option<String> {
    let dir = existing_runtime_dir(project_root)?;
    scope_blocker(&dir, project_root, &ScopeKey::new(role, spec))
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
