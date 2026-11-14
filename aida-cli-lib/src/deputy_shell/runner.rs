//! The script deputy's runner: claim ONE job at a time from the queue, check
//! who submitted it, show the script on the operator's terminal, run it as a
//! child `bash --noprofile --norc <script.sh>` on a fresh pty (its own
//! session, the pty its controlling terminal and fds 0-2), relay the
//! operator's terminal to it, tee every byte to the job's transcript, and
//! file the result. Between jobs it refreshes the grant on a clock, prunes
//! old jobs, and honours pause, cancel, the mandate and the grant's end.
// trace:STORY-2338 | ai:claude

use anyhow::{Context, Result};
use chrono::{DateTime, Duration as CDuration, Utc};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::attribution::{self, Probes, Refusal};
use super::{
    script_digest, Attribution, Clock, JobState, Queue, RunnerInfo, RunnerRef, ShellJob,
    ShellState, JOB_ENV, NAME_ENV, OPERATOR_TTY, RC_CANCELLED, RC_REFUSED, RC_TIMEOUT, SCHEMA,
};
use crate::deputy::worker::{GrantState, MandateProbe};

/// The validity of a grant id, as the runner sees it.
pub(crate) type GrantProbe = Arc<dyn Fn(&str) -> GrantState + Send + Sync>;
/// Re-mint the runner's grant; returns the new grant id.
pub(crate) type Minter = Arc<dyn Fn() -> Result<String> + Send + Sync>;

/// A grant with less than this left is refreshed at the next gap.
pub(crate) const REFRESH_BEFORE_EXPIRY: CDuration = CDuration::hours(1);
/// Idle pruning cadence.
const PRUNE_EVERY: CDuration = CDuration::hours(1);

/// The operator's terminal: where the script is shown and the job relayed.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Terminal {
    pub input: RawFd,
    pub output: RawFd,
}

#[derive(Clone)]
pub(crate) struct RunnerConfig {
    pub project_root: PathBuf,
    /// The grant the runner starts under (the operator's shell's).
    pub grant_id: String,
    pub grant: GrantProbe,
    pub mint: Minter,
    /// `--refresh`: re-mint once this long has passed since the last mint.
    pub refresh: CDuration,
    pub submit_seats: Vec<String>,
    pub keep_days: u32,
    pub mandate: MandateProbe,
    pub probes: Probes,
    pub clock: Clock,
    pub poll: Duration,
    /// SIGTERM-to-SIGKILL grace when a job is stopped.
    pub kill_grace: Duration,
    /// Mail each submitter its result.
    pub mail: bool,
    /// `None` in tests: nothing is shown or relayed, the transcript still is.
    pub terminal: Option<Terminal>,
    /// A job's working directory (`$HOME`).
    pub job_home: PathBuf,
    pub bash: PathBuf,
    /// Tests: stop once this many jobs were handled (run, refused, cancelled).
    pub max_jobs: Option<usize>,
    /// Tests: stop after this many idle rounds.
    pub max_rounds: Option<usize>,
    /// Serve `submit.sock`, the only way a job reaches the queue. Off only in
    /// tests that file accepted jobs directly.
    // trace:STORY-2343 | ai:claude
    pub serve_socket: bool,
}

/// Holds the single-runner lock for one deputy name.
pub(crate) struct RunnerLock {
    _file: std::fs::File,
}

pub(crate) fn acquire_lock(q: &Queue) -> Result<RunnerLock> {
    use fs2::FileExt;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(q.runner_lock())
        .context("open the script deputy's runner lock")?;
    if file.try_lock_exclusive().is_err() {
        let pid = q
            .runner_info()
            .map(|r| r.pid.to_string())
            .unwrap_or_else(|| "unknown".into());
        anyhow::bail!(
            "a script deputy named {:?} is already running (pid {pid}); give this one another --name",
            q.name
        );
    }
    Ok(RunnerLock { _file: file })
}

/// What a finished job came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub rc: i32,
    pub reason: String,
    pub state: JobState,
}

