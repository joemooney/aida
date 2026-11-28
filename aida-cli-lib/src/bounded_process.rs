// trace:TASK-3428 | ai:codex
use crate::RetryEtxtbsy;

/// BUG-1288: the wall-clock ceiling a single forge-CLI subprocess (`gh`/`glab`)
/// may run before [`command_output_with_timeout`] gives up on it. Measured
/// cause of this spec's `aida status --full` / `aida awaiting --json` stall:
/// one `gh api .../branches/main/protection` call took 10.5s in this
/// repository while every other `gh` call in the same run finished in under a
/// second — `gh` itself has no request-timeout flag, so an occasional slow
/// endpoint or rate-limit backoff can otherwise consume the whole machine-
/// readable budget on ONE subprocess.
// trace:BUG-1288 | ai:claude
pub(crate) const FORGE_CLI_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Review follow-up (BUG-1288): on unix, kill the CHILD'S WHOLE PROCESS
/// GROUP rather than only the direct child. `command_output_with_timeout`
/// spawns with `process_group(0)` below, which makes the child's own pid its
/// process group id, so any grandchild it forks (a credential-manager
/// helper, a background `git` op) inherits that same group — `killpg` reaps
/// the group in one signal instead of leaving a grandchild alive to hold the
/// stdout/stderr pipe write ends open past the timeout. `pid` must be a pid
/// this process spawned with `process_group(0)` (the only caller), which is
/// what makes it both safe to signal and a valid process-group id.
// trace:BUG-1288 | ai:claude
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    // SAFETY: `pid` is this process's own child (see the doc comment above),
    // and `libc::killpg` is a plain signal-delivery syscall — no pointers,
    // no aliasing concerns.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

// trace:TASK-1535 | ai:codex
pub(crate) enum BoundedCommandOutput {
    Completed(std::process::Output),
    SpawnFailed,
    TimedOut,
}

pub(crate) fn command_output_with_timeout_detail(
    cmd: std::process::Command,
    timeout: std::time::Duration,
) -> BoundedCommandOutput {
    command_output_with_timeout_capped(cmd, timeout, u64::MAX)
}

/// [`command_output_with_timeout_detail`] that also stops reading either
/// pipe after `cap` bytes: the reader keeps `cap + 1` bytes, kills the
/// process group and stops, so a child streaming without end can neither
/// grow this process nor run to the timeout. A captured stream longer than
/// `cap` means the output was cut off.
// trace:TASK-2053 | ai:claude
pub(crate) fn command_output_with_timeout_capped(
    cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: u64,
) -> BoundedCommandOutput {
    run_bounded(cmd, timeout, cap, false, false)
        .map_or(BoundedCommandOutput::SpawnFailed, |(out, _, _)| out)
}

/// [`command_output_with_timeout_detail`] with the spawn error kept: `Err`
/// carries its errno (ENOENT, EACCES); `Ok` never holds
/// [`BoundedCommandOutput::SpawnFailed`].
// trace:STORY-3120 | ai:claude
pub(crate) fn command_output_with_timeout_io(
    cmd: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<BoundedCommandOutput> {
    run_bounded(cmd, timeout, u64::MAX, false, false).map(|(out, _, _)| out)
}

/// How long a `<cli> --version` availability probe may run. An agent CLI
/// that waits on a TTY, a login or the network in CI used to hold `aida init`
/// until the CI job's 45-minute limit killed it.
// trace:TASK-3965 | ai:claude
pub(crate) const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Result of [`probe_version`].
// trace:TASK-3965 | ai:claude
pub(crate) enum VersionProbe {
    /// The program exited; its status may still be non-zero.
    Answered(std::process::Output),
    /// The program could not be started (not installed, not executable).
    Missing,
    /// The program was killed after not answering within the bound.
    TimedOut,
}

impl VersionProbe {
    /// The `--version` banner (stdout then stderr) of a successful run.
    pub(crate) fn banner(&self) -> Option<String> {
        match self {
            Self::Answered(out) if out.status.success() => Some(format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )),
            _ => None,
        }
    }
}

