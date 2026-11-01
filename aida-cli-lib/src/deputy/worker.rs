//! The deputy worker loop: claim jobs from the spool, run each as a child
//! process (never sourced), capture its output and result, and ledger every
//! step. Up to `workers` jobs run at once; a `--serial` class runs one at a
//! time. A job runs on a deputy-owned pty only when the TTY policy authorized
//! its argv; every other job runs in a new session with no controlling
//! terminal and stdin from /dev/null.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::policy::{self, TtyDecision};
use super::{
    read_job_file, write_private_new, write_tty_marker, JobBody, JobRecord, JobResult, LedgerRow,
    Spool, TtyMarker, WaitingMarker, JOB_ENV, RC_CANCELLED, RC_REFUSED, RC_TIMEOUT, REQUESTER_ENV,
    SPOOL_ENV, VIA_WRAPPER_ENV,
};

/// The default idle window: a terminal job silent this long shows as waiting
/// on a prompt (display only; it is never stopped for it).
pub(crate) const DEFAULT_TTY_IDLE: Duration = Duration::from_secs(10);

/// Whether the operator's grant still holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GrantState {
    Valid { expires_at: Option<DateTime<Utc>> },
    Invalid(String),
}

pub(crate) type GrantProbe = Arc<dyn Fn() -> GrantState + Send + Sync>;

/// `Some(why)` once the mandate the deputy started under has ended (and the
/// operator did not pass `--keep-after-mandate`); `None` while it holds.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
pub(crate) type MandateProbe = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// A probe for a deputy that never stops for a mandate.
pub(crate) fn no_mandate_stop() -> MandateProbe {
    Arc::new(|| None)
}

#[derive(Clone)]
pub(crate) struct WorkerConfig {
    pub name: String,
    pub workers: usize,
    /// Canonical paths of executables other than `aida` that argv jobs may run.
    pub allow_exec: Vec<PathBuf>,
    pub allow_scripts: bool,
    /// A terminal job silent this long shows as waiting on a prompt in
    /// `aida deputy status`. Display only: the job's own timeout governs.
    pub tty_idle: Duration,
    pub poll: Duration,
    pub grant: GrantProbe,
    pub mandate: MandateProbe,
    /// The checkout whose mailbox gets each requester's result mail; `None`
    /// sends none.
    pub mail_root: Option<PathBuf>,
    /// Tests: stop once this many jobs have finished or been refused.
    pub max_jobs: Option<usize>,
    /// The program an argv job's `aida` resolves to (tests point it at a stub).
    pub aida_program: PathBuf,
}

#[derive(Default)]
struct Active {
    running: usize,
    serial: HashSet<String>,
    handled: usize,
}

/// Holds the single-instance lock for one deputy name.
pub(crate) struct InstanceLock {
    _file: std::fs::File,
}

pub(crate) fn acquire_lock(spool: &Spool) -> Result<InstanceLock> {
    use fs2::FileExt;
    let path = spool.lock_path();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .context("open deputy lock")?;
    if file.try_lock_exclusive().is_err() {
        let mut holder = String::new();
        let _ = file.read_to_string(&mut holder);
        bail!(
            "a deputy named {:?} is already running (pid {}); give this one another --name",
            spool.root.file_name().unwrap_or_default(),
            holder.trim()
        );
    }
    file.set_len(0)?;
    write!(file, "{}", std::process::id())?;
    Ok(InstanceLock { _file: file })
}

