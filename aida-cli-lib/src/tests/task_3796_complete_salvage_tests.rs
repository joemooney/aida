//! TASK-3796: the complete salvage a dead lane's lease is released on keeps
//! every byte and every name, refuses what a patch cannot hold, and never
//! overwrites an earlier salvage.
// trace:TASK-3796 | ai:claude

use super::*;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A repository with one commit holding `files`, plus a clean clone of it.
fn tree_and_clean(dir: &Path, files: &[(&[u8], &[u8])]) -> (PathBuf, PathBuf, PathBuf) {
    let root = dir.join("root");
    let tree = dir.join("tree");
    let clean = dir.join("clean");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&tree).unwrap();
    git(&tree, &["init", "-q", "-b", "main"]);
    git(&tree, &["config", "user.email", "t@example.invalid"]);
    git(&tree, &["config", "user.name", "T"]);
    std::fs::write(tree.join("base.txt"), "base\n").unwrap();
    for (name, body) in files {
        std::fs::write(tree.join(std::ffi::OsStr::from_bytes(name)), body).unwrap();
    }
    git(&tree, &["add", "."]);
    git(&tree, &["commit", "-q", "-m", "first"]);
    git(
        dir,
        &[
            "clone",
            "-q",
            tree.to_str().unwrap(),
            clean.to_str().unwrap(),
        ],
    );
    (root, tree, clean)
}

fn complete(root: &Path, tree: &Path) -> Result<Option<PathBuf>> {
    salvage_worktree_patch_complete(root, "BUG-7", Some("implementer"), tree)
}

fn salvage_count(root: &Path) -> usize {
    std::fs::read_dir(root.join(".aida").join("salvage"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

fn apply(clean: &Path, patch: &Path) {
    git(clean, &["apply", "--check", patch.to_str().unwrap()]);
    git(clean, &["apply", patch.to_str().unwrap()]);
}

// F1: untracked names are passed NUL-separated and untrimmed, so names with
// leading or trailing spaces, a newline or a quote are captured as they are,
// and the patch applies back.
#[test]
fn untracked_names_are_captured_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (root, tree, clean) = tree_and_clean(dir.path(), &[]);
    let names: [&[u8]; 4] = [
        b" lead.txt",
        b"trail.txt ",
        b"new\nline.txt",
        b"quo\"te.txt",
    ];
    for name in names {
        std::fs::write(tree.join(std::ffi::OsStr::from_bytes(name)), name).unwrap();
    }
    let patch = complete(&root, &tree).unwrap().expect("a patch");
    apply(&clean, &patch);
    for name in names {
        let name = std::ffi::OsStr::from_bytes(name);
        assert_eq!(
            std::fs::read(clean.join(name)).unwrap(),
            std::fs::read(tree.join(name)).unwrap(),
            "{name:?} after applying the salvage patch"
        );
    }
}

// F1: what a patch cannot hold fails the complete salvage and writes nothing:
// a nested repository, a symlink to a directory, a fifo, and a file that
// vanished after it was listed.
#[test]
fn untracked_entries_a_patch_cannot_hold_write_no_patch() {
    let dir = tempfile::tempdir().unwrap();
    let (root, tree, _clean) = tree_and_clean(dir.path(), &[]);

    let nested = tree.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    std::fs::write(nested.join("x.txt"), "x\n").unwrap();
    let error = complete(&root, &tree).unwrap_err();
    assert!(
        format!("{error:#}").contains("nested git repository"),
        "{error:#}"
    );
    assert_eq!(salvage_count(&root), 0);
    std::fs::remove_dir_all(&nested).unwrap();

    std::fs::create_dir_all(dir.path().join("elsewhere")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), tree.join("link")).unwrap();
    let error = complete(&root, &tree).unwrap_err();
    assert!(
        format!("{error:#}").contains("symlink to a directory"),
        "{error:#}"
    );
    assert_eq!(salvage_count(&root), 0);
    std::fs::remove_file(tree.join("link")).unwrap();

    // A file listed and then gone: `diff --no-index` exits 1 with "Could not
    // access", which must not read as "the sides differ".
    let error = salvage_untracked(&tree, "gone.txt".as_ref(), true).unwrap_err();
    assert!(
        format!("{error:#}").contains("could not be read"),
        "{error:#}"
    );
    let lossy = salvage_untracked(&tree, "gone.txt".as_ref(), false).unwrap();
    assert!(lossy.is_empty(), "the lossy form keeps what git printed");
}

// F2: uncommitted edits inside a submodule are not in a superproject patch,
// so the complete salvage refuses; the lossy gc salvage still writes one.
#[test]
fn a_dirty_submodule_fails_the_complete_salvage() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    git(&sub, &["init", "-q", "-b", "main"]);
    git(&sub, &["config", "user.email", "t@example.invalid"]);
    git(&sub, &["config", "user.name", "T"]);
    std::fs::write(sub.join("inner.txt"), "inner\n").unwrap();
    git(&sub, &["add", "."]);
    git(&sub, &["commit", "-q", "-m", "sub"]);
    let (root, tree, _clean) = tree_and_clean(dir.path(), &[]);
    git(
        &tree,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().unwrap(),
            "vendored",
        ],
    );
    git(&tree, &["commit", "-q", "-m", "add submodule"]);
    std::fs::write(tree.join("vendored").join("inner.txt"), "edited\n").unwrap();

    let error = complete(&root, &tree).unwrap_err();
    let text = format!("{error:#}");
    assert!(
        text.contains("inside a submodule") && text.contains("vendored"),
        "{text}"
    );
    assert_eq!(salvage_count(&root), 0);
    assert!(salvage_worktree_patch(&root, "BUG-7", None, &tree)
        .unwrap()
        .is_some());
}

