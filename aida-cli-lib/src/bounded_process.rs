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
    run_bounded(cmd, timeout, cap, false).map_or(BoundedCommandOutput::SpawnFailed, |(out, _)| out)
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
    run_bounded(cmd, timeout, cap, true)
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

fn run_bounded(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    cap: u64,
    forward: bool,
) -> std::io::Result<(BoundedCommandOutput, Option<i32>)> {
    use std::io::Read;

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
    std::thread::spawn(move || {
        let _ = stderr_tx.send(read_capped(&mut stderr_pipe));
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
    let read_wait = if killed {
        std::time::Duration::from_millis(500)
    } else {
        std::time::Duration::from_secs(30)
    };
    let stdout = stdout_rx.recv_timeout(read_wait).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(read_wait).unwrap_or_default();
    let output = match status {
        Some(status) => BoundedCommandOutput::Completed(std::process::Output {
            status,
            stdout,
            stderr,
        }),
        None => BoundedCommandOutput::TimedOut,
    };
    Ok((output, interrupted))
}
