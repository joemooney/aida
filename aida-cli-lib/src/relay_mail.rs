//! Relay mail lifetime, AIDA side (`aida relay-mail`).
//!
//! A relay run is one logical mail recipient for its whole life: the relay
//! label is reserved as a handle whose recipient UUID is the run id, and
//! every child session the controller rotates through binds to that same
//! reservation. The run record under `.aida/agents/relay-runs/` binds the run
//! to its controller (PID, kernel start identity, boot id and PID namespace)
//! and to the run lock the controller held at bind time. Records are written
//! only under the handle namespace lock.
//!
//! While no terminal generation is recorded and the controller is not
//! conclusively dead, the run holds its label, and mail sent to the label
//! between sessions is accepted for the next one. A paused controller is
//! still a live controller. The terminal generation is recorded once: by the
//! controller's own `end` (its exit trap, the fast path), or by
//! [`reconcile_relay_runs`] when the controller is provably gone (the crash
//! path, run at relay start and by `aida agent gc`). A controller this
//! process cannot observe is unknown, and an unknown run is never retired.
//!
//! Authority over a run is a secret capability, not a process identity: the
//! run id and controller pid are public, and process ancestry is unreliable
//! under PID-namespaced sandboxes. `bind` generates 256 random bits, prints
//! them once, and stores only their SHA-256. `child-bind` and `end` take the
//! capability on stdin or from a private file, never on argv, and compare it
//! in constant time before they read or write anything else. Crash
//! reconciliation needs no capability: it only retires a controller that is
//! conclusively gone.
//!
//! Live run records live in `relay-runs/`: open runs, and ended runs whose
//! end notices are still owed. Once every notice is inserted and delivered
//! the run is archived to `relay-runs/ended/` with its outbox, so ended runs
//! stop counting against the record budget.
// trace:STORY-1843 | ai:claude

use crate::agent_handle::{self, HandleRequest};
use crate::agent_registry;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[path = "relay_mail_args.rs"]
pub(crate) mod relay_mail_args;
pub(crate) use relay_mail_args::*;

// trace:STORY-1843 | ai:claude
#[path = "relay_mail_outbox.rs"]
pub(crate) mod outbox;

/// The relay mail protocol this binary speaks. A relay compares it exactly;
/// a different version refuses before anything is reserved.
pub(crate) const PROTOCOL: &str = "aida-relay-mail/1";

/// Operations the protocol offers, as `probe` reports them.
const OPERATIONS: &[&str] = &["probe", "bind", "child-bind", "end", "outbox"];

/// Behaviours the protocol guarantees, as `probe` reports them.
const FEATURES: &[&str] = &["gap-acceptance", "terminal-reconcile", "end-notices"];

const MAX_RUN_RECORD_BYTES: u64 = 65536;
/// Live run records read per pass. Archived runs are outside it.
const MAX_OPEN_RUN_RECORDS: usize = 1024;
/// Random bytes in a run capability.
const CAPABILITY_BYTES: usize = 32;
/// Bytes read from a capability source: the hex capability and a newline.
const MAX_CAPABILITY_INPUT: u64 = 256;
/// Child bindings kept in a run record, newest last.
const MAX_CHILD_HISTORY: usize = 64;

/// The process that owns a relay run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ControllerEvidence {
    pub(crate) pid: u32,
    /// Kernel start identity, so a reused PID is not mistaken for the
    /// controller.
    pub(crate) start_identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pid_namespace: Option<u64>,
}

