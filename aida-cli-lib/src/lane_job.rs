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
//! compiler wrapper), and through the host `cargo` on `PATH`, which is the
//! cargo slot-limiting wrapper on a configured host. Each job's output names
//! the `cargo` it resolved to. The run carries the lane marker, so a seat that
//! ends its turn while a job runs is waited for and resumed by the existing
//! background-work loop.
//!
//! Containment (advisor rulings on BUG-2383, rounds 1 and 2):
//!
//! - Only an allow-listed cargo argv runs: `cargo test|check|build --profile
//!   agent ...`, with no option that points cargo at other code, config or
//!   output.
//! - A job still runs code the seat wrote (build scripts, tests, a
//!   seat-written `.cargo/config.toml` runner), so it always runs inside its
//!   own minimal bubblewrap profile
//!   ([`crate::session::lane_job_wrapped_program_and_args`]): the filesystem
//!   is read-only except the seat, its cargo target, cargo home's `registry/`
//!   and `git/` download caches (never its `bin/` or `config.toml`) and the
//!   slot limiter's state. When `[contained] os_wrap` is off or bubblewrap cannot
//!   run, the job is refused and nothing runs; the lane reports
//!   `untested=true`.
//! - A job starts from a cleared environment plus [`HOST_ENV_ALLOWED`], the
//!   seat's build environment and the job's own [`ALLOWED_ENV`]. No grant,
//!   session variable, credential or launcher-only signal file reaches it.
//! - The inbox is opened without following symlinks, component by component,
//!   and each job file is opened and removed through that directory handle,
//!   so a seat that turns its inbox or a job file into a symlink cannot make
//!   the host read or remove anything outside the inbox. Results are written
//!   only into the host's directory.
//!
//! The seat side is `aida lane job run -- cargo test ...` and `aida lane job
//! wait <id>`, both of which return within one tool call.
// trace:BUG-2383 | ai:claude

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
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

/// The launcher's environment variables a job inherits; `LC_*` is also kept.
/// Everything else (grants, `AIDA_SESSION_*`, `AIDA_HANDLE`, `SSH_AUTH_SOCK`,
/// `GH_TOKEN`, the launcher-only signal files, `AIDA_CARGO_NO_SLOT`,
/// `AIDA_CARGO_SLOT_HELD`) is dropped. `XDG_RUNTIME_DIR` and
/// `AIDA_CARGO_SLOTS` carry no authority: the host cargo wrapper keeps its
/// slot locks under `XDG_RUNTIME_DIR`, so without it a job would queue on a
/// sandbox-private `/tmp` and skip the host's slot limit.
pub(crate) const HOST_ENV_ALLOWED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TMPDIR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "CARGO_TERM_COLOR",
    "XDG_RUNTIME_DIR",
    "AIDA_CARGO_SLOTS",
];

/// The seat build-environment variables a job gets (from
/// [`crate::seat_clone::seat_build_env`]).
pub(crate) const BUILD_ENV_ALLOWED: &[&str] = &["CARGO_TARGET_DIR", "RUSTC_WRAPPER"];

/// Whether the launcher's `key` reaches a job.
pub(crate) fn host_key_allowed(key: &str) -> bool {
    HOST_ENV_ALLOWED.contains(&key) || key.starts_with("LC_")
}

/// The launcher environment `vars`, cut down to [`host_key_allowed`] keys.
pub(crate) fn scrub_host_env(
    vars: impl IntoIterator<Item = (String, String)>,
) -> Vec<(String, String)> {
    vars.into_iter()
        .filter(|(k, _)| host_key_allowed(k))
        .collect()
}

