//! BUG-2383: the lane job runner. A headless implementer seat cannot run a
//! cargo test to completion itself: each tool call is capped at 60 seconds,
//! and anything it detaches dies with the call's sandbox. So the seat hands
//! the run to its launcher.
//!
//! The protocol has two directories:
//!
//! - the **inbox**, `<seat>/.aida/lane-jobs/`, which the seat writes. A job
//!   is `<id>.job`, a JSON [`JobRequest`], written to a temporary name and
//!   renamed into place.
//! - the **results** directory, `<main>/.aida/lane-jobs/<lease id>/`, which
//!   only the host writes. The runner puts the run's output in `<id>.out`
//!   and, when it ends, its exit code in `<id>.rc`. It also appends one line
//!   per job to `jobs.log` there.
//!
//! The launcher of a standalone implementer (`aida queue work <SPEC>
//! --no-human`) runs [`Runner`] on a thread for as long as it waits for the
//! lane. The runner takes one job at a time and runs it in the seat's
//! directory, with the seat's build environment (its own cargo target, no
//! compiler wrapper), under the seat's OS wrap when `[contained] os_wrap` is
//! on, and through the host `cargo` on `PATH`, which is the cargo
//! slot-limiting wrapper on a configured host. The run carries the lane
//! marker, so a seat that ends its turn while a job runs is waited for and
//! resumed by the existing background-work loop.
//!
//! Containment (advisor ruling on BUG-2383): only an allow-listed cargo argv
//! ever runs: `cargo test|check|build --profile agent ...`, with no option
//! that points cargo at other code, config or output. The seat cannot make it
//! run a shell string. Results are written only into the host's directory, so
//! a seat that plants a symlink in its inbox cannot redirect a host write.
//!
//! The seat side is `aida lane job run -- cargo test ...` and `aida lane job
//! wait <id>`, both of which return within one tool call.
// trace:BUG-2383 | ai:claude

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The seat's inbox directory, set on the implementer child.
pub(crate) const INBOX_ENV: &str = "AIDA_LANE_JOBS_INBOX";
/// The host's results directory, set on the implementer child.
pub(crate) const RESULTS_ENV: &str = "AIDA_LANE_JOBS_RESULTS";

/// The inbox, relative to the seat root.
pub(crate) const INBOX_RELATIVE: &str = ".aida/lane-jobs";

/// The exit code a job reports when the runner refused it.
pub(crate) const REFUSED_RC: i32 = 126;
/// The exit code a job reports when it ran past its timeout.
pub(crate) const TIMEOUT_RC: i32 = 124;
/// The exit code a job reports when the lane ended while it ran.
pub(crate) const STOPPED_RC: i32 = 137;
/// The exit code `aida lane job run|wait` returns while the job still runs.
pub(crate) const STILL_RUNNING_EXIT: i32 = 75;

/// The default and largest job timeout, in seconds. A job's
/// `AIDA_CARGO_TIMEOUT` sets it within these bounds.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 900;
pub(crate) const MAX_TIMEOUT_SECS: u64 = 3600;

/// The largest job file the runner reads.
const MAX_JOB_BYTES: u64 = 64 * 1024;

/// The environment variables a job may set.
pub(crate) const ALLOWED_ENV: &[&str] = &[
    "AIDA_CARGO_TIMEOUT",
    "RUST_BACKTRACE",
    "RUST_TEST_THREADS",
    "AIDA_UPDATE_SNAPSHOTS",
];

/// The cargo subcommands a job may run.
pub(crate) const ALLOWED_SUBCOMMANDS: &[&str] = &["test", "check", "build"];

/// The only profile a job may build.
pub(crate) const REQUIRED_PROFILE: &str = "agent";

/// One job, as the seat writes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct JobRequest {
    /// The full argv, starting with `cargo`.
    pub(crate) argv: Vec<String>,
    /// Extra environment, only [`ALLOWED_ENV`] names.
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
}