/// Jobs left in `running/` by a deputy that died. They are marked failed and
/// never re-run: a job may not be idempotent (`pr ship`). Call only while
/// holding the instance lock.
pub(crate) fn recover_orphans(spool: &Spool, cfg: &WorkerConfig) -> Result<usize> {
    let name = cfg.name.as_str();
    let ids = spool.list_ids("running");
    for id in &ids {
        let rec = std::fs::read(spool.job_path("running", id))
            .ok()
            .and_then(|b| serde_json::from_slice::<JobRecord>(&b).ok());
        let res = JobResult {
            id: id.clone(),
            rc: RC_CANCELLED,
            started_at: None,
            finished_at: Utc::now(),
            reason: "orphaned: the deputy died while this job ran; it was not re-run".into(),
            tty: false,
            policy_entry: None,
            output: spool.out_path(id),
        };
        // The deputy may have died before opening the .out file; append so
        // any partial output is kept and the result never names a missing file.
        // trace:TASK-2609 | ai:claude
        write_out_note(
            &res.output,
            "\ndeputy died while this job ran; it was not re-run.\n",
        );
        spool.write_result("failed", &res)?;
        let mut row = LedgerRow::new(name, id, "orphaned");
        row.requester = rec.as_ref().map(|r| r.requester.clone());
        row.rc = Some(res.rc);
        row.reason = Some(res.reason.clone());
        spool.ledger(&row)?;
        std::fs::rename(spool.job_path("running", id), spool.job_path("failed", id))?;
        clear_job_markers(spool, id);
        notify(spool, cfg, rec.as_ref(), &res);
    }
    Ok(ids.len())
}

/// Remove a job's terminal, waiting and cancel markers once it has left
/// `running/` (or never will reach it).
fn clear_job_markers(spool: &Spool, id: &str) {
    let _ = std::fs::remove_file(spool.tty_marker_path(id));
    let _ = std::fs::remove_file(spool.waiting_marker_path(id));
    let _ = std::fs::remove_file(spool.cancel_request_path(id));
}

/// Honour cancel requests for jobs still queued, and drop requests for jobs
/// that are neither queued nor running (already finished, or unknown).
/// Running jobs are cancelled by their own wait loop.
fn sweep_cancel_requests(spool: &Spool, cfg: &WorkerConfig, active: &Arc<Mutex<Active>>) {
    for id in spool.cancel_request_ids() {
        let queued = spool.job_path("incoming", &id);
        // A queued file reusing a known id is a forgery the claim round
        // refuses; cancelling it here would file it over the real record.
        if queued.exists() && spool.id_in_use(&id).is_none() {
            let rec = read_job_file(&queued, &id).ok();
            let res = JobResult {
                id: id.clone(),
                rc: RC_CANCELLED,
                started_at: None,
                finished_at: Utc::now(),
                reason: "cancelled before it ran".into(),
                tty: false,
                policy_entry: None,
                output: spool.out_path(&id),
            };
            if std::fs::rename(&queued, spool.job_path("failed", &id)).is_ok() {
                // trace:BUG-B6B1278890774EF993D2EBC327862470-150 | ai:claude
                write_out_note(&res.output, "deputy cancelled this job before it ran.\n");
                let _ = spool.write_result("failed", &res);
                let mut row = LedgerRow::new(&cfg.name, &id, "cancelled");
                row.requester = rec.as_ref().map(|r| r.requester.clone());
                row.rc = Some(res.rc);
                row.reason = Some(res.reason.clone());
                let _ = spool.ledger(&row);
                notify(spool, cfg, rec.as_ref(), &res);
                bump_handled(active);
            }
            let _ = std::fs::remove_file(spool.cancel_request_path(&id));
        } else if !spool.job_path("running", &id).exists() {
            let _ = std::fs::remove_file(spool.cancel_request_path(&id));
        }
    }
}

/// The terminal a claimed job gets, if any. A single-use entry fails CLOSED:
/// unless its use is recorded in the saved policy first, the job runs with no
/// terminal and the entry stays unused.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
pub(crate) fn claim_terminal(
    spool: &Spool,
    cfg_name: &str,
    id: &str,
    decision: TtyDecision,
    now: DateTime<Utc>,
) -> Option<(String, DateTime<Utc>)> {
    let TtyDecision::Tty {
        entry,
        expires_at,
        single_use,
    } = decision
    else {
        return None;
    };
    if single_use {
        if let Err(e) = policy::consume_single_use(spool, &entry, id, now) {
            eprintln!(
                "deputy {cfg_name}: job {id} runs without a terminal: could not use single-use TTY entry {entry}: {e:#}"
            );
            let mut row = LedgerRow::new(cfg_name, id, "policy");
            row.policy_entry = Some(entry);
            row.reason = Some(format!("single-use entry not used, no terminal: {e:#}"));
            let _ = spool.ledger(&row);
            return None;
        }
    }
    Some((entry, expires_at))
}

