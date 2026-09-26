//! BUG-1657 — a spec that landed through a batched integration PR is reapable.
//!
//! An implementer's branch is never merged directly: an integration branch
//! folds several specs' work together and is squash-merged, leaving one commit
//! on the default branch whose body lists one line per spec ending in
//! `(SPEC-ID)`. Neither the ancestry probe nor the per-branch forge lookup can
//! see that, so every such session used to stay forever as "no merge signal".
//!
//! Every test builds its own throwaway git repo under a temp dir; no real
//! worktree, lease, store, or forge is touched. The forge lookup is injected.
//
// trace:BUG-1657 | ai:claude

use super::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(root: &Path, file: &str, content: &str, message: &str) {
    std::fs::write(root.join(file), content).unwrap();
    git(root, &["add", file]);
    git(root, &["commit", "-q", "-m", message]);
}

/// A repo whose `main` holds a batched integration squash for two specs:
///   * `spec-a` (BUG-9001) and `spec-b` (BUG-9002) each forked from `main`
///     and made their own commits;
///   * `main` then gained ONE new commit carrying both specs' content, with the
///     batch subject and one `(SPEC-ID)` body line per spec, plus an unrelated
///     commit after it.
fn batched_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "Test"]);
    git(&root, &["config", "user.email", "test@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit_file(&root, "README.md", "base\n", "init");
    git(&root, &["branch", "-M", "main"]);

    git(&root, &["checkout", "-q", "-b", "spec-a"]);
    commit_file(&root, "a.txt", "a one\n", "fix(a): first part (BUG-9001)");
    commit_file(
        &root,
        "a.txt",
        "a one\na two\n",
        "fix(a): second part (BUG-9001)",
    );

    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["checkout", "-q", "-b", "spec-b"]);
    commit_file(&root, "b.txt", "b\n", "fix(b): the change (BUG-9002)");

    // The integration squash: both specs' content in one brand-new commit.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("a.txt"), "a one\na two\n").unwrap();
    std::fs::write(root.join("b.txt"), "b\n").unwrap();
    git(&root, &["add", "a.txt", "b.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:claude] chore(integrate): batch 7 - BUG-9001 BUG-9002 (#42)",
            "-m",
            "Fix the a thing (BUG-9001)\nFix the b thing (BUG-9002)",
        ],
    );
    commit_file(&root, "c.txt", "later\n", "chore: unrelated later work");
    (tmp, root)
}

fn reap_verdict(worktree: AgentWorktreeFacts, spec_finished: bool) -> ReapVerdict {
    classify_session_reap(&ReapFacts {
        spec_finished,
        process_exited: true,
        locked: false,
        head_on_branch: true,
        worktree,
    })
}

#[test]
fn bug_1657_batch_merged_spec_is_reapable() {
    let (_tmp, root) = batched_repo();
    let forge_called = Cell::new(false);
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-a",
        "BUG-9001",
        false,
        true,
        |_| {
            forge_called.set(true);
            false
        },
    );
    assert!(!facts.ancestor_of_main, "{facts:?}");
    assert!(facts.spec_trailer_on_main, "{facts:?}");
    assert!(facts.unique_unmerged_commits > 0, "{facts:?}");
    assert!(facts.content_fully_landed, "{facts:?}");
    assert!(
        !forge_called.get(),
        "the local trailer signal should spare the forge lookup"
    );
    match reap_verdict(facts, true) {
        ReapVerdict::Reap(reason) => assert!(reason.contains("integration"), "{reason}"),
        v => panic!("a batch-landed session must be reapable, got {v:?}"),
    }
    // The sibling spec in the same batch is recognised too.
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-b",
        "BUG-9002",
        false,
        true,
        |_| false,
    );
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Reap(_)));
}

#[test]
fn bug_1657_branch_with_unlanded_work_for_the_same_spec_is_kept() {
    let (_tmp, root) = batched_repo();
    // The same spec's branch picked up a follow-up commit after the batch
    // landed; that commit never shipped.
    git(&root, &["checkout", "-q", "spec-a"]);
    commit_file(
        &root,
        "a-extra.txt",
        "never shipped\n",
        "fix(a): follow-up (BUG-9001)",
    );
    git(&root, &["checkout", "-q", "main"]);

    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-a",
        "BUG-9001",
        false,
        true,
        |_| false,
    );
    assert!(facts.spec_trailer_on_main, "{facts:?}");
    assert!(!facts.content_fully_landed, "{facts:?}");
    match reap_verdict(facts, true) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("unique unmerged"), "{reason}"),
        v => panic!("unshipped work must never be reaped, got {v:?}"),
    }
}