/// Run the loop until `stop` is set, the grant or mandate ends, or a test
/// limit is reached. Returns how many jobs it handled.
pub(crate) fn run(q: &Queue, cfg: &RunnerConfig, stop: &AtomicBool) -> Result<usize> {
    q.ensure()?;
    let _lock = acquire_lock(q)?;
    // Under the lock: a stale socket belongs to a dead runner.
    // trace:STORY-2571 | ai:codex
    let service_host = crate::review_watch::service::HostAuthority::new(cfg);
    let _services = cfg
        .serve_socket
        .then(|| crate::review_watch::service::Supervisor::start(q.clone(), service_host.clone()));
    let _train_services = cfg.serve_socket.then(|| {
        crate::review_watch::train_service::Supervisor::start(q.clone(), service_host.clone())
    });
    let server = if cfg.serve_socket {
        Some(super::socket::serve_service(
            q,
            cfg.probes.clone(),
            cfg.clock.clone(),
            Some(service_host.clone()),
        )?)
    } else {
        None
    };
    let me = std::process::id();
    let mut grant_id = cfg.grant_id.clone();
    let now = (cfg.clock)();
    recover_orphans(q, cfg, now);
    write_runner_info(q, cfg, &grant_id, now)?;
    q.audit_event(now, "runner-start", None, &actor(&grant_id), |r| {
        r.detail = Some(serde_json::json!({
            "pid": me,
            "submit_seats": cfg.submit_seats,
            "keep_days": cfg.keep_days,
        }))
    });
    note_legacy_incoming(q, cfg, &grant_id, now);
    q.prune(now, cfg.keep_days);
    let mut last_prune = now;
    let mut last_refresh = now;
    let mut refresh_failed = false;
    let mut was_paused = false;
    let mut handled = 0usize;
    let mut rounds = 0usize;
    let stop_why: String = loop {
        if stop.load(Ordering::SeqCst) {
            break "stopped at the operator's terminal".into();
        }
        if cfg.max_jobs.is_some_and(|m| handled >= m) {
            break "test limit reached".into();
        }
        if cfg.max_rounds.is_some_and(|m| rounds >= m) {
            break "test limit reached".into();
        }
        rounds += 1;
        let now = (cfg.clock)();
        // The grant: valid, unexpired, else stop.
        let expires_at = match (cfg.grant)(&grant_id) {
            GrantState::Invalid(why) => break format!("the operator's grant ended: {why}"),
            GrantState::Valid { expires_at } => {
                if let Some(at) = expires_at.filter(|at| *at <= now) {
                    break format!("the operator's grant expired at {}", at.to_rfc3339());
                }
                expires_at
            }
        };
        // Time-based refresh, between jobs only.
        if !refresh_failed {
            let due = now - last_refresh >= cfg.refresh
                || expires_at.is_some_and(|at| at - now < REFRESH_BEFORE_EXPIRY);
            if due {
                match (cfg.mint)() {
                    Ok(new_id) => {
                        let old = actor(&grant_id);
                        q.audit_event(now, "role-refreshed", None, &old, |r| {
                            r.detail = Some(serde_json::json!({
                                "old": prefix(&grant_id),
                                "new": prefix(&new_id),
                            }))
                        });
                        grant_id = new_id;
                        service_host.refreshed(&grant_id);
                        last_refresh = now;
                        let _ = write_runner_info(q, cfg, &grant_id, now);
                        say(
                            cfg,
                            &format!("[deputy shell] grant refreshed: {}\r\n", prefix(&grant_id)),
                        );
                    }
                    Err(e) => {
                        refresh_failed = true;
                        q.audit_event(now, "role-refresh-failed", None, &actor(&grant_id), |r| {
                            r.reason = Some(format!("{e:#}"))
                        });
                        say(
                            cfg,
                            &format!(
                                "[deputy shell] could not refresh the grant: {e:#}. Claiming no more jobs; the runner exits when the grant expires.\r\n"
                            ),
                        );
                    }
                }
            }
        }
        if let Some(why) = (cfg.mandate)() {
            break why;
        }
        let paused = q.paused();
        if paused != was_paused {
            q.audit_event(
                now,
                if paused { "paused" } else { "resumed" },
                None,
                OPERATOR_TTY,
                |_| {},
            );
            was_paused = paused;
        }
        if now - last_prune >= PRUNE_EVERY {
            q.prune(now, cfg.keep_days);
            last_prune = now;
        }
        handled += sweep_cancels(q, cfg, now);
        if !paused && !refresh_failed {
            match q.check() {
                Err(e) => eprintln!(
                    "deputy shell {}: queue check failed, not claiming: {e:#}",
                    q.name
                ),
                Ok(()) => {
                    if let Some(id) = q.queued_ids().into_iter().next() {
                        handle(q, cfg, &id, &grant_id, stop);
                        handled += 1;
                        continue;
                    }
                }
            }
        }
        std::thread::sleep(cfg.poll);
    };
    drop(server);
    let now = (cfg.clock)();
    q.audit_event(now, "runner-stop", None, &actor(&grant_id), |r| {
        r.reason = Some(stop_why.clone())
    });
    let _ = std::fs::remove_file(q.runner_json());
    say(cfg, &format!("[deputy shell] stopped: {stop_why}\r\n"));
    Ok(handled)
}

