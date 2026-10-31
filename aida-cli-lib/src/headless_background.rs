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

/// The message a session is resumed with when its background run was still
/// going at the ceiling and the launcher stopped it.
pub(crate) const TERMINATED_PROMPT: &str =
    "Your background run was TERMINATED by the launcher: it was still running at the wait ceiling, \
so it did not finish and its results are incomplete. Rerun what you need in the foreground (narrow it \
to the tests you touched), finish the work, then commit, push and open the PR. Do not end your turn \
while a build or test you started is still running.";

/// The prompt a resume for `reason` is sent with.
pub(crate) fn resume_prompt(reason: &ResumeReason) -> &'static str {
    match reason {
        ResumeReason::Terminated => TERMINATED_PROMPT,
        _ => RESUME_PROMPT,
    }
}

/// How often [`wait_for_background_work`] looks again, and how often it
/// reports that it is still waiting.
const POLL: Duration = Duration::from_secs(2);
const PROGRESS_EVERY: Duration = Duration::from_secs(60);

/// Phrases a final result uses when the session parked on background work
/// rather than finishing. Matched case-insensitively, and only in a clause
/// that also names build or test work ([`BUILD_WORDS`]): "waiting for the
/// advisor" or "the dev server is still running" is not a parked build.
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

/// Word stems that make a clause about build or test work. A word matches
/// when it starts with one; `runs` must match whole.
const BUILD_WORDS: &[&str] = &[
    "build", "test", "cargo", "nextest", "compil", "clippy", "rustc",
];

/// Whether `text` (a final result) says the session is waiting on build or
/// test work it left running: one clause (split on `.`, `;`, `!`, `?`, `:`
/// and newlines) carries a waiting phrase and a build word.
pub(crate) fn says_waiting_on_background(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.split(['.', ';', '!', '?', ':', '\n']).any(|clause| {
        WAITING_PHRASES.iter().any(|p| clause.contains(p))
            && clause
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w == "runs" || BUILD_WORDS.iter().any(|b| w.starts_with(b)))
    })
}

/// Process names that are build or test work, not an MCP server, a shell
/// or a long-lived daemon. `sccache` is left out on purpose: a session's
/// first cargo can start the sccache server, which inherits the marker and
/// outlives the build by its idle timeout (or forever).
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
];

/// Whether a process named `comm` running `exe` (its argv[0]) is build or
/// test work: a compiler driver, or a test executable, which cargo always
/// builds under `target/<profile>/deps/`. A server run from `target/debug/`
/// (say by `cargo run`) is not test work and does not hold the lane.
pub(crate) fn is_build_work(comm: &str, exe: &str) -> bool {
    BUILD_COMMS.contains(&comm)
        || (comm.starts_with("cargo-") && comm != "cargo-sccache")
        || (exe.contains("/target/") && exe.contains("/deps/"))
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
    build_work_among(marked_processes(
        crate::standalone_implementer::MARKER_ENV,
        lease_id,
    ))
}

/// The build or test work in a snapshot of marked processes: a lingering
/// daemon (sccache, a server) that inherited the marker is dropped. Pure, so
/// the decision is tested on an injected snapshot, not the host's `/proc`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn build_work_among(
    snapshot: impl IntoIterator<Item = MarkedProcess>,
) -> Vec<MarkedProcess> {
    snapshot
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

/// Send SIGTERM to the build or test work `live()` finds, give it a few
/// seconds, then SIGKILL what is left. Used when the work outlived the wait
/// ceiling.
pub(crate) fn terminate_background_work(live: impl Fn() -> Vec<MarkedProcess>) {
    #[cfg(unix)]
    {
        let signal_all = |sig: libc::c_int| {
            for p in live() {
                // SAFETY: kill(2) on a pid just read from /proc; no memory is touched.
                unsafe {
                    libc::kill(p.pid as libc::pid_t, sig);
                }
            }
        };
        signal_all(libc::SIGTERM);
        let started = Instant::now();
        while !live().is_empty() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(100));
        }
        signal_all(libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = live;
    }
}

/// The launcher's hold on the linger reaper for one run: on while the
/// session's build or test work is live, off once it ends or the ceiling
/// passes. Hitting the ceiling is recorded, since the reaper then kills
/// that work with the vendor's tree.
pub(crate) struct ReapHold {
    ceiling: Duration,
    started: std::cell::Cell<Option<Instant>>,
    held: std::cell::Cell<bool>,
    expired: std::cell::Cell<bool>,
}

impl ReapHold {
    pub(crate) fn new(ceiling: Duration) -> Self {
        Self {
            ceiling,
            started: std::cell::Cell::new(None),
            held: std::cell::Cell::new(false),
            expired: std::cell::Cell::new(false),
        }
    }

    /// Whether to keep holding, given whether work is live now. The ceiling
    /// counts from the first check.
    pub(crate) fn check(&self, live: bool) -> bool {
        let started = self.started.get().unwrap_or_else(Instant::now);
        self.started.set(Some(started));
        if !live {
            return false;
        }
        if started.elapsed() >= self.ceiling {
            self.expired.set(true);
            return false;
        }
        self.held.set(true);
        true
    }

    /// Whether the reaper was held off for live work at all.
    pub(crate) fn held(&self) -> bool {
        self.held.get()
    }

    /// Whether the hold ran out with the work still live.
    pub(crate) fn expired(&self) -> bool {
        self.expired.get()
    }
}

/// How the background work of a run that is about to be resumed ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settled {
    /// It exited by itself (or there was none to wait for).
    Finished,
    /// It was still running at the ceiling and was stopped.
    Terminated,
}

/// Before a resume: wait for the work `live()` finds, bounded by `ceiling`.
/// Work still running at the ceiling, or a run whose reaper hold already
/// expired (`hold_expired`: the reaper killed that work), is stopped with
/// `terminate` and reported [`Settled::Terminated`], never as finished.
pub(crate) fn settle_background_work(
    ceiling: Duration,
    hold_expired: bool,
    live: impl Fn() -> Vec<MarkedProcess>,
    terminate: impl FnOnce(),
) -> Settled {
    if hold_expired {
        terminate();
        return Settled::Terminated;
    }
    if live().is_empty() || wait_for_background_work(ceiling, &live) {
        return Settled::Finished;
    }
    terminate();
    Settled::Terminated
}

/// Why a run that ended without a PR is resumed, or `None` to report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumeReason {
    /// The session's background run outlived the wait ceiling and was
    /// stopped; the session must rerun it.
    Terminated,
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
/// `hold_expired` is a reaper hold that ran out with the work still live.
pub(crate) fn resume_reason(
    resumes_so_far: u32,
    can_resume: bool,
    final_result: Option<&str>,
    live_work: bool,
    held_for_work: bool,
    hold_expired: bool,
) -> Option<ResumeReason> {
    if resumes_so_far >= MAX_AUTO_RESUMES || !can_resume {
        return None;
    }
    let text = final_result?;
    if hold_expired {
        Some(ResumeReason::Terminated)
    } else if live_work {
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
/// when it cannot be read. Built with the hardened host git builder, which
/// refuses inside a seat clone (`None` then).
pub(crate) fn uncommitted_paths(worktree: &Path) -> Option<usize> {
    let out = aida_core::git_ops::host_git_command(worktree)
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