/// Run the claim loop until `stop` is set (or `max_jobs` is reached).
pub(crate) fn run(spool: &Spool, cfg: &WorkerConfig, stop: &AtomicBool) -> Result<usize> {
    spool.ensure()?;
    let _lock = acquire_lock(spool)?;
    recover_orphans(spool, cfg)?;
    let active = Arc::new(Mutex::new(Active::default()));
    let mut handles = Vec::new();
    // Once set, the deputy claims nothing more and exits when its running
    // jobs finish: the grant or the mandate it ran under has ended.
    // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
    let mut stopping: Option<String> = None;
    let mut was_paused = false;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let running = {
            let a = active.lock().unwrap_or_else(|p| p.into_inner());
            if cfg.max_jobs.is_some_and(|m| a.handled >= m) && a.running == 0 {
                break;
            }
            a.running
        };
        if stopping.is_none() {
            stopping = stop_reason(cfg);
            if let Some(why) = &stopping {
                eprintln!(
                    "deputy {}: claiming no new jobs: {why}. It exits once its {running} running job(s) finish. Renew in this shell (`aida role enter <seat>`), then start `aida deputy` again.",
                    cfg.name
                );
            }
        }
        if let Some(why) = &stopping {
            if running == 0 {
                let mut row = LedgerRow::new(&cfg.name, "-", "stopped");
                row.reason = Some(why.clone());
                let _ = spool.ledger(&row);
                eprintln!("deputy {}: stopped: {why}", cfg.name);
                break;
            }
            std::thread::sleep(cfg.poll);
            continue;
        }
        let paused = spool.paused().is_some();
        if paused != was_paused {
            let event = if paused { "paused" } else { "resumed" };
            let _ = spool.ledger(&LedgerRow::new(&cfg.name, "-", event));
            eprintln!(
                "deputy {}: {}",
                cfg.name,
                if paused {
                    "paused: claiming no new jobs (running jobs finish); `aida deputy resume` at your terminal resumes"
                } else {
                    "resumed"
                }
            );
            was_paused = paused;
        }
        if paused {
            // Cancels of queued jobs still apply; nothing new is claimed.
            sweep_cancel_requests(spool, cfg, &active);
        } else if let GrantState::Valid { expires_at } = (cfg.grant)() {
            if let Err(e) = spool.check_owned() {
                eprintln!(
                    "deputy {}: spool check failed, not claiming: {e:#}",
                    cfg.name
                );
            } else {
                claim_round(spool, cfg, &active, expires_at, &mut handles);
            }
        }
        std::thread::sleep(cfg.poll);
    }
    for h in handles {
        let _ = h.join();
    }
    let handled = active.lock().unwrap_or_else(|p| p.into_inner()).handled;
    Ok(handled)
}

/// Why the deputy must stop claiming, if it must: the grant is invalid or
/// past its expiry, or the mandate it started under has ended. A grant that
/// says it is valid but whose expiry has passed counts as expired, so a
/// cached probe never extends it.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
pub(crate) fn stop_reason(cfg: &WorkerConfig) -> Option<String> {
    match (cfg.grant)() {
        GrantState::Invalid(why) => return Some(format!("the operator's grant ended: {why}")),
        GrantState::Valid {
            expires_at: Some(at),
        } if at <= Utc::now() => {
            return Some(format!(
                "the operator's grant expired at {}",
                at.to_rfc3339()
            ))
        }
        GrantState::Valid { .. } => {}
    }
    (cfg.mandate)()
}

