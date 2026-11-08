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
//! The launcher of a headless implementer (a standalone `aida queue work
//! <SPEC> --no-human`, or the `--auto-complete --no-human` drain's phase-1
//! implementer, see [`offer`]) runs [`Runner`] on a thread for as long as it
//! waits for the lane. The runner takes one job at a time and runs it in the seat's
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
//!   is read-only except the seat, its cargo target and the slot limiter's
//!   state. All of cargo home is read-only, including `registry/src/` and
//!   `git/checkouts/` (the extracted dependency sources whose `build.rs` the
//!   host's next cargo runs unsandboxed), and the job runs offline
//!   (`CARGO_NET_OFFLINE=true`) against the dependencies the host already
//!   fetched (advisor ruling, round 3). When `[contained] os_wrap` is off or bubblewrap cannot
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
//!
//! Discovery (advisor signoff 2026-11-02, transport sketch part 1): no
//! lane-job variable reaches the seat. A Codex tool shell under
//! `shell_environment_policy.inherit = "core"` keeps only the public seat
//! context of [`crate::codex_tool_env`] (BUG-2398), so every vendor finds
//! its runner the same way, through one selector that whitelist already
//! carries: the lease id in `AIDA_SESSION_ID`, scoped by
//! `AIDA_PROJECT_ROOT`. While a [`Runner`] serves the lane, the host stamps a
//! typed [`RunnerRecord`] on that lease ([`LEASE_FIELD`]); the seat side
//! ([`resolve_seat_dirs`]) reads the lease without following a symlink,
//! requires its cwd to be inside the lease's own worktree and a fresh
//! [`Heartbeat`] from the recorded runner in the host-only results directory,
//! and derives both directories from the lease. A seat tool shell (Claude's
//! and Codex's alike) runs in a private PID namespace and cannot see the
//! runner's pid, so the pid check stays host-side only. The lease id is a name, not a capability: no
//! token, path or grant travels in env or argv, and any failure means
//! `untested=true`, never a direct cargo run.
// trace:BUG-2383 | ai:claude

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The lease field the host stamps while a runner serves the lane.
// trace:BUG-2383 | ai:claude
pub(crate) const LEASE_FIELD: &str = "lane_jobs";
/// The lane-job protocol version a [`RunnerRecord`] names.
pub(crate) const PROTOCOL: u32 = 1;

/// The typed record on a lease saying a lane job runner serves it. It names
/// the host process the runner thread lives in and that process's kernel
/// start identity (so a recycled pid is not a live runner); it carries no
/// path, token or grant.
// trace:BUG-2383 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub(crate) struct RunnerRecord {
    pub(crate) protocol: u32,
    pub(crate) runner_pid: u32,
    pub(crate) runner_started_at: String,
}

/// The heartbeat file in a lane's results directory.
pub(crate) const HEARTBEAT_FILE: &str = "heartbeat.json";
/// How often a published runner rewrites its heartbeat.
pub(crate) const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// How old a heartbeat may be for the seat to treat the runner as live.
pub(crate) const HEARTBEAT_FRESH: Duration = Duration::from_secs(30);

/// The runner's liveness proof, rewritten every [`HEARTBEAT_EVERY`] by the
/// host in the lane's results directory, which no seat can write (advisor
/// ruling on fork 1, 2026-11-02 06:36). It names the record it belongs to by
/// protocol and runner start identity, so a heartbeat left by an earlier
/// runner never vouches for a later record.
// trace:BUG-2383 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Heartbeat {
    pub(crate) protocol: u32,
    pub(crate) runner_started_at: String,
    /// Increases by one with every rewrite.
    pub(crate) counter: u64,
    /// Wall time of the rewrite, milliseconds since the Unix epoch.
    pub(crate) written_at_ms: u64,
}

/// `<results>/heartbeat.json`.
pub(crate) fn heartbeat_path(results: &Path) -> PathBuf {
    results.join(HEARTBEAT_FILE)
}