#[test]
fn bug_1657_spec_still_in_progress_is_kept() {
    let (_tmp, root) = batched_repo();
    // A spec whose branch has work but never landed and is not finished.
    git(&root, &["checkout", "-q", "-b", "spec-c", "main"]);
    commit_file(&root, "c2.txt", "wip\n", "fix(c): wip (BUG-9003)");
    git(&root, &["checkout", "-q", "main"]);

    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-c",
        "BUG-9003",
        false,
        false,
        |_| panic!("no forge lookup for an unfinished spec"),
    );
    assert!(!facts.spec_trailer_on_main, "{facts:?}");
    assert!(matches!(reap_verdict(facts, false), ReapVerdict::Skip(_)));

    // Even if marked finished, no landing commit names it → no merge signal.
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-c",
        "BUG-9003",
        false,
        true,
        |_| false,
    );
    assert!(!facts.spec_trailer_on_main, "{facts:?}");
    match reap_verdict(facts, true) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("no merge signal"), "{reason}"),
        v => panic!("expected a skip, got {v:?}"),
    }
}

#[test]
fn bug_1657_dirty_worktree_is_never_reaped() {
    let (tmp, root) = batched_repo();
    let wt = tmp.path().join("wt-spec-a");
    git(
        &root,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "spec-a"],
    );
    std::fs::write(wt.join("scratch.txt"), "uncommitted\n").unwrap();

    let dirty = !worktree_dirty_entries(&wt).is_empty();
    assert!(dirty, "fixture worktree should read as dirty");
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-a",
        "BUG-9001",
        dirty,
        true,
        |_| panic!("no forge lookup for a dirty worktree"),
    );
    match reap_verdict(facts, true) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("uncommitted"), "{reason}"),
        v => panic!("a dirty worktree must never be reaped, got {v:?}"),
    }
}

#[test]
fn bug_1657_direct_merges_still_reap() {
    let (_tmp, root) = batched_repo();
    // A true merge: the branch becomes an ancestor of main.
    git(&root, &["checkout", "-q", "-b", "spec-d", "main"]);
    commit_file(&root, "d.txt", "d\n", "fix(d): change (BUG-9004)");
    git(&root, &["checkout", "-q", "main"]);
    git(
        &root,
        &["merge", "-q", "--no-ff", "-m", "Merge spec-d", "spec-d"],
    );
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-d",
        "BUG-9004",
        false,
        true,
        |_| panic!("no forge lookup once ancestry proves the merge"),
    );
    assert!(facts.ancestor_of_main, "{facts:?}");
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Reap(_)));

    // A direct squash-merged PR whose landing commit carries no trailer: the
    // forge signal plus patch-id content proof still reaps it.
    git(&root, &["checkout", "-q", "-b", "spec-e", "main"]);
    commit_file(&root, "e.txt", "e\n", "fix(e): change");
    git(&root, &["checkout", "-q", "main"]);
    commit_file(&root, "e.txt", "e\n", "fix(e): change (#43)");
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-e",
        "BUG-9005",
        false,
        true,
        |_| true,
    );
    assert!(facts.pr_merged && !facts.spec_trailer_on_main, "{facts:?}");
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Reap(_)));
}

#[test]
fn bug_1657_trailer_match_is_exact_and_ignores_plan_commits() {
    let (_tmp, root) = batched_repo();
    // A prefix of a landed id is not that id.
    assert!(!spec_trailer_landed_on(&root, "main", "spec-a", "BUG-900"));
    assert!(spec_trailer_landed_on(&root, "main", "spec-a", "bug-9001"));

    // A plan commit names what it plans, not what shipped.
    git(&root, &["checkout", "-q", "-b", "spec-f", "main"]);
    commit_file(&root, "f.txt", "f\n", "fix(f): change (BUG-9006)");
    git(&root, &["checkout", "-q", "main"]);
    commit_file(
        &root,
        "plan.md",
        "plan\n",
        "[AI:claude] docs(plans): plan f (BUG-9006)",
    );
    assert!(!spec_trailer_landed_on(&root, "main", "spec-f", "BUG-9006"));
}

#[test]
fn bug_1657_per_path_probe_keeps_reverted_work() {
    let (_tmp, root) = batched_repo();
    assert!(crate::doctor_cmd::branch_paths_match_default(
        &root, "main", "spec-a"
    ));
    // The batch landed, then the spec's change was reverted on main: merging
    // main no longer holds the branch's a.txt, so it is not provably shipped.
    std::fs::remove_file(root.join("a.txt")).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "revert a"]);
    assert!(!crate::doctor_cmd::branch_paths_match_default(
        &root, "main", "spec-a"
    ));
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-a",
        "BUG-9001",
        false,
        true,
        |_| false,
    );
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Skip(_)));
}