/// The one way to run `<program> --version`: stdin is /dev/null, the run is
/// bounded by `timeout`, and the whole process group is killed at the bound.
// trace:TASK-3965 | ai:claude
pub(crate) fn probe_version_within(
    program: impl AsRef<std::ffi::OsStr>,
    timeout: std::time::Duration,
) -> VersionProbe {
    let program = program.as_ref();
    let mut cmd = std::process::Command::new(program);
    cmd.arg("--version").stdin(std::process::Stdio::null());
    match command_output_with_timeout_io(cmd, timeout) {
        Ok(BoundedCommandOutput::Completed(out)) => VersionProbe::Answered(out),
        Ok(BoundedCommandOutput::TimedOut) => {
            eprintln!(
                "Note: {}",
                version_probe_timeout_message(&program.to_string_lossy(), timeout)
            );
            VersionProbe::TimedOut
        }
        Ok(BoundedCommandOutput::SpawnFailed) | Err(_) => VersionProbe::Missing,
    }
}

/// [`probe_version_within`] at [`VERSION_PROBE_TIMEOUT`].
pub(crate) fn probe_version(program: impl AsRef<std::ffi::OsStr>) -> VersionProbe {
    probe_version_within(program, VERSION_PROBE_TIMEOUT)
}

/// True when `program --version` exits 0 within [`VERSION_PROBE_TIMEOUT`].
pub(crate) fn version_probe_available(program: impl AsRef<std::ffi::OsStr>) -> bool {
    probe_version(program).banner().is_some()
}

pub(crate) fn version_probe_timeout_message(program: &str, timeout: std::time::Duration) -> String {
    format!(
        "`{program}` did not answer --version within {}s; treating it as unavailable.",
        timeout.as_secs()
    )
}

/// Retain stderr collected after killing a scheduled child for phase diagnostics.
// trace:BUG-2667 | ai:codex
pub(crate) fn command_output_with_timeout_stderr(
    cmd: std::process::Command,
    timeout: std::time::Duration,
) -> (BoundedCommandOutput, Vec<u8>) {
    run_bounded(cmd, timeout, u64::MAX, false, true).map_or(
        (BoundedCommandOutput::SpawnFailed, Vec::new()),
        |(out, _, timeout_stderr)| (out, timeout_stderr),
    )
}

/// [`command_output_with_timeout_capped`] for a long headless child that
/// launches work in process groups of its own (a reviewer `aida queue work`
/// and its vendor). The child gets its own group, with the stdin `cmd` sets;
/// SIGINT, SIGTERM and SIGHUP this process receives while it
/// runs are forwarded to the child's whole tree, and at the bound, the cap or
/// a forwarded signal's grace the tree is SIGKILLed, descendants in other
/// groups included. Returns the forwarded signal, if any, for the caller to
/// re-raise. `run_review_verb` re-raises it right after one log line, before
/// any outcome is recorded, so the dying worker's request is closed later by
/// the dead-worker reap, not with a forwarded-signal summary. A spawn failure comes back as
/// `Err` with its cause; `Ok` never holds [`BoundedCommandOutput::SpawnFailed`].
// trace:STORY-1917 | ai:claude
pub(crate) fn command_output_with_timeout_forwarding(
    cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: u64,
) -> std::io::Result<(BoundedCommandOutput, Option<i32>)> {
    run_bounded(cmd, timeout, cap, true, false).map(|(out, signal, _)| (out, signal))
}

/// How long a child forwarded a terminal signal may take to exit before its
/// tree is SIGKILLed.
#[cfg(unix)]
const FORWARDED_SIGNAL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Kill a child spawned by [`run_bounded`]: its group, and with `forward`
/// also every group its descendants moved into.
// trace:STORY-1917 | ai:claude
#[cfg(unix)]
fn kill_bounded_child(pid: u32, forward: bool) {
    if forward {
        let tree = crate::exit_signal::collect_process_tree(pid);
        crate::forwarded_signals::signal_child_tree(pid, &tree, libc::SIGKILL);
    } else {
        kill_process_group(pid);
    }
}

/// Test hooks (TASK-3792), kept in a cfg(test) module so the ambient-input
/// inventory never sees them as production inputs.
#[cfg(test)]
pub(crate) mod test_hooks {
    /// Every child [`super::run_bounded`] spawns while a scoped read deadline
    /// is active carries this variable, so a fixture's fake binary can count
    /// the spawns that ran without one.
    pub(crate) const SCOPED_DEADLINE_SPAWN_ENV: &str = "AIDA_TEST_SCOPED_DEADLINE_SPAWN";
}