// F3: content and names that are not UTF-8 stay bytes, tracked and untracked,
// and the patch reproduces them exactly.
#[test]
fn non_utf8_tracked_and_untracked_work_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let tracked_name: &[u8] = b"tracked-\xff.txt";
    let (root, tree, clean) = tree_and_clean(dir.path(), &[(tracked_name, b"one\n")]);
    let tracked = tree.join(std::ffi::OsStr::from_bytes(tracked_name));
    std::fs::write(&tracked, b"one\n\xfe\xff latin \xe9\n").unwrap();
    std::fs::write(tree.join("base.txt"), b"base\n\xc3\x28\n").unwrap();
    let untracked = tree.join(std::ffi::OsStr::from_bytes(b"new-\xe9.txt"));
    std::fs::write(&untracked, b"\x80\x81 not utf-8\n").unwrap();

    let patch = complete(&root, &tree).unwrap().expect("a patch");
    apply(&clean, &patch);
    for path in [&tracked, &untracked, &tree.join("base.txt")] {
        let name = path.file_name().unwrap();
        assert_eq!(
            std::fs::read(clean.join(name)).unwrap(),
            std::fs::read(path).unwrap(),
            "{name:?} after applying the salvage patch"
        );
    }
}

// F4: two lanes of the same spec and role salvaged in the same second each
// get their own file; the first is never overwritten.
#[test]
fn two_salvages_of_one_spec_and_role_in_one_second_keep_both() {
    let dir = tempfile::tempdir().unwrap();
    let (root, first, _) = tree_and_clean(&dir.path().join("a"), &[]);
    let (_, second, _) = tree_and_clean(&dir.path().join("b"), &[]);
    std::fs::write(first.join("base.txt"), "first lane\n").unwrap();
    std::fs::write(second.join("base.txt"), "second lane\n").unwrap();
    let one = complete(&root, &first).unwrap().expect("a patch");
    let two = complete(&root, &second).unwrap().expect("a patch");
    assert_ne!(one, two);
    assert!(String::from_utf8_lossy(&std::fs::read(&one).unwrap()).contains("+first lane"));
    assert!(String::from_utf8_lossy(&std::fs::read(&two).unwrap()).contains("+second lane"));

    // The same stem twice, as two attempts in one instant would make.
    let salvage = root.join(".aida").join("salvage");
    let a = write_new_salvage_file(&salvage, "BUG-7-implementer-attempt-x", b"a").unwrap();
    let b = write_new_salvage_file(&salvage, "BUG-7-implementer-attempt-x", b"b").unwrap();
    assert_ne!(a, b);
    assert_eq!(std::fs::read(&a).unwrap(), b"a");
    assert_eq!(std::fs::read(&b).unwrap(), b"b");
}

