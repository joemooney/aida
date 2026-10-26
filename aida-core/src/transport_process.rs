//! Bounded transport/preparation subprocess capture, with group cleanup.
// trace:STORY-1742 | ai:codex
use anyhow::{bail, Context, Result};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

struct Running(std::process::Child);
impl Drop for Running {
    fn drop(&mut self) {
        // Cover every return, including a parent that exited while one of its
        // helpers retained stdout. Never leave that helper running after an
        // incomplete read. The group ID remains reserved while members live.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn bounded(
    mut cmd: Command,
    cap: usize,
    deadline: Instant,
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let child = cmd
        .spawn()
        .map_err(|_| anyhow::anyhow!("Transport graph reader unavailable"))?;
    let mut running = Running(child);
    let child = &mut running.0;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(cap as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
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
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            bail!("Transport graph read limit or deadline exceeded");
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| anyhow::anyhow!("Transport graph reader failed"))?
        {
            let bytes = output
                .or_else(|| rx.recv_timeout(Duration::from_millis(100)).ok())
                .context("Transport graph output incomplete")?
                .map_err(|_| anyhow::anyhow!("Transport graph output unreadable"))?;
            if bytes.len() > cap {
                bail!("Transport graph output limit exceeded");
            }
            return Ok((status, bytes));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
