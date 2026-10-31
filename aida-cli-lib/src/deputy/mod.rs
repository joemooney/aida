//! `aida deputy`: a stand-in for the operator that runs jobs other sessions
//! submit, under the grant held by the operator's own interactive shell.
//!
//! The spool lives in the main checkout under `.aida/deputy/` (0700, owner
//! checked at every scan):
//!
//! - `incoming/<name>/` is the ONLY part a contained agent may write (its
//!   sandbox allowlist names `.aida/deputy/incoming`, nothing else). A submit
//!   writes one `<id>.job` there atomically; a cancel request is a
//!   `<id>.cancel` file beside it.
//! - `<name>/` holds what only the deputy writes: `running/`, `done/`,
//!   `failed/`, `out/`, the ledger, the TTY policy and the terminal markers.
//!
//! The worker claims a job by moving it into `running/` without replacing
//! anything there, so two submits never clobber each other, a job runs once,
//! and a submission that reuses a known id is refused as a duplicate. Each job runs as a child
//! process, never sourced into the loop. Its output goes to `out/<id>.out`,
//! its result to `done|failed/<id>.rc.json`, and every step to the
//! append-only `deputy.ledger`.
//!
//! The central rule: a job gets a terminal only when its argv matches a live
//! entry in the operator-authored TTY policy. Every other job runs with no
//! controlling terminal, so each human-TTY gate refuses it as it would refuse
//! an agent.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

pub(crate) mod hooks;
pub(crate) mod policy;
#[cfg(unix)]
pub(crate) mod worker;

#[cfg(test)]
mod tests;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Environment a job runs under: its id and the spool it came from. The
/// integrity floor reads both (`gate::kernel_deps::deputy_job_floor_authorized`).
pub(crate) const JOB_ENV: &str = crate::gate::kernel_deps::DEPUTY_JOB_ENV;
pub(crate) const SPOOL_ENV: &str = crate::gate::kernel_deps::DEPUTY_SPOOL_ENV;
pub(crate) const REQUESTER_ENV: &str = "AIDA_DEPUTY_REQUESTER";
/// Set as a per-command prefix by the `aida()` shell function only, so a bare
/// binary run (no function) is told to start the deputy from the shell.
pub(crate) const VIA_WRAPPER_ENV: &str = "AIDA_DEPUTY_VIA_WRAPPER";
/// The capability the `aida()` shell function advertises in
/// `AIDA_SHELL_WRAPPER` once it routes `aida deputy` (an older function does
/// not, and gets the update hint).
pub(crate) const WRAPPER_CAP: &str = "deputy";

pub(crate) const DEFAULT_NAME: &str = "default";
pub(crate) const STATES: [&str; 4] = ["incoming", "running", "done", "failed"];

/// The flags that bypass a gate. A job carrying one never gets a terminal.
pub(crate) const BYPASS_FLAGS: [&str; 5] = [
    "--force",
    "--no-verify",
    "--admin",
    "--yes-i-know",
    "--override-stale-approval",
];

/// The directory under a main checkout that holds every deputy spool.
pub(crate) fn deputy_base(main_root: &Path) -> PathBuf {
    main_root.join(".aida").join("deputy")
}

/// The submit side of every deputy under a main checkout: the one directory a
/// contained session may write.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn deputy_incoming_base(main_root: &Path) -> PathBuf {
    deputy_base(main_root).join("incoming")
}

/// One deputy's spool.
#[derive(Debug, Clone)]
pub(crate) struct Spool {
    /// `<base>/<name>`: everything only the deputy writes.
    pub root: PathBuf,
    /// `<base>/incoming/<name>`: where submits and cancel requests land.
    pub incoming: PathBuf,
}

