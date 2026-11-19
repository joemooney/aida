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
        judge_behind_base(2, &names(&["notes/a.txt"]), &names(&["notes/b.txt"]), ok),
        BehindBase::Disjoint
    );
    assert_eq!(
        judge_behind_base(
            2,
            &names(&["notes/a.txt"]),
            &names(&["notes/a.txt", "x"]),
            ok
        ),
        BehindBase::Overlap(names(&["notes/a.txt"]))
    );
    // A gate path never merges stale, even when the base is disjoint.
    let gate = "aida-cli-lib/src/gate/ship.rs";
    assert!(
        crate::gate::mandate_gates::touches_gates(&names(&[gate]), &floor).is_some(),
        "the floor protects gate/"
    );
    assert_eq!(
        judge_behind_base(1, &names(&[gate]), &names(&["notes/b.txt"]), ok),
        BehindBase::GatePath(Some(gate.to_string()))
    );
    // An unreadable protected set fails closed.
    assert_eq!(
        judge_behind_base(
            1,
            &names(&["notes/a.txt"]),
            &names(&["notes/b.txt"]),
            Err(())
        ),
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
    let (_t, repo) = behind("notes/a.txt", "notes/b.txt");
    assert_eq!(review_gated_behind_base(&repo, "feat", 7, "main"), Ok(()));
}