/// The only cargo-home subdirectories a job's sandbox may write: the
/// registry and git download caches. Everything else there (`bin/`,
/// `config.toml`, `credentials.toml`, `env`) stays read-only.
pub(crate) const CARGO_HOME_RW: &[&str] = &["registry", "git"];

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
/// [`MAX_TIMEOUT_SECS`], else [`DEFAULT_TIMEOUT_SECS`] on a seeded target
/// and [`MAX_TIMEOUT_SECS`] on a `cold` (unseeded) one. A cold build of
/// aida-cli-lib alone takes about 27 minutes, so on a host without reflinks
/// (ext4) the default would time every first job out (round-3 finding 7).
// trace:BUG-2383 | ai:claude
pub(crate) fn timeout_for(job: &JobRequest, cold: bool) -> Duration {
    let default = if cold {
        MAX_TIMEOUT_SECS
    } else {
        DEFAULT_TIMEOUT_SECS
    };
    let secs = job
        .env
        .get("AIDA_CARGO_TIMEOUT")
        .and_then(|v| v.trim().trim_end_matches('s').parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(default)
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
    /// Of those, the `cargo test` jobs: a check or build tests nothing.
    pub(crate) tested: AtomicU32,
    /// Jobs refused, timed out or stopped.
    pub(crate) incomplete: AtomicU32,
}

impl Tally {
    /// Whether the lane ran no `cargo test` to an exit code of its own: its
    /// result line says `untested=true`.
    pub(crate) fn untested(&self) -> bool {
        self.tested.load(Ordering::Relaxed) == 0
    }

    /// Count a job that exited with an exit code of its own.
    pub(crate) fn record_exit(&self, job: &JobRequest) {
        self.completed.fetch_add(1, Ordering::Relaxed);
        if runs_tests(job) {
            self.tested.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Whether `job` runs tests: a `cargo test` without `--no-run` (cargo's
/// option, before `--`) or `--list` (the test harness's, after it). Both
/// build and exit without running a test (round-3 finding 6).
// trace:BUG-2383 | ai:claude
pub(crate) fn runs_tests(job: &JobRequest) -> bool {
    if job.argv.get(1).map(String::as_str) != Some("test") {
        return false;
    }
    let mut harness = false;
    for arg in &job.argv[2..] {
        if arg == "--" && !harness {
            harness = true;
            continue;
        }
        if (!harness && arg == "--no-run") || (harness && arg == "--list") {
            return false;
        }
    }
    true
}

/// How a job is sandboxed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sandbox {
    /// The job's own bubblewrap profile; refused when it cannot run.
    Bwrap,
    /// Tests only: run the fixture program directly.
    #[cfg(test)]
    TestUnwrapped,
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
    /// The seat's build environment (its cargo target, no compiler wrapper).
    pub(crate) build_env: Vec<(String, String)>,
    /// The launcher environment a job inherits, already cut down by
    /// [`scrub_host_env`].
    pub(crate) host_env: Vec<(String, String)>,
    /// The program a job's `cargo` resolves to, looked up on the job's
    /// `PATH`: there, the slot-limiting wrapper on a configured host. Tests
    /// point it at a fixture.
    pub(crate) program: String,
    /// How each job is sandboxed.
    pub(crate) sandbox: Sandbox,
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
            // vars_os: a non-UTF-8 variable is dropped, never a panic.
            host_env: scrub_host_env(
                std::env::vars_os()
                    .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
            ),
            program: "cargo".to_string(),
            sandbox: Sandbox::Bwrap,
        }
    }

    /// The whole environment of a job: the scrubbed launcher environment,
    /// the seat's build environment, the job's own allow-listed variables and
    /// the lane marker. Each source is filtered again here, so nothing outside
    /// the allow-lists reaches a job whatever a caller put in `self`.
    pub(crate) fn job_env(&self, job: &JobRequest) -> BTreeMap<String, String> {
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &self.host_env {
            if host_key_allowed(k) {
                env.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in &self.build_env {
            if BUILD_ENV_ALLOWED.contains(&k.as_str()) {
                env.insert(k.clone(), v.clone());
            }
        }
        for (k, v) in &job.env {
            if ALLOWED_ENV.contains(&k.as_str()) {
                env.insert(k.clone(), v.clone());
            }
        }
        env.insert(
            crate::headless_background::LANE_MARKER_ENV.to_string(),
            self.lease_id.clone(),
        );
        env
    }

    /// The paths a job's sandbox may write besides the seat: its cargo
    /// target, cargo home's download caches, and the host cargo wrapper's
    /// slot locks and slot log, so the job queues for a slot with every other
    /// cargo on the host and its run is recorded.
    ///
    /// Only [`CARGO_HOME_RW`] (`registry/`, `git/`) is writable in cargo
    /// home. `bin/` holds the rustup proxies (`cargo`, `rustc`) and
    /// `config.toml` can name a `build.rustc-wrapper`; a job that could write
    /// either would run its code on the host's next cargo, unsandboxed
    /// (round-3 finding 5).
    // trace:BUG-2383 | ai:claude
    pub(crate) fn sandbox_rw_paths(&self, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
        let mut rw = Vec::new();
        if let Some(target) = self.target_dir() {
            rw.push(target);
        }
        let home = env.get("HOME").map(PathBuf::from);
        if let Some(cargo_home) = env
            .get("CARGO_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".cargo")))
        {
            rw.extend(CARGO_HOME_RW.iter().map(|sub| cargo_home.join(sub)));
        }
        let runtime = env
            .get("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        rw.push(runtime.join(format!("cargo-slots-{}", current_uid())));
        if let Some(home) = home {
            rw.push(home.join(".local/state/cargo-slots"));
        }
        rw
    }

    /// The seat's cargo target directory, from its build environment.
    fn target_dir(&self) -> Option<PathBuf> {
        self.build_env
            .iter()
            .find(|(k, _)| k == "CARGO_TARGET_DIR")
            .map(|(_, v)| PathBuf::from(v))
    }

    /// Create the inbox and the results directory. Refuses an inbox path
    /// the seat turned into a symlink, so the host never creates a directory
    /// outside the seat.
    pub(crate) fn prepare(&self) -> Result<()> {
        #[cfg(not(unix))]
        anyhow::bail!("the lane job runner needs a unix host");
        let mut path = self.seat.clone();
        for part in Path::new(INBOX_RELATIVE).components() {
            path.push(part);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_dir()) {
                anyhow::bail!(
                    "{} is not a directory (a symlink?); the lane job runner will not use it",
                    path.display()
                );
            }
        }
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

#[cfg(unix)]
fn run_loop(lane: &LaneJobs, stop: &AtomicBool, tally: &Tally) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut seed_note: Option<SeedNote> = None;
    let mut warned = false;
    while !stop.load(Ordering::Relaxed) {
        let inbox = match inbox::InboxDir::open(lane) {
            Ok(inbox) => inbox,
            Err(e) => {
                if !warned {
                    warned = true;
                    lane.log(&format!(
                        "cannot open the inbox {} without following a symlink: {e}",
                        lane.inbox.display()
                    ));
                }
                std::thread::sleep(POLL);
                continue;
            }
        };
        match next_job(lane, &inbox, &mut seen) {
            Some((id, job)) => {
                let note = seed_note.get_or_insert_with(|| seed_lane_target(lane));
                run_one(lane, &id, job, stop, tally, note);
            }
            None => std::thread::sleep(POLL),
        }
    }
}

#[cfg(not(unix))]
fn run_loop(_lane: &LaneJobs, _stop: &AtomicBool, _tally: &Tally) {}

/// What the seed before the first job found.
#[derive(Debug, Clone)]
pub(crate) struct SeedNote {
    /// Which path ran, for the lane log and every job's output.
    pub(crate) note: String,
    /// Nothing was seeded or already warm: the first build is cold, so a job
    /// gets [`MAX_TIMEOUT_SECS`] by default ([`timeout_for`]).
    pub(crate) cold: bool,
}

/// Seed the seat's cargo target from the primary's before the first job, and
/// say which path ran.
fn seed_lane_target(lane: &LaneJobs) -> SeedNote {
    let Some(target) = lane.target_dir() else {
        return SeedNote {
            note: "the seat has no cargo target of its own; not seeded".into(),
            cold: true,
        };
    };
    let (note, cold) = match target.parent().and_then(Path::parent) {
        Some(main_target) if main_target.file_name() == Some(std::ffi::OsStr::new("target")) => {
            let results = crate::seat_target_seed::seed_seat_target(
                main_target,
                &target,
                crate::seat_target_seed::LOCK_WAIT,
            );
            (
                crate::seat_target_seed::describe(&results),
                crate::seat_target_seed::is_cold(&results),
            )
        }
        _ => ("not under the primary's target; not seeded".into(), true),
    };
    let note = format!("seat target {}: {note}", target.display());
    lane.log(&note);
    SeedNote { note, cold }
}

/// The next job in the inbox, oldest name first, taken out of the inbox
/// through the inbox's directory handle. A file that is not a regular file
/// (a symlink included), is too large, or does not parse is answered with a
/// refusal.
#[cfg(unix)]
fn next_job(
    lane: &LaneJobs,
    inbox: &inbox::InboxDir,
    seen: &mut HashSet<String>,
) -> Option<(String, std::result::Result<JobRequest, String>)> {
    let mut names: Vec<String> = inbox
        .names(&lane.inbox)
        .into_iter()
        .filter_map(|n| n.strip_suffix(".job").map(String::from))
        .filter(|id| !seen.contains(id))
        .collect();
    names.sort();
    let id = names.into_iter().next()?;
    seen.insert(id.clone());
    let name = format!("{id}.job");
    let parsed = inbox.read_job(&name);
    inbox.remove(&name);
    if !valid_id(&id) {
        lane.log(&format!("ignored a job file with an invalid id {id:?}"));
        return None;
    }
    Some((id, parsed))
}

/// The inbox, held as a directory handle opened with `O_NOFOLLOW` at every
/// component below the seat root (finding 9 of the BUG-2383 review).
#[cfg(unix)]
mod inbox {
    use super::{JobRequest, LaneJobs, INBOX_RELATIVE, MAX_JOB_BYTES};
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io::Read;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::io::{AsRawFd, FromRawFd};
    use std::path::Path;

    pub(super) struct InboxDir {
        dir: File,
    }

    /// `openat(dir, name, flags | O_NOFOLLOW | O_CLOEXEC)`.
    fn openat_nofollow(dir: &File, name: &OsStr, flags: libc::c_int) -> std::io::Result<File> {
        let c = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: `c` is NUL-terminated and `dir` stays open for the call.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                c.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` was just returned by openat and nothing else owns it.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    impl InboxDir {
        /// Open `<seat>/.aida/lane-jobs` without following a symlink at
        /// `.aida` or `lane-jobs`.
        pub(super) fn open(lane: &LaneJobs) -> std::io::Result<Self> {
            let mut dir = File::open(&lane.seat)?;
            for part in Path::new(INBOX_RELATIVE).components() {
                dir = openat_nofollow(&dir, part.as_os_str(), libc::O_RDONLY | libc::O_DIRECTORY)?;
            }
            Ok(Self { dir })
        }

        /// The names in the inbox. They are listed by path, so the listing
        /// is used only when that path is still the directory this handle
        /// holds; every open and removal goes through the handle.
        pub(super) fn names(&self, path: &Path) -> Vec<String> {
            let (Ok(held), Ok(listed)) = (self.dir.metadata(), std::fs::symlink_metadata(path))
            else {
                return Vec::new();
            };
            if !listed.is_dir() || held.dev() != listed.dev() || held.ino() != listed.ino() {
                return Vec::new();
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                return Vec::new();
            };
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .collect()
        }

        /// Read job file `name`: a regular file of at most
        /// [`MAX_JOB_BYTES`], never through a symlink, never blocking on a
        /// FIFO.
        pub(super) fn read_job(&self, name: &str) -> std::result::Result<JobRequest, String> {
            let file = openat_nofollow(
                &self.dir,
                OsStr::new(name),
                libc::O_RDONLY | libc::O_NONBLOCK,
            )
            .map_err(|e| {
                if e.raw_os_error() == Some(libc::ELOOP) {
                    "the job file is a symlink, not a regular file".to_string()
                } else {
                    format!("read the job: {e}")
                }
            })?;
            let meta = file.metadata().map_err(|e| format!("read the job: {e}"))?;
            if !meta.is_file() {
                return Err("the job file is not a regular file".into());
            }
            if meta.len() > MAX_JOB_BYTES {
                return Err(format!("the job file is over {MAX_JOB_BYTES} bytes"));
            }
            let mut body = String::new();
            file.take(MAX_JOB_BYTES)
                .read_to_string(&mut body)
                .map_err(|e| format!("read the job: {e}"))?;
            serde_json::from_str(&body).map_err(|e| format!("the job file does not parse: {e}"))
        }

        /// Remove `name` from the inbox (never a path outside it).
        pub(super) fn remove(&self, name: &str) {
            let Ok(c) = CString::new(name) else {
                return;
            };
            // SAFETY: `c` is NUL-terminated and `self.dir` is open.
            unsafe { libc::unlinkat(self.dir.as_raw_fd(), c.as_ptr(), 0) };
        }
    }
}

/// The real uid, for the cargo wrapper's slot directory name.
fn current_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    #[cfg(unix)]
    let uid = unsafe { libc::getuid() };
    #[cfg(not(unix))]
    let uid = 0;
    uid
}

/// Where `program` resolves on `env`'s `PATH` (a path with a `/` is taken
/// as given).
pub(crate) fn resolve_program(program: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let path = env.get("PATH")?;
    std::env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
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
    seed: &SeedNote,
) {
    let seed_note = seed.note.as_str();
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
    let env = lane.job_env(&job);
    let Some(cargo) = resolve_program(&lane.program, &env) else {
        return refuse(&format!("`{}` is not on the job's PATH", lane.program));
    };
    let args = job.argv[1..].to_vec();
    let (program, args, how_wrapped) = match lane.sandbox {
        Sandbox::Bwrap => match crate::session::lane_job_wrapped_program_and_args(
            &lane.seat,
            &lane.sandbox_rw_paths(&env),
            &cargo.to_string_lossy(),
            args,
        ) {
            Ok((program, args)) => (program, args, "inside bubblewrap"),
            Err(e) => return refuse(&format!("{e:#}")),
        },
        #[cfg(test)]
        Sandbox::TestUnwrapped => (
            cargo.to_string_lossy().into_owned(),
            args,
            "unwrapped (test fixture)",
        ),
    };
    let mut out = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_file)
    {
        Ok(f) => f,
        Err(e) => return refuse(&format!("create {}: {e}", out_file.display())),
    };
    let ran_with = format!("cargo is {}, run {how_wrapped}", cargo.display());
    {
        use std::io::Write;
        let _ = writeln!(out, "lane job: {seed_note}\nlane job: {ran_with}");
    }
    let err = match out.try_clone() {
        Ok(f) => f,
        Err(e) => return refuse(&format!("open the output: {e}")),
    };
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&args)
        .current_dir(&lane.seat)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        // A job never inherits the launcher's environment: no grant, no
        // session or launcher-only variable, no credential.
        .env_clear()
        .envs(&env);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let timeout = timeout_for(&job, seed.cold);
    lane.log(&format!(
        "{id} started: {} (timeout {}s; {ran_with})",
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
            // A run killed by a signal has no exit code of its own: it is not
            // a completed run (round-3 finding 6). trace:BUG-2383 | ai:claude
            Ok(Some(status)) => match status.code() {
                Some(code) => break (code, "exited"),
                None => break (STOPPED_RC, "killed by a signal"),
            },
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
        tally.record_exit(&job);
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