fn prefix(grant_id: &str) -> String {
    grant_id.chars().take(8).collect()
}

fn actor(grant_id: &str) -> String {
    format!("{} {}", prefix(grant_id), super::RUNNER_SEAT)
}

fn write_runner_info(
    q: &Queue,
    cfg: &RunnerConfig,
    grant_id: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let pid = std::process::id();
    let info = RunnerInfo {
        pid,
        pid_start: aida_core::liveness::process_start_identity(pid),
        tty: cfg.terminal.and_then(|t| tty_name(t.input)),
        boot_id: cfg.probes.host.boot_id.clone(),
        grant_prefix: prefix(grant_id),
        submit_seats: cfg.submit_seats.clone(),
        started_at: now,
    };
    let tmp = q.root.join(format!(".runner.{}.tmp", uuid::Uuid::new_v4()));
    crate::deputy::write_private_new(&tmp, &serde_json::to_vec_pretty(&info)?)?;
    std::fs::rename(&tmp, q.runner_json()).context("write runner.json")?;
    Ok(())
}

fn tty_name(fd: RawFd) -> Option<String> {
    std::fs::read_link(format!("/proc/self/fd/{fd}"))
        .ok()
        .map(|p| p.display().to_string())
}

/// Write `text` to the operator's terminal, if there is one.
/// Jobs left in the retired submitter-writable `incoming/` directory (from
/// before submit moved to the socket) are never claimed: nothing vouches for
/// who wrote them. Say so once at start, on the terminal and in the audit, so
/// they do not simply vanish; `status`/`wait` report them as unknown.
// trace:STORY-2343 | ai:claude
fn note_legacy_incoming(q: &Queue, cfg: &RunnerConfig, grant_id: &str, now: DateTime<Utc>) {
    let legacy = q.root.join("incoming");
    let ids: Vec<String> = std::fs::read_dir(&legacy)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    if ids.is_empty() {
        return;
    }
    q.audit_event(
        now,
        "legacy-incoming-ignored",
        None,
        &actor(grant_id),
        |r| {
            r.reason = Some(format!(
            "{} file(s) in the retired incoming/ directory were not claimed; resubmit those jobs",
            ids.len()
        ));
            r.detail = Some(serde_json::json!({ "dir": legacy, "files": ids }));
        },
    );
    say(
        cfg,
        &format!(
            "[deputy shell] {} job file(s) in {} predate the socket submit and will never run; resubmit them, then remove that directory\r\n",
            ids.len(),
            legacy.display()
        ),
    );
}

fn say(cfg: &RunnerConfig, text: &str) {
    if let Some(t) = cfg.terminal {
        write_fd(t.output, text.as_bytes());
    }
}

fn write_fd(fd: RawFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: writing a byte slice we own to an fd the caller owns.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n <= 0 {
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        bytes = &bytes[n as usize..];
    }
}

/// Jobs left `running` by a runner that died are done, rc 130, `orphaned`.
/// They are never re-run: a script may not be idempotent. Call under the lock.
pub(crate) fn recover_orphans(q: &Queue, cfg: &RunnerConfig, now: DateTime<Utc>) -> usize {
    let mut n = 0;
    for id in q.job_ids() {
        let Some(mut state) = q.read_state(&id) else {
            continue;
        };
        if state.state != JobState::Running {
            continue;
        }
        state.state = JobState::Done;
        state.rc = Some(RC_CANCELLED);
        state.reason = Some("orphaned".into());
        state.finished_at = Some(now);
        let _ = q.write_state(&id, &state);
        let job = q.read_job(&id);
        q.audit_event(
            now,
            "orphaned",
            Some(&id),
            &job.as_ref()
                .map_or_else(|| "unknown".into(), |j| j.submitter.actor()),
            |r| {
                r.rc = Some(RC_CANCELLED);
                r.reason =
                    Some("orphaned: the runner died while this job ran; it was not re-run".into());
            },
        );
        if let Some(job) = &job {
            notify(q, cfg, job, &state);
        }
        n += 1;
    }
    n
}

