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
//! **A process-tree walk for interactive launches; a process GROUP for headless
//! ones (TASK-298).** When the orchestrator launches an *interactive* REPL,
//! `setsid` / `Command::process_group` would detach the child from the
//! controlling terminal and break REPL interactivity in default (non-`--zen`)
//! mode, where the user is still typing at it — so interactive launches pass
//! `own_process_group: false` and are reaped by walking the parent-pointer tree
//! at reap time, leaving the child in the orchestrator's session and foreground
//! group. A *headless* phase (`--no-human` implementer / reviewer) has no REPL
//! to keep interactive, so it passes `own_process_group: true`: the child is put
//! in its own process group (`Setpgid` via `Command::process_group(0)` on Unix)
//! and the **whole group** is reaped — `kill(-pgid, …)` — on *every* exit path
//! (success, error, sentinel, watchdog), not just the watchdog/timeout path.
//! The parent-pointer tree walk misses descendants that re-parented to init or
//! `setsid`'d into their own session (leaked agent test-worker pools); the
//! group signal catches them. The tree walk is retained as a belt-and-suspenders
//! pass and as the Windows fallback (process groups there need a Job Object,
//! left as a documented TODO — see [`sweep_process_group`]).
//!
//! trace:TASK-329 trace:TASK-298 | ai:claude

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

/// TASK-298: default `WaitDelay`-style backstop. After a watched child is known
/// to have exited (or been killed), the parent waits at most this long for the
/// direct child to be reaped before giving up rather than blocking forever — so
/// a descendant that inherited and still holds the stdout/stderr pipe open
/// cannot wedge the orchestrator. Override with `AIDA_WAIT_DELAY_MS`.
const DEFAULT_WAIT_DELAY_MS: u64 = 10_000;

/// Polling cadence + SIGTERM→SIGKILL grace window for [`spawn_and_wait`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExitSignalConfig {
    /// How often to check `try_wait` and the sentinel.
    pub(crate) poll: Duration,
    /// How long to wait after SIGTERM before escalating to SIGKILL.
    pub(crate) grace: Duration,
    /// TASK-298: upper bound on any post-exit `wait` for the direct child — the
    /// `WaitDelay` backstop so a held-open pipe cannot wedge the parent.
    pub(crate) wait_delay: Duration,
}

impl Default for ExitSignalConfig {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(DEFAULT_POLL_MS),
            grace: Duration::from_millis(DEFAULT_GRACE_MS),
            wait_delay: Duration::from_millis(DEFAULT_WAIT_DELAY_MS),
        }
    }
}