/// The run lock the controller held when it bound the run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunLockEvidence {
    pub(crate) path: PathBuf,
    /// `<device>:<inode>` of the lock file at bind time.
    pub(crate) identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ChildBinding {
    pub(crate) generation: u64,
    /// Set when the child was bound by native session id; a launch-token
    /// binding completes at the child's own startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) native_session_id: Option<String>,
    pub(crate) bound_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TerminalReason {
    Stop,
    Exit,
    Blocked,
    /// The controller died without recording its end.
    Crash,
}

impl From<EndReason> for TerminalReason {
    fn from(reason: EndReason) -> Self {
        match reason {
            EndReason::Stop => Self::Stop,
            EndReason::Exit => Self::Exit,
            EndReason::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Terminal {
    pub(crate) generation: u64,
    pub(crate) reason: TerminalReason,
    pub(crate) at: DateTime<Utc>,
}

/// One relay run: the durable owner of a relay label.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RelayRun {
    pub(crate) protocol: String,
    /// The run UUID; the same string as the label reservation's
    /// `recipient_id`.
    pub(crate) run_id: String,
    pub(crate) label: String,
    pub(crate) controller: ControllerEvidence,
    pub(crate) run_lock: RunLockEvidence,
    /// SHA-256 (hex) of the run capability. The capability itself is never
    /// stored; a record without a digest accepts no capability.
    #[serde(default)]
    pub(crate) capability_sha256: String,
    pub(crate) bound_at: DateTime<Utc>,
    /// Advances on every lifecycle change: bind, each child bind, end.
    pub(crate) generation: u64,
    #[serde(default)]
    pub(crate) children: Vec<ChildBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal: Option<Terminal>,
    /// A complete pass after the end inserted every end notice owed.
    // trace:STORY-1843 | ai:claude
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) notices_settled: bool,
}

/// Whether a run's controller is alive, as this process can see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ControllerLiveness {
    Live,
    /// Provably gone: the machine rebooted, the PID is free, or the PID now
    /// belongs to another process.
    Dead,
    /// Not observable from here (another PID namespace, unreadable /proc).
    Unknown,
}

/// A run that accepts mail to its label now.
#[derive(Debug, Clone)]
pub(crate) struct AcceptingRun {
    pub(crate) label: String,
    pub(crate) run_id: String,
    pub(crate) liveness: ControllerLiveness,
}

#[derive(Debug, Default)]
pub(crate) struct ReconcileReport {
    /// Runs whose terminal generation this pass recorded.
    pub(crate) ended: Vec<String>,
    /// Open runs whose controller could not be observed; left open.
    pub(crate) unknown: Vec<String>,
}

pub(crate) fn runs_dir(project_root: &Path) -> PathBuf {
    agent_registry::agents_dir(project_root).join("relay-runs")
}

/// The canonical spelling of a run id, or an error. Only the canonical
/// hyphenated lower-case UUID names a record file.
fn canonical_run_id(raw: &str) -> Result<String> {
    let parsed = uuid::Uuid::parse_str(raw.trim())
        .map_err(|_| anyhow::anyhow!("invalid relay run id `{raw}`"))?;
    let canonical = parsed.hyphenated().to_string();
    if canonical != raw.trim() {
        bail!("invalid relay run id `{raw}`; expected `{canonical}`");
    }
    Ok(canonical)
}

/// Where retired run records are archived, outside the live-record budget.
pub(crate) fn ended_runs_dir(project_root: &Path) -> PathBuf {
    runs_dir(project_root).join("ended")
}

fn run_path(project_root: &Path, run_id: &str) -> PathBuf {
    runs_dir(project_root).join(format!("{run_id}.json"))
}

fn ended_run_path(project_root: &Path, run_id: &str) -> PathBuf {
    ended_runs_dir(project_root).join(format!("{run_id}.json"))
}

fn read_run_at(path: &Path, run_id: &str) -> Result<Option<RelayRun>> {
    let Some(body) = agent_handle::read_allocation_text(path, MAX_RUN_RECORD_BYTES)? else {
        return Ok(None);
    };
    let run: RelayRun = serde_json::from_str(&body).context("invalid relay run record")?;
    if run.run_id != run_id {
        bail!("relay run record filename mismatch");
    }
    Ok(Some(run))
}

/// A run record, live or archived. The archive wins: a run is archived
/// before its live record is removed, so a crash between the two leaves the
/// archived copy authoritative.
pub(crate) fn load_run(project_root: &Path, run_id: &str) -> Result<Option<RelayRun>> {
    let run_id = canonical_run_id(run_id)?;
    if let Some(run) = read_run_at(&ended_run_path(project_root, &run_id), &run_id)? {
        return Ok(Some(run));
    }
    read_run_at(&run_path(project_root, &run_id), &run_id)
}

/// Write a run record in place: to the archive once the run is archived,
/// to `relay-runs/` otherwise.
pub(crate) fn write_run(project_root: &Path, run: &RelayRun) -> Result<()> {
    let archived = ended_run_path(project_root, &run.run_id);
    let path = if archived.exists() {
        archived
    } else {
        run_path(project_root, &run.run_id)
    };
    write_run_at(&path, run)
}

fn write_run_at(path: &Path, run: &RelayRun) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating relay run directory {}", dir.display()))?;
    }
    let body = serde_json::to_string_pretty(run).context("serialising relay run")?;
    aida_core::write_atomic(path, body.as_bytes())
        .with_context(|| format!("writing relay run record {}", path.display()))
}