/// Whether `id` is a job id: 1-64 of `[A-Za-z0-9_-]`.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Options before `--` that would point cargo at other code, config, a
/// toolchain or an output location.
const REFUSED_OPTIONS: &[&str] = &[
    "--config",
    "--manifest-path",
    "--target-dir",
    "--artifact-dir",
    "--out-dir",
    "-Z",
    "-C",
];

/// Check a job against the allow-list. `Err` names the reason.
pub(crate) fn validate(job: &JobRequest) -> std::result::Result<(), String> {
    let argv = &job.argv;
    if argv.first().map(String::as_str) != Some("cargo") {
        return Err("a lane job must start with `cargo`".into());
    }
    let sub = argv.get(1).map(String::as_str).unwrap_or("");
    if !ALLOWED_SUBCOMMANDS.contains(&sub) {
        return Err(format!(
            "a lane job runs only `cargo {}`, not `cargo {sub}`",
            ALLOWED_SUBCOMMANDS.join("|")
        ));
    }
    let mut profile = None;
    let mut prev: Option<&str> = None;
    for arg in &argv[2..] {
        if arg == "--" {
            break;
        }
        if arg.starts_with('+') {
            return Err(format!("`{arg}` (a toolchain override) is not allowed"));
        }
        for refused in REFUSED_OPTIONS {
            let attached = refused.len() == 2 && arg.len() > 2 && arg.starts_with(refused);
            if arg == refused || arg.starts_with(&format!("{refused}=")) || attached {
                return Err(format!("`{refused}` is not allowed in a lane job"));
            }
        }
        if let Some(p) = arg.strip_prefix("--profile=") {
            profile = Some(p.to_string());
        } else if prev == Some("--profile") {
            profile = Some(arg.clone());
        }
        prev = Some(arg.as_str());
    }
    if profile.as_deref() != Some(REQUIRED_PROFILE) {
        return Err(format!(
            "a lane job must build `--profile {REQUIRED_PROFILE}`"
        ));
    }
    for key in job.env.keys() {
        if !ALLOWED_ENV.contains(&key.as_str()) {
            return Err(format!("the environment variable `{key}` is not allowed"));
        }
    }
    Ok(())
}