/// Cancel requests: a queued job becomes `cancelled` (claimed into `jobs/`
/// without running); a request for a job that is neither queued nor running
/// is dropped. A running job is cancelled by its own wait loop.
fn sweep_cancels(q: &Queue, cfg: &RunnerConfig, now: DateTime<Utc>) -> usize {
    let mut n = 0;
    for id in q.cancel_ids() {
        if q.accepted_job(&id).exists() {
            if let Some(Claimed { job, queued_at }) = claim(q, &id, now) {
                let job = match job {
                    Ok(job) => job,
                    Err(why) => {
                        file_schema_refusal(q, &id, queued_at, now, &why);
                        n += 1;
                        continue;
                    }
                };
                let mut state = ShellState::queued(queued_at);
                state.state = JobState::Cancelled;
                state.claimed_at = Some(now);
                state.finished_at = Some(now);
                state.rc = Some(RC_CANCELLED);
                state.reason = Some("cancelled".into());
                let _ = q.write_state(&id, &state);
                let _ = std::fs::remove_file(q.cancel_path(&id));
                q.audit_event(
                    now,
                    "cancel-requested",
                    Some(&id),
                    &job.submitter.actor(),
                    |_| {},
                );
                q.audit_event(now, "cancelled", Some(&id), &job.submitter.actor(), |r| {
                    r.sha256 = Some(job.sha256.clone());
                    r.rc = Some(RC_CANCELLED);
                    r.reason = Some("cancelled before it ran".into());
                });
                notify(q, cfg, &job, &state);
                n += 1;
            }
        } else if !q
            .read_state(&id)
            .is_some_and(|s| s.state == JobState::Running)
        {
            let _ = std::fs::remove_file(q.cancel_path(&id));
        }
    }
    n
}

struct Claimed {
    /// The parsed record, or why it is refused as `refused:schema`.
    job: Result<ShellJob, String>,
    queued_at: DateTime<Utc>,
}

/// Move `accepted/<id>.job` to a NEW `jobs/<id>/job.json` (a link, then an
/// unlink: nothing is ever replaced), then read it as data. `None` when the
/// job vanished (another claimer). An id that already has a `jobs/<id>/` is
/// a collision: the submission is filed aside and refused.
fn claim(q: &Queue, id: &str, now: DateTime<Utc>) -> Option<Claimed> {
    use std::os::unix::fs::DirBuilderExt;
    let from = q.accepted_job(id);
    let dir = q.job_dir(id);
    if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            let aside = q
                .jobs()
                .join(format!(".{id}.collision-{}.job", uuid::Uuid::now_v7()));
            let _ = std::fs::rename(&from, &aside);
            let why = format!(
                "job id {id} is already in use; the submission was filed at {}",
                aside.display()
            );
            // A job dir with no state.json is what an interrupted claim
            // leaves: file the refusal there, so `wait` returns 126 and
            // `status` shows it. A dir WITH a state belongs to a real job,
            // which keeps its own record; the refusal is audited only.
            // trace:STORY-b6b1278890774ef993d2ebc327862470-334.ac930a33 | ai:claude
            if q.read_state(id).is_none() {
                file_schema_refusal(q, id, now, now, &why);
            } else {
                q.audit_event(now, "refused", Some(id), "unknown", |r| {
                    r.rc = Some(RC_REFUSED);
                    r.reason = Some(format!("refused:schema: {why}"));
                });
                eprintln!(
                    "deputy shell {}: refused job {id}: its id is already in use",
                    q.name
                );
            }
        }
        return None;
    }
    let to = q.job_json(id);
    if std::fs::hard_link(&from, &to).is_err() {
        let _ = std::fs::remove_dir(&dir);
        return None;
    }
    let _ = std::fs::remove_file(&from);
    let job = read_claimed(&to, id, &q.name);
    let queued_at = job.as_ref().map_or(now, |j| j.created_at);
    // The socket server accepted it; the `queued` row is written at claim.
    if let Ok(job) = &job {
        q.audit_event(
            job.created_at,
            "queued",
            Some(id),
            &job.submitter.actor(),
            |r| {
                r.sha256 = Some(job.sha256.clone());
                r.detail = job
                    .source
                    .as_ref()
                    .map(|s| serde_json::json!({ "source": s }));
            },
        );
    }
    Some(Claimed { job, queued_at })
}

