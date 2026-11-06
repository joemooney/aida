//! Confirmation boundary for destructive forge-driven lifecycle decisions.
// trace:STORY-2401 | ai:codex
use crate::forge::{ChangeMetadata, ChangeState, Forge};
use crate::forge_observed::{confirm_snapshot, Observed};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

pub(crate) fn backoff() {
    std::thread::sleep(Duration::from_secs(3));
}

fn git(root: &Path, args: &[&str], remote: bool) -> Result<Output> {
    // Retain the private-seat clone refusal even for evidence-only fetches.
    let mut cmd = aida_core::git_ops::host_git_command(root);
    cmd.args(args).stdin(Stdio::null());
    if remote {
        aida_core::git_ops::configure_remote_credentials(&mut cmd, root, "origin");
    }
    crate::command_output_with_timeout(cmd, Duration::from_secs(30))
        .context("confirmation Git read failed or exceeded 30 seconds")
}

fn stdout(root: &Path, args: &[&str]) -> Result<String> {
    let out = git(root, args, false)?;
    anyhow::ensure!(
        out.status.success(),
        "confirmation Git evidence unavailable"
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Fetch into an invocation-private ref. Never consult a stale tracking ref or
/// shared FETCH_HEAD. Both ancestry and tree equality handle the train's
/// ordinary and rebase merges; a squash receipt on the fetched base also
/// proves a merged change whose original head is no longer an ancestor.
// trace:STORY-2401 | ai:codex
pub(crate) fn on_base(root: &Path, base: &str, head: &str, pr: u64) -> Observed<bool> {
    Observed::from_result((|| -> Result<bool> {
        anyhow::ensure!(
            matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid pinned head"
        );
        let source = format!("refs/heads/{base}");
        anyhow::ensure!(
            git(root, &["check-ref-format", &source], false)?
                .status
                .success(),
            "invalid base branch"
        );
        let private = format!("refs/aida/confirm/{}", uuid::Uuid::new_v4());
        let spec = format!("+{source}:{private}");
        let result = (|| -> Result<bool> {
            let fetched = git(
                root,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "origin",
                    &spec,
                ],
                true,
            )?;
            anyhow::ensure!(fetched.status.success(), "confirmation base fetch failed");
            let tip = stdout(root, &["rev-parse", "--verify", &private])?;
            let ancestor = git(root, &["merge-base", "--is-ancestor", head, &tip], false)?;
            match ancestor.status.code() {
                Some(0) => return Ok(true),
                Some(1) => {}
                _ => anyhow::bail!("pinned head ancestry unavailable"),
            }
            if stdout(root, &["rev-parse", &format!("{head}^{{tree}}")])?
                == stdout(root, &["rev-parse", &format!("{tip}^{{tree}}")])?
            {
                return Ok(true);
            }
            let suffix = format!(r"\((#|!){pr}\)$");
            let subjects = stdout(
                root,
                &[
                    "log",
                    "--max-count=1",
                    "--format=%s",
                    "--extended-regexp",
                    "--grep",
                    &suffix,
                    &tip,
                ],
            )?;
            Ok(!subjects.is_empty())
        })();
        // Cleanup cannot turn failed evidence into a negative.
        let _ = git(root, &["update-ref", "-d", &private], false);
        result
    })())
}

/// A stable forge snapshot plus independently fetched Git evidence. Unknown
/// evidence leaves the caller's spec/queue/change untouched.
// trace:STORY-2401 | ai:codex
pub(crate) fn metadata(
    root: &Path,
    forge: &dyn Forge,
    id: u64,
    sink: &mut dyn crate::network_retry::RetrySink,
) -> Observed<ChangeMetadata> {
    confirm_snapshot(
        || forge.observe_change_metadata(id, sink),
        |m| {
            forge
                .observe_change_ground_truth(root, id, m)
                .map(|landed| landed == (m.state == ChangeState::Merged))
        },
        backoff,
    )
}
