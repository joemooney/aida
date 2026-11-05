//! `aida deputy shell`: the script deputy. It runs whole bash scripts that
//! advisor sessions submit, ONE at a time, in the operator's own terminal and
//! under the grant that terminal holds. Every job counts as the operator at
//! the keyboard; the controls are attribution (a live, valid, allow-listed
//! submitter grant on this host), serial execution with the script shown on
//! screen first, and an append-only audit.
//!
//! The queue lives under the AIDA home, never in a world-writable temp dir:
//!
//! ```text
//! $AIDA_HOME/.aida/deputy-shell/<name>/
//!   submit.sock              the live runner's socket (submit and cancel)
//!   accepted/<id>.job        a submitted job the runner accepted (runner-only)
//!   accepted/<id>.cancel     a cancel request the runner accepted
//!   jobs/<id>/job.json       the submitted record, moved here at claim
//!   jobs/<id>/script.sh      the exact script bytes (written after the verdict)
//!   jobs/<id>/state.json     state, timestamps, rc, reason, attribution
//!   jobs/<id>/transcript.log full pty output
//!   acks/<id>                a failed job acknowledged
//!   audit.jsonl              append-only, one event per line
//!   runner.json, runner.lock the live runner
//!   paused                   claim nothing while present
//! ```
//!
//! A submitter writes nothing here. It connects to `submit.sock`; the runner
//! reads the peer's credentials from the kernel (`SO_PEERCRED`), accepts the
//! job only when that peer runs under the grant it names, and writes the
//! record into `accepted/` itself ([`socket`]). No seat sandbox is given a
//! socket rule: only unsandboxed processes (the operator's shell, host-run
//! agents, or a submit the operator lists in the sandbox's
//! `excludedCommands`) can reach the runner.
//!
//! Every directory is 0700 and checked at every scan (a symlink, a foreign
//! owner or a group/other bit stops claiming). It reuses the command deputy's
//! private-file helpers, pty, human-terminal check, mandate probe and result
//! mail, but not its spool or worker: those are built around argv jobs, TTY
//! policy and parallel workers.
// trace:STORY-2338 | ai:claude

pub(crate) mod attribution;
#[cfg(unix)]
pub(crate) mod runner;
pub(crate) mod socket;

#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::deputy::{check_private_dir, create_private_dir, validate_id, write_private_new};

/// Set on every script job: its id. A process that carries it cannot submit
/// another script job or start a deputy.
pub(crate) const JOB_ENV: &str = "AIDA_DEPUTY_SHELL_JOB";
/// Set on every script job: the deputy name it runs under.
pub(crate) const NAME_ENV: &str = "AIDA_DEPUTY_SHELL_NAME";
/// The record schema this deputy reads and writes.
pub(crate) const SCHEMA: u32 = 1;
/// Q8: the default job timeout.
pub(crate) const DEFAULT_TIMEOUT: &str = "2h";
/// Retention of finished job directories, in days.
pub(crate) const DEFAULT_KEEP_DAYS: u32 = 30;
/// Q3: who may submit by default, and the seat the runner itself needs.
pub(crate) const DEFAULT_SUBMIT_SEATS: [&str; 1] = ["advisor"];
pub(crate) const RUNNER_SEAT: &str = "advisor";
/// `wait` gave up before the job finished (not a job failure).
pub(crate) const RC_WAIT_TIMEOUT: i32 = 75;
pub(crate) use crate::deputy::{RC_CANCELLED, RC_REFUSED, RC_TIMEOUT};

/// The clock every timestamp and retention decision reads (tests inject one).
pub(crate) type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub(crate) fn system_clock() -> Clock {
    Arc::new(Utc::now)
}

/// `$AIDA_HOME/.aida/deputy-shell`: every script deputy's queue.
pub(crate) fn base_dir() -> Result<PathBuf> {
    let home = crate::gate::kernel_deps::aida_home_dir()
        .context("cannot locate the user's home directory")?;
    Ok(home.join(".aida").join("deputy-shell"))
}

/// One script deputy's queue.
#[derive(Debug, Clone)]
pub(crate) struct Queue {
    pub name: String,
    /// `<base>`, also kept 0700.
    pub base: PathBuf,
    /// `<base>/<name>`.
    pub root: PathBuf,
}