fn claim_round(
    spool: &Spool,
    cfg: &WorkerConfig,
    active: &Arc<Mutex<Active>>,
    grant_expires_at: Option<DateTime<Utc>>,
    handles: &mut Vec<std::thread::JoinHandle<()>>,
) {
    sweep_cancel_requests(spool, cfg, active);
    for id in spool.list_ids("incoming") {
        // Re-checked before every claim, so a pause or an expiry that lands
        // mid-round takes effect at the next job, not the next round.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        if spool.paused().is_some() || stop_reason(cfg).is_some() {
            return;
        }
        let worker_idx = {
            let a = active.lock().unwrap_or_else(|p| p.into_inner());
            if a.running >= cfg.workers {
                return;
            }
            a.running
        };
        let path = spool.job_path("incoming", &id);
        // Before anything else: never let a submission reuse an id the
        // deputy has already claimed (security floor, advisor ruling
        // 2026-10-31). Every other refusal files into `failed/<id>`, which
        // for a reused id would overwrite the real job's record.
        if let Some(where_) = spool.id_in_use(&id) {
            refuse_duplicate(spool, cfg, &id, where_);
            bump_handled(active);
            continue;
        }
        let rec = match read_job_file(&path, &id) {
            Ok(rec) => rec,
            Err(refusal) => {
                refuse(spool, cfg, &id, None, "incoming", &refusal.code());
                bump_handled(active);
                continue;
            }
        };
        if let Some(class) = &rec.serial {
            if active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .serial
                .contains(class)
            {
                continue;
            }
        }
        if let Err(why) = authorize_exec(&rec.body, cfg) {
            refuse(spool, cfg, &id, Some(&rec), "incoming", &why);
            bump_handled(active);
            continue;
        }
        // Slice-3 hooks, stubbed to refuse what they cannot judge yet.
        // trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        if let super::hooks::HookVerdict::Refuse(why) = super::hooks::check(spool, &rec) {
            refuse(spool, cfg, &id, Some(&rec), "incoming", &why);
            bump_handled(active);
            continue;
        }
        let job_file = match spool.claim(&id) {
            Ok(identity) => identity,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                refuse_duplicate(spool, cfg, &id, "running");
                bump_handled(active);
                continue;
            }
            Err(_) => continue, // another claimer won it
        };
        let mut row = LedgerRow::new(&cfg.name, &id, "claimed");
        row.requester = Some(rec.requester.clone());
        row.summary = Some(rec.body.summary());
        row.sha256 = Some(rec.sha256.clone());
        row.worker = Some(worker_idx);
        let _ = spool.ledger(&row);

        let now = Utc::now();
        // A policy that cannot be read (or carries rules this deputy does
        // not know) refuses the job: running it detached would skip whatever
        // that policy says about it. trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
        let decision = match policy::load(spool) {
            Ok(p) => p.decide(&rec.body, now, grant_expires_at),
            Err(e) => {
                refuse(
                    spool,
                    cfg,
                    &id,
                    Some(&rec),
                    "running",
                    &format!("refused:policy-unreadable ({e:#})"),
                );
                bump_handled(active);
                continue;
            }
        };
        let tty = match decision {
            TtyDecision::RefuseBypass(flag) => {
                refuse(
                    spool,
                    cfg,
                    &id,
                    Some(&rec),
                    "running",
                    &format!("refused:bypass-flag {flag} never runs on the operator's terminal"),
                );
                bump_handled(active);
                continue;
            }
            decision => claim_terminal(spool, &cfg.name, &id, decision, now),
        };
        {
            let mut a = active.lock().unwrap_or_else(|p| p.into_inner());
            a.running += 1;
            if let Some(class) = &rec.serial {
                a.serial.insert(class.clone());
            }
        }
        let spool = spool.clone();
        let cfg = cfg.clone();
        let active = Arc::clone(active);
        handles.push(std::thread::spawn(move || {
            let res = run_job(&spool, &cfg, &rec, tty, job_file, worker_idx);
            if let Err(e) = res {
                eprintln!(
                    "deputy {}: job {} bookkeeping failed: {e:#}",
                    cfg.name, rec.id
                );
            }
            let mut a = active.lock().unwrap_or_else(|p| p.into_inner());
            a.running -= 1;
            a.handled += 1;
            if let Some(class) = &rec.serial {
                a.serial.remove(class);
            }
        }));
    }
}