impl ExitSignalConfig {
    /// Resolve from `AIDA_EXIT_POLL_MS` / `AIDA_EXIT_GRACE_MS` /
    /// `AIDA_WAIT_DELAY_MS`, falling back to the defaults on an unset or
    /// unparseable value.
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
            wait_delay: Duration::from_millis(parse_ms(
                std::env::var("AIDA_WAIT_DELAY_MS").ok().as_deref(),
                DEFAULT_WAIT_DELAY_MS,
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
// The orchestrator's real phases all pass a (possibly-`None`) watchdog via
// `spawn_and_wait_watched`; this unwatched convenience wrapper is currently
// exercised only by the reap tests, so silence dead-code in the non-test build.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn spawn_and_wait(
    cmd: Command,
    sentinel: &Path,
    config: &ExitSignalConfig,
) -> std::io::Result<ExitOutcome> {
    spawn_and_wait_watched(cmd, sentinel, config, None, false)
}

/// TASK-298: convenience wrapper that opts into the headless process-GROUP
/// reap (own_process_group = true) without a watchdog. Exercised by the
/// process-group reap tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn spawn_and_wait_grouped(
    cmd: Command,
    sentinel: &Path,
    config: &ExitSignalConfig,
) -> std::io::Result<ExitOutcome> {
    spawn_and_wait_watched(cmd, sentinel, config, None, true)
}

// trace:BUG-420 trace:TASK-298 | ai:claude
/// BUG-420: [`spawn_and_wait`] plus an optional **watchdog**. On each poll tick
/// the watchdog closure is consulted; when it returns `Some(reason)` the child's
/// process tree is reaped (the same SIGTERM→grace→SIGKILL cascade as the
/// sentinel path) and the call returns [`ExitOutcome::WatchdogTripped`]. The
/// orchestrator arms this only for *headless* phases — an interactive REPL
/// legitimately idles waiting for the user, so it passes `None` and behaves
/// exactly as before. The closure owns its own git/mtime polling cadence
/// (rate-limited internally) so this tight poll loop stays cheap.
///
/// TASK-298: `own_process_group` is set by the headless caller (the same
/// headless boolean that arms the watchdog). When true the child is launched
/// in its own process group (`Setpgid` on Unix) and, on *every* exit path —
/// natural success/error, sentinel reap, and watchdog trip — the **whole group**
/// is swept (SIGTERM → bounded grace → SIGKILL to `-pgid`) so a leaked agent
/// test-worker pool that re-parented or `setsid`'d away cannot outlive the
/// phase. Interactive launches pass false and keep the original tree-walk-only
/// behavior so REPL interactivity is untouched.
pub(crate) fn spawn_and_wait_watched(
    mut cmd: Command,
    sentinel: &Path,
    config: &ExitSignalConfig,
    mut watchdog: Option<&mut dyn FnMut() -> Option<String>>,
    own_process_group: bool,
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
    // TASK-298: put the headless child in its own process group so the reap can
    // signal the whole group by negative pgid. On Unix `process_group(0)` makes
    // the child a new group leader (pgid == its own pid). Windows has no
    // equivalent in std — left as a documented TODO; the tree-walk reap still
    // applies there.
    if own_process_group {
        set_own_process_group(&mut cmd);
    }
    let mut child = cmd.spawn()?;
    // On Unix with `process_group(0)`, the child's pgid equals its pid. Capture
    // it now while the handle is alive; we sweep the group after the loop.
    let group_id = if own_process_group {
        Some(child.id())
    } else {
        None
    };

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

    // TASK-298: sweep the process group on EVERY exit path, not just the
    // watchdog one. On the natural-exit path the direct child is already gone
    // but its setsid/re-parented descendants (a leaked test-worker pool) are
    // not — the `reap` tree-walk above only runs on the reaped/watchdog paths
    // and only follows live parent pointers. The group signal catches everyone
    // the original group leader spawned, regardless of re-parenting. No-op when
    // the group is already empty, so the common clean-exit case pays nothing.
    if let Some(pgid) = group_id {
        sweep_process_group(pgid, config);
    }

    // Always clear the sentinel — a no-op on the natural-exit path, cleanup on
    // the reaped path — so a re-run never sees stale state.
    let _ = std::fs::remove_file(sentinel);
    Ok(outcome)
}

// trace:TASK-298 | ai:claude
/// TASK-298: put the to-be-spawned child in its own process group. Unix uses
/// `CommandExt::process_group(0)` (a new group whose pgid == the child's pid),
/// which needs no `unsafe`. Windows has no std equivalent; a true group reap
/// there needs a Job Object — left as a documented TODO, with the tree-walk
/// reap as the fallback.
fn set_own_process_group(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // TODO(TASK-298, Windows): wrap the child in a Job Object so its whole tree
    // can be terminated as a unit. Until then `own_process_group` is a no-op on
    // Windows and we fall back to the parent-pointer tree walk in `reap`.
    #[cfg(not(unix))]
    {
        let _ = cmd;
    }
}

/// Terminate the child's process tree: SIGTERM, wait up to `grace` for a clean
/// exit, then SIGKILL whatever survives. The child handle is `wait`ed at the
/// end so the orchestrator never leaves a zombie. TASK-298: the final wait is
/// bounded by `config.wait_delay` (the `WaitDelay` backstop) so a descendant
/// that inherited and still holds the stdout/stderr pipe open cannot wedge the
/// parent here.
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
    // TASK-298: bound the final reap-wait instead of a blocking `child.wait()`.
    wait_bounded(child, config)
}