/// Move an ended run whose notices are settled out of the live records:
/// its per-run directory (the outbox) first, then its record, then the live
/// record is removed. Call with the namespace lock held.
pub(crate) fn archive_run(project_root: &Path, run: &RelayRun) -> Result<()> {
    if run.terminal.is_none() || !run.notices_settled {
        bail!("relay run {} is not settled; it stays live", run.run_id);
    }
    let live_dir = runs_dir(project_root).join(&run.run_id);
    if live_dir.is_dir() {
        let archived_dir = ended_runs_dir(project_root).join(&run.run_id);
        std::fs::create_dir_all(ended_runs_dir(project_root))?;
        std::fs::rename(&live_dir, &archived_dir)
            .with_context(|| format!("archiving {}", live_dir.display()))?;
    }
    write_run_at(&ended_run_path(project_root, &run.run_id), run)?;
    let live = run_path(project_root, &run.run_id);
    match std::fs::remove_file(&live) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", live.display())),
    }
}

/// Every readable live run record (open, or ended with notices owed),
/// bounded in count. Only `.json` files count; directories (the `ended/`
/// archive, per-run outboxes) and stray entries are skipped uncounted.
/// Unreadable records are skipped: they own nothing, and the label they
/// named stays governed by its handle reservation.
fn list_runs(project_root: &Path) -> Result<Vec<RelayRun>> {
    let entries = match std::fs::read_dir(runs_dir(project_root)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("reading relay run records"),
    };
    let mut runs = Vec::new();
    let mut counted = 0usize;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        counted += 1;
        if counted > MAX_OPEN_RUN_RECORDS {
            bail!("live relay run budget exceeded ({MAX_OPEN_RUN_RECORDS} records)");
        }
        // An archived copy shadows a leftover live record.
        if !ended_run_path(project_root, stem).exists() {
            if let Ok(Some(run)) = load_run(project_root, stem) {
                runs.push(run);
            }
        }
    }
    runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(runs)
}

