// Gated to Linux like the other binary-driving e2e suites. trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! TASK-1321: `aida rebase --dry-run --json` run inside a linked git worktree
//! must classify THAT worktree, not the primary checkout whose canonical store
//! every sibling worktree shares.
//!
//! Fixture: a primary checkout on a clean, up-to-date `main`, plus two linked
//! worktrees tracking `origin/main` that each sit one commit ahead and three
//! behind. `risky` overlaps upstream on a file and carries an uncommitted edit;
//! `safe` touches nothing upstream does and is clean. Before the fix, every
//! invocation reported the primary's `main`, 0 ahead / 0 behind, clean.
//!
//! Hermetic: throwaway HOME/AIDA_HOME, a file-only git transport against a
//! local bare origin, `--no-fetch`, and no forge binary.
// trace:TASK-1321 | ai:claude

use std::path::{Path, PathBuf};
use std::process::Command;

fn aida(cwd: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(cwd);
    cmd.env("HOME", home);
    cmd.env("AIDA_HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    cmd.env("AIDA_TEST_GH_BINARY", "/bin/false");
    cmd.env("GIT_ALLOW_PROTOCOL", "file");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    // Pin git's global config to the fixture's private file so an ambient
    // GIT_CONFIG_GLOBAL cannot override (or suppress) it. trace:TASK-1321 | ai:claude
    cmd.env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"));
    for k in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_PERMISSION_MODE",
        "AIDA_AGENT_OUTPUT",
        "AIDA_HEADLESS_VENDOR",
        "AIDA_AGENT_MODEL",
        "AIDA_AGENT_CMD",
        "AIDA_SESSION_ID",
        "AIDA_STORE",
        "AIDA_OUTPUT_FORMAT",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        cmd.env_remove(k);
    }
    cmd
}

fn git_out(dir: &Path, home: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .env("HOME", home)
        // Private global config, independent of any inherited
        // GIT_CONFIG_GLOBAL / injected -c parameters. trace:TASK-1321 | ai:claude
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(dir: &Path, home: &Path, file: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(file), body).unwrap();
    git_out(dir, home, &["add", file]);
    git_out(dir, home, &["commit", "-q", "-m", msg]);
}

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    primary: PathBuf,
    risky: PathBuf,
    safe: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    let home = base.join("home");
    let origin = base.join("origin.git");
    let primary = base.join("primary");
    let risky = base.join("risky");
    let safe = base.join("safe");
    let upstream = base.join("upstream");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&primary).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\temail = t@t.t\n\tname = t\n[init]\n\tdefaultBranch = main\n",
    )
    .unwrap();

    git_out(
        base,
        &home,
        &["init", "-q", "--bare", origin.to_str().unwrap()],
    );
    git_out(&primary, &home, &["init", "-q", "-b", "main"]);
    commit_file(&primary, &home, "shared.txt", "base\n", "init");

    let init = aida(&primary, &home)
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(
        init.status.success(),
        "aida init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    git_out(&primary, &home, &["add", "-A"]);
    git_out(&primary, &home, &["commit", "-q", "-m", "aida init"]);
    git_out(
        &primary,
        &home,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git_out(&primary, &home, &["push", "-q", "-u", "origin", "main"]);

    for (dir, branch) in [(&risky, "risky"), (&safe, "safe")] {
        git_out(
            &primary,
            &home,
            &[
                "worktree",
                "add",
                "-q",
                "--track",
                "-b",
                branch,
                dir.to_str().unwrap(),
                "origin/main",
            ],
        );
    }
    commit_file(&risky, &home, "shared.txt", "risky\n", "risky edit");
    std::fs::write(risky.join("shared.txt"), "risky, uncommitted\n").unwrap();
    commit_file(&safe, &home, "safe.txt", "safe\n", "safe edit");

    // Three upstream commits, one touching the file `risky` also changed.
    git_out(
        base,
        &home,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            upstream.to_str().unwrap(),
        ],
    );
    commit_file(&upstream, &home, "shared.txt", "upstream\n", "up 1");
    commit_file(&upstream, &home, "up2.txt", "2\n", "up 2");
    commit_file(&upstream, &home, "up3.txt", "3\n", "up 3");
    git_out(&upstream, &home, &["push", "-q", "origin", "main"]);

    // Refresh the shared remote-tracking refs, then bring the primary current
    // so it reads as clean main, 0 ahead / 0 behind.
    git_out(&primary, &home, &["fetch", "-q", "origin"]);
    git_out(
        &primary,
        &home,
        &["merge", "-q", "--ff-only", "origin/main"],
    );
    assert_eq!(
        git_out(&primary, &home, &["status", "--porcelain"]),
        "",
        "the primary checkout must start clean"
    );

    Fixture {
        _tmp: tmp,
        home,
        primary,
        risky,
        safe,
    }
}