// trace:TASK-298 | ai:claude
/// TASK-298: `WaitDelay` backstop — wait for the direct child to be reaped, but
/// no longer than `config.wait_delay`. A child that has been SIGKILLed dies
/// promptly; this only fires in the pathological case where the OS hasn't reaped
/// it yet (e.g. a descendant holding an inherited pipe). On timeout we return a
/// synthetic non-success status rather than block the orchestrator forever — the
/// process group sweep that follows still terminates the group.
fn wait_bounded(child: &mut Child, config: &ExitSignalConfig) -> std::io::Result<ExitStatus> {
    let deadline = Instant::now() + config.wait_delay;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            // Give up waiting on the handle; do not block the parent. The child
            // was already SIGKILLed, so the kernel will reap it shortly; we just
            // refuse to wedge here. Synthesize a failure status for diagnostics.
            return Ok(synthetic_failure_status());
        }
        std::thread::sleep(config.poll);
    }
}

// trace:TASK-298 | ai:claude
/// A non-success `ExitStatus` used by [`wait_bounded`] when the bounded wait
/// elapses. Unix builds a signal-terminated status (SIGKILL) via
/// `ExitStatusExt`; other platforms shell out to a guaranteed-failing command
/// so we never fabricate a `success()` status.
fn synthetic_failure_status() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // 9 == SIGKILL: we asked for a hard kill, so report it as such.
        ExitStatus::from_raw(9)
    }
    #[cfg(not(unix))]
    {
        // Portable fallback: a command that always exits non-zero.
        Command::new("cmd")
            .args(["/C", "exit 1"])
            .status()
            .unwrap_or_else(|_| {
                // Last resort: re-run a trivially failing program.
                Command::new("cmd")
                    .args(["/C", "exit 1"])
                    .status()
                    .expect("a failing status")
            })
    }
}