/// A fresh run capability: 256 random bits, lower-case hex.
fn new_capability() -> Result<String> {
    let mut bytes = [0u8; CAPABILITY_BYTES];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| anyhow::anyhow!("generating relay run capability: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn capability_digest(capability: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(capability.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Equal-length byte comparison whose time does not depend on where the
/// inputs differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Refuse unless `capability` is the one `bind` issued for `run`.
fn verify_capability(run: &RelayRun, capability: &str) -> Result<()> {
    let presented = capability_digest(capability.trim());
    if run.capability_sha256.is_empty()
        || !constant_time_eq(presented.as_bytes(), run.capability_sha256.as_bytes())
    {
        bail!(
            "relay run {} refused: the run capability does not match",
            run.run_id
        );
    }
    Ok(())
}

/// Read a run capability from `cap_file`, or from stdin when none is given.
/// The file must be a regular file (no symlink) readable by its owner only;
/// stdin must not be a terminal. Either source is read to a small bound.
pub(crate) fn read_capability(cap_file: Option<&Path>) -> Result<String> {
    use std::io::Read;
    let mut raw = String::new();
    match cap_file {
        Some(path) => {
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = options
                .open(path)
                .with_context(|| format!("opening capability file {}", path.display()))?;
            let meta = file.metadata()?;
            if !meta.is_file() {
                bail!("capability file {} is not a regular file", path.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.mode() & 0o077 != 0 {
                    bail!(
                        "capability file {} is readable by others; make it 0600",
                        path.display()
                    );
                }
                // SAFETY: geteuid has no preconditions.
                if meta.uid() != unsafe { libc::geteuid() } {
                    bail!("capability file {} is not owned by you", path.display());
                }
            }
            (&file)
                .take(MAX_CAPABILITY_INPUT)
                .read_to_string(&mut raw)
                .context("reading capability file")?;
        }
        None => {
            use std::io::IsTerminal;
            let stdin = std::io::stdin();
            if stdin.is_terminal() {
                bail!("pass the relay run capability on stdin or with --cap-file");
            }
            stdin
                .lock()
                .take(MAX_CAPABILITY_INPUT)
                .read_to_string(&mut raw)
                .context("reading capability from stdin")?;
        }
    }
    let capability = raw.trim();
    if capability.is_empty() {
        bail!("no relay run capability was given");
    }
    Ok(capability.to_string())
}

fn current_boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn process_exists(pid: u32) -> Option<bool> {
    if cfg!(target_os = "linux") {
        Some(Path::new(&format!("/proc/{pid}")).exists())
    } else {
        None
    }
}

/// The controller's liveness as this process sees it (BUG-2212: a PID in a
/// namespace this observer cannot see is unknown, never dead).
pub(crate) fn controller_liveness(controller: &ControllerEvidence) -> ControllerLiveness {
    if let (Some(recorded), Some(now)) = (&controller.boot_id, current_boot_id()) {
        if *recorded != now {
            return ControllerLiveness::Dead;
        }
    }
    if !agent_registry::PidObserver::current().can_observe(controller.pid_namespace) {
        return ControllerLiveness::Unknown;
    }
    match aida_core::liveness::process_start_identity(controller.pid) {
        Some(identity) if identity == controller.start_identity => ControllerLiveness::Live,
        Some(_) => ControllerLiveness::Dead,
        None => match process_exists(controller.pid) {
            Some(false) => ControllerLiveness::Dead,
            _ => ControllerLiveness::Unknown,
        },
    }
}

fn controller_evidence(pid: u32) -> Result<ControllerEvidence> {
    if pid == 0 {
        bail!("invalid controller pid 0");
    }
    let start_identity = aida_core::liveness::process_start_identity(pid)
        .with_context(|| format!("cannot observe relay controller process {pid}"))?;
    Ok(ControllerEvidence {
        pid,
        start_identity,
        boot_id: current_boot_id(),
        pid_namespace: agent_registry::current_pid_namespace(),
    })
}

/// Prove the controller holds its run lock: the file is a regular file (no
/// symlink), its first line names the controller, and a shared try-lock is
/// refused. The probe lock, when it succeeds, is released at once, the same
/// momentary probe the relay's own `--status` takes.
fn run_lock_evidence(path: &Path, controller_pid: u32) -> Result<RunLockEvidence> {
    use fs2::FileExt;
    use std::io::Read;
    if !path.is_absolute() {
        bail!("run lock path must be absolute: {}", path.display());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("opening run lock {}", path.display()))?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        bail!("run lock {} is not a regular file", path.display());
    }
    let mut head = String::new();
    (&file).take(64).read_to_string(&mut head).ok();
    let holder = head.lines().next().map(str::trim).unwrap_or("");
    if holder != controller_pid.to_string() {
        bail!(
            "run lock {} names holder `{holder}`, not controller pid {controller_pid}",
            path.display()
        );
    }
    match FileExt::try_lock_shared(&file) {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            bail!("the controller does not hold run lock {}", path.display());
        }
        Err(e) if e.kind() == fs2::lock_contended_error().kind() => {}
        Err(e) => return Err(e).context("probing run lock"),
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let identity = String::new();
    Ok(RunLockEvidence {
        path: path.to_path_buf(),
        identity,
    })
}

/// The open run that holds `reservation`'s label, if any: a run record for
/// its recipient UUID with no terminal generation and a controller that is
/// not provably dead.
pub(crate) fn run_holding(
    project_root: &Path,
    reservation: &agent_handle::HandleReservation,
) -> Option<String> {
    let run = load_run(project_root, &reservation.recipient_id)
        .ok()
        .flatten()?;
    (run.terminal.is_none()
        && run.label == reservation.handle
        && controller_liveness(&run.controller) != ControllerLiveness::Dead)
        .then_some(run.run_id)
}

/// Runs that accept mail to their label now: open, not provably dead, and
/// still the owner of their label's reservation.
pub(crate) fn accepting_runs(project_root: &Path) -> (Vec<AcceptingRun>, Vec<String>) {
    let runs = match list_runs(project_root) {
        Ok(runs) => runs,
        Err(e) => {
            return (
                Vec::new(),
                vec![format!("warning: skipping relay run records: {e:#}")],
            )
        }
    };
    let accepting = runs
        .into_iter()
        .filter(|run| run.terminal.is_none())
        .filter(|run| {
            agent_handle::checked_reservation(project_root, &run.label)
                .ok()
                .flatten()
                .is_some_and(|r| r.recipient_id == run.run_id)
        })
        .filter_map(|run| {
            let liveness = controller_liveness(&run.controller);
            (liveness != ControllerLiveness::Dead).then_some(AcceptingRun {
                label: run.label,
                run_id: run.run_id,
                liveness,
            })
        })
        .collect();
    (accepting, Vec::new())
}

fn record_terminal(run: &mut RelayRun, reason: TerminalReason, now: DateTime<Utc>) {
    run.generation += 1;
    run.terminal = Some(Terminal {
        generation: run.generation,
        reason,
        at: now,
    });
}

/// Record the terminal generation of every open run whose controller is
/// provably dead. Read-only unless one is found; then each is re-read and
/// re-checked under the namespace lock. Unknown controllers stay open.
pub(crate) fn reconcile_relay_runs(project_root: &Path) -> Result<ReconcileReport> {
    let mut report = ReconcileReport::default();
    let mut dead = Vec::new();
    for run in list_runs(project_root)? {
        if run.terminal.is_some() {
            continue;
        }
        match controller_liveness(&run.controller) {
            ControllerLiveness::Dead => dead.push(run.run_id),
            ControllerLiveness::Unknown => report.unknown.push(run.run_id),
            ControllerLiveness::Live => {}
        }
    }
    if dead.is_empty() {
        return Ok(report);
    }
    // The undelivered mail is read before the lock; its end notices are
    // inserted in the same critical section that records the end.
    // trace:STORY-1843 | ai:claude
    let plans: Vec<_> = dead
        .iter()
        .map(|run_id| {
            load_run(project_root, run_id)
                .ok()
                .flatten()
                .and_then(|run| outbox::plan_end_notices(project_root, &run).ok())
        })
        .collect();
    let lock = agent_handle::lock_namespace(project_root)?;
    for (run_id, plan) in dead.into_iter().zip(plans) {
        let Some(mut run) = load_run(project_root, &run_id)? else {
            continue;
        };
        if run.terminal.is_some()
            || controller_liveness(&run.controller) != ControllerLiveness::Dead
        {
            continue;
        }
        record_terminal(&mut run, TerminalReason::Crash, Utc::now());
        write_run(project_root, &run)?;
        if let Some(plan) = plan {
            outbox::insert_planned(project_root, &run, &plan, &lock)?;
        }
        report.ended.push(run_id);
    }
    Ok(report)
}

#[derive(Debug, Serialize)]
pub(crate) struct BindOutcome {
    pub(crate) run: String,
    pub(crate) label: String,
    pub(crate) protocol: String,
    pub(crate) generation: u64,
    /// The run capability, printed once. The controller keeps it for
    /// `child-bind` and `end` and never exports it to a child.
    pub(crate) capability: String,
}

/// Reserve `label` for a new run owned by `controller_pid`.
pub(crate) fn bind(
    project_root: &Path,
    label: &str,
    controller_pid: u32,
    run_lock: &Path,
    protocol: &str,
) -> Result<BindOutcome> {
    // The capability check comes before anything is observed or reserved.
    if protocol != PROTOCOL {
        bail!("relay mail protocol `{protocol}` is not supported; this aida speaks `{PROTOCOL}`");
    }
    let label = label.trim();
    if let Some(why) = agent_handle::handle_spelling_rejection(label) {
        bail!("invalid relay label `{label}`: {why}");
    }
    // Relay start is a reconciliation point: a crashed earlier run is ended
    // before its label is considered.
    if let Err(e) = reconcile_relay_runs(project_root) {
        eprintln!("warning: relay run reconciliation skipped: {e:#}");
    }
    // trace:STORY-1843 | ai:claude
    if let Err(e) = outbox::settle_ended_runs(project_root) {
        eprintln!("warning: relay end notices stay pending: {e:#}");
    }
    let controller = controller_evidence(controller_pid)?;
    let run_lock = run_lock_evidence(run_lock, controller_pid)?;
    let lock = agent_handle::lock_namespace(project_root)?;
    if let Some(existing) = agent_handle::checked_reservation(project_root, label)? {
        if let Some(holder) = run_holding(project_root, &existing) {
            bail!("relay label `{label}` is in use by relay run {holder}");
        }
        // Mail is keyed by the label, so a new run under an earlier run's
        // label would read that run's unread mail. Reuse needs an explicit
        // leave-with-old or handover choice, which this protocol version
        // does not offer.
        let earlier_run = std::iter::once(&existing.recipient_id)
            .chain(existing.previous_recipients.iter())
            .find(|id| load_run(project_root, id).ok().flatten().is_some());
        if let Some(previous) = earlier_run {
            bail!(
                "relay label `{label}` belonged to relay run {previous}; reusing a relay label is not supported yet, choose a new label"
            );
        }
    }
    let req = HandleRequest {
        agent_type: "relay",
        explicit: Some(label),
        ..Default::default()
    };
    let capability = new_capability()?;
    let reservation = agent_handle::reserve_handle_locked(project_root, &req, &lock)?;
    let run = RelayRun {
        protocol: PROTOCOL.to_string(),
        run_id: canonical_run_id(&reservation.recipient_id)?,
        label: reservation.handle.clone(),
        controller,
        run_lock,
        capability_sha256: capability_digest(&capability),
        bound_at: Utc::now(),
        generation: 1,
        children: Vec::new(),
        terminal: None,
        notices_settled: false,
    };
    write_run(project_root, &run)?;
    Ok(BindOutcome {
        run: run.run_id,
        label: run.label,
        protocol: run.protocol,
        generation: run.generation,
        capability,
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct ChildBindOutcome {
    pub(crate) run: String,
    pub(crate) label: String,
    pub(crate) generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) native_session_id: Option<String>,
    /// For the child to present in `launch_token_env` at its startup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) launch_token: Option<String>,
    pub(crate) launch_token_env: &'static str,
}

/// Bind the run's next child. The run UUID never changes: the label's
/// reservation keeps its recipient and only its session binding moves.
/// Only the holder of the run capability may bind.
pub(crate) fn child_bind(
    project_root: &Path,
    run_id: &str,
    capability: &str,
    native_session_id: Option<&str>,
) -> Result<ChildBindOutcome> {
    let run_id = canonical_run_id(run_id)?;
    let native_session_id = native_session_id.map(str::trim);
    if native_session_id.is_some_and(|sid| sid.is_empty() || sid.chars().any(char::is_whitespace)) {
        bail!("invalid native session id");
    }
    let _lock = agent_handle::lock_namespace(project_root)?;
    let Some(mut run) = load_run(project_root, &run_id)? else {
        bail!("unknown relay run {run_id}");
    };
    verify_capability(&run, capability)?;
    if let Some(t) = &run.terminal {
        bail!(
            "relay run {run_id} ended at generation {}; bind a new run",
            t.generation
        );
    }
    if controller_liveness(&run.controller) == ControllerLiveness::Dead {
        record_terminal(&mut run, TerminalReason::Crash, Utc::now());
        write_run(project_root, &run)?;
        bail!("relay run {run_id} controller is no longer running; the run is ended");
    }
    let Some(mut reservation) = agent_handle::checked_reservation(project_root, &run.label)?
        .filter(|r| r.recipient_id == run.run_id)
    else {
        bail!(
            "relay label `{}` no longer belongs to run {run_id}",
            run.label
        );
    };
    let now = Utc::now();
    reservation.reserved_at = now;
    reservation.registry_id = None;
    let launch_token = match native_session_id {
        Some(sid) => {
            reservation.native_session_id = Some(sid.to_string());
            reservation.launch_token = None;
            reservation.launcher_pid = None;
            None
        }
        None => {
            let token = uuid::Uuid::new_v4().to_string();
            reservation.native_session_id = None;
            reservation.launch_token = Some(token.clone());
            reservation.launcher_pid = Some(run.controller.pid);
            Some(token)
        }
    };
    agent_handle::write_reservation(project_root, &reservation)?;
    agent_handle::release_other_bindings(project_root, &reservation)?;
    run.generation += 1;
    run.children.push(ChildBinding {
        generation: run.generation,
        native_session_id: native_session_id.map(str::to_string),
        bound_at: now,
    });
    if run.children.len() > MAX_CHILD_HISTORY {
        let excess = run.children.len() - MAX_CHILD_HISTORY;
        run.children.drain(..excess);
    }
    write_run(project_root, &run)?;
    Ok(ChildBindOutcome {
        run: run.run_id,
        label: run.label,
        generation: run.generation,
        native_session_id: native_session_id.map(str::to_string),
        launch_token,
        launch_token_env: agent_handle::LAUNCH_TOKEN_ENV,
    })
}

#[derive(Debug, Serialize)]
pub(crate) struct EndOutcome {
    pub(crate) run: String,
    pub(crate) label: String,
    pub(crate) terminal_generation: u64,
    pub(crate) reason: TerminalReason,
    /// False when an earlier `end` or reconciliation already recorded it.
    pub(crate) recorded_now: bool,
    /// End notices inserted and delivered by this call's maintenance pass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) notices: Option<outbox::SettleReport>,
}

/// Record the run's terminal generation once. A repeat reports the
/// recorded end and changes nothing. Only the holder of the run capability
/// may end a run.
pub(crate) fn end(
    project_root: &Path,
    run_id: &str,
    capability: &str,
    reason: TerminalReason,
) -> Result<EndOutcome> {
    let run_id = canonical_run_id(run_id)?;
    // The run's undelivered mail is read before the lock, so its end
    // notices are inserted with the terminal record. trace:STORY-1843 | ai:claude
    let plan = load_run(project_root, &run_id)?
        .filter(|run| run.terminal.is_none() && verify_capability(run, capability).is_ok())
        .map(|run| outbox::plan_end_notices(project_root, &run))
        .transpose()
        .unwrap_or_else(|e| {
            eprintln!("warning: relay end notices deferred to maintenance: {e:#}");
            None
        });
    let (run, recorded_now) = {
        let lock = agent_handle::lock_namespace(project_root)?;
        let Some(mut run) = load_run(project_root, &run_id)? else {
            bail!("unknown relay run {run_id}");
        };
        verify_capability(&run, capability)?;
        let recorded_now = run.terminal.is_none();
        if recorded_now {
            record_terminal(&mut run, reason, Utc::now());
            write_run(project_root, &run)?;
            if let Some(plan) = &plan {
                outbox::insert_planned(project_root, &run, plan, &lock)?;
            }
        }
        (run, recorded_now)
    };
    // With no lock held: catch up any notice the plan missed and deliver.
    let notices = match outbox::settle_ended_runs(project_root) {
        Ok(report) => Some(report),
        Err(e) => {
            eprintln!("warning: relay end notices stay pending: {e:#}");
            None
        }
    };
    let terminal = run.terminal.as_ref().expect("terminal recorded");
    Ok(EndOutcome {
        run: run.run_id.clone(),
        label: run.label.clone(),
        terminal_generation: terminal.generation,
        reason: terminal.reason,
        recorded_now,
        notices,
    })
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn project_root() -> Result<PathBuf> {
    Ok(crate::main_worktree_root_from(
        &crate::find_aida_project_root_from(&std::env::current_dir()?)?,
    ))
}

pub(crate) fn handle_relay_mail_command(cmd: &RelayMailCommand) -> Result<()> {
    match cmd {
        RelayMailCommand::Probe { json } => {
            if *json {
                print_json(&serde_json::json!({
                    "protocol": PROTOCOL,
                    "operations": OPERATIONS,
                    "features": FEATURES,
                }))
            } else {
                println!("{PROTOCOL}");
                println!("operations: {}", OPERATIONS.join(", "));
                println!("features: {}", FEATURES.join(", "));
                Ok(())
            }
        }
        RelayMailCommand::Bind {
            label,
            controller_pid,
            run_lock,
            protocol,
            json,
        } => {
            let out = bind(&project_root()?, label, *controller_pid, run_lock, protocol)?;
            if *json {
                print_json(&out)
            } else {
                println!("{}", out.run);
                println!("{}", out.capability);
                Ok(())
            }
        }
        RelayMailCommand::ChildBind {
            run,
            native_session,
            cap_file,
            json,
        } => {
            let capability = read_capability(cap_file.as_deref())?;
            let out = child_bind(
                &project_root()?,
                run,
                &capability,
                native_session.as_deref(),
            )?;
            if *json {
                print_json(&out)
            } else {
                match &out.launch_token {
                    Some(token) => println!("{}={token}", out.launch_token_env),
                    None => println!(
                        "Bound session to relay label {} (generation {}).",
                        out.label, out.generation
                    ),
                }
                Ok(())
            }
        }
        RelayMailCommand::Outbox { run, json } => {
            let root = project_root()?;
            let runs: Vec<String> = match run {
                Some(run) => vec![canonical_run_id(run)?],
                None => list_runs(&root)?.into_iter().map(|r| r.run_id).collect(),
            };
            let mut entries = Vec::new();
            for run in runs {
                entries.extend(outbox::list_entries(&root, &run)?);
            }
            if *json {
                return print_json(&entries);
            }
            if entries.is_empty() {
                println!("No relay end notices.");
            }
            for e in &entries {
                let state = match &e.state {
                    outbox::NoticeState::Pending {
                        attempts,
                        last_error,
                    } => format!(
                        "pending ({attempts} attempt{}){}",
                        if *attempts == 1 { "" } else { "s" },
                        last_error
                            .as_deref()
                            .map(|err| format!(": {err}"))
                            .unwrap_or_default()
                    ),
                    outbox::NoticeState::Delivered { .. } => "delivered".to_string(),
                };
                println!(
                    "{} {} -> {} about {}: {state}",
                    e.label, e.terminal_generation, e.destination.address, e.message_id
                );
            }
            Ok(())
        }
        RelayMailCommand::End {
            run,
            cap_file,
            reason,
            json,
        } => {
            let capability = read_capability(cap_file.as_deref())?;
            let out = end(&project_root()?, run, &capability, (*reason).into())?;
            if *json {
                print_json(&out)
            } else {
                println!(
                    "Relay run {} {} at generation {}.",
                    out.run,
                    if out.recorded_now {
                        "ended"
                    } else {
                        "had already ended"
                    },
                    out.terminal_generation
                );
                Ok(())
            }
        }
    }
}

// The relay mail commands read and write only `.aida/agents/` records; no
// requirement-store handle is needed.
// trace:STORY-1843 | ai:claude
fn run_relay_mail(
    matches: &clap::ArgMatches,
    _ctx: &crate::command_registry::DispatchCtx,
) -> Result<()> {
    let args = <RelayMailArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    handle_relay_mail_command(&args.inner)
}

// trace:STORY-1843 | ai:claude
pub(crate) static RELAY_MAIL: crate::command_registry::CommandReg =
    crate::command_registry::CommandReg {
        name: "relay-mail",
        help_after: "mailbox",
        command: <RelayMailArgs as clap::CommandFactory>::command,
        policy: |_| crate::command_registry::CmdPolicy {
            stakeholder: crate::stakeholder_role::StakeholderAction::Write,
            per_write_auto_push: false,
            skip_role_restore_prompt: false,
            team_identity_write: false,
        },
        run: crate::command_registry::Handler::Early(run_relay_mail),
    };

#[cfg(test)]
#[path = "tests/story_1843_relay_lifetime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/story_1843_relay_outbox_tests.rs"]
mod outbox_tests;
