//! Read adapters; no store/cache handles or mutating command surface.
// trace:STORY-2704 | ai:codex
use anyhow::{Context, Result};
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

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
/// Read the built-in's public JSON report; never resolve or execute a forge CLI.
pub(crate) fn report(
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
) -> Result<serde_json::Value> {
    let env = aida_core::exec_context::Environment::capture();
    let binary = env
        .var("AIDA_BIN")
        .context("AIDA_BIN is required for flow JSON reads")?;
    let mut cmd = Command::new(binary);
    cmd.args([
        "flow",
        "report",
        "--since",
        &since.to_rfc3339(),
        "--until",
        &until.to_rfc3339(),
        "--json",
    ]);
    let output = command_output_with_timeout(cmd, Duration::from_secs(45))
        .context("AIDA flow JSON read timed out or failed to spawn")?;
    anyhow::ensure!(
        output.status.success(),
        "AIDA flow JSON read failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}