/// The job's timeout: its `AIDA_CARGO_TIMEOUT` (seconds) bounded by
/// [`MAX_TIMEOUT_SECS`], else [`DEFAULT_TIMEOUT_SECS`].
pub(crate) fn timeout_for(job: &JobRequest) -> Duration {
    let secs = job
        .env
        .get("AIDA_CARGO_TIMEOUT")
        .and_then(|v| v.trim().trim_end_matches('s').parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .min(MAX_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// The results directory for lease `lease_id`.
pub(crate) fn results_dir(main_root: &Path, lease_id: &str) -> PathBuf {
    main_root.join(".aida").join("lane-jobs").join(lease_id)
}

/// The output and exit-code files of job `id`.
pub(crate) fn out_path(results: &Path, id: &str) -> PathBuf {
    results.join(format!("{id}.out"))
}
pub(crate) fn rc_path(results: &Path, id: &str) -> PathBuf {
    results.join(format!("{id}.rc"))
}

/// Read job `id`'s exit code, when the job has ended.
pub(crate) fn read_rc(results: &Path, id: &str) -> Option<i32> {
    std::fs::read_to_string(rc_path(results, id))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// How a lane's jobs went, for its result line.
#[derive(Debug, Default)]
pub(crate) struct Tally {
    /// Jobs that ran to an exit code of their own (pass or fail).
    pub(crate) completed: AtomicU32,
    /// Jobs refused, timed out or stopped.
    pub(crate) incomplete: AtomicU32,
}

impl Tally {
    /// Whether the lane ran no job to completion: its result line says
    /// `untested=true`.
    pub(crate) fn untested(&self) -> bool {
        self.completed.load(Ordering::Relaxed) == 0
    }
}

/// Where one lane's jobs run.
#[derive(Debug, Clone)]
pub(crate) struct LaneJobs {
    /// The seat root: jobs run here.
    pub(crate) seat: PathBuf,
    /// `<seat>/.aida/lane-jobs`.
    pub(crate) inbox: PathBuf,
    /// The host-only results directory.
    pub(crate) results: PathBuf,
    /// The lease id, carried as the lane marker on every job.
    pub(crate) lease_id: String,
    /// The environment a job's cargo runs with (the seat's build env).
    pub(crate) build_env: Vec<(String, String)>,
    /// The program a job's `cargo` resolves to: `cargo` on `PATH`, which is
    /// the slot-limiting wrapper on a configured host. Tests point it at a
    /// fixture.
    pub(crate) program: String,
}

impl LaneJobs {
    /// The lane for `lease_id`'s seat at `seat`, with results under
    /// `main_root`. The seat's build environment comes from its host-recorded
    /// clone record, else from the lease's cargo target directory.
    pub(crate) fn for_lease(main_root: &Path, seat: &Path, lease_id: &str) -> Self {
        let build_env: Vec<(String, String)> =
            if let Some(clone) = crate::seat_clone::host_recorded_clone(seat) {
                crate::seat_clone::seat_build_env(&clone)
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect()
            } else {
                crate::list_leases(main_root)
                    .into_iter()
                    .find(|l| l.id == lease_id)
                    .and_then(|l| l.cargo_target_dir)
                    .map(|dir| vec![("CARGO_TARGET_DIR".to_string(), dir.display().to_string())])
                    .unwrap_or_default()
            };
        Self {
            seat: seat.to_path_buf(),
            inbox: seat.join(INBOX_RELATIVE),
            results: results_dir(main_root, lease_id),
            lease_id: lease_id.to_string(),
            build_env,
            program: "cargo".to_string(),
        }
    }

    /// The seat's cargo target directory, from its build environment.
    fn target_dir(&self) -> Option<PathBuf> {
        self.build_env
            .iter()
            .find(|(k, _)| k == "CARGO_TARGET_DIR")
            .map(|(_, v)| PathBuf::from(v))
    }

    /// Create the inbox and the results directory.
    pub(crate) fn prepare(&self) -> Result<()> {
        std::fs::create_dir_all(&self.inbox)
            .with_context(|| format!("create {}", self.inbox.display()))?;
        std::fs::create_dir_all(&self.results)
            .with_context(|| format!("create {}", self.results.display()))?;
        Ok(())
    }

    /// Set the child's environment so `aida lane job` finds the lane.
    pub(crate) fn apply_env(&self, cmd: &mut std::process::Command) {
        cmd.env(INBOX_ENV, &self.inbox)
            .env(RESULTS_ENV, &self.results);
    }

    /// Append one line to the lane's `jobs.log` and the launcher's stderr.
    fn log(&self, line: &str) {
        eprintln!("  lane job: {line}");
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.results.join("jobs.log"))
        {
            let _ = writeln!(f, "{} {line}", chrono::Utc::now().to_rfc3339());
        }
    }
}

/// A running job runner. Dropping it (or [`Runner::stop`]) stops the thread,
/// stopping any job still running.
pub(crate) struct Runner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub(crate) tally: Arc<Tally>,
}

impl Runner {
    /// Start the runner for `lane` on its own thread.
    pub(crate) fn start(lane: LaneJobs) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let tally = Arc::new(Tally::default());
        let handle = {
            let stop = stop.clone();
            let tally = tally.clone();
            std::thread::Builder::new()
                .name("lane-job-runner".into())
                .spawn(move || run_loop(&lane, &stop, &tally))
                .ok()
        };
        Self {
            stop,
            handle,
            tally,
        }
    }

    /// Stop the runner and wait for its thread.
    pub(crate) fn stop(mut self) -> Arc<Tally> {
        self.halt();
        self.tally.clone()
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.halt();
    }
}

const POLL: Duration = Duration::from_millis(500);

fn run_loop(lane: &LaneJobs, stop: &AtomicBool, tally: &Tally) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut seeded = false;
    while !stop.load(Ordering::Relaxed) {
        match next_job(lane, &mut seen) {
            Some((id, job)) => {
                if !seeded {
                    seeded = true;
                    seed_lane_target(lane);
                }
                run_one(lane, &id, job, stop, tally);
            }
            None => std::thread::sleep(POLL),
        }
    }
}