fn bump_handled(active: &Arc<Mutex<Active>>) {
    active.lock().unwrap_or_else(|p| p.into_inner()).handled += 1;
}

/// Move a job from `from` straight to `failed/` with a refusal result and a
/// ledger row (AC2).
fn refuse(
    spool: &Spool,
    cfg: &WorkerConfig,
    id: &str,
    rec: Option<&JobRecord>,
    from: &str,
    reason: &str,
) {
    let res = JobResult {
        id: id.to_string(),
        rc: RC_REFUSED,
        started_at: None,
        finished_at: Utc::now(),
        reason: reason.to_string(),
        tty: false,
        policy_entry: None,
        output: spool.out_path(id),
    };
    // The result names an output file, so write one: the refusal, the job
    // it refused, and what would let it run. trace:BUG-B6B1278890774EF993D2EBC327862470-150 | ai:claude
    write_out_note(&res.output, &refusal_note(reason, rec));
    let _ = spool.write_result("failed", &res);
    let mut row = LedgerRow::new(&cfg.name, id, "refused");
    row.requester = rec.map(|r| r.requester.clone());
    row.summary = rec.map(|r| r.body.summary());
    row.sha256 = rec.map(|r| r.sha256.clone());
    row.rc = Some(res.rc);
    row.reason = Some(reason.to_string());
    let _ = spool.ledger(&row);
    let _ = std::fs::rename(spool.job_path(from, id), spool.job_path("failed", id));
    eprintln!("deputy {}: refused job {id}: {reason}", cfg.name);
    notify(spool, cfg, rec, &res);
}

