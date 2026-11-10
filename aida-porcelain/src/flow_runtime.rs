//! Read adapters; no store/cache handles or mutating command surface.
// trace:STORY-2704 | ai:codex
use anyhow::{Context, Result};
use std::{
    path::PathBuf,
    process::{Command, Output},
};

/// Preserve the reader's output and exit status across the rendering boundary.
#[derive(Debug)]
pub(crate) struct ReaderFailure(pub Output);
impl std::fmt::Display for ReaderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AIDA flow JSON reader exited with {}", self.0.status)
    }
}
impl std::error::Error for ReaderFailure {}

fn reader(env: &aida_core::exec_context::Environment) -> PathBuf {
    if let Some(binary) = env.get("AIDA_BIN") {
        return binary.into();
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("aida");
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from("aida")
}
/// Read the built-in's public JSON report; never resolve or execute a forge CLI.
pub(crate) fn report(
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
    error_format: &str,
) -> Result<serde_json::Value> {
    let env = aida_core::exec_context::Environment::capture();
    let mut cmd = Command::new(reader(&env));
    cmd.args([
        "flow",
        "report",
        "--since",
        &since.to_rfc3339(),
        "--until",
        &until.to_rfc3339(),
        "--json",
        "--format",
        error_format,
    ]);
    let output = cmd
        .output()
        .context("failed to run AIDA flow JSON reader")?;
    if !output.status.success() {
        return Err(ReaderFailure(output).into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}
