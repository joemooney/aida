//! STORY-2897: the landing re-check made immediately before the forge merge
//! call reads its authority and policy fresh. Run against a real local origin:
//! the re-check fetches the change's refs and the trusted default branch.
// trace:STORY-2897 | ai:claude

use super::recheck as final_landing_recheck;
use crate::gate::ship::ShipRefusal;
use std::path::Path;

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn commit_config(dir: &Path, body: &str, message: &str) {
    std::fs::create_dir_all(dir.join(".aida")).unwrap();
    std::fs::write(dir.join(".aida/config.toml"), body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A repo whose origin (a local bare clone) has `main`, `integration` and
/// the change branch `feat`, all fetched; returns (tempdir, repo, origin).
fn repo_with_origin() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    commit_config(&repo, "[land]\nwip_max = 4\n", "base");
    git(&repo, &["branch", "integration"]);
    git(&repo, &["branch", "feat"]);
    git(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["fetch", "-q", "origin"]);
    (temp, repo, origin)
}

fn change_into_integration() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "integration".into(),
        title: None,
    }
}

/// A deputy marker can expire (or be revoked) while the ship waits: the
/// floor authority the ship started under is re-read, and a lapse refuses
/// the merge as a review/authority refusal, not a behind-base one.
#[test]
fn a_floor_authority_that_lapsed_while_waiting_refuses_the_merge() {
    let (_temp, repo, _origin) = repo_with_origin();
    let entry = crate::landing_policy::load(&repo);
    let change = change_into_integration();
    let (kind, why) = final_landing_recheck(&repo, &change, &entry, &[], true, || false)
        .expect_err("a lapsed floor must not merge");
    assert_eq!(kind, ShipRefusal::Review, "{why}");
    assert!(why.contains("lapsed"), "{why}");
    assert_eq!(
        final_landing_recheck(&repo, &change, &entry, &[], true, || true),
        Ok(()),
        "a floor still held merges"
    );
}

/// A ship into a non-default base (`integration`) still decides under the
/// default branch's policy as the remote has it now: a `[landing]` change
/// pushed to origin's `main` while the ship waits is seen and refuses the
/// merge, although the local `origin/main` was stale.
#[test]
fn a_policy_change_on_the_remote_default_branch_refuses_a_non_default_base_ship() {
    let (temp, repo, origin) = repo_with_origin();
    let entry = crate::landing_policy::load(&repo);
    assert_eq!(entry.merger, crate::landing_policy::Merger::AnyNonAuthor);
    // Another machine lands a policy change on main.
    let other = temp.path().join("other");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "t@example.com"]);
    git(&other, &["config", "user.name", "T"]);
    commit_config(
        &other,
        "[landing]\nmerger = \"advisor\"\n",
        "advisor merges",
    );
    git(&other, &["push", "-q", "origin", "main"]);
    let (kind, why) = final_landing_recheck(
        &repo,
        &change_into_integration(),
        &entry,
        &[],
        false,
        || false,
    )
    .expect_err("the superseded policy must not decide the merge");
    assert_eq!(kind, ShipRefusal::Review, "{why}");
    assert!(
        why.contains("landing policy changed") && why.contains("merger any-non-author -> advisor"),
        "{why}"
    );
}