/// Append `text` to a job's output file, creating it (0600) if needed. Best
/// effort: the result and ledger row are the record of the job.
// trace:BUG-B6B1278890774EF993D2EBC327862470-150 | ai:claude
fn write_out_note(path: &Path, text: &str) {
    if let Ok(mut f) = open_out(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// The output-file text for a refused job: the refusal, the job, the remedy.
pub(crate) fn refusal_note(reason: &str, rec: Option<&JobRecord>) -> String {
    let mut note = format!("deputy refused this job; it did not run.\n{reason}\n");
    if let Some(rec) = rec {
        note.push_str(&format!("job: {}\n", rec.body.summary()));
    }
    if let Some(fix) = refusal_remedy(reason) {
        note.push_str(&format!("to run it: {fix}\n"));
    }
    note
}

fn refusal_remedy(reason: &str) -> Option<&'static str> {
    let code = reason.strip_prefix("refused:")?;
    let code = code.split([' ', '(']).next().unwrap_or_default();
    Some(match code {
        "exec-not-allowed" => {
            "run it through `aida`, or restart the deputy with `aida deputy --allow-exec <path>` naming the executable by path"
        }
        "script-jobs-disabled" => "restart the deputy with `aida deputy --allow-scripts`",
        "bypass-flag" => "run the command yourself in your own terminal",
        "empty-argv" => "submit a command: `aida deputy submit -- <command> [args]`",
        "policy-unreadable" => "fix the deputy's policy file, then submit the job again",
        "command-policy-unsupported" => "remove the command policy file, then submit the job again",
        _ => return None,
    })
}

/// Mail the requester a job's result (AC2 of slice 2), and ledger whether it
/// went. A failed send never fails the job: the result is already filed.
// trace:STORY-B6B1278890774EF993D2EBC327862470-155 | ai:claude
fn notify(spool: &Spool, cfg: &WorkerConfig, rec: Option<&JobRecord>, res: &JobResult) {
    let Some(root) = &cfg.mail_root else {
        return;
    };
    let mut row = LedgerRow::new(&cfg.name, &res.id, "mailed");
    match super::mail_result(root, &cfg.name, rec, res) {
        Ok(true) => {
            row.summary = rec.and_then(|r| r.requester.mail_to.clone().or(r.requester.role.clone()))
        }
        Ok(false) => row.reason = Some("no requester mail identity on the job".into()),
        Err(e) => {
            eprintln!(
                "deputy {}: could not mail the result of job {}: {e:#}",
                cfg.name, res.id
            );
            row.reason = Some(format!("mail failed: {e:#}"));
        }
    }
    let _ = spool.ledger(&row);
}

/// Refuse a submission that reuses an id already in use (`where_` says
/// where). It is filed beside the real job (`Spool::duplicate_path`) with
/// a ledger row and NO result file, so the real job's record, result and
/// terminal marker are untouched.
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
fn refuse_duplicate(spool: &Spool, cfg: &WorkerConfig, id: &str, where_: &str) {
    let reason = format!("refused:duplicate-id (job id {id} is already in use: {where_})");
    let mut row = LedgerRow::new(&cfg.name, id, "refused");
    row.rc = Some(RC_REFUSED);
    row.reason = Some(reason.clone());
    let _ = spool.ledger(&row);
    let _ = std::fs::rename(spool.job_path("incoming", id), spool.duplicate_path(id));
    eprintln!("deputy {}: refused job {id}: {reason}", cfg.name);
}

/// Q5: argv jobs may run `aida` or an executable the operator named at start;
/// script bodies need `--allow-scripts`.
pub(crate) fn authorize_exec(body: &JobBody, cfg: &WorkerConfig) -> Result<(), String> {
    match body {
        JobBody::Script(_) if cfg.allow_scripts => Ok(()),
        JobBody::Script(_) => Err(
            "refused:script-jobs-disabled (the operator did not start this deputy with --allow-scripts)"
                .into(),
        ),
        JobBody::Argv(argv) => {
            let Some(first) = argv.first() else {
                return Err("refused:empty-argv".into());
            };
            if first == "aida" {
                return Ok(());
            }
            let canon = std::fs::canonicalize(first).ok();
            if canon.is_some_and(|c| cfg.allow_exec.contains(&c)) {
                Ok(())
            } else {
                Err(format!(
                    "refused:exec-not-allowed ({first} is not `aida` or an --allow-exec path)"
                ))
            }
        }
    }
}

fn open_out(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open {}", path.display()))
}

fn build_command(rec: &JobRecord, spool: &Spool, cfg: &WorkerConfig) -> Command {
    let mut cmd = match &rec.body {
        JobBody::Argv(argv) => {
            let program = if argv[0] == "aida" {
                cfg.aida_program.clone()
            } else {
                PathBuf::from(&argv[0])
            };
            let mut c = Command::new(program);
            c.args(&argv[1..]);
            c
        }
        JobBody::Script(body) => {
            let mut c = Command::new("bash");
            c.args(["--noprofile", "--norc", "-c", body]);
            c
        }
    };
    cmd.env(JOB_ENV, &rec.id)
        .env(SPOOL_ENV, &spool.root)
        .env(
            REQUESTER_ENV,
            rec.requester.session_id.as_deref().unwrap_or("unknown"),
        )
        .env_remove(VIA_WRAPPER_ENV);
    cmd
}

/// A pty pair: (master, slave). The slave becomes the job's controlling
/// terminal.
fn open_pty() -> Result<(OwnedFd, OwnedFd)> {
    // SAFETY: plain libc calls on fds we own; every return is checked.
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if master < 0 {
            bail!("posix_openpt: {}", std::io::Error::last_os_error());
        }
        let master = OwnedFd::from_raw_fd(master);
        use std::os::fd::AsRawFd;
        if libc::grantpt(master.as_raw_fd()) != 0 || libc::unlockpt(master.as_raw_fd()) != 0 {
            bail!("grantpt/unlockpt: {}", std::io::Error::last_os_error());
        }
        let name = pty_name(master.as_raw_fd())?;
        let slave = libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if slave < 0 {
            bail!("open pty slave: {}", std::io::Error::last_os_error());
        }
        Ok((master, OwnedFd::from_raw_fd(slave)))
    }
}