/// Regression: a `.gitattributes` merge driver must not launder an unshipped
/// edit. With `lock.txt merge=ours`, a three-way merge of the branch into main
/// "succeeds" and equals main's tree even though the branch's lock.txt edit
/// never shipped. The per-path proof compares blobs directly and keeps it.
#[test]
fn bug_1657_merge_ours_driver_does_not_hide_unshipped_edit() {
    let (_tmp, root) = batched_repo();
    git(&root, &["config", "merge.ours.driver", "true"]);
    commit_file(&root, ".gitattributes", "lock.txt merge=ours\n", "attrs");
    commit_file(&root, "lock.txt", "v1\n", "add lock");

    git(&root, &["checkout", "-q", "-b", "spec-g", "main"]);
    commit_file(&root, "g.txt", "g\n", "fix(g): change (BUG-9007)");
    commit_file(
        &root,
        "lock.txt",
        "v2 never shipped\n",
        "fix(g): lock edit (BUG-9007)",
    );

    // The batch ships g.txt but NOT the branch's lock.txt edit; main edits
    // lock.txt its own way, so the merge driver is what resolves it.
    git(&root, &["checkout", "-q", "main"]);
    std::fs::write(root.join("g.txt"), "g\n").unwrap();
    std::fs::write(root.join("lock.txt"), "v1 main\n").unwrap();
    git(&root, &["add", "g.txt", "lock.txt"]);
    git(
        &root,
        &[
            "commit",
            "-q",
            "-m",
            "[AI:claude] chore(integrate): batch 8 - BUG-9007 BUG-9008 (#44)",
            "-m",
            "Fix the g thing (BUG-9007)\nOther work (BUG-9008)",
        ],
    );

    // The trap being guarded against: merge-tree reports main's own tree.
    let merged = git(&root, &["merge-tree", "--write-tree", "main", "spec-g"]);
    let main_tree = git(&root, &["rev-parse", "main^{tree}"]);
    assert_eq!(merged.lines().next().unwrap_or(""), main_tree);

    assert!(!crate::doctor_cmd::branch_paths_match_default(
        &root, "main", "spec-g"
    ));
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "spec-g",
        "BUG-9007",
        false,
        true,
        |_| false,
    );
    assert!(facts.spec_trailer_on_main, "{facts:?}");
    assert!(!facts.content_fully_landed, "{facts:?}");
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Skip(_)));
}

/// A failing ancestry count is unknown, not zero: no merge signal is probed
/// and the session is kept.
#[test]
fn bug_1657_failed_ancestry_count_fails_closed() {
    let (_tmp, root) = batched_repo();
    // The landed spec id is real, but the branch cannot be resolved.
    let facts = gather_merge_facts(
        &root,
        Some("main"),
        "no-such-branch",
        "BUG-9001",
        false,
        true,
        |_| panic!("no forge lookup when the ancestry count is unknown"),
    );
    assert!(!facts.ancestor_of_main, "{facts:?}");
    assert!(!facts.spec_trailer_on_main && !facts.pr_merged, "{facts:?}");
    assert!(facts.unique_unmerged_commits > 0, "{facts:?}");
    assert!(matches!(reap_verdict(facts, true), ReapVerdict::Skip(_)));
}

/// A worktree whose HEAD is detached, or switched to another branch, is kept
/// even when the lease branch itself is provably landed.
#[test]
fn bug_1657_worktree_off_its_session_branch_is_kept() {
    let (tmp, root) = batched_repo();
    let wt = tmp.path().join("wt-spec-a");
    git(
        &root,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "spec-a"],
    );
    assert!(worktree_head_on_branch(&wt, "spec-a"));
    assert!(!worktree_head_on_branch(&wt, ""));

    let landed = || {
        gather_merge_facts(
            &root,
            Some("main"),
            "spec-a",
            "BUG-9001",
            false,
            true,
            |_| false,
        )
    };
    let verdict = |head_on_branch: bool| {
        classify_session_reap(&ReapFacts {
            spec_finished: true,
            process_exited: true,
            locked: false,
            head_on_branch,
            worktree: landed(),
        })
    };
    assert!(matches!(
        verdict(worktree_head_on_branch(&wt, "spec-a")),
        ReapVerdict::Reap(_)
    ));

    git(&wt, &["checkout", "-q", "--detach"]);
    assert!(!worktree_head_on_branch(&wt, "spec-a"));
    match verdict(worktree_head_on_branch(&wt, "spec-a")) {
        ReapVerdict::Skip(reason) => assert!(reason.contains("session branch"), "{reason}"),
        v => panic!("a detached worktree must be kept, got {v:?}"),
    }

    git(&wt, &["checkout", "-q", "-b", "side-work"]);
    assert!(!worktree_head_on_branch(&wt, "spec-a"));
    assert!(matches!(
        verdict(worktree_head_on_branch(&wt, "spec-a")),
        ReapVerdict::Skip(_)
    ));
}