/// HEAD=A, index=B, worktree=A: B lives only in the index.
// trace:TASK-3796 | ai:claude
fn stage_then_restore(tree: &Path) {
    std::fs::write(tree.join("base.txt"), "staged B\n").unwrap();
    git(tree, &["add", "base.txt"]);
    std::fs::write(tree.join("base.txt"), "base\n").unwrap();
}

/// The staged-only version is in the patch and applies back to a clean tree.
fn assert_patch_keeps_staged(clean: &Path, patch: &Path) {
    let body = std::fs::read_to_string(patch).unwrap();
    assert!(body.contains("+staged B"), "{body}");
    apply(clean, patch);
    assert_eq!(
        std::fs::read_to_string(clean.join("base.txt")).unwrap(),
        "staged B\n"
    );
}

// Round 8 F2: the lossy salvage destructive cleanup runs keeps the index as
// its own `--cached` diff, so a staged version that neither HEAD nor the
// worktree holds is not lost.
#[test]
fn the_lossy_salvage_keeps_a_staged_version_the_worktree_no_longer_has() {
    let dir = tempfile::tempdir().unwrap();
    let (root, tree, clean) = tree_and_clean(dir.path(), &[]);
    stage_then_restore(&tree);
    let patch = salvage_worktree_patch(&root, "BUG-7", None, &tree)
        .unwrap()
        .expect("a patch");
    assert_patch_keeps_staged(&clean, &patch);
    // The complete form refuses the same tree rather than claim it whole.
    let err = format!("{:#}", complete(&root, &tree).expect_err("refused"));
    assert!(err.contains("staged content"), "{err}");
}

// Round 8 F2, through a destructive caller: doctor's orphan-worktree heal
// salvages before it tears the tree down, and its patch holds B.
#[test]
fn the_doctor_orphan_heal_keeps_a_staged_version_the_worktree_no_longer_has() {
    let _scan = aida_core::mailbox_lifecycle::activation::test_writer_scan(true);
    let _trust = crate::test_env::EnvVarGuard::set("AIDA_TRUST_PID_NAMESPACE", "1");
    let dir = tempfile::tempdir().unwrap();
    let (_root, tree, clean) = tree_and_clean(dir.path(), &[]);
    git(&tree, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let wt = tree.join("wt-orphan");
    git(
        &tree,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "orphan",
            wt.to_str().unwrap(),
        ],
    );
    let wt = wt.canonicalize().unwrap();
    stage_then_restore(&wt);
    let finding = crate::DoctorFinding {
        category: "orphan-worktrees".into(),
        id: wt.display().to_string(),
        summary: String::new(),
        action: String::new(),
        safe_heal: true,
    };
    // Whether the teardown then succeeds is not this test's concern; the
    // salvage runs first. trace:TASK-3796 | ai:claude
    let _ = crate::doctor_cmd::heal_doctor_orphan_worktree(&tree, &finding);
    let patches: Vec<PathBuf> = std::fs::read_dir(tree.join(".aida").join("salvage"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(patches.len(), 1, "{patches:?}");
    assert_patch_keeps_staged(&clean, &patches[0]);
}
