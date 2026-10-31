//! BUG-044: a headless implementer that ends its turn while its own
//! background build or test run is still going.
//!
//! A `claude -p` implementer that starts `cargo test` in the background and
//! then writes "Waiting on the test build." as its final result has not
//! finished; it has parked. Its launcher used to report `no-pr` and a human
//! had to `--resume` the session. Instead the launcher:
//!
//! - holds the linger reaper off while that work runs (it would otherwise
//!   kill the build with the vendor's tree, [`crate::headless_linger`]),
//! - waits for the work to exit, bounded, and
//! - resumes the same session with [`RESUME_PROMPT`], at most
//!   [`MAX_AUTO_RESUMES`] times.
//!
//! "The session's own work" is a build-shaped process carrying the
//! launcher's [`crate::standalone_implementer::MARKER_ENV`] value: every
//! process the child starts inherits it, even after it is reparented.
//!
//! trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude

use std::path::Path;
use std::time::{Duration, Instant};

/// How many times one launch resumes a session that parked on its own
/// background work.
pub(crate) const MAX_AUTO_RESUMES: u32 = 3;

/// The message a parked session is resumed with.
pub(crate) const RESUME_PROMPT: &str =
    "Your background run has finished (the launcher waited for it). \
Continue where you left off: read its output, finish the work, then commit, push and open the PR. \
Do not end your turn while a build or test you started is still running: run it in the foreground \
or poll it in bounded steps.";

/// How often [`wait_for_background_work`] looks again, and how often it
/// reports that it is still waiting.
const POLL: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_secs(60);

/// Phrases a final result uses when the session parked on background work
/// rather than finishing. Matched case-insensitively.
const WAITING_PHRASES: &[&str] = &[
    "in the background",
    "waiting on",
    "waiting for",
    "still running",
    "when they finish",
    "when it finishes",
    "once they finish",
    "once it finishes",
    "once the build",
    "once the test",
    "pick up the results",
    "pick up results",
    "will report back",
];

/// Whether `text` (a final result) says the session is waiting on work it
/// left running.
pub(crate) fn says_waiting_on_background(text: &str) -> bool {
    let lower = text.to_lowercase();
    WAITING_PHRASES.iter().any(|p| lower.contains(p))
}

/// Process names that are build or test work, not an MCP server or shell.
const BUILD_COMMS: &[&str] = &[
    "cargo",
    "rustc",
    "rustdoc",
    "clippy-driver",
    "cargo-clippy",
    "cargo-nextest",
    "make",
    "cc",
    "gcc",
    "ld",
    "rust-lld",
    "sccache",
];

/// Whether a process named `comm` running `exe` (its argv[0]) is build or
/// test work: a compiler driver, or a binary under a `target/` dir (a test
/// executable).
pub(crate) fn is_build_work(comm: &str, exe: &str) -> bool {
    BUILD_COMMS.contains(&comm) || comm.starts_with("cargo-") || exe.contains("/target/")
}

/// One live process the launcher's marker reached.
#[derive(Debug, Clone)]
pub(crate) struct MarkedProcess {
    pub(crate) pid: u32,
    pub(crate) comm: String,
    pub(crate) exe: String,
}

/// Live processes (other than this one) whose environment carries
/// `<marker_env>=<value>`. Linux reads `/proc`; elsewhere it finds none.
pub(crate) fn marked_processes(marker_env: &str, value: &str) -> Vec<MarkedProcess> {
    #[cfg(target_os = "linux")]
    {
        let needle = format!("{marker_env}={value}");
        let own = std::process::id();
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| *pid != own)
            .filter_map(|pid| {
                let dir = Path::new("/proc").join(pid.to_string());
                let environ = std::fs::read(dir.join("environ")).ok()?;
                environ
                    .split(|b| *b == 0)
                    .any(|kv| kv == needle.as_bytes())
                    .then_some(())?;
                let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
                let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
                let exe = cmdline
                    .split(|b| *b == 0)
                    .next()
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .unwrap_or_default();
                Some(MarkedProcess {
                    pid,
                    comm: comm.trim().to_string(),
                    exe,
                })
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (marker_env, value);
        Vec::new()
    }
}

/// The build or test processes the standalone implementer on lease
/// `lease_id` still has running.
pub(crate) fn live_background_work(lease_id: &str) -> Vec<MarkedProcess> {
    marked_processes(crate::standalone_implementer::MARKER_ENV, lease_id)
        .into_iter()
        .filter(|p| is_build_work(&p.comm, &p.exe))
        .collect()
}

/// Wait until `live()` finds nothing, or `ceiling` passes. `true` when the
/// work finished. Reports progress on stderr each minute, so a watcher sees
/// the launcher is waiting, not stuck.
pub(crate) fn wait_for_background_work(
    ceiling: Duration,
    live: impl Fn() -> Vec<MarkedProcess>,
) -> bool {
    let started = Instant::now();
    let mut reported = started;
    loop {
        let running = live();
        if running.is_empty() {
            return true;
        }
        if started.elapsed() >= ceiling {
            return false;
        }
        if reported.elapsed() >= PROGRESS_EVERY {
            reported = Instant::now();
            let names: Vec<String> = running
                .iter()
                .map(|p| format!("{} ({})", p.comm, p.pid))
                .collect();
            eprintln!(
                "  waiting {}s on the implementer's background run: {}",
                started.elapsed().as_secs(),
                names.join(", ")
            );
        }
        std::thread::sleep(POLL.min(ceiling.saturating_sub(started.elapsed())));
    }
}

/// Why a run that ended without a PR is resumed, or `None` to report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumeReason {
    /// Build or test work the session started is still running.
    LiveWork,
    /// The reaper was held off for such work while the vendor lingered.
    HeldForWork,
    /// The final result says the session is waiting on background work.
    SaidWaiting,
}

/// Decide whether to resume a run that ended without a PR. A run is only
/// resumed when it logged a final result (it ended its turn, it did not
/// crash), the vendor can resume, and the budget is not spent.
pub(crate) fn resume_reason(
    resumes_so_far: u32,
    can_resume: bool,
    final_result: Option<&str>,
    live_work: bool,
    held_for_work: bool,
) -> Option<ResumeReason> {
    if resumes_so_far >= MAX_AUTO_RESUMES || !can_resume {
        return None;
    }
    let text = final_result?;
    if live_work {
        Some(ResumeReason::LiveWork)
    } else if held_for_work {
        Some(ResumeReason::HeldForWork)
    } else if says_waiting_on_background(text) {
        Some(ResumeReason::SaidWaiting)
    } else {
        None
    }
}

/// How many paths `git status --porcelain` lists in `worktree`, or `None`
/// when it cannot be read.
pub(crate) fn uncommitted_paths(worktree: &Path) -> Option<usize> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).lines().count())
}

#[cfg(test)]
#[path = "tests/bug_044_headless_background_tests.rs"]
mod bug_044_headless_background_tests;