/// Who submitted a job. Audit data, not authority: a same-uid process can
/// write any value here (slice 3 adds a submit capability).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Requester {
    pub session_id: Option<String>,
    pub role: Option<String>,
    pub seat: Option<String>,
    pub subject: Option<String>,
    /// First 8 characters of the submitter's grant id, never the full handle.
    pub grant_prefix: Option<String>,
    /// Whether `submit` validated the submitter's own grant.
    pub grant_verified: bool,
    pub spec: Option<String>,
    pub pid: u32,
    /// The mail identity the submitter sends as, recorded at submit so the
    /// deputy can mail it the result. Absent on jobs from older submits.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JobBody {
    /// Exec'd directly, no shell. The only form that may get a terminal.
    Argv(Vec<String>),
    /// Run by `bash --noprofile --norc -c`; needs `--allow-scripts`.
    Script(String),
}

impl JobBody {
    pub(crate) fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        format!("{:x}", Sha256::digest(&bytes))
    }

    pub(crate) fn summary(&self) -> String {
        match self {
            Self::Argv(argv) => argv.join(" "),
            Self::Script(body) => {
                let first = body.lines().next().unwrap_or("").trim();
                format!("script: {first}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct JobRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub requester: Requester,
    pub serial: Option<String>,
    pub timeout_secs: u64,
    pub body: JobBody,
    pub sha256: String,
    /// A submit capability or delegated grant (slice 3). This deputy cannot
    /// verify one, so a job that carries one is refused at claim
    /// ([`hooks::submitter_delegation`]), never run as if it were absent.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<serde_json::Value>,
}

/// A finished job's result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct JobResult {
    pub id: String,
    pub rc: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: DateTime<Utc>,
    /// `ok`, `exit`, `timeout`, `cancelled`, `orphaned`, or `refused:<why>`.
    pub reason: String,
    pub tty: bool,
    pub policy_entry: Option<String>,
    pub output: PathBuf,
}

/// Written by the worker at `running/<id>.waiting` while a terminal job has
/// printed nothing for the idle window. Display only: the job keeps running
/// until it ends or its own timeout stops it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct WaitingMarker {
    pub since: DateTime<Utc>,
    pub idle_secs: u64,
}

/// `<root>/paused`: the deputy claims nothing while it exists.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct PauseMarker {
    pub at: Option<DateTime<Utc>>,
    pub by: Option<String>,
}

/// One failed job a requester has not acknowledged yet, as `aida awaiting`
/// lists it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FailedJob {
    pub deputy: String,
    pub id: String,
    pub requester_session: Option<String>,
    pub rc: i32,
    pub reason: String,
    pub output: PathBuf,
}

/// Failed jobs under every deputy of `main_root` that nobody acknowledged,
/// oldest first. With `session`, only the jobs that session submitted; with
/// `None` (the operator's shell), every one. File reads only (no git, no
/// network), so it rides the per-turn notice.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
pub(crate) fn unacked_failed_jobs(main_root: &Path, session: Option<&str>) -> Vec<FailedJob> {
    let base = deputy_base(main_root);
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| validate_name(n).is_ok())
        .collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let Ok(spool) = spool_for(main_root, &name) else {
            continue;
        };
        for id in spool.list_ids("failed") {
            if spool.acknowledged(&id) {
                continue;
            }
            let Some(res) = spool.read_result(&id) else {
                continue;
            };
            let requester_session = std::fs::read(spool.job_path("failed", &id))
                .ok()
                .and_then(|b| serde_json::from_slice::<JobRecord>(&b).ok())
                .and_then(|r| r.requester.session_id);
            if let Some(s) = session {
                if requester_session.as_deref() != Some(s) {
                    continue;
                }
            }
            out.push(FailedJob {
                deputy: name.clone(),
                id,
                requester_session,
                rc: res.rc,
                reason: res.reason,
                output: res.output,
            });
        }
    }
    out
}