/// Seed the seat's cargo target from the primary's before the first job.
fn seed_lane_target(lane: &LaneJobs) {
    let Some(target) = lane.target_dir() else {
        return;
    };
    let Some(main_target) = target.parent().and_then(Path::parent) else {
        return;
    };
    if main_target.file_name() != Some(std::ffi::OsStr::new("target")) {
        return;
    }
    let results = crate::seat_target_seed::seed_seat_target(
        main_target,
        &target,
        crate::seat_target_seed::LOCK_WAIT,
    );
    lane.log(&format!(
        "target {}: {}",
        target.display(),
        crate::seat_target_seed::describe(&results)
    ));
}

/// The next job in the inbox, oldest name first, taken out of the inbox.
/// A file that is not a regular file, is too large, or does not parse is
/// answered with a refusal.
fn next_job(
    lane: &LaneJobs,
    seen: &mut HashSet<String>,
) -> Option<(String, std::result::Result<JobRequest, String>)> {
    let entries = std::fs::read_dir(&lane.inbox).ok()?;
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter_map(|n| n.strip_suffix(".job").map(String::from))
        .filter(|id| !seen.contains(id))
        .collect();
    names.sort();
    let id = names.into_iter().next()?;
    seen.insert(id.clone());
    let path = lane.inbox.join(format!("{id}.job"));
    let parsed = read_job(&path);
    let _ = std::fs::remove_file(&path);
    if !valid_id(&id) {
        lane.log(&format!("ignored a job file with an invalid id {id:?}"));
        return None;
    }
    Some((id, parsed))
}

fn read_job(path: &Path) -> std::result::Result<JobRequest, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("read the job: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("the job file is not a regular file".into());
    }
    if meta.len() > MAX_JOB_BYTES {
        return Err(format!("the job file is over {MAX_JOB_BYTES} bytes"));
    }
    let mut body = String::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(MAX_JOB_BYTES).read_to_string(&mut body))
        .map_err(|e| format!("read the job: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("the job file does not parse: {e}"))
}

/// Write `rc` for job `id` atomically.
fn finish(lane: &LaneJobs, id: &str, rc: i32) {
    let tmp = lane.results.join(format!(".{id}.rc.tmp"));
    if std::fs::write(&tmp, format!("{rc}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, rc_path(&lane.results, id));
    }
}

fn run_one(
    lane: &LaneJobs,
    id: &str,
    job: std::result::Result<JobRequest, String>,
    stop: &AtomicBool,
    tally: &Tally,
) {
    let out_file = out_path(&lane.results, id);
    let refuse = |why: &str| {
        let _ = std::fs::write(&out_file, format!("lane job refused: {why}\n"));
        finish(lane, id, REFUSED_RC);
        tally.incomplete.fetch_add(1, Ordering::Relaxed);
        lane.log(&format!("{id} refused: {why}"));
    };
    let job = match job.and_then(|j| validate(&j).map(|()| j)) {
        Ok(job) => job,
        Err(why) => return refuse(&why),
    };
    let out = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_file)
    {
        Ok(f) => f,
        Err(e) => return refuse(&format!("create {}: {e}", out_file.display())),
    };
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return refuse(&format!("open the output: {e}")),
    };
    let (program, args) = match crate::session::os_wrapped_program_and_args(
        &lane.seat,
        &lane.program,
        job.argv[1..].to_vec(),
    ) {
        Ok(pair) => pair,
        Err(e) => return refuse(&format!("{e:#}")),
    };
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&args)
        .current_dir(&lane.seat)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .env(crate::headless_background::LANE_MARKER_ENV, &lane.lease_id)
        .env_remove(INBOX_ENV)
        .env_remove(RESULTS_ENV);
    for (k, v) in &lane.build_env {
        cmd.env(k, v);
    }
    for (k, v) in &job.env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let timeout = timeout_for(&job);
    lane.log(&format!(
        "{id} started: {} (timeout {}s)",
        job.argv.join(" "),
        timeout.as_secs()
    ));
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return refuse(&format!("start cargo: {e}")),
    };
    let (rc, how) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.code().unwrap_or(STOPPED_RC), "exited"),
            Ok(None) => {}
            Err(_) => break (STOPPED_RC, "lost"),
        }
        if started.elapsed() >= timeout {
            kill_group(&mut child);
            break (TIMEOUT_RC, "timed out");
        }
        if stop.load(Ordering::Relaxed) {
            kill_group(&mut child);
            break (STOPPED_RC, "stopped at lane end");
        }
        std::thread::sleep(POLL);
    };
    if how == "exited" {
        tally.completed.fetch_add(1, Ordering::Relaxed);
    } else {
        tally.incomplete.fetch_add(1, Ordering::Relaxed);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&out_file) {
            let _ = writeln!(f, "\nlane job {how} after {}s", started.elapsed().as_secs());
        }
    }
    finish(lane, id, rc);
    lane.log(&format!(
        "{id} {how} rc={rc} after {}s",
        started.elapsed().as_secs()
    ));
}

