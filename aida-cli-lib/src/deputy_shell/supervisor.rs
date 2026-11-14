//! Process supervision for the operator's script deputy.
// trace:STORY-2775 | ai:codex
use super::Queue;
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::{fs::OpenOptionsExt, process::ExitStatusExt};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct State {
    pid: u32,
    identity: Option<String>,
    restarts: u64,
}

pub(crate) fn record_exit(q: &Queue, reason: &str) -> Result<()> {
    let line = format!(
        "{} deputy {:?}: {}",
        chrono::Utc::now().to_rfc3339(),
        q.name,
        reason.replace(['\r', '\n'], " ")
    );
    eprintln!("{line}");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(q.root.join("runner.log"))
        .context("open runner.log")?;
    writeln!(log, "{line}")?;
    Ok(())
}

pub(crate) fn write_status(q: &Queue, out: &mut dyn Write) -> Result<()> {
    let state = std::fs::read(q.root.join("loop.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<State>(&b).ok());
    if let Some(s) = state.filter(|s| {
        s.identity.is_some() && aida_core::liveness::process_start_identity(s.pid) == s.identity
    }) {
        writeln!(out, "loop: live, pid {}, restarts {}", s.pid, s.restarts)?;
    } else {
        writeln!(out, "loop: off")?;
    }
    Ok(())
}

fn next_delay(previous: Duration, uptime: Duration) -> Duration {
    if uptime >= Duration::from_secs(600) {
        Duration::from_secs(1)
    } else {
        (previous * 2).min(Duration::from_secs(60))
    }
}

pub(crate) fn run(q: &Queue) -> Result<()> {
    let mut command = Command::new("/proc/self/exe");
    command.args(std::env::args_os().skip(1).filter(|a| a != "--loop"));
    run_command(q, &mut command)
}

fn run_command(q: &Queue, command: &mut Command) -> Result<()> {
    // A separate lock spans runner exits and backoff. Never unlink lock files:
    // unlinking a flock inode would allow a second owner on a replacement inode.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(q.root.join("loop.lock"))?;
    lock.try_lock_exclusive()
        .context("a supervisor for this deputy is already running")?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut handlers = Vec::new();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        handlers.push(signal_hook::flag::register(signal, stop.clone())?);
    }
    // HUP may stop the runner, but must not terminate its supervisor.
    handlers.push(signal_hook::flag::register(
        libc::SIGHUP,
        Arc::new(AtomicBool::new(false)),
    )?);
    let result = supervise(q, &stop, command);
    for handler in handlers {
        signal_hook::low_level::unregister(handler);
    }
    let _ = std::fs::remove_file(q.root.join("loop.json"));
    result
}

fn supervise(q: &Queue, stop: &AtomicBool, command: &mut Command) -> Result<()> {
    let mut state = State {
        pid: std::process::id(),
        identity: aida_core::liveness::process_start_identity(std::process::id()),
        restarts: 0,
    };
    let mut delay = Duration::ZERO;
    while !stop.load(Ordering::SeqCst) {
        let tmp = q.root.join(format!(".loop.{}.tmp", uuid::Uuid::new_v4()));
        crate::deputy::write_private_new(&tmp, &serde_json::to_vec(&state)?)?;
        std::fs::rename(tmp, q.root.join("loop.json"))?;
        let began = Instant::now();
        let reason = match command.spawn() {
            Err(e) => format!("spawn failed: {e}"),
            Ok(mut child) => {
                // The child inherits the foreground process group and terminal.
                // Forward TERM also when only the supervisor PID was targeted.
                let mut forwarded = false;
                let status = loop {
                    if stop.load(Ordering::SeqCst) && !forwarded {
                        unsafe {
                            libc::kill(child.id() as i32, libc::SIGTERM);
                        }
                        forwarded = true;
                    }
                    match child.try_wait() {
                        Ok(Some(status)) => break status,
                        Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                        Err(e) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(e.into());
                        }
                    }
                };
                format!(
                    "rc={:?} signal={:?} uptime={}s",
                    status.code(),
                    status.signal(),
                    began.elapsed().as_secs()
                )
            }
        };
        record_exit(q, &reason)?;
        if stop.load(Ordering::SeqCst) {
            break;
        }
        delay = next_delay(delay, began.elapsed()).max(Duration::from_secs(1));
        let until = Instant::now() + delay;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
        if !stop.load(Ordering::SeqCst) {
            state.restarts += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn story_2775_operator_signals_stop_supervisor() {
        const CHILD_ROOT: &str = "AIDA_TEST_2775_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let q = super::super::queue_in(std::path::Path::new(&root), "test").unwrap();
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", "echo $$ > child.pid; exec sleep 30"])
                .current_dir(&q.root);
            run_command(&q, &mut command).unwrap();
            return;
        }
        for signal in [libc::SIGINT, libc::SIGTERM] {
            let dir = tempfile::tempdir().unwrap();
            let base = dir.path().join("queues");
            let q = super::super::queue_in(&base, "test").unwrap();
            q.ensure().unwrap();
            let mut child = Command::new("/proc/self/exe")
                .args([
                    "--exact",
                    "deputy_shell::supervisor::tests::story_2775_operator_signals_stop_supervisor",
                    "--nocapture",
                ])
                .env(CHILD_ROOT, &base)
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            while !q.root.join("child.pid").exists() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if !q.root.join("child.pid").exists() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("supervisor child did not become ready");
            }
            let runner_pid: u32 = std::fs::read_to_string(q.root.join("child.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            unsafe {
                libc::kill(child.id() as i32, signal);
            }
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    unsafe {
                        libc::kill(runner_pid as i32, libc::SIGKILL);
                    }
                    panic!("supervisor failed to stop");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(status.success(), "{status}");
            assert!(aida_core::liveness::process_start_identity(runner_pid).is_none());
            assert!(!q.root.join("loop.json").exists());
            let lock = std::fs::OpenOptions::new()
                .write(true)
                .open(q.root.join("loop.lock"))
                .unwrap();
            lock.try_lock_exclusive().unwrap();
            let log = std::fs::read_to_string(q.root.join("runner.log")).unwrap();
            assert_eq!(log.lines().count(), 1, "{log}");
        }
    }

    fn exercise(
        script: &str,
        stop_after: impl Fn(&Queue) -> bool + Send + 'static,
    ) -> (String, String) {
        let dir = tempfile::tempdir().unwrap();
        let q = super::super::queue_in(&dir.path().join("queues"), "test").unwrap();
        q.ensure().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let monitor_stop = stop.clone();
        let monitor_q = q.clone();
        let monitor = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(6);
            while !stop_after(&monitor_q) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            monitor_stop.store(true, Ordering::SeqCst);
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command.current_dir(&q.root);
        supervise(&q, &stop, &mut command).unwrap();
        monitor.join().unwrap();
        let log = std::fs::read_to_string(q.root.join("runner.log")).unwrap();
        let mut status = Vec::new();
        write_status(&q, &mut status).unwrap();
        (log, String::from_utf8(status).unwrap())
    }

    #[test]
    fn story_2775_restarts_clean_and_signal_exits() {
        for script in ["exit 0", "kill -KILL $$"] {
            let (log, status) = exercise(script, |q| {
                std::fs::read_to_string(q.root.join("runner.log"))
                    .is_ok_and(|s| s.lines().count() >= 2)
            });
            assert!(log.lines().count() >= 2, "{log}");
            assert!(status.contains("restarts 1"), "{status}");
            if script == "exit 0" {
                assert!(log.contains("rc=Some(0)"));
            } else {
                assert!(log.contains("signal=Some(9)"));
            }
        }
    }

    #[test]
    fn story_2775_stop_reaps_live_runner() {
        let (log, _) = exercise("exec sleep 30", |q| q.root.join("loop.json").exists());
        assert!(log.contains("signal=Some(15)"), "{log}");
        assert_eq!(log.lines().count(), 1);
    }

    #[test]
    fn story_2775_stop_during_backoff_does_not_restart() {
        let (log, status) = exercise("exit 7", |q| q.root.join("runner.log").exists());
        assert_eq!(log.lines().count(), 1);
        assert!(log.contains("rc=Some(7)"));
        assert!(status.contains("restarts 0"));
    }

    #[test]
    fn story_2775_backoff_caps_and_resets() {
        let mut delay = Duration::ZERO;
        let mut observed = Vec::new();
        for _ in 0..9 {
            delay = next_delay(delay, Duration::ZERO).max(Duration::from_secs(1));
            observed.push(delay.as_secs());
        }
        assert_eq!(observed, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(
            next_delay(delay, Duration::from_secs(600)),
            Duration::from_secs(1)
        );
    }
}
