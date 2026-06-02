//! Skill → orchestrator graceful-exit signal (TASK-329).
//!
//! # The problem
//!
//! `aida queue work --auto-complete` launches each Claude phase as a
//! subprocess and, today, blocks on `Command::status()` until that child
//! exits on its own. In interactive `--zen` mode the skill auto-resolves its
//! end-of-drain prompt and goes idle — but the Claude Code REPL stays open at
//! `❯`. There is no EOF for the skill to synthesize from inside its own
//! session (BUG-230), so the orchestrator waits forever on a child that will
//! never exit.
//!
//! # The protocol
//!
//! A one-way file signal — the skill says "I'm done, reap me":
//!
//! 1. The orchestrator picks a **sentinel path** under `.aida/sessions/`
//!    (`<session-id>.exit-requested` — a sibling of the `<session-id>.toml`
//!    lease files) and exports its absolute path to the child via the
//!    [`SENTINEL_ENV`] (`AIDA_EXIT_SENTINEL`) environment variable.
//! 2. The skill, as its **absolute last action** — after every commit, PR
//!    open, push, comment, and verdict write — runs
//!    `touch "$AIDA_EXIT_SENTINEL"`.
//! 3. The orchestrator, instead of a blocking `.status()`, spawns the child
//!    and polls ([`spawn_and_wait`]): each tick it checks whether the child
//!    exited on its own and whether the sentinel appeared. On the sentinel it
//!    terminates the child's process tree (SIGTERM, a grace window, then
//!    SIGKILL) and reaps it.
//!
//! The sentinel is purely **additive**: a child that never touches it is
//! still reaped the instant it exits on its own (the `try_wait` check), so
//! the non-orchestrated and interactive-non-`--zen` paths are unaffected.
//!
//! # Design notes
//!
//! **One env var carrying a full path, not `AIDA_SESSION_DIR` + a recomputed
//! `<session-id>`.** The orchestrator owns the path end to end, so the skill
//! never has to recompute a session id — it touches exactly the file the
//! orchestrator polls. That also keeps the protocol trivially extensible:
//! STORY-287's deferred `--no-human` punt case can ship a second env var
//! pointing at a `.punt-requested` sibling without touching this code. We
//! deliberately stop there — a generalized multi-message signal channel is
//! out of scope (see TASK-329 "Out of scope").
//!
//! **A process-tree walk, not `setsid` + `kill(-pgid)`.** The orchestrator
//! launches an *interactive* REPL. `setsid` (or `Command::process_group`)
//! would move the child into its own process group and detach it from the
//! controlling terminal — breaking REPL interactivity in default (non-`--zen`)
//! mode, where the user is still typing at it. Walking the tree at reap time
//! leaves the child in the orchestrator's session and foreground group, so
//! interactivity is untouched, and the same walk gives the Windows cascade
//! the spec calls for as a `tree-walk fallback`.
//!
//! trace:TASK-329 | ai:claude

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

/// Environment variable the orchestrator exports to the child; its value is
/// the absolute path of the sentinel file the skill `touch`es to ask for a
/// reap.
pub(crate) const SENTINEL_ENV: &str = "AIDA_EXIT_SENTINEL";

/// Default poll interval — tight enough that the reap feels instant, loose
/// enough not to spin a core. Override with `AIDA_EXIT_POLL_MS`.
const DEFAULT_POLL_MS: u64 = 100;

/// Default grace window between SIGTERM and SIGKILL. Override with
/// `AIDA_EXIT_GRACE_MS`.
const DEFAULT_GRACE_MS: u64 = 2_000;

/// Polling cadence + SIGTERM→SIGKILL grace window for [`spawn_and_wait`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExitSignalConfig {
    /// How often to check `try_wait` and the sentinel.
    pub(crate) poll: Duration,
    /// How long to wait after SIGTERM before escalating to SIGKILL.
    pub(crate) grace: Duration,
}

impl Default for ExitSignalConfig {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(DEFAULT_POLL_MS),
            grace: Duration::from_millis(DEFAULT_GRACE_MS),
        }
    }
}

