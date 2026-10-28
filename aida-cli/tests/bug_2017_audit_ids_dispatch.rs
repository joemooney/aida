// BUG-2017: `aida db audit-ids` through the real binary and dispatch. A store
// whose HEAD already holds two object UUIDs claiming one display ID must
// audit as `conflicts_in_head` and exit 2 without `--against` (a sync admits
// it); a claim `--against` would introduce audits as `conflicts` and exits 1.
// The audit must leave the store's refs and working tree exactly as they were.
// trace:BUG-2017 | ai:claude
#![cfg(target_os = "linux")]

#[path = "support/test_git.rs"]
mod test_git;

use aida_core::models::Requirement;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn aida(repo: &Path, home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aida"));
    command
        .current_dir(repo)
        .env("HOME", home)
        .env("AIDA_TELEMETRY", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .stdin(Stdio::null());
    for name in [
        "AIDA_STORE",
        "AIDA_PROJECT_ROOT",
        "AIDA_DRIVE_ROOT",
        "GIT_DIR",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(name);
    }
    let store = repo.join(".aida-store");
    if store.join("objects").is_dir() {
        command.env("AIDA_STORE", store);
    }
    command.args(args).output().expect("run aida")
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = test_git::git_command()
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn snapshot(store: &Path) -> (String, String, String) {
    (
        git(
            store,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        ),
        git(store, &["status", "--porcelain", "--ignored"]),
        git(store, &["rev-parse", "HEAD"]),
    )
}

fn commit_all(store: &Path, msg: &str) {
    git(store, &["add", "-A"]);
    git(
        store,
        &[
            "-c",
            "user.name=BUG-2017 Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            msg,
        ],
    );
}

/// One object claiming `spec`, stored at `path_spec`'s object path.
fn claim_at(store: &Path, spec: &str, path_spec: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let mut r = Requirement::new("title".into(), "body".into());
    r.spec_id = Some(spec.into());
    let written = aida_core::object_store::write_object(scratch.path(), &r).unwrap();
    let target = aida_core::object_store::object_path(&store.join("objects"), path_spec).unwrap();
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(written, target).unwrap();
}

#[test]
fn bug_2017_audit_ids_cli_exits_2_for_head_conflict_and_1_for_refused_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    test_git::enroll_synthetic_home(&repo);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "BUG-2017 Test"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let init = aida(
        &repo,
        &home,
        &[
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let store = repo.join(".aida-store");
    assert!(store.join("objects").is_dir(), "init must create the store");

    let clean = aida(&repo, &home, &["db", "audit-ids", "--json"]);
    let clean_out = String::from_utf8_lossy(&clean.stdout);
    assert!(
        clean.status.success(),
        "{clean_out}\n{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&clean_out).unwrap();
    assert_eq!(json["status"], "clean");

    claim_at(&store, "FR-9001", "FR-9001");
    claim_at(&store, "FR-9001", "FR-9002");
    commit_all(&store, "two claimants of one display ID");
    let before = snapshot(&store);

    let out = aida(&repo, &home, &["db", "audit-ids", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["status"], "conflicts_in_head", "{stdout}");
    assert_eq!(json["counts"]["conflicts"]["display_id"], 1, "{stdout}");
    assert!(
        !stdout.contains("FR-9001"),
        "display IDs never appear: {stdout}"
    );

    let human = aida(&repo, &home, &["db", "audit-ids"]);
    let human_out = String::from_utf8_lossy(&human.stdout);
    assert_eq!(human.status.code(), Some(2), "{human_out}");
    assert!(
        human_out.starts_with("Allocation claims audit: conflicts_in_head"),
        "{human_out}"
    );
    assert!(human_out.contains("[already in HEAD]"), "{human_out}");

    assert_eq!(snapshot(&store), before, "the audit must write nothing");

    // FR-9005 claimed independently on a fork and on HEAD: a sync with the
    // fork introduces that dispute, so the audit refuses with exit 1.
    let branch = git(&store, &["rev-parse", "--abbrev-ref", "HEAD"]);
    git(&store, &["checkout", "-q", "-b", "fork"]);
    claim_at(&store, "FR-9005", "FR-9005");
    commit_all(&store, "fork claims FR-9005");
    git(&store, &["checkout", "-q", &branch]);
    claim_at(&store, "FR-9005", "FR-9006");
    commit_all(&store, "HEAD claims FR-9005");
    let before = snapshot(&store);

    let out = aida(
        &repo,
        &home,
        &["db", "audit-ids", "--json", "--against", "fork"],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["status"], "conflicts", "{stdout}");
    // FR-9001 stays a dispute inside HEAD; FR-9005 is the one a sync refuses.
    // (Both IDs sit above every recorded range, so `unissued` rows appear too.)
    let mut display_scopes: Vec<&str> = json["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "display_id")
        .map(|c| c["scope"].as_str().unwrap())
        .collect();
    display_scopes.sort_unstable();
    assert_eq!(display_scopes, ["in_head", "introduced"], "{stdout}");

    let human = aida(&repo, &home, &["db", "audit-ids", "--against", "fork"]);
    let human_out = String::from_utf8_lossy(&human.stdout);
    assert_eq!(human.status.code(), Some(1), "{human_out}");
    assert!(
        human_out.starts_with("Allocation claims audit: conflicts ("),
        "{human_out}"
    );
    assert!(human_out.contains("[sync refuses]"), "{human_out}");

    assert_eq!(snapshot(&store), before, "the audit must write nothing");
}