/// The queue for deputy `name` under `base`.
pub(crate) fn queue_in(base: &Path, name: &str) -> Result<Queue> {
    validate_name(name)?;
    Ok(Queue {
        name: name.to_string(),
        base: base.to_path_buf(),
        root: base.join(name),
    })
}

/// The queue for deputy `name` under this user's AIDA home.
pub(crate) fn current_queue(name: &str) -> Result<Queue> {
    queue_in(&base_dir()?, name)
}

fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        bail!("deputy name must be 1-40 letters, digits, '-' or '_' (got {name:?})");
    }
    Ok(())
}

/// Where the submitter's machine was when it submitted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Host {
    pub hostname: String,
    pub boot_id: String,
}

impl Host {
    /// This machine, as of this boot.
    pub(crate) fn current() -> Self {
        Self {
            hostname: crate::coordination::hostname(),
            boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
        }
    }
}

/// Who submitted a job, taken from the submitter's VALIDATED grant record and
/// its own process, never from free text. The runner checks it again at
/// claim ([`attribution::verdict`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submitter {
    /// The submitting session (`AIDA_SESSION_ID`, else the agent's own
    /// session id): scopes `aida awaiting`, `cancel` and `ack`.
    pub session_id: Option<String>,
    pub seat: String,
    pub subject: String,
    pub principal: String,
    /// The full grant id. At rest only in this 0700 queue; display and audit
    /// use its 8-character prefix. A grant authorizes only a process its
    /// binding names, so knowing the id is not holding it.
    #[serde(default)]
    pub grant_id: Option<String>,
    /// The binding token the submitter presented (`proc:<pid>@<start>`,
    /// `sid:…` or `key:…`).
    #[serde(default)]
    pub presenter_token: Option<String>,
    pub pid: u32,
    pub pid_start: Option<String>,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub host: Host,
    pub mail_to: Option<String>,
    /// The kernel-reported peer that handed this job to the runner over its
    /// socket. Only the runner sets it (a client's value is overwritten), and
    /// the claim side refuses a job without it.
    #[serde(default)]
    pub peer: Option<Peer>,
}

/// What the kernel reported about the process that submitted a job over the
/// runner's socket, and the grant binding its ancestry reached.
// trace:STORY-2343 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Peer {
    pub pid: u32,
    pub uid: u32,
    pub start: Option<String>,
    /// The grant binding token (`proc:`/`sid:`) an ancestor of the peer
    /// (or the peer itself) matched.
    pub matched: String,
    pub attested: bool,
}

impl Submitter {
    pub(crate) fn grant_prefix(&self) -> String {
        self.grant_id
            .as_deref()
            .map(|g| g.chars().take(8).collect())
            .unwrap_or_else(|| "-".into())
    }
    /// `<grant prefix> <seat>`, the audit `actor` for this submitter.
    pub(crate) fn actor(&self) -> String {
        format!("{} {}", self.grant_prefix(), self.seat)
    }
}

/// `job.json`: what was submitted. Immutable once claimed. An unknown field
/// is refused at claim, never dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShellJob {
    pub schema: u32,
    pub id: String,
    pub deputy: String,
    pub created_at: DateTime<Utc>,
    pub script: String,
    /// Hex sha256 of the script BYTES, so `sha256sum script.sh` checks it.
    pub sha256: String,
    /// The file it was submitted from (informational).
    pub source: Option<String>,
    pub timeout_secs: u64,
    #[serde(default)]
    pub spec: Option<String>,
    pub submitter: Submitter,
}

pub(crate) fn script_digest(script: &str) -> String {
    format!("{:x}", Sha256::digest(script.as_bytes()))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JobState {
    Queued,
    Running,
    Done,
    Cancelled,
}

impl JobState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunnerRef {
    pub pid: u32,
    pub grant_prefix: String,
}

/// The claim-time attribution verdict and the checks it passed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Attribution {
    /// `ok` or `refused:<code>`.
    pub verdict: String,
    pub checks: Vec<String>,
}

/// `state.json`: the mutable half of a claimed job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ShellState {
    pub state: JobState,
    pub queued_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub rc: Option<i32>,
    /// `ok`, `exit`, `timeout`, `cancelled`, `orphaned` or `refused:<code>`.
    pub reason: Option<String>,
    pub runner: Option<RunnerRef>,
    pub attribution: Option<Attribution>,
    pub transcript_bytes: Option<u64>,
}