/// Inspect a claimed record without following links, then parse it
/// strictly: an unknown field, another schema, a mismatched id or deputy, or
/// a sha256 that is not the script's is a refusal.
fn read_claimed(path: &Path, id: &str, name: &str) -> Result<ShellJob, String> {
    use std::os::unix::fs::MetadataExt;
    let md = std::fs::symlink_metadata(path).map_err(|e| format!("unreadable ({e})"))?;
    if !md.file_type().is_file() {
        return Err("not a regular file".into());
    }
    if md.uid() != crate::deputy::current_uid() {
        return Err("not owned by you".into());
    }
    if md.mode() & 0o022 != 0 {
        return Err("writable by group or others".into());
    }
    if md.nlink() > 1 {
        return Err("hard-linked elsewhere".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("unreadable ({e})"))?;
    let job: ShellJob = serde_json::from_slice(&bytes).map_err(|e| format!("malformed ({e})"))?;
    if job.schema != SCHEMA {
        return Err(format!(
            "schema {} (this deputy reads {SCHEMA})",
            job.schema
        ));
    }
    if job.id != id || job.deputy != name {
        return Err("its id or deputy name does not match where it was queued".into());
    }
    if job.sha256 != script_digest(&job.script) {
        return Err("its sha256 is not the sha256 of its script".into());
    }
    Ok(job)
}

fn file_schema_refusal(
    q: &Queue,
    id: &str,
    queued_at: DateTime<Utc>,
    now: DateTime<Utc>,
    why: &str,
) {
    let mut state = ShellState::queued(queued_at);
    state.state = JobState::Done;
    state.claimed_at = Some(now);
    state.finished_at = Some(now);
    state.rc = Some(RC_REFUSED);
    state.reason = Some("refused:schema".into());
    state.attribution = Some(Attribution {
        verdict: "refused:schema".into(),
        checks: vec![],
    });
    write_note(
        q,
        id,
        &format!("the script deputy refused this job; it did not run.\nrefused:schema: {why}\n"),
    );
    let _ = q.write_state(id, &state);
    q.audit_event(now, "refused", Some(id), "unknown", |r| {
        r.rc = Some(RC_REFUSED);
        r.reason = Some(format!("refused:schema: {why}"));
    });
    eprintln!(
        "deputy shell {}: refused job {id}: refused:schema: {why}",
        q.name
    );
}

fn write_note(q: &Queue, id: &str, text: &str) {
    if let Ok(mut f) = open_transcript(&q.transcript_path(id)) {
        let _ = f.write_all(text.as_bytes());
    }
}

fn open_transcript(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

fn notify(q: &Queue, cfg: &RunnerConfig, job: &ShellJob, state: &ShellState) {
    if !cfg.mail {
        return;
    }
    if let Err(e) = super::mail_result(q, job, state) {
        eprintln!(
            "deputy shell {}: could not mail the result of job {}: {e:#}",
            q.name, job.id
        );
    }
}

/// Claim, judge, show and run job `id`, and file its result.
fn handle(q: &Queue, cfg: &RunnerConfig, id: &str, grant_id: &str, stop: &AtomicBool) {
    let now = (cfg.clock)();
    let Some(Claimed { job, queued_at }) = claim(q, id, now) else {
        return;
    };
    let job = match job {
        Ok(job) => job,
        Err(why) => return file_schema_refusal(q, id, queued_at, now, &why),
    };
    let mut state = ShellState::queued(queued_at);
    state.claimed_at = Some(now);
    state.runner = Some(RunnerRef {
        pid: std::process::id(),
        grant_prefix: prefix(grant_id),
    });
    if q.cancel_requested(id) {
        state.state = JobState::Cancelled;
        state.finished_at = Some(now);
        state.rc = Some(RC_CANCELLED);
        state.reason = Some("cancelled".into());
        let _ = q.write_state(id, &state);
        let _ = std::fs::remove_file(q.cancel_path(id));
        q.audit_event(
            now,
            "cancel-requested",
            Some(id),
            &job.submitter.actor(),
            |_| {},
        );
        q.audit_event(now, "cancelled", Some(id), &job.submitter.actor(), |r| {
            r.sha256 = Some(job.sha256.clone());
            r.rc = Some(RC_CANCELLED);
            r.reason = Some("cancelled before it ran".into());
        });
        notify(q, cfg, &job, &state);
        return;
    }
    let checks = match attribution::verdict(&job, &cfg.submit_seats, &cfg.probes, now) {
        Ok(checks) => checks,
        Err(refusal) => return refuse(q, cfg, &job, state, &refusal, now),
    };
    state.attribution = Some(Attribution {
        verdict: "ok".into(),
        checks,
    });
    q.audit_event(now, "claimed", Some(id), &job.submitter.actor(), |r| {
        r.sha256 = Some(job.sha256.clone())
    });
    // The exact bytes, only now that the verdict passed.
    if let Err(e) = crate::deputy::write_private_new(&q.script_path(id), job.script.as_bytes()) {
        let refusal = Refusal {
            code: "schema",
            why: format!("could not write the script: {e:#}"),
        };
        return refuse(q, cfg, &job, state, &refusal, now);
    }
    state.state = JobState::Running;
    state.started_at = Some((cfg.clock)());
    let _ = q.write_state(id, &state);
    show_header(cfg, &job);
    q.audit_event(
        state.started_at.unwrap_or(now),
        "started",
        Some(id),
        &job.submitter.actor(),
        |r| r.sha256 = Some(job.sha256.clone()),
    );
    let outcome = run_on_pty(q, cfg, &job, grant_id, stop);
    let finished = (cfg.clock)();
    state.state = outcome.state;
    state.rc = Some(outcome.rc);
    state.reason = Some(outcome.reason.clone());
    state.finished_at = Some(finished);
    state.transcript_bytes = std::fs::metadata(q.transcript_path(id))
        .ok()
        .map(|m| m.len());
    let _ = q.write_state(id, &state);
    let event = if outcome.state == JobState::Cancelled {
        "cancelled"
    } else {
        "finished"
    };
    let _ = std::fs::remove_file(q.cancel_path(id));
    if outcome.reason == "cancelled" && !stop.load(Ordering::SeqCst) {
        q.audit_event(
            finished,
            "cancel-requested",
            Some(id),
            &job.submitter.actor(),
            |_| {},
        );
    }
    q.audit_event(finished, event, Some(id), &job.submitter.actor(), |r| {
        r.sha256 = Some(job.sha256.clone());
        r.rc = Some(outcome.rc);
        r.reason = Some(outcome.reason.clone());
    });
    say(
        cfg,
        &format!(
            "\r\n\x1b[1m== script job {} finished: rc {} ({}) ==\x1b[0m\r\n",
            job.id, outcome.rc, outcome.reason
        ),
    );
    notify(q, cfg, &job, &state);
}

fn refuse(
    q: &Queue,
    cfg: &RunnerConfig,
    job: &ShellJob,
    mut state: ShellState,
    refusal: &Refusal,
    now: DateTime<Utc>,
) {
    state.state = JobState::Done;
    state.finished_at = Some(now);
    state.rc = Some(RC_REFUSED);
    state.reason = Some(refusal.reason());
    state.attribution = Some(Attribution {
        verdict: refusal.reason(),
        checks: vec![],
    });
    write_note(
        q,
        &job.id,
        &format!(
            "the script deputy refused this job; it did not run.\n{}: {}\n",
            refusal.reason(),
            refusal.why
        ),
    );
    let _ = q.write_state(&job.id, &state);
    q.audit_event(now, "refused", Some(&job.id), &job.submitter.actor(), |r| {
        r.sha256 = Some(job.sha256.clone());
        r.rc = Some(RC_REFUSED);
        r.reason = Some(format!("{}: {}", refusal.reason(), refusal.why));
    });
    say(
        cfg,
        &format!(
            "[deputy shell] refused script job {}: {}: {}\r\n",
            job.id,
            refusal.reason(),
            refusal.why
        ),
    );
    notify(q, cfg, job, &state);
}

/// Show the job before it runs: id, submitter, sha256, and the script.
fn show_header(cfg: &RunnerConfig, job: &ShellJob) {
    let mut text = format!(
        "\r\n\x1b[1m== script job {} from {} (session {}) ==\x1b[0m\r\nsha256 {}\r\n",
        job.id,
        job.submitter.actor(),
        job.submitter.session_id.as_deref().unwrap_or("?"),
        job.sha256
    );
    for line in job.script.lines() {
        text.push_str(line);
        text.push_str("\r\n");
    }
    text.push_str("-----\r\n");
    say(cfg, &text);
}

/// Raw mode on the operator's terminal for a job's duration; restored on
/// drop (a panic or an early return included).
struct RawMode {
    fd: RawFd,
    saved: Option<libc::termios>,
}

impl RawMode {
    fn enter(fd: RawFd) -> Self {
        // SAFETY: tcgetattr/tcsetattr on an fd we were handed; a non-tty
        // fails tcgetattr and we leave it alone.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut t) != 0 {
                return Self { fd, saved: None };
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(fd, libc::TCSANOW, &t);
            Self {
                fd,
                saved: Some(saved),
            }
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            // SAFETY: restoring the attributes read in `enter`.
            unsafe {
                libc::tcsetattr(self.fd, libc::TCSANOW, &saved);
            }
        }
    }
}

fn winsize(fd: RawFd) -> Option<libc::winsize> {
    // SAFETY: TIOCGWINSZ fills a winsize we own.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        (libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) == 0).then_some(ws)
    }
}

