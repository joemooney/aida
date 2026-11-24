//! Fetch the canonical store branch without advancing its checkout.
// trace:BUG-2750 | ai:codex

use std::path::Path;
use std::time::{Duration, Instant};

/// Return the object ID reported by this fetch. Reading the shared tracking
/// ref afterwards could pick up a different concurrent fetch.
pub(super) fn fetch_tip(
    store: &Path,
    remote: &str,
    branch: &str,
    budget: Duration,
) -> anyhow::Result<String> {
    let deadline = Instant::now() + budget;
    anyhow::ensure!(
        !remote.is_empty() && !remote.starts_with('-') && !remote.contains(['/', ':', '@']),
        "invalid store remote name"
    );
    let source = format!("refs/heads/{branch}");
    let mut validate = aida_core::git_ops::host_git_command(store);
    validate.args(["check-ref-format", &source]);
    anyhow::ensure!(
        crate::gate::kernel_deps::output_before(validate, Some(deadline))?
            .status
            .success(),
        "invalid store branch"
    );
    let mut cmd = aida_core::git_ops::with_config_read_deadline(Some(deadline), || {
        aida_core::git_ops::host_remote_git_command(store, remote)
    });
    let destination = format!("refs/remotes/{remote}/{branch}");
    cmd.args([
        "fetch",
        "--porcelain",
        "--verbose",
        "--no-write-fetch-head",
        "--no-tags",
        "--no-recurse-submodules",
        "--no-auto-maintenance",
        "--refmap=",
        remote,
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
    let row = std::str::from_utf8(&output.stdout)?.trim_end();
    // Git uses a space as the fast-forward flag. Skip the flag and its separator.
    let fields: Vec<_> = row.get(2..).unwrap_or_default().split(' ').collect();
    anyhow::ensure!(
        fields.len() == 3
            && fields[2] == destination
            && matches!(fields[1].len(), 40 | 64)
            && fields[1].bytes().all(|b| b.is_ascii_hexdigit()),
        "store refresh unavailable: unexpected git fetch result"
    );
    Ok(fields[1].to_owned())
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
        let tip = fetch_tip(store, "origin", "aida-store", Duration::from_secs(5)).unwrap();
        let head = crate::test_git::git_command()
            .current_dir(store)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(tip, String::from_utf8(head.stdout).unwrap().trim());
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
        let error = fetch_tip(store, "origin", "aida-store", Duration::from_secs(2)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(store.join("observed-unlocked").exists());
        assert!(error.to_string().contains("store refresh timed out"));
    }
}
