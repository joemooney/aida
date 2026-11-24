// trace:TASK-3852 | ai:codex
#[path = "support/test_git.rs"]
mod test_git;

use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn task_3852_budget_doctor_and_land_use_cached_pinned_reading() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("main");
    let checkout = tmp.path().join("lane");
    let bin = tmp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        assert!(test_git::git_command()
            .current_dir(&root)
            .args(args)
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-qb", "main"]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.org",
        "commit",
        "--allow-empty",
        "-qm",
        "fixture",
    ]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/project.git",
    ]);
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "lane",
        checkout.to_str().unwrap(),
    ]);
    fs::create_dir_all(root.join(".aida-store/objects")).unwrap();
    for dir in [&root, &checkout] {
        fs::create_dir_all(dir.join(".aida")).unwrap();
        fs::write(
            dir.join(".aida/config.toml"),
            "[forge]\nprovider = \"gitlab\"\n",
        )
        .unwrap();
    }
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
case "$1" in
--version) echo 'gh version 2.0';;
api)
    [ "$PWD" = "$EXPECTED_ROOT" ] && [ "$GH_HOST" = github.com ] &&
        [ -z "${GH_REPO+x}" ] && [ "$*" = 'api rate_limit --cache 60s' ] || exit 1
    printf '{"resources":{"core":{"remaining":%s,"limit":100,"reset":3600}}}\n' "$BUDGET"
    ;;
pr) echo '[]';;
*) exit 1;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(gh, fs::Permissions::from_mode(0o700)).unwrap();

    for args in [
        &["doctor", "check", "github-core-budget"][..],
        &["land", "status"],
    ] {
        for remaining in [19, 20] {
            let out = Command::new(env!("CARGO_BIN_EXE_aida"))
                .env_clear()
                .env("HOME", tmp.path())
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("GH_HOST", "other.invalid")
                .env("GH_REPO", "other/repo")
                .env("EXPECTED_ROOT", &root)
                .env("BUDGET", remaining.to_string())
                .current_dir(&checkout)
                .args(["--format", "human"])
                .args(args)
                .output()
                .unwrap();
            let text = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(out.status.success(), "{text}");
            assert!(
                text.contains(&format!(
                    "{remaining}/100 remaining, resets 1970-01-01 01:00:00 UTC"
                )),
                "{text}"
            );
            assert_eq!(
                text.contains("WARNING: below 20%"),
                remaining < 20,
                "{text}"
            );
        }
    }
}