fn set_winsize(fd: RawFd, ws: &libc::winsize) {
    // SAFETY: TIOCSWINSZ reads a winsize we own; the kernel sends SIGWINCH
    // to the pty's foreground group when the size changes.
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ, ws);
    }
}

fn same_size(a: &libc::winsize, b: &libc::winsize) -> bool {
    a.ws_row == b.ws_row && a.ws_col == b.ws_col
}

/// Run the claimed job on a fresh pty and wait for it.
fn run_on_pty(
    q: &Queue,
    cfg: &RunnerConfig,
    job: &ShellJob,
    grant_id: &str,
    stop: &AtomicBool,
) -> Outcome {
    let fail = |why: String| Outcome {
        rc: RC_REFUSED,
        reason: format!("spawn-failed: {why}"),
        state: JobState::Done,
    };
    let transcript = match open_transcript(&q.transcript_path(&job.id)) {
        Ok(f) => f,
        Err(e) => return fail(format!("open the transcript: {e}")),
    };
    let (master, slave) = match crate::deputy::worker::open_pty() {
        Ok(p) => p,
        Err(e) => return fail(format!("{e:#}")),
    };
    let mut last_size = cfg.terminal.and_then(|t| winsize(t.output));
    if let Some(ws) = &last_size {
        set_winsize(slave.as_raw_fd(), ws);
    }
    let mut cmd = Command::new(&cfg.bash);
    cmd.args(["--noprofile", "--norc"])
        .arg(q.script_path(&job.id))
        .current_dir(&cfg.job_home)
        // The operator at their terminal: no command-deputy job marker, so
        // the gate floor treats it as the person (signed Q1).
        .env_remove(crate::deputy::JOB_ENV)
        .env_remove(crate::deputy::SPOOL_ENV)
        .env_remove(crate::deputy::VIA_WRAPPER_ENV)
        .env(crate::gate::seat_authority::GRANT_ENV, grant_id)
        .env(JOB_ENV, &job.id)
        .env(NAME_ENV, &q.name);
    let stdio = || slave.try_clone().map(Stdio::from);
    match (stdio(), stdio(), stdio()) {
        (Ok(i), Ok(o), Ok(e)) => {
            cmd.stdin(i).stdout(o).stderr(e);
        }
        _ => return fail("dup the pty".into()),
    }
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
    let raw = cfg.terminal.map(|t| RawMode::enter(t.input));
    let spawned = cmd.spawn();
    drop(cmd);
    drop(slave);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            drop(raw);
            return fail(e.to_string());
        }
    };
    let mut master_out = std::fs::File::from(master);
    let master_in = master_out.try_clone().ok();
    // Held here for resizes: the master sets the size the job's pty reports.
    let master_ctl = master_out.try_clone().ok();
    let done = Arc::new(AtomicBool::new(false));
    let out_fd = cfg.terminal.map(|t| t.output);
    let mut sink = transcript;
    let reader = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match master_out.read(&mut buf) {
                Ok(0) | Err(_) => break, // EIO once every slave fd closed
                Ok(n) => {
                    let _ = sink.write_all(&buf[..n]);
                    if let Some(fd) = out_fd {
                        write_fd(fd, &buf[..n]);
                    }
                }
            }
        }
    });
    let relay = match (cfg.terminal, master_in) {
        (Some(t), Some(mut master_in)) => {
            let done = Arc::clone(&done);
            Some(std::thread::spawn(move || {
                let mut buf = [0u8; 1024];
                while !done.load(Ordering::SeqCst) {
                    let mut pfd = libc::pollfd {
                        fd: t.input,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // SAFETY: polling one fd we were handed.
                    let r = unsafe { libc::poll(&mut pfd, 1, 100) };
                    if r <= 0 || pfd.revents & libc::POLLIN == 0 {
                        if r > 0 && pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                            break;
                        }
                        continue;
                    }
                    // SAFETY: reading into a buffer we own.
                    let n = unsafe { libc::read(t.input, buf.as_mut_ptr().cast(), buf.len()) };
                    if n <= 0 {
                        break;
                    }
                    if master_in.write_all(&buf[..n as usize]).is_err() {
                        break;
                    }
                }
            }))
        }
        _ => None,
    };
    let begun = Instant::now();
    let timeout = Duration::from_secs(job.timeout_secs.max(1));
    let mut forced: Option<(i32, &str, JobState)> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if begun.elapsed() > timeout {
            kill_group(&mut child, cfg.kill_grace);
            forced = Some((RC_TIMEOUT, "timeout", JobState::Done));
            break None;
        }
        if q.cancel_requested(&job.id) {
            kill_group(&mut child, cfg.kill_grace);
            forced = Some((RC_CANCELLED, "cancelled", JobState::Cancelled));
            break None;
        }
        if stop.load(Ordering::SeqCst) {
            kill_group(&mut child, cfg.kill_grace);
            forced = Some((RC_CANCELLED, "cancelled", JobState::Cancelled));
            break None;
        }
        if let Some(t) = cfg.terminal {
            if let Some(ws) = winsize(t.output) {
                if !last_size.as_ref().is_some_and(|old| same_size(old, &ws)) {
                    if let Some(m) = &master_ctl {
                        set_winsize(m.as_raw_fd(), &ws);
                    }
                    last_size = Some(ws);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let status = status.or_else(|| child.wait().ok());
    // Output still in flight drains; a grandchild holding the pty open does
    // not hold the result.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if reader.is_finished() {
        let _ = reader.join();
    }
    done.store(true, Ordering::SeqCst);
    if let Some(r) = relay {
        let _ = r.join();
    }
    drop(raw);
    match (forced, status) {
        (Some((rc, why, state)), _) => {
            write_note(q, &job.id, &format!("\n[deputy shell] {why}\n"));
            Outcome {
                rc,
                reason: why.into(),
                state,
            }
        }
        (None, Some(s)) => {
            let rc = exit_code(s);
            Outcome {
                rc,
                reason: if rc == 0 { "ok" } else { "exit" }.into(),
                state: JobState::Done,
            }
        }
        (None, None) => Outcome {
            rc: RC_CANCELLED,
            reason: "orphaned".into(),
            state: JobState::Done,
        },
    }
}

/// SIGTERM the job's process group, give it `grace`, then SIGKILL what is
/// left and reap the leader. The group id is the leader's pid, so it is only
/// signalled while it cannot have been recycled: before the leader is reaped
/// (its zombie holds the pid), or after, only while a member still exists
/// (a live session/group member keeps the id in use; ESRCH means none does).
fn kill_group(child: &mut std::process::Child, grace: Duration) {
    let pid = child.id() as libc::pid_t;
    // SAFETY: signalling the job's own process group (it called setsid);
    // the leader is not reaped yet, so the id is still the job's.
    unsafe {
        libc::kill(-pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + grace;
    let mut reaped = false;
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            reaped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !reaped {
        // SAFETY: as above; the unreaped leader still holds the id. SIGKILL
        // first, then reap.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait();
        return;
    }
    // The leader is gone. Probe the group: ESRCH means it is empty and the
    // id may be reused, so nothing is sent.
    // SAFETY: signal 0 only checks for existence.
    let members_left = unsafe { libc::kill(-pid, 0) } == 0;
    if members_left {
        // SAFETY: a member still carries the id as its group, so the kernel
        // cannot have handed it to another process.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(RC_CANCELLED)
}
