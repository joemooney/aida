//! Bounded transport/preparation subprocess capture, with group cleanup.
// trace:STORY-1742 | ai:codex
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A spawned process group. The leader stays unreaped (a zombie still holds
/// its PID and group ID) until `reap`, so signalling the group can never
/// reach an unrelated, recycled process.
pub struct Group {
    pub child: Child,
    reaped: bool,
}

impl Group {
    pub fn spawn(mut cmd: Command) -> Result<Self> {
        let child = cmd
            .spawn()
            .map_err(|_| anyhow::anyhow!("Transport Git process unavailable"))?;
        Ok(Self {
            child,
            reaped: false,
        })
    }

    /// Exit observed without reaping, so the group stays addressable.
    pub fn exited(&self) -> bool {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc != 0 {
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
        }
        info.si_signo == libc::SIGCHLD
    }

    pub fn signal(&self, signal: i32) {
        if !self.reaped {
            unsafe {
                libc::kill(-(self.child.id() as i32), signal);
            }
        }
    }

    pub fn reap(&mut self) -> Result<ExitStatus> {
        let status = self
            .child
            .wait()
            .map_err(|_| anyhow::anyhow!("Transport Git process status unavailable"))?;
        self.reaped = true;
        Ok(status)
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        // Cover every early return, including a leader that exited while one
        // of its helpers retained a pipe: never leave a helper running.
        if !self.reaped {
            self.signal(libc::SIGKILL);
            let _ = self.reap();
        }
    }
}

pub fn drain(
    source: impl Read + Send + 'static,
    cap: usize,
) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = source
            .take(cap as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    rx
}

/// Run a local reader to completion under a cap and deadline. On exit, the
/// whole group is killed before reaping so no helper outlives the read.
pub fn bounded(
    mut cmd: Command,
    input: &[u8],
    cap: usize,
    deadline: Instant,
) -> Result<(ExitStatus, Vec<u8>)> {
    cmd.stdin(if input.is_empty() {
        Stdio::null()
    } else {
        Stdio::piped()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .process_group(0);
    let mut group = Group::spawn(cmd)?;
    let rx = drain(group.child.stdout.take().unwrap(), cap);
    if let Some(mut stdin) = group.child.stdin.take() {
        // Feed requests concurrently: a large object-ID list must not
        // deadlock against output the reader is still producing.
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut output = None;
    loop {
        if let Ok(result) = rx.try_recv() {
            output = Some(result);
        }
        if output
            .as_ref()
            .is_some_and(|r| r.as_ref().map_or(true, |b| b.len() > cap))
            || Instant::now() >= deadline
        {
            bail!("Transport graph read limit or deadline exceeded");
        }
        if group.exited() {
            let bytes = output
                .or_else(|| rx.recv_timeout(Duration::from_millis(100)).ok())
                .context("Transport graph output incomplete")?
                .map_err(|_| anyhow::anyhow!("Transport graph output unreadable"))?;
            group.signal(libc::SIGKILL);
            let status = group.reap()?;
            if bytes.len() > cap {
                bail!("Transport graph output limit exceeded");
            }
            return Ok((status, bytes));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