impl ExitSignalConfig {
    /// Resolve from `AIDA_EXIT_POLL_MS` / `AIDA_EXIT_GRACE_MS`, falling back to
    /// the defaults on an unset or unparseable value.
    pub(crate) fn from_env() -> Self {
        Self {
            poll: Duration::from_millis(parse_ms(
                std::env::var("AIDA_EXIT_POLL_MS").ok().as_deref(),
                DEFAULT_POLL_MS,
            )),
            grace: Duration::from_millis(parse_ms(
                std::env::var("AIDA_EXIT_GRACE_MS").ok().as_deref(),
                DEFAULT_GRACE_MS,
            )),
        }
    }
}

/// Parse a millisecond override. An unset / blank / unparseable value falls
/// back to `default`; the poll loop must never spin, so the result is floored
/// at 1ms. Pure — unit-tested directly.
fn parse_ms(raw: Option<&str>, default: u64) -> u64 {
    raw.and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(default)
        .max(1)
}

/// The exit sentinel for an orchestrated session — a sibling of the
/// `<session-id>.toml` lease file under `.aida/sessions/`.
pub(crate) fn sentinel_path(sessions_dir: &Path, session_id: &str) -> PathBuf {
    sessions_dir.join(format!("{session_id}.exit-requested"))
}

/// How a child watched by [`spawn_and_wait`] ended.
#[derive(Debug)]
pub(crate) enum ExitOutcome {
    /// The child exited on its own — `status` is its real exit status. The
    /// caller decides whether a non-success status is a phase failure.
    Natural(ExitStatus),
    /// The skill touched the sentinel and the orchestrator reaped the child.
    /// This is a *clean* completion: the skill ran to the end of its flow and
    /// asked to be reaped, so the caller proceeds regardless of the
    /// (signal-terminated) status. The status is retained for diagnostics and
    /// the reap tests — the orchestrator itself does not branch on it.
    Reaped(#[allow(dead_code)] ExitStatus),
    /// BUG-420: the phase watchdog tripped — the child made no progress for the
    /// no-progress window (a degenerate echo/sleep spin) or blew past the
    /// wall-clock ceiling — so the orchestrator killed its process tree. The
    /// `String` is the one-line trip reason for the phase failure / shelve.
    /// This is NOT a clean completion: the caller turns it into a shelvable
    /// phase failure. trace:BUG-420 | ai:claude
    WatchdogTripped(String),
}

/// Spawn `cmd` and wait for it to finish — either by exiting on its own or by
/// the skill touching `sentinel`.
///
/// Exports `AIDA_EXIT_SENTINEL=<sentinel>` to the child so the skill knows
/// which file to touch, clears any stale sentinel before spawning, and removes
/// the sentinel again before returning so a re-run starts clean.
pub(crate) fn spawn_and_wait(
    cmd: Command,
    sentinel: &Path,
    config: &ExitSignalConfig,
) -> std::io::Result<ExitOutcome> {
    spawn_and_wait_watched(cmd, sentinel, config, None)
}

/// BUG-420: [`spawn_and_wait`] plus an optional **watchdog**. On each poll tick
/// the watchdog closure is consulted; when it returns `Some(reason)` the child's
/// process tree is reaped (the same SIGTERM→grace→SIGKILL cascade as the
/// sentinel path) and the call returns [`ExitOutcome::WatchdogTripped`]. The
/// orchestrator arms this only for *headless* phases — an interactive REPL
/// legitimately idles waiting for the user, so it passes `None` and behaves
/// exactly as before. The closure owns its own git/mtime polling cadence
/// (rate-limited internally) so this tight poll loop stays cheap.
/// trace:BUG-420 | ai:claude
pub(crate) fn spawn_and_wait_watched(
    mut cmd: Command,
    sentinel: &Path,
    config: &ExitSignalConfig,
    mut watchdog: Option<&mut dyn FnMut() -> Option<String>>,
) -> std::io::Result<ExitOutcome> {
    // A stale sentinel from a crashed prior run would make us reap the new
    // child instantly — clear it before spawning.
    let _ = std::fs::remove_file(sentinel);
    // The skill `touch`es the sentinel; make sure its parent dir exists even
    // if the child has not created the lease dir yet.
    if let Some(parent) = sentinel.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    cmd.env(SENTINEL_ENV, sentinel);
    let mut child = cmd.spawn()?;

    let outcome = loop {
        // Check 1: did the child exit on its own? This also covers the race
        // the spec calls out — the skill touches the sentinel and immediately
        // exits — because a natural exit is detected here first and treated
        // as a clean end.
        if let Some(status) = child.try_wait()? {
            break ExitOutcome::Natural(status);
        }
        // Check 2: did the skill ask to be reaped?
        if sentinel.exists() {
            break ExitOutcome::Reaped(reap(&mut child, config)?);
        }
        // Check 3 (BUG-420): did the watchdog trip? A degenerate headless phase
        // that stops making progress (no commit / file-change) or runs past the
        // ceiling is killed here rather than blocking the drain forever.
        if let Some(ref mut wd) = watchdog {
            if let Some(reason) = wd() {
                // Reap the tree before returning so we never leave a zombie /
                // an orphaned `claude -p` spinning in the background.
                let _ = reap(&mut child, config)?;
                break ExitOutcome::WatchdogTripped(reason);
            }
        }
        std::thread::sleep(config.poll);
    };

    // Always clear the sentinel — a no-op on the natural-exit path, cleanup on
    // the reaped path — so a re-run never sees stale state.
    let _ = std::fs::remove_file(sentinel);
    Ok(outcome)
}

/// Terminate the child's process tree: SIGTERM, wait up to `grace` for a clean
/// exit, then SIGKILL whatever survives. The child handle is `wait`ed at the
/// end so the orchestrator never leaves a zombie.
fn reap(child: &mut Child, config: &ExitSignalConfig) -> std::io::Result<ExitStatus> {
    let root = child.id();

    // SIGTERM the whole tree — give Claude (and any bash-tool / language-server
    // children) a chance to shut down cleanly.
    signal_process_tree(&collect_process_tree(root), TreeSignal::Term);

    let deadline = Instant::now() + config.grace;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(config.poll);
    }

