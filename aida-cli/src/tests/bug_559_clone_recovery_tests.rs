use super::*;
use std::process::Command;

fn git(dir: &std::path::Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git spawn")
        .status
        .success();
    assert!(ok, "git {args:?} failed");
}

/// Decision logic: a code branch is available → check it out (don't detach,
/// so the user lands on real code, not a headless store tree).
#[test]
fn recovery_prefers_code_branch_when_available() {
    let tmp = tempfile::TempDir::new().unwrap();
    let chosen = choose_store_attach_recovery(tmp.path(), &["main".to_string()]);
    assert_eq!(
        chosen,
        StoreAttachRecovery::CheckoutCodeBranch("main".to_string())
    );
}

/// Decision logic: no code branch → detach HEAD to free the `aida-store`
/// ref (still recovers; never errors out the way the pre-fix path did).
#[test]
fn recovery_detaches_when_no_code_branch() {
    let tmp = tempfile::TempDir::new().unwrap();
    let chosen = choose_store_attach_recovery(tmp.path(), &[]);
    assert_eq!(chosen, StoreAttachRecovery::DetachHead);
}

/// Decision logic: `main` is preferred over `master` when both are offered.
#[test]
fn recovery_prefers_main_over_master() {
    let tmp = tempfile::TempDir::new().unwrap();
    let chosen =
        choose_store_attach_recovery(tmp.path(), &["main".to_string(), "master".to_string()]);
    assert_eq!(
        chosen,
        StoreAttachRecovery::CheckoutCodeBranch("main".to_string())
    );
}

/// Sanity: a normal (non-GitLab) checkout — not on `aida-store` — picks no
/// recovery candidates from a tree that only has `aida-store`, but the
/// candidate collector finds the real `main` when present.
#[test]
fn candidate_collector_finds_local_main() {
    let tmp = tempfile::TempDir::new().unwrap();
    let p = tmp.path();
    git(p, &["init", "-q", "-b", "main"]);
    git(p, &["config", "user.email", "t@example.com"]);
    git(p, &["config", "user.name", "t"]);
    git(p, &["commit", "--allow-empty", "-qm", "init"]);
    let candidates = local_code_branch_candidates(p);
    assert!(
        candidates.contains(&"main".to_string()),
        "expected main among candidates, got {candidates:?}"
    );
}

/// End-to-end recovery of the exact BUG-559 state: a clone whose default
/// branch is `aida-store` (orphan store checked out as the working tree)
/// with `origin/main` and `origin/aida-store` present. `try_attach_store_worktree`
/// must NOT error — it must switch off `aida-store`, fetch the store ref,
/// and attach the `.aida-store` worktree.
#[test]
fn try_attach_recovers_when_aida_store_is_default_branch() {
    let base = tempfile::TempDir::new().unwrap();
    let bare = base.path().join("remote.git");
    let seed = base.path().join("seed");
    // Bare "remote".
    git(
        base.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    // Seed repo: a code commit on main + an orphan aida-store branch, then
    // push aida-store FIRST (the GitLab push-to-create order) and main.
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    git(&seed, &["config", "user.email", "t@example.com"]);
    git(&seed, &["config", "user.name", "t"]);
    git(&seed, &["commit", "--allow-empty", "-qm", "main commit"]);
    git(&seed, &["checkout", "-q", "--orphan", "aida-store"]);
    let _ = Command::new("git")
        .arg("-C")
        .arg(&seed)
        .args(["rm", "-rfq", "--cached", "."])
        .output();
    git(&seed, &["commit", "--allow-empty", "-qm", "store init"]);
    git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&seed, &["push", "-q", "-u", "origin", "aida-store"]);
    git(&seed, &["push", "-q", "origin", "main"]);
    // Make aida-store the remote default (the GitLab quirk).
    git(&bare, &["symbolic-ref", "HEAD", "refs/heads/aida-store"]);

    // Fresh clone → checks out aida-store as the working tree.
    let clone = base.path().join("clone");
    git(
        base.path(),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    // Precondition: we really are in the failing state.
    assert_eq!(
        aida_core::git_ops::current_branch(&clone).unwrap(),
        "aida-store"
    );

    // The fix: recover instead of erroring.
    let store_path = try_attach_store_worktree(&clone)
        .expect("recovery should attach the store worktree, not error");
    assert!(store_path.ends_with(".aida-store"));
    assert!(store_path.exists(), "store worktree dir should exist");
    // We must have moved OFF aida-store onto the code branch.
    assert_ne!(
        aida_core::git_ops::current_branch(&clone).ok().as_deref(),
        Some("aida-store"),
        "working tree should no longer be on aida-store"
    );
}