fn unix_ms(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write `beat` atomically (a temporary file renamed into place).
// trace:BUG-2383 | ai:claude
pub(crate) fn write_heartbeat(results: &Path, beat: &Heartbeat) -> Result<()> {
    let tmp = results.join(format!(".{HEARTBEAT_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(beat)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, heartbeat_path(results))
        .with_context(|| format!("publish {}", heartbeat_path(results).display()))
}

/// Whether `results` holds a heartbeat from `record`'s runner younger than
/// [`HEARTBEAT_FRESH`] at `now`. Missing, unreadable, symlinked, foreign or
/// stale means not live, so the seat reports `untested=true` rather than
/// waiting on a runner that is gone. A heartbeat stamped in the future (more
/// than [`HEARTBEAT_EVERY`] ahead) is not trusted either.
// trace:BUG-2383 | ai:claude
pub(crate) fn heartbeat_fresh(
    results: &Path,
    record: &RunnerRecord,
    now: std::time::SystemTime,
) -> bool {
    let path = heartbeat_path(results);
    if std::fs::symlink_metadata(&path).map_or(true, |m| !m.file_type().is_file()) {
        return false;
    }
    let Ok(body) = std::fs::read(&path) else {
        return false;
    };
    let Ok(beat) = serde_json::from_slice::<Heartbeat>(&body) else {
        return false;
    };
    let now = unix_ms(now);
    beat.protocol == record.protocol
        && beat.runner_started_at == record.runner_started_at
        && beat.written_at_ms <= now + HEARTBEAT_EVERY.as_millis() as u64
        && now.saturating_sub(beat.written_at_ms) < HEARTBEAT_FRESH.as_millis() as u64
}

/// The seat side's liveness check: a fresh heartbeat in `results`.
pub(crate) fn seat_runner_alive(record: &RunnerRecord, results: &Path) -> bool {
    heartbeat_fresh(results, record, std::time::SystemTime::now())
}

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
pub(crate) const BUILD_ENV_ALLOWED: &[&str] = &[
    "CARGO_TARGET_DIR",
    "RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_SERVER_UDS",
    "SCCACHE_SERVER_PORT",
    "SCCACHE_IDLE_TIMEOUT",
    "CARGO_INCREMENTAL",
];

/// Host cargo config opts jobs into a lane-only cache. Never inherit the
/// operator's wrapper/cache/server environment or consult seat cargo config.
// trace:STORY-2641 | ai:codex
fn lane_sccache_env(parent: &aida_core::exec_context::Environment) -> Vec<(String, String)> {
    let Some(home) = parent.get("HOME").map(PathBuf::from) else {
        return vec![];
    };
    let cargo = parent
        .get("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cargo"));
    let config = std::fs::read_to_string(cargo.join("config.toml"))
        .or_else(|_| std::fs::read_to_string(cargo.join("config")));
    let Some(wrapper) = config
        .ok()
        .and_then(|text| text.parse::<toml::Value>().ok())
        .and_then(|config| {
            config
                .get("build")?
                .get("rustc-wrapper")?
                .as_str()
                .map(str::to_owned)
        })
    else {
        return vec![];
    };
    if Path::new(&wrapper).file_name().and_then(|s| s.to_str()) != Some("sccache") {
        return vec![];
    }
    let cache = home.join(".cache/aida/lane-sccache");
    // Each job's bubblewrap mounts a fresh private /tmp. Keep the socket
    // outside the shared writable cache, avoiding stale or lane-planted endpoints.
    let socket = PathBuf::from("/tmp/aida-lane-sccache.sock");
    crate::launch::resources::Sccache::Lane {
        wrapper,
        cache,
        socket,
    }
    .env()
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_string_lossy().into_owned()))
    .collect()
}

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

/// Set on every job: cargo must not download. Cargo home is read-only in
/// the sandbox, so a job builds only against dependencies the host already
/// fetched and extracted, and a missing one fails with cargo's own offline
/// error instead of a write into the host's cache.
pub(crate) const OFFLINE_ENV: (&str, &str) = ("CARGO_NET_OFFLINE", "true");

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

/// The marker a lane's launcher leaves in its results directory when the
/// lane ran no local test to completion, for a launcher above it (the
/// `--auto-complete` drain) to report.
pub(crate) const UNTESTED_MARKER: &str = "untested";

/// The lane job runner for `lease_id`'s seat at `seat`, when it can be
/// offered: its directories are ready and `[contained] os_wrap` is on (a job
/// runs only inside bubblewrap, so with it off the runner would refuse every
/// job). An unset os_wrap defaults on for this lane's runner when bubblewrap
/// works here (BUG-2434). Otherwise warns why and returns `None`; the lane
/// then reports `untested=true` and records [`OS_WRAP_OFF_MARKER`]. Shared by
/// the standalone implementer and the drain's phase-1 implementer.
// trace:BUG-2383 | ai:claude
pub(crate) fn offer(main_root: &Path, seat: &Path, lease_id: &str) -> Option<LaneJobs> {
    offer_with(main_root, seat, lease_id, || {
        crate::session::bwrap_availability() == crate::session::BwrapAvailability::Ok
    })
}