fn run_bounded(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: u64,
    forward: bool,
    retain_timeout_stderr: bool,
) -> std::io::Result<(BoundedCommandOutput, Option<i32>, Vec<u8>)> {
    use std::io::Read;

    // TASK-3792: a scoped read deadline (`aida pr train --check --timeout`)
    // tightens every bounded run made under it, whatever its own ceiling,
    // and nothing is spawned once it has passed: the caller reads that as a
    // timeout, never as a result.
    // trace:TASK-3792 | ai:claude
    let scoped = crate::forge_lookup::forge_lookup_deadline();
    let timeout = match scoped {
        Some(deadline) => {
            timeout.min(deadline.saturating_duration_since(std::time::Instant::now()))
        }
        None => timeout,
    };
    if scoped.is_some() && timeout.is_zero() {
        return Ok((BoundedCommandOutput::TimedOut, None, Vec::new()));
    }
    #[cfg(test)]
    if scoped.is_some() {
        cmd.env(test_hooks::SCOPED_DEADLINE_SPAWN_ENV, "1");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // Installed before the spawn so a signal during it is forwarded too.
    #[cfg(unix)]
    let mut forwarding = if forward {
        Some(crate::forwarded_signals::Forwarding::install()?)
    } else {
        None
    };
    #[cfg(not(unix))]
    let _ = forward;

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // BUG-1735: a bare spawn here turns a transient ETXTBSY into
        // `SpawnFailed`, which discards the errno and reads as "could not
        // start". Every bounded-run caller inherits that, so the retry
        // belongs in the wrapper, not at each call site.
        // trace:BUG-1735 | ai:claude
        .spawn_retrying_etxtbsy()?;
    // Only used to target the kill below; on non-unix targets nothing reads
    // it, so it is cfg-gated too rather than left as a dead binding.
    #[cfg(unix)]
    let pid = child.id();
    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    // trace:TASK-2053 | ai:claude
    let read_capped = move |pipe: &mut dyn Read| {
        let mut buf = Vec::new();
        let _ = pipe.take(cap.saturating_add(1)).read_to_end(&mut buf);
        if buf.len() as u64 > cap {
            #[cfg(unix)]
            kill_bounded_child(pid, forward);
        }
        buf
    };
    std::thread::spawn(move || {
        let _ = stdout_tx.send(read_capped(&mut stdout_pipe));
    });
    // A detached descendant can keep stderr open after the kill. Keep a
    // bounded snapshot while reading so the scheduled child can still report
    // its progress if the reader never reaches EOF.
    const STDERR_SNAPSHOT_LIMIT: usize = 64 * 1024;
    let stderr_snapshot =
        retain_timeout_stderr.then(|| std::sync::Arc::new(std::sync::Mutex::new(Vec::new())));
    let reader_snapshot = stderr_snapshot.clone();
    std::thread::spawn(move || {
        let stderr = if let Some(snapshot) = reader_snapshot {
            let mut pipe = stderr_pipe.take(cap.saturating_add(1));
            let mut stderr = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        stderr.extend_from_slice(&chunk[..n]);
                        if let Ok(mut snapshot) = snapshot.lock() {
                            let remaining = STDERR_SNAPSHOT_LIMIT - snapshot.len();
                            snapshot.extend_from_slice(&chunk[..n.min(remaining)]);
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            if stderr.len() as u64 > cap {
                #[cfg(unix)]
                kill_bounded_child(pid, forward);
            }
            stderr
        } else {
            read_capped(&mut stderr_pipe)
        };
        let _ = stderr_tx.send(stderr);
    });
    let start = std::time::Instant::now();
    let mut killed = false;
    // BUG-1993: back off from 1 ms to 20 ms. A flat 20 ms poll added up to
    // 20 ms to every short bounded call (most git probes finish in ~10 ms),
    // which `aida show` pays several times per run.
    // trace:BUG-1993 | ai:claude
    let mut poll = std::time::Duration::from_millis(1);
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut interrupted: Option<i32> = None;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut interrupt_deadline: Option<std::time::Instant> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                // trace:STORY-1917 | ai:claude
                #[cfg(unix)]
                if let Some(sig) = forwarding.as_mut().and_then(|f| f.take()) {
                    let tree = crate::exit_signal::collect_process_tree(pid);
                    crate::forwarded_signals::signal_child_tree(pid, &tree, sig);
                    interrupted = Some(sig);
                    interrupt_deadline
                        .get_or_insert(std::time::Instant::now() + FORWARDED_SIGNAL_GRACE);
                }
                if start.elapsed() >= timeout
                    || interrupt_deadline.is_some_and(|d| std::time::Instant::now() >= d)
                {
                    killed = true;
                    #[cfg(unix)]
                    kill_bounded_child(pid, forward);
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(poll);
                poll = (poll * 2).min(std::time::Duration::from_millis(20));
            }
            // BUG-1288 fix-up: a `try_wait` error leaves the child un-reaped
            // exactly like the timeout branch above — kill and wait it here
            // too, or it leaks as an orphan/zombie every time this arm is
            // hit instead of only on the timeout path.
            // trace:BUG-1288 | ai:claude
            Err(_) => {
                killed = true;
                #[cfg(unix)]
                kill_bounded_child(pid, forward);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // Bounded on the kill path (short — the group is already dead or dying,
    // this is only a safety net for a descendant that escaped it), generous
    // on the normal-exit path (the child already closed its own pipe ends;
    // this bound exists so a hypothetical stuck reader still can't hang the
    // caller forever, not because it is expected to be hit).
    //
    // TASK-3792: a descendant that inherited the pipes can hold them after the
    // leader exits, so the caller's timeout still bounds the read: when it (or
    // the generous wait) passes with a pipe open, the rest of the group is
    // killed, and a read cut off by the timeout is a timeout, not a result.
    // trace:TASK-3792 | ai:claude
    let read_deadline = {
        let now = std::time::Instant::now();
        if killed {
            now + std::time::Duration::from_millis(500)
        } else {
            let generous = now + std::time::Duration::from_secs(30);
            start
                .checked_add(timeout)
                .map_or(generous, |bound| bound.min(generous))
        }
    };
    let recv_until = |rx: &std::sync::mpsc::Receiver<Vec<u8>>, until: std::time::Instant| {
        rx.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
            .ok()
    };
    let mut stdout = recv_until(&stdout_rx, read_deadline);
    let mut stderr = recv_until(&stderr_rx, read_deadline);
    let mut status = status;
    if !killed && (stdout.is_none() || stderr.is_none()) {
        #[cfg(unix)]
        kill_bounded_child(pid, forward);
        let grace = std::time::Instant::now() + std::time::Duration::from_millis(500);
        stdout = stdout.or_else(|| recv_until(&stdout_rx, grace));
        stderr = stderr.or_else(|| recv_until(&stderr_rx, grace));
        if start.elapsed() >= timeout {
            status = None;
        }
    }
    let stdout = stdout.unwrap_or_default();
    let stderr = stderr.unwrap_or_else(|| {
        stderr_snapshot
            .and_then(|snapshot| snapshot.try_lock().ok().map(|bytes| bytes.clone()))
            .unwrap_or_default()
    });
    match status {
        Some(status) => Ok((
            BoundedCommandOutput::Completed(std::process::Output {
                status,
                stdout,
                stderr,
            }),
            interrupted,
            Vec::new(),
        )),
        None => Ok((BoundedCommandOutput::TimedOut, interrupted, stderr)),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod task_3792_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// True once `pid` is gone or a zombie awaiting its reaper.
    fn stopped(pid: &str) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| stat.rsplit_once(") ").unwrap().1.starts_with('Z'))
            .unwrap_or(true)
    }

    fn wait_stopped(pid: &str) -> bool {
        let until = Instant::now() + Duration::from_secs(2);
        while !stopped(pid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        stopped(pid)
    }

    // The leader exits at once; a child it forked keeps stdout and stderr.
    // The timeout still bounds the call, and the child is killed.
    // trace:TASK-3792 | ai:claude
    #[test]
    fn task_3792_exited_leader_with_inherited_pipes_is_bounded_and_killed() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("child");
        let mut cmd = std::process::Command::new("sh");
        cmd.args([
            "-c",
            "sleep 30 & echo $! > \"$1\"; echo partial; exit 0",
            "sh",
        ])
        .arg(&pid_file);
        let started = Instant::now();
        let out = command_output_with_timeout_detail(cmd, Duration::from_secs(1));
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
        assert!(matches!(out, BoundedCommandOutput::TimedOut));
        let child = std::fs::read_to_string(&pid_file).unwrap();
        assert!(wait_stopped(child.trim()), "inherited-pipe child survived");

        // Control: a leader whose children close the pipes completes.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 1 >/dev/null 2>&1 & echo done"]);
        match command_output_with_timeout_detail(cmd, Duration::from_secs(5)) {
            BoundedCommandOutput::Completed(out) => {
                assert!(out.status.success());
                assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "done");
            }
            _ => panic!("a finished leader with closed pipes must complete"),
        }
    }
}