#[test]
fn an_overlapping_behind_base_head_needs_an_update_and_a_delta_review() {
    let (_t, repo) = behind("notes/a.txt", "notes/a.txt");
    let (kind, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert_eq!(kind, crate::gate::ship::ShipRefusal::BehindBase);
    assert!(
        why.contains("touch notes/a.txt") && why.contains("delta review"),
        "{why}"
    );
}

/// Ruling S1 (STORY-2903, 20:58): a protected-path change behind its base
/// always refuses, even when the base's commits are disjoint from it.
#[test]
fn a_stale_gate_path_change_always_refuses() {
    let (_t, repo) = behind("aida-cli-lib/src/gate/ship.rs", "notes/b.txt");
    let (_, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert!(
        why.contains("never merges behind its base") && why.contains("delta review"),
        "{why}"
    );
}

/// F2: an edit to a change's path that a later base commit reverted still
/// touched it: the change needs an update and a delta review.
#[test]
fn a_base_edit_later_reverted_still_counts_as_a_touch() {
    let (_t, repo) = behind("notes/a.txt", "notes/b.txt");
    // Edit the change's file on main, then put it back.
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, "notes/a.txt", "base edit");
    git(&repo, &["rm", "-q", "notes/a.txt"]);
    git(&repo, &["commit", "-q", "-m", "revert"]);
    let origin = repo.parent().unwrap().join("origin.git");
    git(&repo, &["push", "-q", origin.to_str().unwrap(), "main"]);
    let (_, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert!(
        why.contains("notes/a.txt") && why.contains("delta review"),
        "{why}"
    );
}

/// F3: a protected path whose name git would quote (non-ASCII, a control
/// character) is still recognised as protected.
#[test]
fn quoted_protected_paths_are_still_protected() {
    for name in [
        "aida-cli-lib/src/gate/é.rs",
        "aida-cli-lib/src/gate/a\tb.rs",
    ] {
        let (_t, repo) = behind(name, "notes/b.txt");
        let (_, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
        assert!(
            why.contains("never merges behind its base"),
            "{name:?}: {why}"
        );
    }
}

/// A review-gated repo (its default branch's config says so) whose `feat`
/// changes `pr_file` while main moved with `base_file`.
fn review_gated(pr_file: &str, base_file: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    commit(
        &repo,
        ".aida/config.toml",
        "[landing]\nmode = \"review-gated\"\n",
    );
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
    git(&repo, &["fetch", "-q", "origin"]);
    (temp, repo)
}

fn change() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "main".into(),
        title: None,
    }
}

/// F1: base freshness is judged inside the merge boundary, so the burndown
/// and release callers (which run no gate of their own) are refused too,
/// on the refs fetched at the merge.
#[test]
fn the_boundary_refuses_a_stale_overlapping_head_for_every_caller() {
    use super::{merge, Caller, Entry, MergeError};
    let (_t, repo) = review_gated("notes/a.txt", "notes/a.txt");
    let entry = Entry {
        policy: crate::landing_policy::load(&repo),
        floor: false,
    };
    assert_eq!(
        entry.policy.mode,
        crate::landing_policy::LandingMode::ReviewGated
    );
    let forge = crate::forge::fake::RecordingForge::new();
    for caller in [
        Caller::Ship,
        Caller::Drain,
        Caller::Burndown,
        Caller::Release,
    ] {
        let got = merge(
            &forge,
            &repo,
            caller,
            &entry,
            &change(),
            &[],
            &crate::forge::MergeOptions {
                method: crate::forge::MergeMethod::Squash,
                squash_subject: None,
                squash_body: None,
                delete_branch: false,
                match_head: None,
            },
            &mut crate::network_retry::NoopSink,
        );
        match got {
            Err(MergeError::Refused(kind, why)) => {
                assert_eq!(
                    kind,
                    crate::gate::ship::ShipRefusal::BehindBase,
                    "{caller:?}"
                );
                assert!(why.contains("delta review"), "{caller:?}: {why}");
            }
            other => panic!("{caller:?}: {other:?}"),
        }
    }
}

/// F5: a merge path that skipped CI under review-gated mode carries that
/// snapshot to the merge; the mode changing back to ci-gated meanwhile
/// refuses the merge instead of landing an untested head.
#[test]
fn a_mode_change_after_the_ci_skip_refuses_the_merge() {
    use super::{merge, Caller, Entry, MergeError};
    let (_t, repo) = review_gated("notes/a.txt", "notes/b.txt");
    let skipped_under = Entry {
        policy: crate::landing_policy::load(&repo),
        floor: false,
    };
    assert!(!skipped_under.policy.waits_for_ci());
    // Another merge puts main back to ci-gated.
    let other = repo.parent().unwrap().join("other");
    let origin = repo.parent().unwrap().join("origin.git");
    git(
        repo.parent().unwrap(),
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["config", "user.email", "t@example.com"]);
    git(&other, &["config", "user.name", "T"]);
    commit(
        &other,
        ".aida/config.toml",
        "[landing]\nmode = \"ci-gated\"\n",
    );
    git(&other, &["push", "-q", "origin", "main"]);
    for caller in [Caller::Drain, Caller::Burndown] {
        let got = merge(
            &crate::forge::fake::RecordingForge::new(),
            &repo,
            caller,
            &skipped_under,
            &change(),
            &[],
            &crate::forge::MergeOptions {
                method: crate::forge::MergeMethod::Squash,
                squash_subject: None,
                squash_body: None,
                delete_branch: false,
                match_head: None,
            },
            &mut crate::network_retry::NoopSink,
        );
        match got {
            Err(MergeError::Refused(_, why)) => {
                assert!(why.contains("landing policy changed"), "{caller:?}: {why}")
            }
            other => panic!("{caller:?}: {other:?}"),
        }
    }
}

/// Round 2 (finding 2): a change that moves a protected file out of its
/// protected prefix still changes the protected path (renames report both
/// sides), and a base commit that renamed a file the change edits overlaps.
#[test]
fn renames_count_both_their_source_and_destination() {
    // The change renames a gate file to outside gate/.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    let body = "fn gate() {}\n".repeat(40);
    commit(&repo, "aida-cli-lib/src/gate/moved.rs", &body);
    commit(&repo, "notes/edited.txt", &"line\n".repeat(40));
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    git(
        &repo,
        &["mv", "aida-cli-lib/src/gate/moved.rs", "notes/moved.rs"],
    );
    git(&repo, &["commit", "-q", "-m", "move out of gate/"]);
    git(&repo, &["checkout", "-q", "main"]);
    commit(&repo, "notes/b.txt", "moved");
    let origin = root.join("origin.git");
    git(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    let (_, why) = review_gated_behind_base(&repo, "feat", 7, "main").unwrap_err();
    assert!(
        why.contains("protected path aida-cli-lib/src/gate/moved.rs"),
        "{why}"
    );

    // The base renames a file the change edits.
    git(&repo, &["checkout", "-q", "-b", "feat2", "main~1"]);
    std::fs::write(
        repo.join("notes/edited.txt"),
        "line\n".repeat(40) + "added\n",
    )
    .unwrap();
    git(&repo, &["commit", "-qam", "edit"]);
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["mv", "notes/edited.txt", "notes/renamed.txt"]);
    git(&repo, &["commit", "-q", "-m", "rename on main"]);
    git(
        &repo,
        &["push", "-q", origin.to_str().unwrap(), "main", "feat2"],
    );
    let (_, why) = review_gated_behind_base(&repo, "feat2", 8, "main").unwrap_err();
    assert!(
        why.contains("notes/edited.txt") && why.contains("delta review"),
        "{why}"
    );
}

/// STORY-2905 (advisor call (c)): the auto-train stands down in
/// review-gated mode, where the land loop is the one merge hand, and runs
/// in ci-gated mode.
#[test]
fn the_auto_train_stands_down_only_in_review_gated_mode() {
    let (_t, repo) = review_gated("notes/a.txt", "notes/b.txt");
    assert_eq!(
        super::train_stand_down(&repo),
        Some("standing down: landing.mode=review-gated")
    );
    let (_t, plain) = behind("notes/a.txt", "notes/b.txt");
    assert_eq!(super::train_stand_down(&plain), None);
}