/// [`offer`] with the host's bubblewrap probe injected. The lane default is
/// scoped to the returned runner ([`Sandbox::BwrapDefaultOn`]): nothing here
/// touches this process's environment, so the seat launch itself and every
/// other launch in the process keep their own os_wrap resolution.
// trace:BUG-2434 | ai:claude
pub(crate) fn offer_with(
    main_root: &Path,
    seat: &Path,
    lease_id: &str,
    bwrap_live: impl FnOnce() -> bool,
) -> Option<LaneJobs> {
    use colored::Colorize;
    let os_wrap = crate::session::implementer_lane_os_wrap(seat, bwrap_live);
    record_os_wrap_off(main_root, lease_id, !os_wrap.on());
    if os_wrap == crate::session::LaneOsWrap::DefaultOn {
        eprintln!(
            "  {} bubblewrap works on this host, so this lane's test runner is on and runs \
             each job inside bubblewrap (AIDA_OS_WRAP=0 opts out)",
            crate::glyph(crate::glyphs::Glyph::InfoAlt).cyan()
        );
    }
    let mut lane_jobs = LaneJobs::for_lease(main_root, seat, lease_id);
    if os_wrap == crate::session::LaneOsWrap::DefaultOn {
        lane_jobs.sandbox = Sandbox::BwrapDefaultOn;
    }
    match lane_jobs.prepare() {
        Ok(()) if !os_wrap.on() => {
            eprintln!(
                "{} {}",
                "Warning:".yellow().bold(),
                no_runner_reason(os_wrap)
            );
            None
        }
        Ok(()) => Some(lane_jobs),
        Err(e) => {
            eprintln!(
                "{} the lane job runner is unavailable, so the implementer cannot run tests: {e:#}",
                "Warning:".yellow().bold()
            );
            None
        }
    }
}

/// Why the lane gets no job runner when os_wrap stays off: an explicit
/// opt-out or a host whose bubblewrap does not work, each with its fix.
// trace:BUG-2434 | ai:claude
pub(crate) fn no_runner_reason(os_wrap: crate::session::LaneOsWrap) -> String {
    let why = match os_wrap {
        crate::session::LaneOsWrap::Explicit(_) => {
            "`[contained] os_wrap` is set off (AIDA_OS_WRAP or .aida/config.toml); unset it or \
             set it on"
        }
        _ => "bubblewrap does not work on this host; run `aida doctor --fix-sandbox`",
    };
    format!(
        "the lane job runner needs `[contained] os_wrap` (jobs run only inside bubblewrap), so \
         the implementer cannot run tests and its result says untested=true: {why}"
    )
}

/// The marker a lane's launcher leaves in its results directory when the
/// lane ran without `[contained] os_wrap`, so it had no job runner at all
/// (BUG-2434): the advisor-visible cause behind its `untested=true`.
pub(crate) const OS_WRAP_OFF_MARKER: &str = "os-wrap-off";

/// Record whether `lease_id`'s lane runs without os_wrap; a lane that has it
/// clears a stale marker from an earlier round. trace:BUG-2434 | ai:claude
pub(crate) fn record_os_wrap_off(main_root: &Path, lease_id: &str, off: bool) {
    let dir = results_dir(main_root, lease_id);
    let marker = dir.join(OS_WRAP_OFF_MARKER);
    if off {
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(&marker, "os_wrap=off\n");
        }
    } else {
        let _ = std::fs::remove_file(&marker);
    }
}

/// Whether `lease_id`'s lane ran without os_wrap (no job runner).
// trace:BUG-2434 | ai:claude
pub(crate) fn lane_ran_without_os_wrap(main_root: &Path, lease_id: &str) -> bool {
    results_dir(main_root, lease_id)
        .join(OS_WRAP_OFF_MARKER)
        .exists()
}

/// Record whether `lease_id`'s lane ran a local test: leave
/// explicit true/false evidence in [`UNTESTED_MARKER`]. Missing evidence is untested.
// trace:BUG-2383 | ai:claude
// trace:BUG-2383 | ai:codex
pub(crate) fn record_untested(main_root: &Path, lease_id: &str, untested: bool) {
    let dir = results_dir(main_root, lease_id);
    let marker = dir.join(UNTESTED_MARKER);
    if untested {
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(&marker, "untested=true\n");
        }
    } else {
        let _ = std::fs::write(&marker, "untested=false\n");
    }
}

/// Only an explicit tested result clears untested; absence/read failure refuses.
pub(crate) fn lane_was_untested(main_root: &Path, lease_id: &str) -> bool {
    std::fs::read_to_string(results_dir(main_root, lease_id).join(UNTESTED_MARKER))
        .map(|value| value != "untested=false\n")
        .unwrap_or(true)
}

/// Phase output fails closed on absent/unreadable evidence or lease discovery.
// trace:BUG-2383 | ai:codex
/// BUG-2434: a lane that ran without os_wrap says so, so the advisor reading
/// the phase output sees why it had no test runner.
// trace:BUG-2434 | ai:claude
pub(crate) fn phase_test_warning(main_root: &Path, lease: Option<&str>) -> Option<String> {
    if lease
        .map(|lease| lane_was_untested(main_root, lease))
        .unwrap_or(true)
    {
        let line = "the implementer has no completed local test evidence (untested=true); its push reaches CI untested";
        Some(with_os_wrap_off(
            line.to_string(),
            lease.is_some_and(|lease| lane_ran_without_os_wrap(main_root, lease)),
        ))
    } else {
        None
    }
}

