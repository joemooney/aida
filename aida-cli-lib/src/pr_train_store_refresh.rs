//! Fetch the canonical store branch without advancing its checkout.
// trace:BUG-2750 | ai:codex

use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn refresh(store: &Path, budget: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + budget;
    anyhow::ensure!(store.join(".git").exists(), "missing store checkout");
    let mut branch = aida_core::git_ops::host_git_command(store);
    branch.args(["symbolic-ref", "--quiet", "HEAD"]);
    let head = crate::gate::kernel_deps::output_before(branch, Some(deadline))
        .map_err(|_| anyhow::anyhow!("store refresh timed out"))?;
    anyhow::ensure!(head.status.success(), "store branch unavailable");
    let source = std::str::from_utf8(&head.stdout)?.trim_end_matches('\n');
    let branch = source
        .strip_prefix("refs/heads/")
        .ok_or_else(|| anyhow::anyhow!("unexpected store HEAD"))?;
    let mut cmd = aida_core::git_ops::with_config_read_deadline(Some(deadline), || {
        aida_core::git_ops::host_remote_git_command(store, "origin")
    });
    let destination = format!("refs/remotes/origin/{branch}");
    cmd.args([
        "fetch",
        "--quiet",
        "--no-prune",
        "--no-write-fetch-head",
        "--no-tags",
        "--no-recurse-submodules",
        "--no-auto-maintenance",
        "--refmap=",
        "origin",
        &format!("+{source}:{destination}"),
    ])
    .env("GIT_TERMINAL_PROMPT", "0")
    .stdin(std::process::Stdio::null());
    let remaining = deadline.saturating_duration_since(Instant::now());
    anyhow::ensure!(
        !remaining.is_zero(),
        "store refresh unavailable: store refresh timed out"
    );
    let output = match crate::command_output_with_timeout_capped(cmd, remaining, 64 * 1024) {
        crate::BoundedCommandOutput::Completed(output) => output,
        crate::BoundedCommandOutput::TimedOut => {
            anyhow::bail!("store refresh unavailable: store refresh timed out")
        }
        crate::BoundedCommandOutput::SpawnFailed => anyhow::bail!("store refresh unavailable"),
    };
    anyhow::ensure!(output.status.success(), "store refresh unavailable");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn bug2750_stalled_store_fetch_has_deadline_and_no_writer_lock() {
        let temp = tempfile::tempdir().unwrap();
        let store = temp.path();
        let git = |args: &[&str]| {
            assert!(crate::test_git::git_command()
                .current_dir(store)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "aida-store"]);
        git(&["remote", "add", "origin", "."]);
        git(&[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ]);
        refresh(store, Duration::from_secs(5)).unwrap();
        let head = crate::test_git::git_command()
            .current_dir(store)
            .args([
                "rev-list",
                "--left-right",
                "--count",
                "HEAD...refs/remotes/origin/aida-store",
            ])
            .output()
            .unwrap();
        assert!(head.status.success());
        assert_eq!("0\t0", String::from_utf8(head.stdout).unwrap().trim());
        assert!(!store.join(".git/FETCH_HEAD").exists());
        let upload = store.join("upload");
        std::fs::write(&upload, "#!/bin/sh\nflock -n .aida/store-write.lock touch observed-unlocked || exit 1\nexec sleep 20\n").unwrap();
        std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(store.join(".aida")).unwrap();
        git(&[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ]);
        let start = Instant::now();
        let error = refresh(store, Duration::from_secs(2)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(store.join("observed-unlocked").exists());
        assert!(error.to_string().contains("store refresh timed out"));
    }
}
