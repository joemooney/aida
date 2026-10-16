//! Scoring-local deadline across process exit and both captured output streams.
// trace:TASK-1791 | ai:codex
use super::*;
use crate::process_retry::RetryEtxtbsy;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct CommandTimedOut;

impl std::fmt::Display for CommandTimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("regen command timed out")
    }
}
impl std::error::Error for CommandTimedOut {}

// Keep the direct child unreaped while pipes are open. Its identity pins the
// group ID even if it exits before a pipe-holding descendant. Error/timeout
// cleanup always signals that group before reaping; no detached reader threads.
struct Running {
    child: Child,
    pgid: libc::pid_t,
    reaped: bool,
}
impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped {
            // SAFETY: positive group ID of our own unreaped child, established
            // by process_group(0). Never a shell-supplied or persisted PID.
            unsafe { libc::killpg(self.pgid, libc::SIGKILL) };
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn nonblocking(fd: RawFd) -> Result<()> {
    // SAFETY: live owned pipe descriptor; no pointer arguments.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    anyhow::ensure!(
        flags >= 0,
        "read pipe flags: {}",
        std::io::Error::last_os_error()
    );
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, eof: &mut bool) -> Result<()> {
    if *eof {
        return Ok(());
    }
    let mut buffer = [0u8; 8192];
    // Finite work per iteration so a noisy stream cannot starve the deadline
    // or the other stream. Preserve the existing full-output capture contract.
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => {
                *eof = true;
                break;
            }
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn run(mut cmd: Command, timeout: Duration) -> Result<Output> {
    let start = Instant::now();
    let child = cmd
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_retrying_etxtbsy()
        .context("launch regen command")?;
    let pgid = libc::pid_t::try_from(child.id()).expect("Linux child PID fits pid_t");
    let mut running = Running {
        child,
        pgid,
        reaped: false,
    };
    let mut stdout_pipe = running.child.stdout.take().context("missing stdout pipe")?;
    let mut stderr_pipe = running.child.stderr.take().context("missing stderr pipe")?;
    nonblocking(stdout_pipe.as_raw_fd())?;
    nonblocking(stderr_pipe.as_raw_fd())?;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut stdout_eof, mut stderr_eof) = (false, false);
    loop {
        if start.elapsed() >= timeout {
            return Err(CommandTimedOut.into());
        }
        drain(&mut stdout_pipe, &mut stdout, &mut stdout_eof)?;
        drain(&mut stderr_pipe, &mut stderr, &mut stderr_eof)?;
        // SAFETY: initialized siginfo storage. WNOWAIT observes the child exit
        // without reaping it, preserving safe group cleanup until EOF/deadline.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                running.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error.into());
            }
        } else if unsafe { info.si_pid() } != 0 && stdout_eof && stderr_eof {
            let status = running.child.wait()?;
            running.reaped = true;
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        std::thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(start.elapsed())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn exited_parent_with_pipe_holding_descendant_still_times_out() {
        let started = Instant::now();
        let error = run(
            shell("sleep 60 & printf 'partial output'; exit 0"),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(
            error.downcast_ref::<CommandTimedOut>().is_some(),
            "{error:#}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn drains_both_streams_beyond_pipe_capacity_and_preserves_exit_status() {
        let output = run(
            shell("head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2; exit 7"),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout.len(), 200000);
        assert_eq!(output.stderr.len(), 200000);
    }

    #[test]
    fn closed_pipes_do_not_end_deadline_before_process_exit() {
        let error = run(
            shell("exec 1>&- 2>&-; sleep 60"),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.downcast_ref::<CommandTimedOut>().is_some());
    }

    #[test]
    fn launch_failure_is_not_a_timeout() {
        let error = run(
            Command::new("/nonexistent/regen-command"),
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.downcast_ref::<CommandTimedOut>().is_none());
    }
}
