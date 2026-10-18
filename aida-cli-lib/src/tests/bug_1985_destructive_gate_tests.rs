//! BUG-1985 review round 4: every destructive dirty gate sees an uncommitted
//! edit inside a registered submodule of a linked worktree (a tree a
//! contained session may have written, where host git carries
//! `diff.ignoreSubmodules=dirty`), so the tree is kept instead of being
//! removed with force. One test per gate family: session reap, doctor heal,
//! worktree gc/pool destroy; plus the host staging refusal `aida compete` runs
//! before `git add -A`.
//!
//! trace:BUG-1985 | ai:claude

use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `<tmp>/main` with a registered submodule at `dep`, and a linked worktree
/// `<tmp>/wt` with the submodule initialized. Returns (tmp, main, wt).
fn submodule_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let dep = parent.join("dep-origin");
    std::fs::create_dir_all(&dep).unwrap();
    git(&dep, &["init", "-q", "-b", "main"]);
    std::fs::write(dep.join("lib.txt"), "lib\n").unwrap();
    git(&dep, &["add", "lib.txt"]);
    git(&dep, &["commit", "-qm", "dep"]);
    let main = parent.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    std::fs::write(main.join("README.md"), "main\n").unwrap();
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-qm", "init"]);
    git(
        &main,
        &["submodule", "add", "-q", dep.to_str().unwrap(), "dep"],
    );
    git(&main, &["commit", "-qm", "add dep"]);
    let wt = parent.join("wt");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    git(&wt, &["submodule", "update", "-q", "--init"]);
    assert!(aida_core::git_ops::is_contained_writable_tree(&wt));
    (tmp, main, wt)
}

/// Leave an uncommitted edit inside the worktree's submodule; returns its path.
fn edit_submodule(wt: &Path) -> PathBuf {
    let edited = wt.join("dep").join("lib.txt");
    std::fs::write(&edited, "unsaved submodule edit\n").unwrap();
    edited
}

// trace:BUG-1985.acba0e36 | ai:claude
#[test]
fn reap_gate_keeps_a_tree_with_a_submodule_edit() {
    let (_tmp, _main, wt) = submodule_worktree();
    assert!(
        crate::status_sections::worktree_dirty_entries(&wt).is_empty(),
        "a fresh tree with a clean submodule reads clean"
    );
    assert!(!crate::session_reap::worktree_status_dirty(&wt));
    edit_submodule(&wt);
    let entries = crate::status_sections::worktree_dirty_entries(&wt);
    assert!(
        entries.iter().any(|e| e.contains("dep")),
        "the submodule edit must read as dirty: {entries:?}"
    );
    assert!(
        crate::session_reap::worktree_status_dirty(&wt),
        "the reap gate must keep the tree"
    );
}

// trace:BUG-1985.acba0e36 | ai:claude
#[test]
fn doctor_heal_skips_a_tree_with_a_submodule_edit() {
    let (_tmp, main, wt) = submodule_worktree();
    let edited = edit_submodule(&wt);
    let finding = crate::DoctorFinding {
        category: "merged-agent-worktrees".to_string(),
        id: wt.display().to_string(),
        summary: "merged agent worktree".to_string(),
        action: "aida doctor --heal --category merged-agent-worktrees".to_string(),
        safe_heal: true,
    };
    let result = super::heal_doctor_merged_agent_worktree(&main, &finding).unwrap();
    assert_eq!(result.status, "skipped", "{:?}", result.detail);
    assert!(wt.exists(), "the doctor heal must not remove the tree");
    assert_eq!(
        std::fs::read_to_string(&edited).unwrap(),
        "unsaved submodule edit\n"
    );
}

// The gc salvage gate sees the edit, and pool destroy (worktree gc's pool
// path) classifies the tree dirty through `git_ops::worktree_is_dirty`.
// trace:BUG-1985.acba0e36 | ai:claude
#[test]
fn gc_and_pool_destroy_gates_see_a_submodule_edit() {
    let (_tmp, main, wt) = submodule_worktree();
    assert!(!aida_core::git_ops::worktree_is_dirty(&wt));
    edit_submodule(&wt);
    assert!(aida_core::git_ops::worktree_is_dirty(&wt));
    let patch = crate::worktree_gc::salvage_worktree_patch(&main, "BUG-1985", None, &wt).unwrap();
    assert!(
        patch.is_some(),
        "the gc salvage gate must see the tree as dirty"
    );
}

// `aida compete` refuses host staging over a tree holding an untracked
// embedded repository, which `git add -A` would stage as a gitlink.
// trace:BUG-1985.ac027ce2 | ai:claude
#[test]
fn compete_staging_refuses_an_untracked_embedded_repo() {
    let (_tmp, _main, wt) = submodule_worktree();
    assert!(aida_core::git_ops::refuse_untrusted_staging(&wt).is_ok());
    let planted = wt.join("planted");
    std::fs::create_dir_all(&planted).unwrap();
    git(&planted, &["init", "-q"]);
    std::fs::write(planted.join("x.txt"), "x").unwrap();
    let err = aida_core::git_ops::refuse_untrusted_staging(&wt)
        .unwrap_err()
        .to_string();
    assert!(err.contains("planted/"), "{err}");
}