/// Mail the requester the job's result: rc, reason and output path, plus how
/// to acknowledge it when it failed. Returns `Ok(false)` when the job names
/// no one to mail. `project_root` is the checkout whose mailbox gets it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
pub(crate) fn mail_result(
    project_root: &Path,
    deputy: &str,
    rec: Option<&JobRecord>,
    res: &JobResult,
) -> Result<bool> {
    let Some(to) = rec.and_then(|r| {
        r.requester
            .mail_to
            .clone()
            .or_else(|| r.requester.role.clone())
            .filter(|s| !s.trim().is_empty())
    }) else {
        return Ok(false);
    };
    let short: String = res.id.chars().take(8).collect();
    let outcome = if res.rc == 0 { "done" } else { "FAILED" };
    let mut body = format!(
        "Deputy {deputy:?} finished job {id}: rc {rc} ({reason}).\n",
        id = res.id,
        rc = res.rc,
        reason = res.reason
    );
    if let Some(r) = rec {
        body.push_str(&format!("Command: {}\n", r.body.summary()));
    }
    body.push_str(&format!("Output: {}\n", res.output.display()));
    body.push_str(&format!("Read it with `aida deputy log {}`.\n", res.id));
    if res.rc != 0 {
        body.push_str(&format!(
            "`aida awaiting` lists it until you run `aida deputy ack {}`.\n",
            res.id
        ));
    }
    let msg = aida_core::mailbox::Message {
        subject: Some(format!("deputy job {short} {outcome}: rc {}", res.rc)),
        // One message per job result, so a retry never duplicates it.
        id: format!("deputy-{deputy}-{}", res.id),
        thread_id: format!("deputy-{}", res.id),
        from: format!("deputy:{deputy}"),
        to: aida_core::mailbox::Recipient::Agent(to),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    crate::mailbox_store::write_message(project_root, &msg)?;
    Ok(true)
}

/// rc for a timed-out job (as `timeout(1)`).
pub(crate) const RC_TIMEOUT: i32 = 124;
/// rc for a job refused before it ran.
pub(crate) const RC_REFUSED: i32 = 126;
/// rc for a cancelled or orphaned job.
pub(crate) const RC_CANCELLED: i32 = 130;

/// The spool for deputy `name` under the main checkout.
pub(crate) fn spool_for(main_root: &Path, name: &str) -> Result<Spool> {
    validate_name(name)?;
    let base = deputy_base(main_root);
    Ok(Spool {
        root: base.join(name),
        incoming: base.join("incoming").join(name),
    })
}

/// The spool for `name` in the current project's main checkout.
pub(crate) fn current_spool(name: &str) -> Result<Spool> {
    let main = crate::worktree_gc::find_main_worktree_root()
        .context("aida deputy needs a git checkout")?;
    spool_for(&main, name)
}

fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name != "incoming"
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        bail!("deputy name must be 1-40 letters, digits, '-' or '_', and not `incoming` (got {name:?})");
    }
    Ok(())
}

/// Job ids are UUIDv7, so they sort by submit time. Anything else is refused,
/// which also keeps ids path-safe.
pub(crate) fn validate_id(id: &str) -> Result<()> {
    match uuid::Uuid::parse_str(id) {
        Ok(u) if u.hyphenated().to_string() == id => Ok(()),
        _ => bail!("not a deputy job id: {id:?}"),
    }
}