fn dry_run_json(cwd: &Path, home: &Path) -> serde_json::Value {
    let out = aida(cwd, home)
        .args(["rebase", "--dry-run", "--json", "--no-fetch"])
        .output()
        .expect("run aida rebase");
    assert!(
        out.status.success(),
        "aida rebase --dry-run failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "non-JSON output ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn heads(f: &Fixture) -> Vec<String> {
    [&f.primary, &f.risky, &f.safe]
        .iter()
        .map(|d| git_out(d, &f.home, &["rev-parse", "HEAD"]))
        .collect()
}

#[test]
fn task_1321_dry_run_json_reports_the_invoked_linked_worktree() {
    let f = fixture();
    let before = heads(&f);

    let risky = dry_run_json(&f.risky, &f.home);
    assert_eq!(risky["branch"], "risky", "{risky:#}");
    assert_eq!(risky["upstream"], "origin/main", "{risky:#}");
    assert_eq!(risky["ahead"], 1, "{risky:#}");
    assert_eq!(risky["behind"], 3, "{risky:#}");
    assert_eq!(risky["classification"], "diverged-risky", "{risky:#}");
    assert_eq!(
        risky["overlap"],
        serde_json::json!(["shared.txt"]),
        "{risky:#}"
    );
    assert_eq!(risky["working_tree_clean"], false, "{risky:#}");
    assert_eq!(risky["executed"], false, "{risky:#}");

    // A nested directory inside the linked worktree resolves the same checkout.
    let sub = f.risky.join("nested");
    std::fs::create_dir_all(&sub).unwrap();
    let nested = dry_run_json(&sub, &f.home);
    assert_eq!(nested["branch"], "risky", "{nested:#}");
    assert_eq!(nested["classification"], "diverged-risky", "{nested:#}");

    let safe = dry_run_json(&f.safe, &f.home);
    assert_eq!(safe["branch"], "safe", "{safe:#}");
    assert_eq!(safe["ahead"], 1, "{safe:#}");
    assert_eq!(safe["behind"], 3, "{safe:#}");
    assert_eq!(safe["classification"], "diverged-safe", "{safe:#}");
    assert_eq!(safe["overlap"], serde_json::json!([]), "{safe:#}");
    assert_eq!(safe["working_tree_clean"], true, "{safe:#}");

    // The primary checkout still reports itself.
    let primary = dry_run_json(&f.primary, &f.home);
    assert_eq!(primary["branch"], "main", "{primary:#}");
    assert_eq!(primary["ahead"], 0, "{primary:#}");
    assert_eq!(primary["behind"], 0, "{primary:#}");
    assert_eq!(primary["classification"], "clean", "{primary:#}");
    assert_eq!(primary["working_tree_clean"], true, "{primary:#}");

    assert_eq!(heads(&f), before, "a dry run must not move any checkout");
    assert_eq!(
        std::fs::read_to_string(f.risky.join("shared.txt")).unwrap(),
        "risky, uncommitted\n",
        "a dry run must leave the linked worktree's edit in place"
    );
}