/// Stop the job's whole process group: SIGTERM, then SIGKILL.
fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        // SAFETY: signalling the job's own process group (process_group(0)).
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

// ---- seat side ------------------------------------------------------------

/// The inbox and results directories this process was given, or an error
/// that says the lane has no runner.
pub(crate) fn seat_dirs() -> Result<(PathBuf, PathBuf)> {
    let get = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    match (get(INBOX_ENV), get(RESULTS_ENV)) {
        (Some(inbox), Some(results)) => Ok((inbox, results)),
        _ => anyhow::bail!(
            "this session has no lane job runner ({INBOX_ENV} is not set). Only a \
             standalone implementer launched by `aida queue work <SPEC> --no-human` \
             has one. Report the work as untested (untested=true)."
        ),
    }
}

/// A new job id: time-ordered, unique per process.
pub(crate) fn new_id() -> String {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    format!(
        "j{}-{}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%3f"),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Submit `job` to `inbox` as `id`.
pub(crate) fn submit(inbox: &Path, id: &str, job: &JobRequest) -> Result<()> {
    std::fs::create_dir_all(inbox).with_context(|| format!("create {}", inbox.display()))?;
    let tmp = inbox.join(format!(".{id}.job.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(job)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, inbox.join(format!("{id}.job")))
        .with_context(|| format!("submit job {id}"))?;
    Ok(())
}

/// What a bounded wait for a job found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Waited {
    /// The job ended with this exit code.
    Done(i32),
    /// Still running (or not yet taken: `taken` is false) when the wait ended.
    Running { taken: bool },
}

/// Wait at most `wait` for job `id`, calling `progress(elapsed)` about every
/// `every`.
pub(crate) fn wait_for(
    inbox: &Path,
    results: &Path,
    id: &str,
    wait: Duration,
    every: Duration,
    mut progress: impl FnMut(Duration, bool),
) -> Waited {
    let started = Instant::now();
    let mut reported = started;
    loop {
        if let Some(rc) = read_rc(results, id) {
            return Waited::Done(rc);
        }
        let taken = !inbox.join(format!("{id}.job")).exists();
        if started.elapsed() >= wait {
            return Waited::Running { taken };
        }
        if reported.elapsed() >= every {
            reported = Instant::now();
            progress(started.elapsed(), taken);
        }
        std::thread::sleep(Duration::from_millis(500).min(wait.saturating_sub(started.elapsed())));
    }
}

/// The last `n` lines of `text`.
pub(crate) fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
#[path = "tests/bug_2383_lane_job_tests.rs"]
mod tests;
