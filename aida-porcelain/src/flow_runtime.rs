//! Read adapters; no store/cache handles or mutating command surface.
// trace:STORY-2704 | ai:codex
use anyhow::{Context, Result};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

pub fn find_project_root() -> Result<PathBuf> {
    std::env::current_dir()?
        .ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_path_buf)
        .context("not inside a git repository")
}
pub use aida_core::gate_ledger_read::store_root;
pub fn command_output_with_timeout(mut cmd: Command, timeout: Duration) -> Option<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let (send, receive) = std::sync::mpsc::channel();
    let err_send = send.clone();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let result = stdout.read_to_end(&mut b).map(|_| b);
        let _ = send.send((true, result));
    });
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let result = stderr.read_to_end(&mut b).map(|_| b);
        let _ = err_send.send((false, result));
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut out = Vec::new();
    let mut err = Vec::new();
    for _ in 0..2 {
        let (is_stdout, bytes) = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()?;
        if is_stdout {
            out = bytes.ok()?;
        } else {
            err = bytes.ok()?;
        }
    }
    Some(Output {
        status,
        stdout: out,
        stderr: err,
    })
}
pub fn forge_read(root: &Path, args: &[&str]) -> Result<serde_json::Value> {
    anyhow::ensure!(
        matches!(args, ["pr", "view", ..] | ["run", "list", ..]),
        "unsupported flow read"
    );
    let mut cmd = Command::new("gh");
    cmd.current_dir(root).args(args);
    let output = command_output_with_timeout(cmd, Duration::from_secs(15))
        .context("flow forge evidence unavailable")?;
    anyhow::ensure!(
        output.status.success(),
        "flow forge evidence unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