/// Append the advisor-visible `os_wrap=off` token to a lane's result or
/// phase line when the lane ran without os_wrap, so no job runner existed:
/// the fix is the host's bubblewrap or its os_wrap setting, not the lane.
// trace:BUG-2434 | ai:claude
pub(crate) fn with_os_wrap_off(line: String, off: bool) -> String {
    if off {
        format!("{line} os_wrap=off")
    } else {
        line
    }
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
    /// Whether the lane has no executed-test result evidence: its
    /// result line says `untested=true`.
    pub(crate) fn untested(&self) -> bool {
        self.tested.load(Ordering::Relaxed) == 0
    }

    /// Count a job that exited with an exit code of its own.
    pub(crate) fn record_exit(&self, job: &JobRequest, output: impl std::io::Read) {
        self.completed.fetch_add(1, Ordering::Relaxed);
        if runs_tests(job) && executed_tests(output) {
            self.tested.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Require a libtest result with at least one executed test, passing or failing.
/// Cargo errors and zero-match/ignored-only runs supply no such evidence. Read
/// at most 16 MiB; missing, unreadable or truncated evidence stays untested.
// trace:BUG-2383 | ai:codex
fn executed_tests(output: impl std::io::Read) -> bool {
    use std::io::{BufRead, BufReader, Read};
    BufReader::new(output.take(16 * 1024 * 1024))
        .lines()
        .map_while(Result::ok)
        .any(|line| {
            let Some(summary) = line.strip_prefix("test result: ") else {
                return false;
            };
            let Some((verdict, counts)) = summary.split_once(". ") else {
                return false;
            };
            if !matches!(verdict, "ok" | "FAILED") {
                return false;
            }
            counts.split(';').any(|field| {
                let mut words = field.split_whitespace();
                matches!((words.next().and_then(|n| n.parse::<u64>().ok()), words.next()),
                    (Some(n), Some("passed" | "failed")) if n > 0)
            })
        })
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
    /// The job's own bubblewrap profile; refused when it cannot run or the
    /// seat's `[contained] os_wrap` is off.
    Bwrap,
    /// The same profile for a lane whose unset os_wrap defaulted on because
    /// bubblewrap works here (BUG-2434). The default lives on this runner
    /// only, never in the process environment. trace:BUG-2434 | ai:claude
    BwrapDefaultOn,
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
        let mut build_env: Vec<(String, String)> =
            if let Some(clone) = crate::seat_clone::host_recorded_clone(main_root, seat) {
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
        // trace:STORY-2641 | ai:codex
        let parent = aida_core::exec_context::Environment::capture();
        for (key, value) in lane_sccache_env(&parent) {
            build_env.retain(|(existing, _)| existing != &key);
            build_env.push((key, value));
        }
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
        // trace:BUG-2383 | ai:claude
        env.insert(OFFLINE_ENV.0.to_string(), OFFLINE_ENV.1.to_string());
        env
    }

    /// The paths a job's sandbox may write besides the seat: its cargo
    /// target and the host cargo wrapper's slot locks and slot log, so the
    /// job queues for a slot with every other cargo on the host and its run
    /// is recorded.
    ///
    /// Nothing in cargo home is writable. `bin/` holds the rustup proxies
    /// (`cargo`, `rustc`), `config.toml` can name a `build.rustc-wrapper`,
    /// and `registry/src/` and `git/checkouts/` hold the extracted
    /// dependency sources, `build.rs` included, that the host's next cargo
    /// compiles and runs. A job that could write any of them would run its
    /// code on the host, unsandboxed. The download caches are not writable
    /// either: cargo does not re-verify a cached `.crate` before extracting
    /// it. Jobs run offline instead ([`OFFLINE_ENV`]).
    // trace:BUG-2383 | ai:claude
    pub(crate) fn sandbox_rw_paths(&self, env: &BTreeMap<String, String>) -> Vec<PathBuf> {
        let mut rw = Vec::new();
        if let Some(target) = self.target_dir() {
            rw.push(target);
        }
        // trace:STORY-2641 | ai:codex
        if let Some(cache) = env.get("SCCACHE_DIR") {
            rw.push(PathBuf::from(cache));
        }
        let home = env.get("HOME").map(PathBuf::from);
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
        // Only the host-selected lane cache is made writable; cargo home stays read-only.
        // trace:STORY-2641 | ai:codex
        if let Some((_, cache)) = self.build_env.iter().find(|(key, _)| key == "SCCACHE_DIR") {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(cache)?;
            let metadata = std::fs::symlink_metadata(cache)?;
            anyhow::ensure!(
                metadata.is_dir()
                    && metadata.uid() == current_uid()
                    && metadata.mode() & 0o7777 == 0o700,
                "lane sccache must be an owned private directory"
            );
        }
        std::fs::create_dir_all(&self.results)
            .with_context(|| format!("create {}", self.results.display()))?;
        Ok(())
    }

    /// Give the child its lane's one public selector, the lease id in
    /// `AIDA_SESSION_ID` (on the BUG-2398 Codex tool-shell whitelist), so
    /// `aida lane job` resolves the runner through the lease. No path or
    /// token is set.
    // trace:BUG-2383 | ai:claude
    pub(crate) fn apply_selector(&self, cmd: &mut std::process::Command) {
        cmd.env("AIDA_SESSION_ID", &self.lease_id);
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
    /// The heartbeat thread of a published runner.
    heartbeat: Option<std::thread::JoinHandle<()>>,
    /// The results directory the heartbeat is written to.
    results: PathBuf,
    pub(crate) tally: Arc<Tally>,
    /// The main root and lease id whose [`RunnerRecord`] this runner
    /// stamped, cleared again when it stops.
    published: Option<(PathBuf, String)>,
}

impl Runner {
    /// Start the runner for `lane` and publish it on the lane's lease under
    /// `main_root`, so the seat finds it through [`resolve_seat_dirs`]
    /// whatever its vendor strips from a tool shell's environment. When the
    /// record cannot be written the runner still starts, but the seat cannot
    /// find it and the lane reports `untested=true`.
    // trace:BUG-2383 | ai:claude
    pub(crate) fn start_published(lane: LaneJobs, main_root: &Path) -> Self {
        let lease_id = lane.lease_id.clone();
        let mut runner = Self::start(lane);
        match publish(main_root, &lease_id) {
            Ok(record) => {
                runner.heartbeat = start_heartbeat(&runner.results, record, runner.stop.clone());
                runner.published = Some((main_root.to_path_buf(), lease_id));
            }
            Err(e) => eprintln!(
                "  lane job: could not publish the runner on lease {lease_id}, so the \
                 implementer cannot find it and its result says untested=true: {e:#}"
            ),
        }
        runner
    }

    /// Start the runner for `lane` on its own thread, unpublished.
    pub(crate) fn start(lane: LaneJobs) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let tally = Arc::new(Tally::default());
        let results = lane.results.clone();
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
            heartbeat: None,
            results,
            tally,
            published: None,
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
        if let Some(handle) = self.heartbeat.take() {
            let _ = handle.join();
            let _ = std::fs::remove_file(heartbeat_path(&self.results));
        }
        if let Some((main_root, lease_id)) = self.published.take() {
            let _ = unpublish(main_root.as_path(), &lease_id);
        }
    }
}

/// Write the first heartbeat for `record` now, then rewrite it every
/// [`HEARTBEAT_EVERY`] on its own thread until `stop`. Writing the first one
/// before returning means a seat launched right after publication already
/// finds the runner live. `None` when the first write fails: the seat then
/// never sees the runner as live and the lane reports `untested=true`.
// trace:BUG-2383 | ai:claude
fn start_heartbeat(
    results: &Path,
    record: RunnerRecord,
    stop: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    let beat = move |counter: u64| Heartbeat {
        protocol: record.protocol,
        runner_started_at: record.runner_started_at.clone(),
        counter,
        written_at_ms: unix_ms(std::time::SystemTime::now()),
    };
    if let Err(e) = write_heartbeat(results, &beat(0)) {
        eprintln!(
            "  lane job: could not write the runner heartbeat, so the implementer cannot \
             find the runner and its result says untested=true: {e:#}"
        );
        return None;
    }
    let results = results.to_path_buf();
    std::thread::Builder::new()
        .name("lane-job-heartbeat".into())
        .spawn(move || {
            let mut counter = 0;
            let mut last = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                if last.elapsed() >= HEARTBEAT_EVERY {
                    counter += 1;
                    let _ = write_heartbeat(&results, &beat(counter));
                    last = Instant::now();
                }
            }
        })
        .ok()
}

/// The lease file of `lease_id` under `main_root`.
fn lease_path(main_root: &Path, lease_id: &str) -> PathBuf {
    main_root
        .join(".aida")
        .join("sessions")
        .join(format!("{lease_id}.toml"))
}

/// Rewrite `lease_id`'s [`LEASE_FIELD`] under the lease write lock: set it to
/// `record`, or remove it.
// trace:BUG-2383 | ai:claude
fn rewrite_record(main_root: &Path, lease_id: &str, record: Option<&RunnerRecord>) -> Result<()> {
    use toml_edit::{value, DocumentMut, Item, Table};
    if !valid_lease_id(lease_id) {
        anyhow::bail!("`{lease_id}` is not a lease id");
    }
    let path = lease_path(main_root, lease_id);
    crate::session::with_lease_write_lock(main_root, || {
        let body = std::fs::read_to_string(&path)
            .with_context(|| format!("reading lease {}", path.display()))?;
        let mut doc = body
            .parse::<DocumentMut>()
            .with_context(|| format!("parsing lease {}", path.display()))?;
        match record {
            Some(record) => {
                let mut table = Table::new();
                table["protocol"] = value(i64::from(record.protocol));
                table["runner_pid"] = value(i64::from(record.runner_pid));
                table["runner_started_at"] = value(record.runner_started_at.clone());
                doc[LEASE_FIELD] = Item::Table(table);
            }
            None => {
                doc.remove(LEASE_FIELD);
            }
        }
        aida_core::write_atomic(&path, doc.to_string())
            .with_context(|| format!("writing lease {}", path.display()))
    })
}

/// Stamp this process as `lease_id`'s lane job runner, returning the record.
// trace:BUG-2383 | ai:claude
pub(crate) fn publish(main_root: &Path, lease_id: &str) -> Result<RunnerRecord> {
    let pid = std::process::id();
    let started = crate::process_probe::process_start_identity(pid)
        .context("this process has no readable start identity")?;
    let record = RunnerRecord {
        protocol: PROTOCOL,
        runner_pid: pid,
        runner_started_at: started,
    };
    rewrite_record(main_root, lease_id, Some(&record))?;
    Ok(record)
}

/// Clear `lease_id`'s runner record.
pub(crate) fn unpublish(main_root: &Path, lease_id: &str) -> Result<()> {
    rewrite_record(main_root, lease_id, None)
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
    /// Which path ran, for the lane log and every job's output: `seed:
    /// reflink`, `seed: warm`, `seed: overlay gen=<id>` or `seed: cold
    /// (<reason>)`.
    pub(crate) note: String,
    /// Nothing was seeded or already warm: the first build is cold, so a job
    /// gets [`MAX_TIMEOUT_SECS`] by default ([`timeout_for`]).
    pub(crate) cold: bool,
    /// The overlay every job of this lane mounts its profile directory on.
    #[cfg(unix)]
    pub(crate) overlay: Option<crate::seat_target_overlay::LaneOverlay>,
}

impl SeedNote {
    fn cold(target: Option<&Path>, why: &str) -> Self {
        Self::new(target, format!("seed: cold ({why})"), true)
    }

    fn new(target: Option<&Path>, note: String, cold: bool) -> Self {
        let note = match target {
            Some(t) => format!("seat target {}: {note}", t.display()),
            None => note,
        };
        Self {
            note,
            cold,
            #[cfg(unix)]
            overlay: None,
        }
    }
}

/// The main checkout of `lane`, from its results directory
/// (`<main>/.aida/lane-jobs/<lease>`).
fn lane_main_root(lane: &LaneJobs) -> Option<&Path> {
    lane.results.ancestors().nth(3)
}

/// Seed the seat's cargo target from the primary's before the first job, and
/// say which path ran.
fn seed_lane_target(lane: &LaneJobs) -> SeedNote {
    let note = seed_lane_target_with(lane, "unshare");
    lane.log(&note.note);
    note
}

/// [`seed_lane_target`] with the `unshare` program to use. Order: a lane
/// already on an overlay keeps it; else a reflink seed (or an already warm
/// target); else a generation overlay (BUG-2383 part 2); else cold. A cold
/// fallback only reports `seed: cold (<reason>)`: it never fails the lane.
// trace:BUG-2383 | ai:claude
pub(crate) fn seed_lane_target_with(lane: &LaneJobs, unshare: &str) -> SeedNote {
    let Some(target) = lane.target_dir() else {
        return SeedNote::cold(None, "the seat has no cargo target of its own");
    };
    let Some(main_target) = target
        .parent()
        .and_then(Path::parent)
        .filter(|t| t.file_name() == Some(std::ffi::OsStr::new("target")))
    else {
        return SeedNote::cold(Some(&target), "not under the primary's target");
    };
    #[cfg(unix)]
    let on_overlay = crate::seat_target_overlay::lane_has_overlay(main_target, &lane.lease_id);
    #[cfg(not(unix))]
    let on_overlay = false;
    let reflink_why = if on_overlay {
        "the lane runs on an overlay".to_string()
    } else {
        use crate::seat_target_seed::{seed_seat_target, Seeded, LOCK_WAIT};
        let results = seed_seat_target(main_target, &target, LOCK_WAIT);
        if results.iter().any(|(_, s)| *s == Seeded::Reflinked) {
            return SeedNote::new(Some(&target), "seed: reflink".into(), false);
        }
        if results.iter().any(|(_, s)| *s == Seeded::AlreadyWarm) {
            return SeedNote::new(Some(&target), "seed: warm".into(), false);
        }
        crate::seat_target_seed::describe(&results)
    };
    #[cfg(unix)]
    {
        match overlay_for(lane, main_target, &target, unshare) {
            Ok(overlay) => {
                let mut note = SeedNote::new(
                    Some(&target),
                    format!("seed: overlay gen={}", overlay.generation),
                    false,
                );
                note.overlay = Some(overlay);
                note
            }
            Err(why) => SeedNote::cold(Some(&target), &format!("{reflink_why}; overlay: {why}")),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = unshare;
        SeedNote::cold(Some(&target), &reflink_why)
    }
}

/// Snapshot (or reuse) the primary's current generation and pin it for this
/// lane (one step, under the lane-base lock), collect unpinned ones, ready
/// the lane's host-only overlay and prove it mounts.
#[cfg(unix)]
fn overlay_for(
    lane: &LaneJobs,
    main_target: &Path,
    target: &Path,
    unshare: &str,
) -> std::result::Result<crate::seat_target_overlay::LaneOverlay, String> {
    use crate::seat_target_overlay as ov;
    use crate::seat_target_seed::{LOCK_WAIT, SEED_PROFILES};
    let profile = SEED_PROFILES[0];
    let generation = ov::snapshot(main_target, profile, &lane.lease_id, LOCK_WAIT)?;
    if let Some(main_root) = lane_main_root(lane) {
        ov::gc(main_target, main_root, Some(&generation.dir), LOCK_WAIT);
    }
    let overlay = ov::prepare_lane(main_target, target, profile, &lane.lease_id, &generation)?;
    ov::probe(unshare, &overlay)?;
    Ok(overlay)
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
    // The sandbox binds the seat's cargo target with `--bind-try`, which
    // skips an absent path: on a cold (unseeded) target cargo would then
    // find it read-only and fail before compiling. The host creates it.
    // trace:BUG-2383 | ai:claude
    if let Some(target) = lane.target_dir() {
        if let Err(e) = std::fs::create_dir_all(&target) {
            return refuse(&format!(
                "create the cargo target {}: {e}",
                target.display()
            ));
        }
    }
    let (program, args, how_wrapped) = match lane.sandbox {
        // trace:BUG-2434 | ai:claude
        Sandbox::Bwrap | Sandbox::BwrapDefaultOn => {
            match crate::session::lane_job_wrapped_program_and_args(
                &lane.seat,
                lane.sandbox == Sandbox::BwrapDefaultOn
                    || crate::session::os_wrap_enabled(&lane.seat),
                &lane.sandbox_rw_paths(&env),
                &cargo.to_string_lossy(),
                args,
            ) {
                Ok((program, args)) => match overlay_wrapped(seed, program, args) {
                    Ok((program, args, how)) => (program, args, how),
                    Err(why) => return refuse(&why),
                },
                Err(e) => return refuse(&format!("{e:#}")),
            }
        }
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
        match std::fs::File::open(&out_file) {
            Ok(output) => tally.record_exit(&job, output),
            Err(_) => tally.record_exit(&job, std::io::empty()),
        }
    } else {
        tally.incomplete.fetch_add(1, Ordering::Relaxed);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&out_file) {
            let _ = writeln!(f, "\nlane job {how} after {}s", started.elapsed().as_secs());
        }
    }
    #[cfg(unix)]
    if let Some(overlay) = &seed.overlay {
        if crate::seat_target_overlay::stale(&overlay.lower) {
            use std::io::Write;
            let line = format!(
                "seed: overlay-stale (the primary rewrote a file of generation {} in place; \
                 only this seat's build is affected)",
                overlay.generation
            );
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&out_file) {
                let _ = writeln!(f, "lane job: {line}");
            }
            lane.log(&line);
        }
    }
    finish(lane, id, rc);
    lane.log(&format!(
        "{id} {how} rc={rc} after {}s",
        started.elapsed().as_secs()
    ));
}

/// The bubblewrap launch, inside the lane's overlay namespace when the seed
/// set one up (BUG-2383 part 2).
// trace:BUG-2383 | ai:claude
fn overlay_wrapped(
    seed: &SeedNote,
    program: String,
    args: Vec<String>,
) -> std::result::Result<(String, Vec<String>, &'static str), String> {
    #[cfg(unix)]
    if let Some(overlay) = &seed.overlay {
        // trace:BUG-2435 | ai:claude
        let (program, args) =
            crate::seat_target_overlay::wrap_lane_job("unshare", overlay, &program, args)?;
        return Ok((
            program,
            args,
            "inside bubblewrap, on the lane's target overlay",
        ));
    }
    let _ = seed;
    Ok((program, args, "inside bubblewrap"))
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

/// A lease id: the same shape as a job id, so it is one path component.
fn valid_lease_id(id: &str) -> bool {
    valid_id(id)
}

/// The fields of a lease the seat side reads; everything else is ignored.
#[derive(Debug, serde::Deserialize)]
struct LeaseView {
    worktree_path: PathBuf,
    #[serde(default)]
    lane_jobs: Option<RunnerRecord>,
}

/// Whether `record` names a live runner: its pid is running and has the
/// recorded start identity. Host side only (reap and diagnostics): a seat
/// tool shell has its own PID namespace and checks [`seat_runner_alive`].
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn runner_alive(record: &RunnerRecord) -> bool {
    crate::process_probe::process_start_identity(record.runner_pid).as_deref()
        == Some(record.runner_started_at.as_str())
}

/// Read `<main>/.aida/sessions/<id>.toml` refusing a symlink at any
/// component below `main`.
fn read_lease_nofollow(main: &Path, lease_id: &str) -> Result<String> {
    let mut path = main.to_path_buf();
    let file = format!("{lease_id}.toml");
    for part in [".aida", "sessions", file.as_str()] {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path)
            .with_context(|| format!("{} is missing", path.display()))?;
        if meta.file_type().is_symlink() {
            anyhow::bail!("{} is a symlink", path.display());
        }
    }
    #[cfg(unix)]
    let body = {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let mut body = String::new();
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.read_to_string(&mut body)
            .with_context(|| format!("read {}", path.display()))?;
        body
    };
    #[cfg(not(unix))]
    let body =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(body)
}

/// Resolve this seat's inbox and results directories from its lease:
/// `lease_id` is the one selector (`AIDA_SESSION_ID`), scoped by
/// `project_root` (`AIDA_PROJECT_ROOT`, else the git root of `cwd`); `cwd` is
/// the caller's directory. The project root may be the seat itself (a Codex
/// child's) or the main checkout; the lease is read from the main checkout
/// either way.
// trace:BUG-2383 | ai:claude
pub(crate) fn resolve_seat_dirs(
    project_root: Option<&Path>,
    lease_id: Option<&str>,
    cwd: &Path,
    alive: impl Fn(&RunnerRecord, &Path) -> bool,
) -> Result<(PathBuf, PathBuf)> {
    let lease_id = lease_id
        .filter(|id| valid_lease_id(id))
        .context("AIDA_SESSION_ID is not a lease id")?;
    let root = match project_root {
        Some(root) if root.is_absolute() => root.to_path_buf(),
        Some(_) => anyhow::bail!("AIDA_PROJECT_ROOT is not an absolute path"),
        None => crate::find_project_root_from(cwd)?,
    };
    let main = crate::main_worktree_root_from(&root);
    let lease: LeaseView = toml::from_str(&read_lease_nofollow(&main, lease_id)?)
        .with_context(|| format!("lease {lease_id} does not parse"))?;
    let worktree = std::fs::canonicalize(&lease.worktree_path)
        .with_context(|| format!("lease {lease_id}'s worktree does not resolve"))?;
    let here = std::fs::canonicalize(cwd).context("the current directory does not resolve")?;
    if !here.starts_with(&worktree) {
        anyhow::bail!(
            "the current directory {} is not inside lease {lease_id}'s worktree",
            here.display()
        );
    }
    let record = lease
        .lane_jobs
        .with_context(|| format!("lease {lease_id} names no lane job runner"))?;
    if record.protocol != PROTOCOL {
        anyhow::bail!(
            "lease {lease_id}'s runner speaks protocol {}, this aida speaks {PROTOCOL}",
            record.protocol
        );
    }
    let results = results_dir(&main, lease_id);
    if !alive(&record, &results) {
        anyhow::bail!(
            "lease {lease_id}'s lane job runner has no fresh heartbeat (it stopped, or never \
             started)"
        );
    }
    Ok((worktree.join(INBOX_RELATIVE), results))
}

/// This seat's inbox and results directories, resolved through its lease
/// ([`resolve_seat_dirs`]), or an error that says the lane has no runner.
pub(crate) fn seat_dirs() -> Result<(PathBuf, PathBuf)> {
    let root = std::env::var_os("AIDA_PROJECT_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let lease = std::env::var("AIDA_SESSION_ID").ok();
    let cwd = std::env::current_dir().context("no current directory")?;
    resolve_seat_dirs(root.as_deref(), lease.as_deref(), &cwd, seat_runner_alive).map_err(|why| {
        anyhow::anyhow!(
            "this session has no lane job runner ({why:#}). Only a headless implementer \
             launched by `aida queue work <SPEC> --no-human` has one. Report the work as \
             untested (untested=true)."
        )
    })
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

#[cfg(test)]
#[path = "tests/bug_2434_lane_os_wrap_tests.rs"]
mod bug_2434_tests;