impl Spool {
    pub(crate) fn dir(&self, state: &str) -> PathBuf {
        if state == "incoming" {
            self.incoming.clone()
        } else {
            self.root.join(state)
        }
    }
    pub(crate) fn out_dir(&self) -> PathBuf {
        self.root.join("out")
    }
    pub(crate) fn ledger_path(&self) -> PathBuf {
        self.root.join("deputy.ledger")
    }
    pub(crate) fn policy_path(&self) -> PathBuf {
        self.root.join("deputy.policy")
    }
    pub(crate) fn lock_path(&self) -> PathBuf {
        self.root.join(".deputy.lock")
    }
    pub(crate) fn job_path(&self, state: &str, id: &str) -> PathBuf {
        self.dir(state).join(format!("{id}.job"))
    }
    pub(crate) fn result_path(&self, state: &str, id: &str) -> PathBuf {
        self.dir(state).join(format!("{id}.rc.json"))
    }
    pub(crate) fn out_path(&self, id: &str) -> PathBuf {
        self.out_dir().join(format!("{id}.out"))
    }
    /// Present while a job runs on a terminal the policy authorized.
    pub(crate) fn tty_marker_path(&self, id: &str) -> PathBuf {
        crate::gate::kernel_deps::deputy_tty_marker_path(&self.root, id)
    }
    /// A cancel request: written on the submit side, honoured by the worker.
    pub(crate) fn cancel_request_path(&self, id: &str) -> PathBuf {
        self.incoming.join(format!("{id}.cancel"))
    }
    /// Present while a terminal job looks to be waiting on a prompt.
    pub(crate) fn waiting_marker_path(&self, id: &str) -> PathBuf {
        self.dir("running").join(format!("{id}.waiting"))
    }
    /// The requester's acknowledgement of a failed job: written on the
    /// submit side, so a contained session can clear its own failures.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    pub(crate) fn ack_path(&self, id: &str) -> PathBuf {
        self.incoming.join(format!("{id}.ack"))
    }
    /// Present while the deputy is paused (`aida deputy pause`). Only the
    /// operator's side writes it; a contained agent cannot reach `root`.
    pub(crate) fn paused_path(&self) -> PathBuf {
        self.root.join("paused")
    }
    /// The slice-3 per-deputy command policy. This deputy cannot enforce one,
    /// so while the file exists it claims nothing ([`hooks::command_policy`]).
    pub(crate) fn command_policy_path(&self) -> PathBuf {
        self.root.join("deputy.commands")
    }

    /// Whether the deputy is paused. Any entry at the marker path counts, so
    /// a marker that cannot be read still pauses (fails closed).
    pub(crate) fn paused(&self) -> Option<PauseMarker> {
        let p = self.paused_path();
        std::fs::symlink_metadata(&p).ok()?;
        Some(
            std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(PauseMarker { at: None, by: None }),
        )
    }

    /// Pause: write the marker (idempotent). Returns false if already paused.
    pub(crate) fn pause(&self, by: Option<String>) -> Result<bool> {
        if self.paused().is_some() {
            return Ok(false);
        }
        let marker = PauseMarker {
            at: Some(Utc::now()),
            by,
        };
        match write_private_new(&self.paused_path(), &serde_json::to_vec(&marker)?) {
            Ok(()) => Ok(true),
            Err(_) if self.paused().is_some() => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Resume: remove the marker. Returns false if it was not paused.
    pub(crate) fn resume(&self) -> Result<bool> {
        match std::fs::remove_file(self.paused_path()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).context("remove the deputy's pause marker"),
        }
    }

    /// Acknowledge failed job `id` so `aida awaiting` stops listing it.
    /// Writes only under `incoming/`, like a submit.
    pub(crate) fn acknowledge(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        self.check_incoming()?;
        match write_private_new(&self.ack_path(id), b"ack\n") {
            Ok(()) => Ok(()),
            Err(_) if self.acknowledged(id) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn acknowledged(&self, id: &str) -> bool {
        std::fs::symlink_metadata(self.ack_path(id)).is_ok()
    }

    fn private_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.root.clone()];
        if let Some(base) = self.incoming.parent() {
            dirs.push(base.to_path_buf());
        }
        dirs.push(self.incoming.clone());
        for state in ["running", "done", "failed"] {
            dirs.push(self.dir(state));
        }
        dirs.push(self.out_dir());
        dirs
    }

    /// Create the spool tree, 0700, and check it.
    pub(crate) fn ensure(&self) -> Result<()> {
        for d in self.private_dirs() {
            create_private_dir(&d)?;
        }
        self.check_owned()
    }

    /// Refuse a spool directory not owned by us or open to group/others.
    pub(crate) fn check_owned(&self) -> Result<()> {
        for d in self.private_dirs() {
            check_private_dir(&d)?;
        }
        Ok(())
    }

    fn check_incoming(&self) -> Result<()> {
        if !self.incoming.is_dir() {
            bail!(
                "no deputy is set up at {} — the operator starts one with `aida deputy` in their terminal",
                self.root.display()
            );
        }
        if let Some(base) = self.incoming.parent() {
            check_private_dir(base)?;
        }
        check_private_dir(&self.incoming)
    }

    /// Write `rec` as `incoming/<name>/<id>.job`: 0600, `create_new` on a
    /// dot-tmp, then a rename, so a half-written job is never visible and an
    /// existing job is never overwritten. Touches nothing outside `incoming/`.
    pub(crate) fn submit(&self, rec: &JobRecord) -> Result<PathBuf> {
        validate_id(&rec.id)?;
        self.check_incoming()?;
        let tmp = self.incoming.join(format!(".{}.tmp", rec.id));
        let dest = self.job_path("incoming", &rec.id);
        write_private_new(&tmp, &serde_json::to_vec_pretty(rec)?)?;
        if dest.exists() {
            let _ = std::fs::remove_file(&tmp);
            bail!("job {} already exists", rec.id);
        }
        std::fs::rename(&tmp, &dest).context("publish job")?;
        Ok(dest)
    }

    /// Ask the worker to cancel job `id`, queued or running. Like a submit it
    /// writes only under `incoming/`.
    pub(crate) fn request_cancel(&self, id: &str) -> Result<()> {
        validate_id(id)?;
        self.check_incoming()?;
        match write_private_new(&self.cancel_request_path(id), b"cancel\n") {
            Ok(()) => Ok(()),
            Err(_) if self.cancel_request_path(id).is_file() => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn cancel_requested(&self, id: &str) -> bool {
        std::fs::symlink_metadata(self.cancel_request_path(id))
            .is_ok_and(|md| md.file_type().is_file())
    }

    /// Where job `id` is now.
    pub(crate) fn locate(&self, id: &str) -> Result<Option<(&'static str, PathBuf)>> {
        validate_id(id)?;
        for state in STATES {
            let p = self.job_path(state, id);
            if p.exists() {
                return Ok(Some((state, p)));
            }
        }
        Ok(None)
    }

    pub(crate) fn read_result(&self, id: &str) -> Option<JobResult> {
        for state in ["done", "failed"] {
            if let Ok(bytes) = std::fs::read(self.result_path(state, id)) {
                if let Ok(r) = serde_json::from_slice(&bytes) {
                    return Some(r);
                }
            }
        }
        None
    }

    /// The waiting-on-a-prompt marker of a running terminal job, if any.
    pub(crate) fn waiting(&self, id: &str) -> Option<WaitingMarker> {
        let bytes = std::fs::read(self.waiting_marker_path(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// `<state>/*.job` ids, oldest first (UUIDv7 sorts by time).
    pub(crate) fn list_ids(&self, state: &str) -> Vec<String> {
        let mut ids: Vec<String> = std::fs::read_dir(self.dir(state))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".job")?;
                validate_id(id).ok()?;
                Some(id.to_string())
            })
            .collect();
        ids.sort();
        ids
    }

    /// Where job id `id` has already been used, if anywhere: a job file or
    /// result in `running/`, `done/` or `failed/`, a terminal marker, or an
    /// output file. A submit that reuses such an id is a forgery (an agent
    /// can write `incoming/`) and is never claimed: the claim would replace
    /// the real job's record and inherit its terminal marker.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
    pub(crate) fn id_in_use(&self, id: &str) -> Option<&'static str> {
        let present = |p: PathBuf| std::fs::symlink_metadata(p).is_ok();
        for state in ["running", "done", "failed"] {
            if present(self.job_path(state, id)) || present(self.result_path(state, id)) {
                return Some(state);
            }
        }
        if present(self.tty_marker_path(id)) {
            return Some("a terminal marker");
        }
        if present(self.out_path(id)) {
            return Some("an output file");
        }
        None
    }

    /// Where a refused duplicate submission is filed: beside, never over, the
    /// real job's record. The name is not `<uuid>.job`, so no listing or
    /// lookup ever mistakes it for job `id`.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
    pub(crate) fn duplicate_path(&self, id: &str) -> PathBuf {
        self.dir("failed")
            .join(format!("{id}.duplicate-{}.job", new_job_id()))
    }

    /// Claim `incoming/<id>.job` into `running/` WITHOUT replacing anything
    /// there (link, then unlink the source; `rename(2)` would overwrite), and
    /// return the claimed file's (device, inode) for the terminal marker.
    /// `AlreadyExists` means the id is taken; `NotFound` means another
    /// claimer won.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
    pub(crate) fn claim(&self, id: &str) -> std::io::Result<(u64, u64)> {
        let (from, to) = (self.job_path("incoming", id), self.job_path("running", id));
        std::fs::hard_link(&from, &to)?;
        if let Err(e) = std::fs::remove_file(&from) {
            let _ = std::fs::remove_file(&to);
            return Err(e);
        }
        let md = std::fs::symlink_metadata(&to)?;
        Ok(crate::gate::kernel_deps::deputy_file_identity(&md))
    }

    /// Ids with a cancel request on file.
    pub(crate) fn cancel_request_ids(&self) -> Vec<String> {
        std::fs::read_dir(&self.incoming)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".cancel")?;
                validate_id(id).ok()?;
                Some(id.to_string())
            })
            .collect()
    }

    /// Append one ledger row in a single `O_APPEND` write.
    pub(crate) fn ledger(&self, row: &LedgerRow) -> Result<()> {
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
            .open(self.ledger_path())
            .context("open deputy ledger")?;
        f.write_all(&line).context("append deputy ledger")?;
        Ok(())
    }

    /// Write a result atomically into `state` (`done` or `failed`).
    pub(crate) fn write_result(&self, state: &str, res: &JobResult) -> Result<()> {
        let tmp = self.dir(state).join(format!(".{}.rc.tmp", res.id));
        let _ = std::fs::remove_file(&tmp);
        write_private_new(&tmp, &serde_json::to_vec_pretty(res)?)?;
        std::fs::rename(&tmp, self.result_path(state, &res.id))?;
        Ok(())
    }
}

