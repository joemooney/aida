//! BUG-2195 (TASK-2067): the mandate store snapshot is compared by what can
//! change the exclusion reader's `--all` history of `objects/`, not byte for
//! byte over every ref of the shared code repository. TASK-2019: a dirty store
//! is a typed, retryable error.
// trace:TASK-2067 | ai:antigravity
use super::*;

struct Store(tempfile::TempDir);

impl Store {
    fn new() -> Self {
        let s = Self(tempfile::tempdir().unwrap());
        s.git(&["init", "-q", "-b", "main"]);
        s.git(&["config", "user.name", "Test"]);
        s.git(&["config", "user.email", "test@example.invalid"]);
        s.git(&["config", "core.hooksPath", "/dev/null"]);
        s.git(&["config", "commit.gpgsign", "false"]);
        s
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn git(&self, args: &[&str]) -> String {
        let out = crate::test_git::git_command()
            .current_dir(self.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    /// Commit `path` = `body` on the current branch; returns the commit.
    fn commit(&self, path: &str, body: &str) -> String {
        let file = self.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", "fixture"]);
        self.git(&["rev-parse", "HEAD"])
    }
    /// Commit `path` = `body` on `branch` (created from `from`) without
    /// moving the store's HEAD; returns the commit.
    fn commit_on(&self, branch: &str, from: &str, path: &str, body: &str) -> String {
        let head = self.git(&["symbolic-ref", "--short", "HEAD"]);
        self.git(&["checkout", "-q", "-B", branch, from]);
        let sha = self.commit(path, body);
        self.git(&["checkout", "-q", &head]);
        sha
    }
    fn snap(&self) -> StoreSnapshot {
        store_snapshot(self.path()).unwrap()
    }
    fn stable(&self, before: &StoreSnapshot) -> bool {
        store_snapshot_stable(self.path(), before, &self.snap())
    }
}

/// A store with one object version on `main` (HEAD) and a code branch whose
/// history never touches `objects/`.
fn store() -> (Store, String) {
    let s = Store::new();
    let base = s.commit("objects/TASK/000/TASK-1.yaml", "title: one\n");
    s.commit_on("code", &base, "src/lib.rs", "fn a() {}\n");
    (s, base)
}

#[test]
fn an_unrelated_code_ref_move_is_stable() {
    let (s, _) = store();
    let before = s.snap();
    let code = s.git(&["rev-parse", "code"]);
    s.commit_on("code", &code, "src/lib.rs", "fn b() {}\n");
    let after = s.snap();
    assert_ne!(before, after, "the old byte comparison saw a change here");
    assert!(store_snapshot_stable(s.path(), &before, &after));
}

#[test]
fn a_new_code_ref_without_object_history_is_stable() {
    let (s, base) = store();
    let before = s.snap();
    s.commit_on("feature", &base, "README.md", "x\n");
    s.git(&["update-ref", "refs/remotes/origin/main", &base]);
    assert!(s.stable(&before));
}

#[test]
fn a_ref_bringing_a_new_object_version_is_unstable() {
    let (s, base) = store();
    let before = s.snap();
    s.commit_on(
        "sticky",
        &base,
        "objects/TASK/000/TASK-1.yaml",
        "title: two\n",
    );
    assert!(!s.stable(&before), "an added ref with a new object version");

    let (s, _) = store();
    let before = s.snap();
    let code = s.git(&["rev-parse", "code"]);
    s.commit_on("code", &code, "objects/TASK/000/TASK-2.yaml", "title: x\n");
    assert!(
        !s.stable(&before),
        "a changed ref with a new object version"
    );
}

#[test]
fn a_head_move_is_unstable_even_without_object_changes() {
    let (s, _) = store();
    let before = s.snap();
    s.commit("notes.txt", "x\n");
    assert!(!s.stable(&before));
}

#[test]
fn a_deleted_ref_is_stable() {
    let (s, base) = store();
    s.commit_on(
        "sticky",
        &base,
        "objects/TASK/000/TASK-1.yaml",
        "title: old\n",
    );
    let before = s.snap();
    s.git(&["update-ref", "-d", "refs/heads/sticky"]);
    s.git(&["update-ref", "-d", "refs/heads/code"]);
    assert!(s.stable(&before));
}

/// Advisor condition C3: a force-moved ref whose new tip drops commits that
/// touched `objects/` is unstable, even though it brings nothing new.
#[test]
fn a_force_move_dropping_object_history_is_unstable() {
    let (s, base) = store();
    s.commit_on(
        "sticky",
        &base,
        "objects/TASK/000/TASK-1.yaml",
        "title: old\n",
    );
    let before = s.snap();
    s.git(&["update-ref", "refs/heads/sticky", &base]);
    assert!(!s.stable(&before));
}

#[test]
fn a_force_move_dropping_only_code_history_is_stable() {
    let (s, base) = store();
    let before = s.snap();
    s.git(&["update-ref", "refs/heads/code", &base]);
    assert!(s.stable(&before));
}

/// A git failure is never read as stable: an unknown tip forces a retry.
#[test]
fn an_unreadable_ref_range_is_unstable() {
    let (s, _) = store();
    let before = s.snap();
    let mut after = s.snap();
    after.refs.insert(
        "refs/heads/ghost".into(),
        "1111111111111111111111111111111111111111".into(),
    );
    assert!(!store_snapshot_stable(s.path(), &before, &after));
}

/// TASK-2019: a dirty store is the typed, retryable [`StoreDirty`] error, and
/// the snapshot's own status options win over repository config that would
/// hide an untracked file (advisor condition C1).
#[test]
fn a_dirty_store_is_a_typed_error_whatever_the_status_config() {
    let (s, _) = store();
    s.git(&["config", "status.showUntrackedFiles", "no"]);
    s.git(&["config", "core.quotePath", "true"]);
    std::fs::write(s.path().join("objects/untracked é.yaml"), "x\n").unwrap();
    let err = store_snapshot(s.path()).unwrap_err();
    assert!(err.is::<StoreDirty>(), "{err:#}");
    assert!(format!("{err:#}").contains("uncommitted evidence"));
}

/// TASK-2144: `git log --all` walks another worktree's detached HEAD, so an
/// object version committed there during collection is a change.
// trace:TASK-2067 | ai:claude
#[test]
fn an_object_version_on_another_worktrees_detached_head_is_unstable() {
    let (s, base) = store();
    let other = tempfile::tempdir().unwrap();
    let wt = other.path().join("wt");
    s.git(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        wt.to_str().unwrap(),
        &base,
    ]);
    let before = s.snap();
    assert!(
        before
            .refs
            .keys()
            .any(|k| k.starts_with("HEAD of worktree ")),
        "{before:?}"
    );
    std::fs::write(wt.join("objects/TASK/000/TASK-1.yaml"), "title: two\n").unwrap();
    for args in [&["add", "-A"][..], &["commit", "-qm", "detached"]] {
        let out = crate::test_git::git_command()
            .current_dir(&wt)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }
    assert!(
        !s.stable(&before),
        "the detached HEAD brought a new version"
    );
}

/// TASK-2145: an excludes file (here repository-local; the global one is
/// overridden the same way) never hides an untracked object from the dirty
/// check.
// trace:TASK-2067 | ai:claude
#[test]
fn an_excludes_file_does_not_hide_an_untracked_object() {
    let (s, _) = store();
    let dir = tempfile::tempdir().unwrap();
    let excludes = dir.path().join("excludes");
    std::fs::write(&excludes, "*.yaml\n").unwrap();
    s.git(&["config", "core.excludesFile", excludes.to_str().unwrap()]);
    std::fs::write(s.path().join("objects/TASK/000/TASK-9.yaml"), "x\n").unwrap();
    assert_eq!(s.git(&["status", "--porcelain"]), "", "the file hides it");
    let err = store_snapshot(s.path()).unwrap_err();
    assert!(err.is::<StoreDirty>(), "{err:#}");
}

/// TASK-2230: neither `$GIT_DIR/info/exclude` nor a `.gitignore` hides an
/// untracked object from the dirty check.
// trace:TASK-2238 | ai:claude
#[test]
fn task_2238_info_exclude_and_gitignore_do_not_hide_an_untracked_object() {
    for hide in ["info/exclude", ".gitignore"] {
        let (s, _) = store();
        if hide == ".gitignore" {
            s.commit(".gitignore", "objects/TASK/000/TASK-9.yaml\n");
        } else {
            let common = s.path().join(s.git(&["rev-parse", "--git-common-dir"]));
            std::fs::create_dir_all(common.join("info")).unwrap();
            std::fs::write(common.join("info/exclude"), "*.yaml\n").unwrap();
        }
        std::fs::write(s.path().join("objects/TASK/000/TASK-9.yaml"), "x\n").unwrap();
        assert_eq!(s.git(&["status", "--porcelain"]), "", "{hide} hides it");
        let err = store_snapshot(s.path()).unwrap_err();
        assert!(err.is::<StoreDirty>(), "{hide}: {err:#}");
    }
}

/// A lease can be staged between a coordination writer's add and commit;
/// mailbox/runtime dirt must not turn that window into a mandate refusal.
// trace:BUG-2213 | ai:codex
#[test]
fn bug_2213_coordination_and_other_non_evidence_dirt_allow_a_snapshot() {
    let (s, _) = store();
    s.commit(
        "coordination/leases/pr-17-reviewer.toml",
        "lease = \"old\"\n",
    );
    s.commit("notes.txt", "old\n");
    let before = s.snap();
    std::fs::write(
        s.path().join("coordination/leases/pr-17-reviewer.toml"),
        "lease = \"renewed\"\n",
    )
    .unwrap();
    s.git(&["add", "coordination/leases/pr-17-reviewer.toml"]);
    std::fs::write(
        s.path().join("coordination/leases/pr-18-new.toml"),
        "lease = \"new\"\n",
    )
    .unwrap();
    s.git(&["add", "coordination/leases/pr-18-new.toml"]);
    std::fs::create_dir_all(s.path().join("coordination/mailbox")).unwrap();
    std::fs::write(s.path().join("coordination/mailbox/message.toml"), "mail\n").unwrap();
    std::fs::write(s.path().join("notes.txt"), "changed\n").unwrap();
    std::fs::create_dir_all(s.path().join("objects-backup")).unwrap();
    std::fs::write(
        s.path().join("objects-backup/untracked.yaml"),
        "not evidence\n",
    )
    .unwrap();
    let status = s.git(&["status", "--porcelain"]);
    assert!(
        status.contains("M  coordination/leases/pr-17-reviewer.toml"),
        "{status}"
    );
    assert!(
        status.contains("A  coordination/leases/pr-18-new.toml"),
        "{status}"
    );
    let after = store_snapshot(s.path()).expect("non-evidence dirt permits a snapshot");
    assert!(store_snapshot_stable(s.path(), &before, &after));
}

/// Scoped status must still reject indexed and working-tree changes of any
/// kind below objects/, not merely YAML modifications seen by porcelain.
// trace:BUG-2213 | ai:codex
#[test]
fn bug_2213_dirty_objects_refuse_including_staged_deletions_and_renames() {
    for change in [
        "modified",
        "staged",
        "untracked",
        "untracked-other",
        "deleted",
        "renamed-out",
        "root-replaced",
    ] {
        let (s, _) = store();
        let object = s.path().join("objects/TASK/000/TASK-1.yaml");
        match change {
            "modified" | "staged" => {
                std::fs::write(&object, "tags: [security]\n").unwrap();
                if change == "staged" {
                    s.git(&["add", "objects/"]);
                }
            }
            "untracked" => {
                std::fs::write(
                    s.path().join("objects/TASK/000/TASK-2.yaml"),
                    "tags: [security]\n",
                )
                .unwrap();
            }
            "untracked-other" => {
                std::fs::create_dir_all(s.path().join("objects/new-subdir")).unwrap();
                std::fs::write(
                    s.path().join("objects/new-subdir/not-yaml.txt"),
                    "new evidence\n",
                )
                .unwrap();
            }
            "deleted" => {
                s.git(&["rm", "objects/TASK/000/TASK-1.yaml"]);
            }
            "renamed-out" => {
                s.git(&["mv", "objects/TASK/000/TASK-1.yaml", "removed-object.yaml"]);
            }
            "root-replaced" => {
                std::fs::remove_dir_all(s.path().join("objects")).unwrap();
                std::fs::write(s.path().join("objects"), "directory replaced\n").unwrap();
            }
            _ => unreachable!(),
        }
        let error = store_snapshot(s.path()).unwrap_err();
        assert!(error.is::<StoreDirty>(), "{change}: {error:#}");
    }
}

/// Git's literal mode disables :(...) magic. The production pathspec must
/// keep both real dirt queries nonempty under every inherited pathspec mode,
/// without mutating the process environment shared by concurrent tests.
// trace:BUG-2213 | ai:codex
#[test]
fn bug_2213_inherited_pathspec_modes_cannot_hide_object_dirt() {
    let (s, _) = store();
    std::fs::write(
        s.path().join("objects/TASK/000/TASK-1.yaml"),
        "tags: [security]\n",
    )
    .unwrap();
    std::fs::write(s.path().join("objects/untracked.yaml"), "security\n").unwrap();
    let modes = [
        "GIT_LITERAL_PATHSPECS",
        "GIT_GLOB_PATHSPECS",
        "GIT_NOGLOB_PATHSPECS",
        "GIT_ICASE_PATHSPECS",
    ];
    for mode in std::iter::once(None).chain(modes.iter().copied().map(Some)) {
        for args in [
            vec![
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--no-renames",
                "--",
                STORE_OBJECTS_PATH,
            ],
            vec!["ls-files", "--others", "-z", "--", STORE_OBJECTS_PATH],
        ] {
            let mut cmd = snapshot_command(s.path());
            for key in modes {
                cmd.env_remove(key);
            }
            if let Some(key) = mode {
                cmd.env(key, "1");
            }
            let out = cmd.args(&args).output().unwrap();
            assert!(out.status.success(), "{mode:?} {args:?}: {out:?}");
            assert!(
                !out.stdout.is_empty(),
                "{mode:?} {args:?} hid object evidence"
            );
        }
    }
}
