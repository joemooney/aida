//! STORY-2903 S1c (advisor ruling (a)): in review-gated mode there is no
//! train, so a head behind its base merges as it is only when the base's
//! new commits touch none of its paths and it touches no gate path.
// trace:STORY-2903 | ai:claude

use super::{judge_behind_base, review_gated_behind_base, BehindBase};
use std::path::Path;

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn the_review_gated_behind_base_rule() {
    let floor = crate::gate::mandate_gates::ProtectedPaths::floor();
    let ok = Ok(&floor);
    assert_eq!(
        judge_behind_base(0, &names(&["a.rs"]), &[], ok),
        BehindBase::Current
    );
    assert_eq!(
        judge_behind_base(2, &names(&["docs/a.md"]), &names(&["docs/b.md"]), ok),
        BehindBase::Disjoint
    );
    assert_eq!(
        judge_behind_base(2, &names(&["docs/a.md"]), &names(&["docs/a.md", "x"]), ok),
        BehindBase::Overlap(names(&["docs/a.md"]))
    );
    // A gate path never merges stale, even when the base is disjoint.
    let gate = "aida-cli-lib/src/gate/ship.rs";
    assert!(
        crate::gate::mandate_gates::touches_gates(&names(&[gate]), &floor).is_some(),
        "the floor protects gate/"
    );
    assert_eq!(
        judge_behind_base(1, &names(&[gate]), &names(&["docs/b.md"]), ok),
        BehindBase::GatePath(Some(gate.to_string()))
    );
    // An unreadable protected set fails closed.
    assert_eq!(
        judge_behind_base(1, &names(&["docs/a.md"]), &names(&["docs/b.md"]), Err(())),
        BehindBase::GatePath(None)
    );
}

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn commit(dir: &Path, file: &str, body: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// main has moved past `feat` with a commit touching `base_file`; `feat`
/// changes `pr_file`. Returns the repo, served from a local origin.
fn behind(pr_file: &str, base_file: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    commit(&repo, "seed.txt", "seed");
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    commit(&repo, pr_file, "change");
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, base_file, "moved");
    let origin = root.join("origin.git");
    git(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    (temp, repo)
}

#[test]
fn a_disjoint_behind_base_head_merges_in_review_gated_mode() {
    let (_t, repo) = behind("docs/a.md", "docs/b.md");
    assert_eq!(review_gated_behind_base(&repo, "feat", 7, "main"), Ok(()));
}

#[test]
fn an_overlapping_behind_base_head_needs_an_update_and_a_delta_review() {
    let (_t, repo) = behind("docs/a.md", "docs/a.md");
    let (kind, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert_eq!(kind, crate::gate::ship::ShipRefusal::BehindBase);
    assert!(
        why.contains("touch docs/a.md") && why.contains("delta review"),
        "{why}"
    );
}

#[test]
fn a_stale_gate_path_change_needs_full_ci_on_the_current_base() {
    let (_t, repo) = behind("aida-cli-lib/src/gate/ship.rs", "docs/b.md");
    let (_, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert!(why.contains("full CI"), "{why}");
}
