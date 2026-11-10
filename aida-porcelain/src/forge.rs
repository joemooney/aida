//! Closed forge-read boundary for the independent porcelain binary.
// trace:STORY-2704 | ai:codex
use crate::flow_runtime::command_output_with_timeout;
use anyhow::{Context, Result};
use std::{path::Path, process::Command, time::Duration};

pub(crate) fn flow_report_read(root: &Path, args: &[&str]) -> Result<serde_json::Value> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_boundary_refuses_mutations_before_spawn() {
        for args in [
            &["pr", "merge", "42"][..],
            &["pr", "edit", "42"],
            &["run", "cancel", "42"],
            &["run", "rerun", "42"],
            &["api", "--method", "POST"],
            &[],
        ] {
            let error = flow_report_read(Path::new("."), args).unwrap_err();
            assert_eq!(error.to_string(), "unsupported flow read");
        }
    }
}