impl ShellState {
    /// A failure `aida awaiting` lists: done with a non-zero rc (refusals
    /// included); a cancellation is not one.
    pub(crate) fn failed(&self) -> bool {
        self.state == JobState::Done && self.rc.is_some_and(|rc| rc != 0)
    }
}

/// One `audit.jsonl` line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct AuditRow {
    pub at: DateTime<Utc>,
    pub deputy: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// Grant prefix + seat, or `operator-tty`.
    pub actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// The actor of an event the operator's own terminal caused.
pub(crate) const OPERATOR_TTY: &str = "operator-tty";

/// `runner.json`: the live runner, for `list` and for submit's seat check.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RunnerInfo {
    pub pid: u32,
    pub pid_start: Option<String>,
    pub tty: Option<String>,
    pub boot_id: String,
    pub grant_prefix: String,
    pub submit_seats: Vec<String>,
    pub started_at: DateTime<Utc>,
}

/// What `status`, `list` and `wait` see of a job.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JobView {
    pub id: String,
    pub job: Option<ShellJob>,
    pub state: ShellState,
    /// A cancel request is on file (queued or running jobs).
    pub cancel_requested: bool,
}

impl Queue {
    /// The runner-only spool of accepted jobs and cancel requests.
    pub(crate) fn accepted(&self) -> PathBuf {
        self.root.join("accepted")
    }
    /// The runner's submit socket.
    pub(crate) fn socket_path(&self) -> PathBuf {
        self.root.join("submit.sock")
    }
    pub(crate) fn jobs(&self) -> PathBuf {
        self.root.join("jobs")
    }
    pub(crate) fn acks(&self) -> PathBuf {
        self.root.join("acks")
    }
    pub(crate) fn accepted_job(&self, id: &str) -> PathBuf {
        self.accepted().join(format!("{id}.job"))
    }
    pub(crate) fn cancel_path(&self, id: &str) -> PathBuf {
        self.accepted().join(format!("{id}.cancel"))
    }
    pub(crate) fn job_dir(&self, id: &str) -> PathBuf {
        self.jobs().join(id)
    }
    pub(crate) fn job_json(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("job.json")
    }
    pub(crate) fn script_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("script.sh")
    }
    pub(crate) fn state_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("state.json")
    }
    pub(crate) fn transcript_path(&self, id: &str) -> PathBuf {
        self.job_dir(id).join("transcript.log")
    }
    pub(crate) fn ack_path(&self, id: &str) -> PathBuf {
        self.acks().join(id)
    }
    pub(crate) fn audit_path(&self) -> PathBuf {
        self.root.join("audit.jsonl")
    }
    pub(crate) fn runner_json(&self) -> PathBuf {
        self.root.join("runner.json")
    }
    pub(crate) fn runner_lock(&self) -> PathBuf {
        self.root.join("runner.lock")
    }
    pub(crate) fn paused_path(&self) -> PathBuf {
        self.root.join("paused")
    }

    fn private_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.base.clone(),
            self.root.clone(),
            self.accepted(),
            self.jobs(),
            self.acks(),
        ]
    }

    /// Create the queue tree (0700, umask 077 for what it holds) and check it.
    pub(crate) fn ensure(&self) -> Result<()> {
        #[cfg(unix)]
        // SAFETY: umask only sets this process's file-creation mask.
        unsafe {
            libc::umask(0o077);
        }
        for d in self.private_dirs() {
            create_private_dir(&d)?;
        }
        self.check()
    }

    /// Refuse a queue directory that is a symlink, foreign-owned, or open to
    /// group/others. The runner stops claiming on a failure; it never repairs.
    pub(crate) fn check(&self) -> Result<()> {
        for d in self.private_dirs() {
            check_private_dir(&d)?;
        }
        Ok(())
    }

    /// Append one audit line: `O_APPEND`, one write, then fsync.
    pub(crate) fn audit(&self, row: &AuditRow) -> Result<()> {
        let mut line = serde_json::to_vec(row)?;
        line.push(b'\n');
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(self.audit_path())
            .context("open the script deputy's audit log")?;
        f.write_all(&line).context("append to the audit log")?;
        f.sync_data().context("sync the audit log")?;
        Ok(())
    }

    /// Best-effort audit: an event that cannot be written is reported, and
    /// the caller's action still stands.
    pub(crate) fn audit_event(
        &self,
        now: DateTime<Utc>,
        event: &str,
        job: Option<&str>,
        actor: &str,
        fill: impl FnOnce(&mut AuditRow),
    ) {
        let mut row = AuditRow {
            at: now,
            deputy: self.name.clone(),
            event: event.to_string(),
            job: job.map(str::to_string),
            actor: actor.to_string(),
            sha256: None,
            rc: None,
            reason: None,
            detail: None,
        };
        fill(&mut row);
        if let Err(e) = self.audit(&row) {
            eprintln!("deputy shell {}: could not audit {event}: {e:#}", self.name);
        }
    }

    pub(crate) fn read_audit(&self) -> Vec<AuditRow> {
        std::fs::read_to_string(self.audit_path())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// RUNNER SIDE: file an accepted `job` as `accepted/<id>.job`: 0600,
    /// create-new on a dot-tmp, then a link that never replaces, so a
    /// half-written or clobbered job is never visible. Only the runner's
    /// socket server calls it, after the kernel-attested peer check.
    // trace:STORY-2343 | ai:claude
    pub(crate) fn accept(&self, job: &ShellJob) -> Result<PathBuf> {
        validate_id(&job.id)?;
        self.check()?;
        let tmp = self.accepted().join(format!(".{}.tmp", job.id));
        let dest = self.accepted_job(&job.id);
        write_private_new(&tmp, &serde_json::to_vec_pretty(job)?)?;
        if std::fs::symlink_metadata(&dest).is_ok() || self.job_dir(&job.id).exists() {
            let _ = std::fs::remove_file(&tmp);
            bail!("job {} already exists", job.id);
        }
        // A link, then unlink: unlike rename(2) it never replaces a file.
        std::fs::hard_link(&tmp, &dest).context("queue the job")?;
        let _ = std::fs::remove_file(&tmp);
        Ok(dest)
    }

    /// Ids of queued jobs, oldest first (UUIDv7 sorts by time).
    pub(crate) fn queued_ids(&self) -> Vec<String> {
        ids_with_suffix(&self.accepted(), ".job")
    }

    /// Ids with a cancel request on file.
    pub(crate) fn cancel_ids(&self) -> Vec<String> {
        ids_with_suffix(&self.accepted(), ".cancel")
    }

    /// Ids of claimed jobs (a directory under `jobs/`), oldest first.
    pub(crate) fn job_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.jobs())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| validate_id(n).is_ok())
            .collect();
        ids.sort();
        ids
    }

    pub(crate) fn read_job(&self, id: &str) -> Option<ShellJob> {
        let bytes = std::fs::read(self.job_json(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub(crate) fn read_state(&self, id: &str) -> Option<ShellState> {
        let bytes = std::fs::read(self.state_path(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write `state.json` atomically (tmp + rename inside the job's own dir).
    pub(crate) fn write_state(&self, id: &str, state: &ShellState) -> Result<()> {
        let tmp = self
            .job_dir(id)
            .join(format!(".state.{}.tmp", uuid::Uuid::new_v4()));
        write_private_new(&tmp, &serde_json::to_vec_pretty(state)?)?;
        std::fs::rename(&tmp, self.state_path(id)).context("write the job state")?;
        Ok(())
    }

    pub(crate) fn cancel_requested(&self, id: &str) -> bool {
        std::fs::symlink_metadata(self.cancel_path(id)).is_ok_and(|m| m.file_type().is_file())
    }

    /// Where job `id` is now: claimed (its `state.json`), or still queued.
    pub(crate) fn locate(&self, id: &str) -> Result<Option<JobView>> {
        validate_id(id)?;
        let cancel_requested = self.cancel_requested(id);
        if let Some(state) = self.read_state(id) {
            return Ok(Some(JobView {
                id: id.to_string(),
                job: self.read_job(id),
                state,
                cancel_requested,
            }));
        }
        let queued = self.accepted_job(id);
        if std::fs::symlink_metadata(&queued).is_ok() {
            let job: Option<ShellJob> = std::fs::read(&queued)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let queued_at = job.as_ref().map_or_else(Utc::now, |j| j.created_at);
            return Ok(Some(JobView {
                id: id.to_string(),
                job,
                state: ShellState::queued(queued_at),
                cancel_requested,
            }));
        }
        Ok(None)
    }

    /// RUNNER SIDE: file a cancel request for job `id` (the socket server
    /// accepted it); the runner acts on it at its next check.
    pub(crate) fn request_cancel(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        self.check()?;
        match write_private_new(&self.cancel_path(id), b"cancel\n") {
            Ok(()) => Ok(()),
            Err(_) if self.cancel_requested(id) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn acknowledged(&self, id: &str) -> bool {
        std::fs::symlink_metadata(self.ack_path(id)).is_ok()
    }

    pub(crate) fn acknowledge(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        match write_private_new(&self.ack_path(id), b"ack\n") {
            Ok(()) => Ok(()),
            Err(_) if self.acknowledged(id) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn paused(&self) -> bool {
        std::fs::symlink_metadata(self.paused_path()).is_ok()
    }

    /// Pause (idempotent). Returns false if it was already paused.
    pub(crate) fn pause(&self) -> Result<bool> {
        if self.paused() {
            return Ok(false);
        }
        match write_private_new(&self.paused_path(), b"paused\n") {
            Ok(()) => Ok(true),
            Err(_) if self.paused() => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Resume. Returns false if it was not paused.
    pub(crate) fn resume(&self) -> Result<bool> {
        match std::fs::remove_file(self.paused_path()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("remove the pause marker"),
        }
    }

    /// The runner record, when one was written (live or not).
    pub(crate) fn runner_info(&self) -> Option<RunnerInfo> {
        let bytes = std::fs::read(self.runner_json()).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// The runner record when its process is still the one that wrote it.
    pub(crate) fn live_runner(&self) -> Option<RunnerInfo> {
        let info = self.runner_info()?;
        let alive = match info.pid_start.as_deref() {
            Some(start) => aida_core::liveness::process_identity_matches_strict(info.pid, start),
            None => false,
        };
        alive.then_some(info)
    }

    /// Retention: remove every finished job directory older than `keep_days`
    /// (queued and running jobs never), with its ack, auditing each removal
    /// first so the decision trail outlives the transcript. Returns how many
    /// were pruned.
    pub(crate) fn prune(&self, now: DateTime<Utc>, keep_days: u32) -> usize {
        let cutoff = now - chrono::Duration::days(i64::from(keep_days));
        let mut pruned = 0;
        for id in self.job_ids() {
            let Some(state) = self.read_state(&id) else {
                continue;
            };
            if !matches!(state.state, JobState::Done | JobState::Cancelled) {
                continue;
            }
            let Some(finished) = state.finished_at else {
                continue;
            };
            if finished >= cutoff {
                continue;
            }
            let sha = self.read_job(&id).map(|j| j.sha256);
            self.audit_event(now, "pruned", Some(&id), OPERATOR_TTY, |r| {
                r.sha256 = sha;
                r.rc = state.rc;
                r.detail = Some(serde_json::json!({ "finished_at": finished }));
            });
            if std::fs::remove_dir_all(self.job_dir(&id)).is_ok() {
                let _ = std::fs::remove_file(self.ack_path(&id));
                pruned += 1;
            }
        }
        pruned
    }
}

impl ShellState {
    pub(crate) fn queued(queued_at: DateTime<Utc>) -> Self {
        Self {
            state: JobState::Queued,
            queued_at,
            claimed_at: None,
            started_at: None,
            finished_at: None,
            rc: None,
            reason: None,
            runner: None,
            attribution: None,
            transcript_bytes: None,
        }
    }
}

fn ids_with_suffix(dir: &Path, suffix: &str) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id = name.strip_suffix(suffix)?;
            validate_id(id).ok()?;
            Some(id.to_string())
        })
        .collect();
    ids.sort();
    ids
}

/// Names of every script deputy under `base`.
pub(crate) fn queue_names(base: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| validate_name(n).is_ok())
        .collect();
    names.sort();
    names
}

/// Failed script jobs nobody acknowledged, submitted from `project_root`,
/// oldest first. With `session`, only that session's; with `None` (the
/// operator's shell), every one. File reads only, so it rides the per-turn
/// notice.
pub(crate) fn unacked_failed_jobs_in(
    base: &Path,
    project_root: &Path,
    session: Option<&str>,
) -> Vec<crate::deputy::FailedJob> {
    let same_root = |p: &Path| {
        p == project_root
            || std::fs::canonicalize(p).ok() == std::fs::canonicalize(project_root).ok()
    };
    let mut out = Vec::new();
    for name in queue_names(base) {
        let Ok(q) = queue_in(base, &name) else {
            continue;
        };
        for id in q.job_ids() {
            let Some(state) = q.read_state(&id) else {
                continue;
            };
            if !state.failed() || q.acknowledged(&id) {
                continue;
            }
            let Some(job) = q.read_job(&id) else {
                continue;
            };
            if !same_root(&job.submitter.project_root) {
                continue;
            }
            if let Some(s) = session {
                if job.submitter.session_id.as_deref() != Some(s) {
                    continue;
                }
            }
            out.push(crate::deputy::FailedJob {
                deputy: name.clone(),
                id: id.clone(),
                requester_session: job.submitter.session_id.clone(),
                rc: state.rc.unwrap_or_default(),
                reason: state.reason.clone().unwrap_or_default(),
                output: q.transcript_path(&id),
                kind: crate::deputy::DeputyKind::Script,
            });
        }
    }
    out
}

/// [`unacked_failed_jobs_in`] under this user's AIDA home.
pub(crate) fn unacked_failed_jobs(
    project_root: &Path,
    session: Option<&str>,
) -> Vec<crate::deputy::FailedJob> {
    match base_dir() {
        Ok(base) => unacked_failed_jobs_in(&base, project_root, session),
        Err(_) => Vec::new(),
    }
}

/// `[deputy_shell]` in the project's `.aida/config.toml`: `keep_days` only.
/// Missing or unreadable means the default.
///
/// Who may submit is deliberately NOT a config key: the file is tracked, so
/// any merged PR could widen the seats whose scripts run with the operator's
/// at-the-terminal authority. The advisor seat is the fixed default and only
/// the operator's `--allow-seat` at `run` widens it (Q3). A `submit_seats`
/// entry in the file is ignored.
// trace:STORY-b6b1278890774ef993d2ebc327862470-334.Authority | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellConfig {
    pub keep_days: u32,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            keep_days: DEFAULT_KEEP_DAYS,
        }
    }
}

/// The seats that may submit before any `--allow-seat`: the advisor.
pub(crate) fn default_submit_seats() -> Vec<String> {
    DEFAULT_SUBMIT_SEATS.iter().map(|s| s.to_string()).collect()
}

pub(crate) fn parse_config(body: &str) -> ShellConfig {
    let mut cfg = ShellConfig::default();
    let Ok(value) = toml::from_str::<toml::Value>(body) else {
        return cfg;
    };
    let Some(table) = value.get("deputy_shell") else {
        return cfg;
    };
    if let Some(days) = table.get("keep_days").and_then(|v| v.as_integer()) {
        if let Ok(days) = u32::try_from(days) {
            cfg.keep_days = days.max(1);
        }
    }
    cfg
}

pub(crate) fn load_config(project_root: &Path) -> ShellConfig {
    std::fs::read_to_string(project_root.join(".aida").join("config.toml"))
        .map(|b| parse_config(&b))
        .unwrap_or_default()
}

/// Mail the submitter a script job's result. A failed send never fails the
/// job; the result is already filed.
pub(crate) fn mail_result(q: &Queue, job: &ShellJob, state: &ShellState) -> Result<bool> {
    let Some(to) = job
        .submitter
        .mail_to
        .clone()
        .filter(|s| !s.trim().is_empty())
    else {
        return Ok(false);
    };
    let rc = state.rc.unwrap_or_default();
    let reason = state.reason.clone().unwrap_or_default();
    let first = job.script.lines().next().unwrap_or("").trim().to_string();
    let mut body = format!(
        "Script deputy {name:?} finished script job {id}: rc {rc} ({reason}).\n",
        name = q.name,
        id = job.id
    );
    body.push_str(&format!("Script: {first}\n"));
    body.push_str(&format!(
        "Transcript: {}\n",
        q.transcript_path(&job.id).display()
    ));
    body.push_str(&format!(
        "Read it with `aida deputy shell log {} --name {}`.\n",
        job.id, q.name
    ));
    if state.failed() {
        body.push_str(&format!(
            "`aida awaiting` lists it until you run `aida deputy shell ack {} --name {}`.\n",
            job.id, q.name
        ));
    }
    crate::deputy::mail_job_result(
        &job.submitter.project_root,
        &format!("deputy-shell:{}", q.name),
        &to,
        &job.id,
        "script job",
        rc,
        body,
    )?;
    Ok(true)
}