// trace:TASK-298 | ai:claude
/// TASK-298: sweep a process group — SIGTERM to `-pgid`, a bounded grace, then
/// SIGKILL to `-pgid` — catching every member regardless of re-parenting. A
/// `kill(-pgid, 0)` probe short-circuits when the group is already empty (the
/// common clean-exit case) so no latency is added there. Unix-only; on other
/// platforms this is a no-op (the tree-walk reap is the fallback — see
/// [`set_own_process_group`]).
fn sweep_process_group(pgid: u32, config: &ExitSignalConfig) {
    #[cfg(unix)]
    {
        // Probe: is anyone still in the group? signal 0 tests existence.
        if !process_group_alive(pgid) {
            return;
        }
        kill_process_group(pgid, libc::SIGTERM);

        let deadline = Instant::now() + config.grace;
        while Instant::now() < deadline {
            if !process_group_alive(pgid) {
                return;
            }
            std::thread::sleep(config.poll);
        }
        // Survivors after the grace window — hard-kill the whole group.
        kill_process_group(pgid, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = (pgid, config);
    }
}

/// `true` if the process group `pgid` still has at least one member. Uses
/// `kill(-pgid, 0)`: success or `EPERM` ⇒ a member exists; `ESRCH` ⇒ empty.
#[cfg(unix)]
fn process_group_alive(pgid: u32) -> bool {
    // SAFETY: `kill` with signal 0 performs only the existence/permission check
    // and delivers no signal. The negative pid targets the process group.
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    if rc == 0 {
        return true;
    }
    // errno == EPERM means the group exists but we may not signal it (still
    // "alive" for our purposes); ESRCH means no such group/process.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Send `signal` to the whole process group `pgid` via `kill(-pgid, signal)`.
#[cfg(unix)]
fn kill_process_group(pgid: u32, signal: libc::c_int) {
    // SAFETY: a negative pid directs the signal to the process group; we ignore
    // the result (ESRCH simply means the group already drained).
    unsafe {
        libc::kill(-(pgid as libc::pid_t), signal);
    }
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
            wait_delay: Duration::from_millis(5_000),
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
            wait_delay: Duration::from_millis(5_000),
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
            false,
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
            wait_delay: Duration::from_millis(5_000),
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

    /// TASK-298: the config now carries the `WaitDelay` backstop, and `from_env`
    /// resolves all three knobs (poll / grace / wait_delay) with the documented
    /// fallbacks.
    #[test]
    fn config_default_carries_wait_delay() {
        let c = ExitSignalConfig::default();
        assert_eq!(c.wait_delay, Duration::from_millis(DEFAULT_WAIT_DELAY_MS));
        assert_eq!(c.poll, Duration::from_millis(DEFAULT_POLL_MS));
        assert_eq!(c.grace, Duration::from_millis(DEFAULT_GRACE_MS));
    }

    /// TASK-298: the bounded-wait backstop never fabricates a `success()` status
    /// when it gives up on a wedged child — the caller must still see a failure.
    #[test]
    fn synthetic_failure_status_is_not_success() {
        assert!(!synthetic_failure_status().success());
    }

    /// TASK-298 (pure): the group-liveness probe is true for our own (obviously
    /// non-empty) process group and false for a pgid with no members.
    #[cfg(unix)]
    #[test]
    fn process_group_alive_distinguishes_present_and_empty() {
        // SAFETY: getpgrp() only reads the calling process's group id.
        let own = unsafe { libc::getpgrp() } as u32;
        assert!(
            process_group_alive(own),
            "our own process group must read as alive"
        );
        // A pgid far above any plausible pid_max — no such group exists.
        assert!(
            !process_group_alive(0x7FFF_FFF0),
            "an impossible pgid must read as empty"
        );
    }

    /// TASK-298 acceptance: a headless launch puts the child in its own process
    /// group; when the child exits *naturally* (success), a grandchild it
    /// backgrounded — which re-parents away from the child and so escapes the
    /// parent-pointer tree walk — is still terminated by the process-group
    /// sweep. This is the orphan-leak case (a leaked agent test-worker pool):
    /// the previous reap only ran on the sentinel/watchdog paths, never on the
    /// natural-exit path, so the pool survived. The group sweep closes it.
    #[cfg(unix)]
    #[test]
    fn group_reap_kills_grandchild_on_natural_exit() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");
        let pidfile = dir.path().join("gc.pid");

        let mut cmd = Command::new("sh");
        // Background a long-lived grandchild, record its pid, then EXIT WITHOUT
        // WAITING. The grandchild re-parents to init (escaping the tree walk)
        // but stays in the process group, so only the group sweep can reap it.
        cmd.arg("-c").arg(format!(
            r#"sleep 600 & echo $! > "{}"; exit 0"#,
            pidfile.display()
        ));

        let outcome = spawn_and_wait_grouped(cmd, &sentinel, &fast_config()).unwrap();
        assert!(
            matches!(&outcome, ExitOutcome::Natural(s) if s.success()),
            "expected a clean natural exit, got {outcome:?}"
        );

        // The grandchild pid is written just before the parent exits; poll for it.
        let mut gc_pid: Option<u32> = None;
        for _ in 0..100 {
            if let Ok(s) = std::fs::read_to_string(&pidfile) {
                if let Ok(v) = s.trim().parse() {
                    gc_pid = Some(v);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let gc_pid = gc_pid.expect("grandchild pid should have been recorded");

        let mut gone = false;
        for _ in 0..100 {
            if !pid_is_live(gc_pid) {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            gone,
            "grandchild pid {gc_pid} survived the process-group reap on a natural exit"
        );
    }

    /// TASK-298: an interactive (own_process_group = false) launch must behave
    /// exactly as before — no process group is created, and a child that exits
    /// on its own is still reported as a clean natural exit. (Guards the
    /// invariant that the REPL path is untouched.)
    #[cfg(unix)]
    #[test]
    fn ungrouped_natural_exit_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("s.exit-requested");

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("exit 3");

        let outcome = spawn_and_wait_watched(cmd, &sentinel, &fast_config(), None, false).unwrap();
        match outcome {
            ExitOutcome::Natural(status) => assert_eq!(status.code(), Some(3)),
            other => panic!("expected Natural(3), got {other:?}"),
        }
    }
}