    // Grace window elapsed — re-enumerate (a child may have spawned since the
    // SIGTERM pass) and SIGKILL whatever is still standing.
    signal_process_tree(&collect_process_tree(root), TreeSignal::Kill);
    child.wait()
}

/// Which signal [`signal_process_tree`] sends.
#[derive(Debug, Clone, Copy)]
enum TreeSignal {
    Term,
    Kill,
}

/// Collect `root` plus every transitive descendant PID, via `sysinfo`'s
/// parent-pointer graph. Returns `root` even when the process table can't be
/// read, so the caller always has at least the immediate child to signal.
fn collect_process_tree(root: u32) -> Vec<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};

    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());

    let mut tree = vec![Pid::from_u32(root)];
    let mut idx = 0;
    // Breadth-first walk: for each known process, pull in its children. Only
    // unseen PIDs are added, so a parent-pointer cycle can't loop forever.
    while idx < tree.len() {
        let parent = tree[idx];
        idx += 1;
        for (pid, proc_) in sys.processes() {
            if proc_.parent() == Some(parent) && !tree.contains(pid) {
                tree.push(*pid);
            }
        }
    }
    tree.into_iter().map(|p| p.as_u32()).collect()
}

/// Send `signal` to each PID. `sysinfo::Process::kill_with` returns `None`
/// when the platform has no such signal (Windows has no SIGTERM) — there we
/// fall back to a hard `kill()`.
fn signal_process_tree(pids: &[u32], signal: TreeSignal) {
    use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, Signal, System};

    let mut sys =
        System::new_with_specifics(RefreshKind::new().with_processes(ProcessRefreshKind::new()));
    sys.refresh_processes_specifics(ProcessRefreshKind::new());

    let sig = match signal {
        TreeSignal::Term => Signal::Term,
        TreeSignal::Kill => Signal::Kill,
    };
    for &pid in pids {
        if let Some(proc_) = sys.process(Pid::from_u32(pid)) {
            if proc_.kill_with(sig).is_none() {
                proc_.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_config() -> ExitSignalConfig {
        ExitSignalConfig {
            poll: Duration::from_millis(20),
            grace: Duration::from_millis(2_000),
        }
    }

    #[test]
    fn parse_ms_falls_back_and_floors() {
        assert_eq!(parse_ms(None, 100), 100);
        assert_eq!(parse_ms(Some(""), 100), 100);
        assert_eq!(parse_ms(Some("  "), 100), 100);
        assert_eq!(parse_ms(Some("not-a-number"), 100), 100);
        assert_eq!(parse_ms(Some("250"), 100), 250);
        assert_eq!(parse_ms(Some("  250  "), 100), 250);
        // The poll loop must never spin: a 0 is floored to 1ms.
        assert_eq!(parse_ms(Some("0"), 100), 1);
    }

    #[test]
    fn sentinel_path_is_a_lease_sibling() {
        let p = sentinel_path(Path::new("/proj/.aida/sessions"), "019e3869abcd");
        assert_eq!(
            p,
            Path::new("/proj/.aida/sessions/019e3869abcd.exit-requested")
        );
    }

    /// Integration: a mock subprocess touches the sentinel after a delay; the
    /// orchestrator must reap it well inside the grace + polling window rather
    /// than block on the (never-ending) child.
    #[cfg(unix)]
    #[test]
    fn reaps_child_after_sentinel_touch() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        // Touch the sentinel, then idle "forever" — the skill going idle at
        // the REPL after its last action.
        cmd.arg("-c")
            .arg(r#"sleep 0.2; touch "$AIDA_EXIT_SENTINEL"; sleep 600"#);

        let start = Instant::now();
        let outcome = spawn_and_wait(cmd, &sentinel, &fast_config()).unwrap();
        let elapsed = start.elapsed();

        assert!(
            matches!(outcome, ExitOutcome::Reaped(_)),
            "expected Reaped, got {outcome:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "reap took too long: {elapsed:?}"
        );
        assert!(!sentinel.exists(), "sentinel should be cleaned up");
    }

    /// Integration: a mock subprocess that exits on its own without ever
    /// touching the sentinel goes down the existing path, and the
    /// sentinel-cleanup attempt is a harmless no-op.
    #[cfg(unix)]
    #[test]
    fn natural_exit_without_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("exit 0");

        let outcome = spawn_and_wait(cmd, &sentinel, &fast_config()).unwrap();
        match outcome {
            ExitOutcome::Natural(status) => assert!(status.success()),
            other => panic!("expected Natural, got {other:?}"),
        }
        assert!(!sentinel.exists());
    }

    /// Integration: the race the spec calls out — the skill touches the
    /// sentinel and immediately exits. Whichever check wins, the outcome is
    /// clean (never a `Natural` failure) and the sentinel is cleaned up.
    #[cfg(unix)]
    #[test]
    fn sentinel_touch_then_immediate_exit() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(r#"touch "$AIDA_EXIT_SENTINEL"; exit 0"#);

        let outcome = spawn_and_wait(cmd, &sentinel, &fast_config()).unwrap();
        let clean = match outcome {
            ExitOutcome::Natural(status) => status.success(),
            ExitOutcome::Reaped(_) => true,
            ExitOutcome::WatchdogTripped(_) => false,
        };
        assert!(clean, "race outcome should be a clean exit");
        assert!(!sentinel.exists());
    }

    /// BUG-420: a watchdog that trips reaps a long-running child and returns
    /// `WatchdogTripped` with the reason — the orchestrator never blocks forever
    /// on a degenerate phase. The child here would otherwise loop indefinitely.
    #[cfg(unix)]
    #[test]
    fn watchdog_trip_reaps_a_running_child() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        // Never touches the sentinel, never exits — only the watchdog can stop it.
        cmd.arg("-c").arg("while true; do sleep 0.05; done");

        let config = ExitSignalConfig {
            poll: Duration::from_millis(20),
            grace: Duration::from_millis(200),
        };
        // Trip on the second poll so the child is genuinely running first.
        let mut ticks = 0;
        let mut watchdog = move || {
            ticks += 1;
            (ticks >= 2).then(|| "no-progress (test)".to_string())
        };
        let start = Instant::now();
        let outcome = spawn_and_wait_watched(
            cmd,
            &sentinel,
            &config,
            Some(&mut watchdog as &mut dyn FnMut() -> Option<String>),
        )
        .unwrap();
        let elapsed = start.elapsed();

        match outcome {
            ExitOutcome::WatchdogTripped(reason) => {
                assert_eq!(reason, "no-progress (test)");
            }
            other => panic!("expected WatchdogTripped, got {other:?}"),
        }
        assert!(
            elapsed < Duration::from_secs(5),
            "watchdog reap hung: {elapsed:?}"
        );
        assert!(!sentinel.exists());
    }

    /// Integration: a child that ignores SIGTERM must still be reaped — the
    /// orchestrator escalates to SIGKILL once the grace window elapses.
    #[cfg(unix)]
    #[test]
    fn sigkill_fires_when_sigterm_is_ignored() {
        use std::os::unix::process::ExitStatusExt;

        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        // `trap '' TERM` ignores SIGTERM; the `while` loop keeps `sh` itself
        // alive as the process (so the trap is never lost to an `exec` of a
        // trailing simple command).
        cmd.arg("-c")
            .arg(r#"trap '' TERM; touch "$AIDA_EXIT_SENTINEL"; while true; do sleep 0.05; done"#);

        let config = ExitSignalConfig {
            poll: Duration::from_millis(20),
            grace: Duration::from_millis(300),
        };
        let start = Instant::now();
        let outcome = spawn_and_wait(cmd, &sentinel, &config).unwrap();
        let elapsed = start.elapsed();

        let status = match outcome {
            ExitOutcome::Reaped(status) => status,
            other => panic!("expected Reaped, got {other:?}"),
        };
        // SIGTERM was ignored, so the reap had to wait out the grace window.
        assert!(
            elapsed >= Duration::from_millis(250),
            "reap returned before the grace window: {elapsed:?}"
        );
        assert!(elapsed < Duration::from_secs(5), "reap hung: {elapsed:?}");
        // The immediate child was killed by SIGKILL (signal 9).
        assert_eq!(status.signal(), Some(9), "expected SIGKILL");
        assert!(!sentinel.exists());
    }

    /// Integration: the reap must cascade — a grandchild spawned by the mock
    /// subprocess is killed along with its parent (the process-tree walk).
    #[cfg(unix)]
    #[test]
    fn reap_cascades_to_descendant_processes() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");
        let pidfile = dir.path().join("grandchild.pid");

        let mut cmd = Command::new("sh");
        // Background a long-lived grandchild, record its PID, signal exit,
        // then `wait` so the parent itself stays alive at the sentinel.
        cmd.arg("-c").arg(format!(
            r#"sleep 600 & echo $! > "{}"; touch "$AIDA_EXIT_SENTINEL"; wait"#,
            pidfile.display()
        ));

        spawn_and_wait(cmd, &sentinel, &fast_config()).unwrap();

        let gc_pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        // The grandchild should be gone — poll briefly to let the OS reap it.
        let mut gone = false;
        for _ in 0..100 {
            if !pid_is_live(gc_pid) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone, "grandchild pid {gc_pid} survived the reap");
    }

    #[cfg(unix)]
    fn pid_is_live(pid: u32) -> bool {
        use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System};
        let mut sys = System::new_with_specifics(
            RefreshKind::new().with_processes(ProcessRefreshKind::new()),
        );
        sys.refresh_processes_specifics(ProcessRefreshKind::new());
        sys.process(Pid::from_u32(pid)).is_some()
    }
}
