use super::*;

fn git(root: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// BUG-442: a fresh clone has NO local `.aida/config.toml`, so
/// `distributed_mode_declared_from` is None — but the project IS distributed
/// when the `aida-store` branch exists. The dispatch must detect that from
/// the git ref (and then auto-attach / refuse the silent ambient fallback),
/// never treat a config-less clone as a non-AIDA dir. This locks the two
/// halves of that signal so a future change can't re-gate detection on a
// local config. trace:BUG-442 | ai:claude
#[test]
fn distributed_detected_from_git_ref_without_local_config() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@t.t"]);
    git(root, &["config", "user.name", "t"]);
    std::fs::write(root.join("f"), "x").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-q", "-m", "init"]);
    // Simulate the orphan store branch existing locally (as a fresh clone's
    // origin/aida-store would), with NO `.aida/config.toml`.
    git(root, &["branch", "aida-store"]);

    assert!(
        distributed_mode_declared_from(root).is_none(),
        "no config.toml → not declared (the fresh-clone state)"
    );
    assert!(
        branch_exists_anywhere(root, "aida-store"),
        "aida-store ref present → the git-only distributed signal must fire"
    );
    // Without the ref it must NOT look distributed (would otherwise refuse
    // every plain git repo).
    assert!(
        !branch_exists_anywhere(root, "no-such-branch"),
        "absent ref → not distributed"
    );
}
