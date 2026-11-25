//! Fetch the canonical store branch without advancing its checkout.
// trace:BUG-2750 | ai:codex

use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn pre_pass(root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = aida_core::git_ops::with_config_read_deadline(Some(deadline), || {
        aida_core::store_locate::detect_distributed_store_from(root)
            .map_or(Ok(()), |store| refresh(&store, deadline))
    });
    if Instant::now() >= deadline {
        eprintln!("train: store refresh timed out; continuing with cached store");
    } else if let Err(error) = result {
        eprintln!("train: {error}; continuing with cached store");
    }
}

fn refresh(store: &Path, deadline: Instant) -> anyhow::Result<()> {
    anyhow::ensure!(store.join(".git").exists(), "missing store checkout");
    let mut branch = aida_core::git_ops::host_fetch_git_command(store);
    branch.args(["symbolic-ref", "--quiet", "HEAD"]);
    let head = crate::gate::kernel_deps::output_before(branch, Some(deadline))
        .map_err(|_| anyhow::anyhow!("store refresh timed out"))?;
    anyhow::ensure!(head.status.success(), "store branch unavailable");
    let source = std::str::from_utf8(&head.stdout)?.trim_end_matches('\n');
    let branch = source
        .strip_prefix("refs/heads/")
        .ok_or_else(|| anyhow::anyhow!("unexpected store HEAD"))?;
    let mut cmd = aida_core::git_ops::host_fetch_git_command(store);
    aida_core::git_ops::configure_remote_credentials(&mut cmd, store, "origin");
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

    // trace:BUG-2750.AC1 | trace:BUG-2750.AC2 | ai:codex
    #[test]
    fn bug2750_stalled_store_fetch_has_deadline_and_no_writer_lock() {
        if let Some(out) = crate::test_env::run_path_isolated_test_output() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let diagnostic = "train: store refresh timed out; continuing with cached store";
            assert_eq!(
                stderr.lines().filter(|line| *line == diagnostic).count(),
                2,
                "{stderr}"
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        crate::test_git::commit_fixture(root);
        let store = &root.join(".aida-store");
        std::fs::create_dir_all(store.join("objects")).unwrap();
        std::fs::write(store.join("metadata.yaml"), "{}").unwrap();
        std::fs::create_dir(root.join(".aida")).unwrap();
        std::fs::write(
            root.join(".aida/config.toml"),
            "store_path = '.aida-store'\n[forge]\nprovider = \"github\"\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            assert!(crate::test_git::git_command()
                .current_dir(store)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        crate::test_git::commit_fixture(store);
        git(&["remote", "add", "origin", "."]);
        let stalled = || {
            let start = Instant::now();
            let error = crate::pr_train::pr_train_handler(&crate::pr_train::TrainOptions {
                prs: &[1],
                all_ready: false,
                dry_run: false,
                local_ci: None,
                max: 1,
                no_pull: true,
            })
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("could not read origin's default branch"),
                "{error:#}"
            );
            assert!(start.elapsed() < Duration::from_millis(5750));
        };
        std::fs::create_dir(store.join(".aida")).unwrap();
        let shim = root.join("git");
        std::fs::write(&shim, "#!/bin/sh\ncase \" $* \" in *' config --get-regexp '*) if test -f metadata.yaml; then flock -n .aida/store-write.lock touch prep-stalled || exit 1; exec sleep 20; fi;; esac\nexec /usr/bin/git \"$@\"\n").unwrap();
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o700)).unwrap();
        {
            let path = format!("{}:/usr/bin:/bin", root.display());
            let _env =
                crate::test_env::AmbientGuard::hermetic_and(root, None, &[("PATH", Some(&path))]);
            stalled();
            assert!(store.join("prep-stalled").exists());
        }
        pre_pass(root);
        assert!(!std::fs::read_dir(store.join(".git"))
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .as_encoded_bytes()
                .starts_with(b"aida-store-hooks-")));
        let head = crate::test_git::git_command()
            .current_dir(store)
            .args([
                "rev-list",
                "--left-right",
                "--count",
                "HEAD...refs/remotes/origin/main",
            ])
            .output()
            .unwrap();
        assert!(head.status.success());
        assert_eq!("0\t0", String::from_utf8(head.stdout).unwrap().trim());
        assert!(!store.join(".git/FETCH_HEAD").exists());
        let upload = store.join("upload");
        std::fs::write(&upload, "#!/bin/sh\nflock -n .aida/store-write.lock touch observed-unlocked || exit 1\nexec sleep 20\n").unwrap();
        std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o700)).unwrap();
        git(&[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ]);
        let _env = crate::test_env::AmbientGuard::hermetic(root, None);
        stalled();
        assert!(store.join("observed-unlocked").exists());
    }
}