#[cfg(target_os = "linux")]
unsafe fn pty_name(fd: libc::c_int) -> Result<std::ffi::CString> {
    let mut buf = vec![0 as libc::c_char; 128];
    // SAFETY: buf is valid for buf.len() bytes.
    if unsafe { libc::ptsname_r(fd, buf.as_mut_ptr(), buf.len()) } != 0 {
        bail!("ptsname_r: {}", std::io::Error::last_os_error());
    }
    // SAFETY: ptsname_r NUL-terminates on success.
    Ok(unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_owned())
}

#[cfg(not(target_os = "linux"))]
unsafe fn pty_name(fd: libc::c_int) -> Result<std::ffi::CString> {
    // SAFETY: ptsname returns a static buffer; we copy it at once.
    let p = unsafe { libc::ptsname(fd) };
    if p.is_null() {
        bail!("ptsname: {}", std::io::Error::last_os_error());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(p) }.to_owned())
}

fn kill_group(child: &mut std::process::Child) {
    let pid = child.id() as libc::pid_t;
    // SAFETY: signalling the job's own process group (it called setsid).
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            // The leader exited; still sweep the group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Run one claimed job to completion and file its result.
pub(crate) fn run_job(
    spool: &Spool,
    cfg: &WorkerConfig,
    rec: &JobRecord,
    tty: Option<(String, DateTime<Utc>)>,
    job_file: (u64, u64),
    worker_idx: usize,
) -> Result<JobResult> {
    let out_path = spool.out_path(&rec.id);
    let mut out = open_out(&out_path)?;
    let started = Utc::now();
    let mut row = LedgerRow::new(&cfg.name, &rec.id, "started");
    row.requester = Some(rec.requester.clone());
    row.summary = Some(rec.body.summary());
    row.worker = Some(worker_idx);
    row.tty = Some(tty.is_some());
    row.policy_entry = tty.as_ref().map(|(e, _)| e.clone());
    row.output = Some(out_path.clone());
    spool.ledger(&row)?;

    let mut cmd = build_command(rec, spool, cfg);
    let last_output = Arc::new(AtomicI64::new(now_ms()));
    let mut reader = None;
    let spawned = if let Some((entry, expires_at)) = &tty {
        write_tty_marker(
            spool,
            &TtyMarker {
                job: rec.id.clone(),
                entry: entry.clone(),
                expires_at: *expires_at,
                // Bound to the file this claim moved into `running/`.
                job_dev: job_file.0,
                job_ino: job_file.1,
            },
        )?;
        let (master, slave) = open_pty()?;
        cmd.stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: setsid and ioctl are async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let spawned = cmd.spawn();
        drop(cmd); // closes our copies of the slave
        let mut master_file = std::fs::File::from(master);
        let mut sink = out.try_clone()?;
        let stamp = Arc::clone(&last_output);
        reader = Some(std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match master_file.read(&mut buf) {
                    Ok(0) | Err(_) => break, // EIO once every slave fd closed
                    Ok(n) => {
                        let _ = sink.write_all(&buf[..n]);
                        stamp.store(now_ms(), Ordering::SeqCst);
                    }
                }
            }
        }));
        spawned
    } else {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(out.try_clone()?))
            .stderr(Stdio::from(out.try_clone()?));
        // SAFETY: setsid is async-signal-safe. A new session has no
        // controlling terminal, so the job cannot reach the operator's TTY.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd.spawn()
    };

    let mut forced: Option<(i32, String)> = None;
    let rc = match spawned {
        Err(e) => {
            let _ = writeln!(out, "deputy: could not start the job: {e}");
            forced = Some((RC_REFUSED, format!("spawn-failed: {e}")));
            RC_REFUSED
        }
        Ok(mut child) => {
            let begun = Instant::now();
            let timeout = Duration::from_secs(rec.timeout_secs.max(1));
            let mut waiting = false;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {}
                    Err(_) => break None,
                }
                if begun.elapsed() > timeout {
                    kill_group(&mut child);
                    forced = Some((RC_TIMEOUT, format!("timeout after {}s", timeout.as_secs())));
                    break None;
                }
                if tty.is_some() {
                    // Display only (advisor ruling 04:24): a silent terminal
                    // job is shown as waiting on a prompt, never stopped for
                    // it. Its own timeout still governs.
                    let idle_ms = now_ms() - last_output.load(Ordering::SeqCst);
                    let idle = idle_ms > cfg.tty_idle.as_millis() as i64;
                    if idle && !waiting {
                        mark_waiting(spool, cfg, rec, worker_idx);
                        waiting = true;
                    } else if !idle && waiting {
                        let _ = std::fs::remove_file(spool.waiting_marker_path(&rec.id));
                        waiting = false;
                    }
                }
                if spool.cancel_requested(&rec.id) {
                    kill_group(&mut child);
                    forced = Some((RC_CANCELLED, "cancelled".into()));
                    break None;
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            let status = match status {
                Some(s) => Some(s),
                None => child.wait().ok(),
            };
            match (&forced, status) {
                (Some((rc, _)), _) => *rc,
                (None, Some(s)) => exit_code(s),
                (None, None) => RC_CANCELLED,
            }
        }
    };
    if let Some(r) = reader {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !r.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if r.is_finished() {
            let _ = r.join();
        }
    }
    let reason = match &forced {
        Some((_, why)) => why.clone(),
        None if rc == 0 => "ok".into(),
        None => "exit".into(),
    };
    if let Some((_, why)) = &forced {
        let _ = writeln!(out, "\n[deputy] {why}");
    }
    let res = JobResult {
        id: rec.id.clone(),
        rc,
        started_at: Some(started),
        finished_at: Utc::now(),
        reason: reason.clone(),
        tty: tty.is_some(),
        policy_entry: tty.as_ref().map(|(e, _)| e.clone()),
        output: out_path.clone(),
    };
    let state = if rc == 0 { "done" } else { "failed" };
    spool.write_result(state, &res)?;
    let mut row = LedgerRow::new(&cfg.name, &rec.id, "finished");
    row.requester = Some(rec.requester.clone());
    row.summary = Some(rec.body.summary());
    row.sha256 = Some(rec.sha256.clone());
    row.worker = Some(worker_idx);
    row.tty = Some(res.tty);
    row.policy_entry = res.policy_entry.clone();
    row.rc = Some(rc);
    row.reason = Some(reason);
    row.output = Some(out_path);
    spool.ledger(&row)?;
    std::fs::rename(
        spool.job_path("running", &rec.id),
        spool.job_path(state, &rec.id),
    )?;
    clear_job_markers(spool, &rec.id);
    notify(spool, cfg, Some(rec), &res);
    Ok(res)
}

/// Record that a terminal job has gone quiet: a `running/<id>.waiting` marker
/// for `aida deputy status`, and one ledger row.
fn mark_waiting(spool: &Spool, cfg: &WorkerConfig, rec: &JobRecord, worker_idx: usize) {
    let marker = WaitingMarker {
        since: Utc::now(),
        idle_secs: cfg.tty_idle.as_secs(),
    };
    let path = spool.waiting_marker_path(&rec.id);
    let _ = std::fs::remove_file(&path);
    if let Ok(bytes) = serde_json::to_vec(&marker) {
        let _ = write_private_new(&path, &bytes);
    }
    let mut row = LedgerRow::new(&cfg.name, &rec.id, "waiting");
    row.requester = Some(rec.requester.clone());
    row.worker = Some(worker_idx);
    row.tty = Some(true);
    row.reason = Some(format!(
        "waiting on a prompt: no terminal output for {}s",
        cfg.tty_idle.as_secs()
    ));
    let _ = spool.ledger(&row);
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(RC_CANCELLED)
}