/// One `deputy.ledger` row.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct LedgerRow {
    pub ts: DateTime<Utc>,
    pub job: String,
    /// `claimed|refused|started|waiting|finished|cancelled|orphaned|policy`.
    pub event: String,
    pub deputy: String,
    pub requester: Option<Requester>,
    pub summary: Option<String>,
    pub sha256: Option<String>,
    pub worker: Option<usize>,
    pub tty: Option<bool>,
    pub policy_entry: Option<String>,
    pub rc: Option<i32>,
    pub reason: Option<String>,
    pub output: Option<PathBuf>,
}

impl LedgerRow {
    pub(crate) fn new(deputy: &str, job: &str, event: &str) -> Self {
        Self {
            ts: Utc::now(),
            job: job.to_string(),
            event: event.to_string(),
            deputy: deputy.to_string(),
            ..Default::default()
        }
    }
}

pub(crate) fn create_private_dir(d: &Path) -> Result<()> {
    if !d.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(d)
                .with_context(|| format!("create {}", d.display()))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(d)?;
    }
    Ok(())
}

/// A spool directory must be a real directory (not a symlink), owned by us,
/// with no group/other bits.
pub(crate) fn check_private_dir(d: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(d).with_context(|| format!("stat {}", d.display()))?;
    if !md.file_type().is_dir() {
        bail!("{} is not a directory", d.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != current_uid() {
            bail!("{} is not owned by you; refusing it", d.display());
        }
        if md.mode() & 0o077 != 0 {
            bail!(
                "{} is open to group/other (mode {:o}); it must be 0700",
                d.display(),
                md.mode() & 0o777
            );
        }
    }
    Ok(())
}

pub(crate) fn write_private_new(p: &Path, bytes: &[u8]) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(p)
        .with_context(|| format!("create {}", p.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Why a job file is refused before it is parsed as authority (AC2).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobFileRefusal {
    NotRegular,
    ForeignOwner,
    Writable,
    Linked,
    Unreadable(String),
    Malformed(String),
    IdMismatch,
}

impl JobFileRefusal {
    pub(crate) fn code(&self) -> String {
        match self {
            Self::NotRegular => "refused:not-a-regular-file".into(),
            Self::ForeignOwner => "refused:foreign-owner".into(),
            Self::Writable => "refused:group-or-world-writable".into(),
            Self::Linked => "refused:hard-linked".into(),
            Self::Unreadable(e) => format!("refused:unreadable ({e})"),
            Self::Malformed(e) => format!("refused:malformed ({e})"),
            Self::IdMismatch => "refused:id-mismatch".into(),
        }
    }
}

/// Inspect a job file without following links, then parse it.
pub(crate) fn read_job_file(path: &Path, id: &str) -> Result<JobRecord, JobFileRefusal> {
    let md =
        std::fs::symlink_metadata(path).map_err(|e| JobFileRefusal::Unreadable(e.to_string()))?;
    if !md.file_type().is_file() {
        return Err(JobFileRefusal::NotRegular);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if md.uid() != current_uid() {
            return Err(JobFileRefusal::ForeignOwner);
        }
        if md.mode() & 0o022 != 0 {
            return Err(JobFileRefusal::Writable);
        }
        if md.nlink() > 1 {
            return Err(JobFileRefusal::Linked);
        }
    }
    let bytes = std::fs::read(path).map_err(|e| JobFileRefusal::Unreadable(e.to_string()))?;
    let rec: JobRecord =
        serde_json::from_slice(&bytes).map_err(|e| JobFileRefusal::Malformed(e.to_string()))?;
    if rec.id != id || rec.sha256 != rec.body.digest() {
        return Err(JobFileRefusal::IdMismatch);
    }
    Ok(rec)
}

/// The submitter's identity, as this process can see it. The grant is
/// validated with the existing presenter check, so `grant_verified` is only
/// true when this process actually holds the grant it names.
pub(crate) fn current_requester(spec: Option<String>) -> Requester {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let mut r = Requester {
        session_id: env("AIDA_SESSION_ID"),
        role: env("AIDA_SESSION_ROLE"),
        spec,
        pid: std::process::id(),
        // The identity this session sends mail as is where its results go.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        mail_to: Some(crate::resolve_mail_sender_identity(None).0),
        ..Default::default()
    };
    if let Ok(main) = crate::worktree_gc::find_main_worktree_root() {
        if let crate::gate::seat_authority::GrantCheck::Valid(g) =
            crate::gate::seat_authority::grant_check(&main)
        {
            r.seat = Some(g.seat.clone());
            r.subject = Some(g.subject.clone());
            r.grant_prefix = Some(g.id.chars().take(8).collect());
            r.grant_verified = true;
        }
    }
    r
}

/// The marker a terminal job's floor check reads; its shape lives on the
/// gate floor, which is what reads it.
pub(crate) type TtyMarker = crate::gate::kernel_deps::DeputyTtyMarker;

pub(crate) fn write_tty_marker(spool: &Spool, marker: &TtyMarker) -> Result<()> {
    let p = spool.tty_marker_path(&marker.job);
    let _ = std::fs::remove_file(&p);
    write_private_new(&p, &serde_json::to_vec(marker)?)
}

pub(crate) fn new_job_id() -> String {
    uuid::Uuid::now_v7().hyphenated().to_string()
}
