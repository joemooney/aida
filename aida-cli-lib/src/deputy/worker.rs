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
    read_job_file, write_tty_marker, JobBody, JobRecord, JobResult, LedgerRow, Spool, TtyMarker,
    JOB_ENV, RC_CANCELLED, RC_NEEDS_OPERATOR, RC_REFUSED, RC_TIMEOUT, REQUESTER_ENV, SPOOL_ENV,
    VIA_WRAPPER_ENV,
};

/// Whether the operator's grant still holds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GrantState {
    Valid { expires_at: Option<DateTime<Utc>> },
    Invalid(String),
}

pub(crate) type GrantProbe = Arc<dyn Fn() -> GrantState + Send + Sync>;

#[derive(Clone)]
pub(crate) struct WorkerConfig {
    pub name: String,
    pub workers: usize,
    /// Canonical paths of executables other than `aida` that argv jobs may run.
    pub allow_exec: Vec<PathBuf>,
    pub allow_scripts: bool,
    /// A terminal job silent this long is treated as waiting on a prompt.
    pub tty_idle: Duration,
    pub poll: Duration,
    pub grant: GrantProbe,
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
pub(crate) fn recover_orphans(spool: &Spool, name: &str) -> Result<usize> {
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
        spool.write_result("failed", &res)?;
        let mut row = LedgerRow::new(name, id, "orphaned");
        row.requester = rec.map(|r| r.requester);
        row.rc = Some(res.rc);
        row.reason = Some(res.reason.clone());
        spool.ledger(&row)?;
        std::fs::rename(spool.job_path("running", id), spool.job_path("failed", id))?;
        let _ = std::fs::remove_file(spool.tty_marker_path(id));
        let _ = std::fs::remove_file(spool.cancel_marker_path(id));
    }
    Ok(ids.len())
}

/// Run the claim loop until `stop` is set (or `max_jobs` is reached).
pub(crate) fn run(spool: &Spool, cfg: &WorkerConfig, stop: &AtomicBool) -> Result<usize> {
    spool.ensure()?;
    let _lock = acquire_lock(spool)?;
    recover_orphans(spool, &cfg.name)?;
    let active = Arc::new(Mutex::new(Active::default()));
    let mut handles = Vec::new();
    let mut last_grant_note: Option<String> = None;
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        {
            let a = active.lock().unwrap_or_else(|p| p.into_inner());
            if cfg.max_jobs.is_some_and(|m| a.handled >= m) && a.running == 0 {
                break;
            }
        }
        match (cfg.grant)() {
            GrantState::Valid { expires_at } => {
                last_grant_note = None;
                if let Err(e) = spool.check_owned() {
                    eprintln!(
                        "deputy {}: spool check failed, not claiming: {e:#}",
                        cfg.name
                    );
                } else {
                    claim_round(spool, cfg, &active, expires_at, &mut handles);
                }
            }
            GrantState::Invalid(why) => {
                if last_grant_note.as_deref() != Some(why.as_str()) {
                    eprintln!(
                        "deputy {}: not claiming new jobs: {why}. Renew in this shell (`aida role enter <seat>`), then start `aida deputy` again.",
                        cfg.name
                    );
                    last_grant_note = Some(why);
                }
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

fn claim_round(
    spool: &Spool,
    cfg: &WorkerConfig,
    active: &Arc<Mutex<Active>>,
    grant_expires_at: Option<DateTime<Utc>>,
    handles: &mut Vec<std::thread::JoinHandle<()>>,
) {
    for id in spool.list_ids("incoming") {
        let worker_idx = {
            let a = active.lock().unwrap_or_else(|p| p.into_inner());
            if a.running >= cfg.workers {
                return;
            }
            a.running
        };
        let path = spool.job_path("incoming", &id);
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
        if std::fs::rename(&path, spool.job_path("running", &id)).is_err() {
            continue; // another claimer won it
        }
        let mut row = LedgerRow::new(&cfg.name, &id, "claimed");
        row.requester = Some(rec.requester.clone());
        row.summary = Some(rec.body.summary());
        row.sha256 = Some(rec.sha256.clone());
        row.worker = Some(worker_idx);
        let _ = spool.ledger(&row);

        let now = Utc::now();
        let decision = match policy::load(spool) {
            Ok(p) => p.decide(&rec.body, now, grant_expires_at),
            Err(e) => {
                eprintln!(
                    "deputy {}: TTY policy unreadable, running detached: {e:#}",
                    cfg.name
                );
                TtyDecision::NoTty
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
            TtyDecision::NoTty => None,
            TtyDecision::Tty { entry, expires_at } => {
                if let Ok(mut p) = policy::load(spool) {
                    if p.consume(&entry, &id, now) {
                        let _ = policy::save(spool, &p);
                    }
                }
                Some((entry, expires_at))
            }
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
            let res = run_job(&spool, &cfg, &rec, tty, worker_idx);
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
                    let idle_ms = now_ms() - last_output.load(Ordering::SeqCst);
                    if idle_ms > cfg.tty_idle.as_millis() as i64 {
                        kill_group(&mut child);
                        forced = Some((
                            RC_NEEDS_OPERATOR,
                            format!(
                                "needs-operator: no output for {}s on the terminal, so it is probably waiting on a prompt",
                                cfg.tty_idle.as_secs()
                            ),
                        ));
                        break None;
                    }
                }
                if spool.cancel_marker_path(&rec.id).exists() {
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
    let _ = std::fs::remove_file(spool.tty_marker_path(&rec.id));
    let _ = std::fs::remove_file(spool.cancel_marker_path(&rec.id));
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
    Ok(res)
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(RC_CANCELLED)
}
