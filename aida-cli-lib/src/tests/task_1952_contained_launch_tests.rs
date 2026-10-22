//! TASK-1952 sweep: the contained launch resolves its main checkout only from
//! host-owned data (BUG-2016) and grants no store write when the store's
//! contained marker cannot be written (BUG-2154).
//!
//! trace:TASK-1952 | ai:claude

use std::path::{Path, PathBuf};

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository at `<parent>/<name>` with one commit.
fn repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
    dir
}

/// `main` plus a linked worktree `<parent>/wt`, and an unrelated repository
/// `other` that carries a `.aida-store` directory a redirect would grant.
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = std::fs::canonicalize(tmp.path()).unwrap();
    let main = repo(&parent, "main");
    let wt = parent.join("wt");
    git(
        &main,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    let other = repo(&parent, "other");
    std::fs::create_dir_all(other.join(".aida-store")).unwrap();
    (tmp, main, wt, other)
}

// BUG-2016: an intact linked worktree verifies against its main checkout.
// trace:TASK-1952 | trace:BUG-2016 | ai:claude
#[cfg(unix)]
#[test]
fn intact_linked_worktree_verifies_against_its_main() {
    let (_tmp, main, wt, other) = fixture();
    crate::session::verify_linked_worktree_main(&wt, &main).expect("intact pointer verifies");
    crate::session::verify_linked_worktree_main(&main, &main).expect("the main checkout itself");
    assert!(crate::session::verify_linked_worktree_main(&wt, &other).is_err());
}

// BUG-2016: a worktree whose `.git` pointer was rewritten at another
// repository refuses the contained launch, and nothing of the other
// repository's store or cache is granted.
// trace:TASK-1952 | trace:BUG-2016 | ai:claude
#[cfg(unix)]
#[test]
fn rewritten_git_pointer_refuses_and_grants_nothing_of_the_other_repo() {
    let (_tmp, _main, wt, other) = fixture();
    // The direct redirect the reviewer reproduced, then a planted linked
    // gitdir under the other repository's `worktrees/` with no back-pointer.
    let planted = other.join(".git/worktrees/wt");
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::write(planted.join("commondir"), "../..\n").unwrap();
    std::fs::write(planted.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    for target in [other.join(".git"), planted] {
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", target.display())).unwrap();
        let roots = crate::contained_boundary::contained_write_roots(&wt);
        let err = aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
            crate::session::claude_contained_settings_json(&roots)
        })
        .expect_err("a rewritten pointer must refuse the launch");
        let msg = format!("{err:#}");
        assert!(msg.contains("contained launch refused"), "{msg}");
        assert!(!msg.contains(".aida-store"), "{msg}");
    }
}

// BUG-2154: a marker write failure grants no store write path; a successful
// one keeps the grant.
// trace:TASK-1952 | trace:BUG-2154 | ai:claude
#[cfg(unix)]
#[test]
fn marker_write_failure_grants_no_store_write_paths() {
    let (_tmp, main, _wt, _other) = fixture();
    let store = main.join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        let failed = crate::session::contained_store_grant(&main, &store, |_| {
            anyhow::bail!("injected marker write failure")
        });
        assert!(failed.is_empty(), "{failed:?}");
        let marked = crate::session::contained_store_grant(&main, &store, |_| Ok(()));
        assert!(!marked.is_empty(), "the control grant must be non-empty");
    });
}

/// Mint a host lease for Claude session `claude_id` whose tree is `tree`.
fn mint_lease(main: &Path, lease_id: &str, claude_id: &str, tree: &Path) {
    let sessions = main.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join(format!("{lease_id}.toml")),
        format!(
            "id = \"{lease_id}\"\nbranch = \"wt\"\nworktree_path = {}\n",
            aida_core::toml_quote::toml_string(&tree.display().to_string())
        ),
    )
    .unwrap();
    let manifest = crate::session_manifest::SessionManifest {
        session_id: lease_id.to_string(),
        planned_at: chrono::Utc::now(),
        plan_source: "queue work".to_string(),
        claude_session_id: Some(claude_id.to_string()),
        batch_name: None,
        plan: None,
        items: vec![],
    };
    crate::session_manifest::save(
        &crate::session_manifest::manifest_path(main, lease_id),
        &manifest,
    )
    .unwrap();
}

/// The `--settings` write-allow rules of a resume command.
fn resume_allow_rules(cmd: &std::process::Command) -> Vec<String> {
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let settings = args
        .windows(2)
        .find(|w| w[0] == "--settings")
        .map(|w| w[1].clone())
        .expect("contained resume carries --settings");
    let settings: serde_json::Value = serde_json::from_str(&settings).unwrap();
    settings["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.as_str().map(str::to_string))
        .filter(|r| r.starts_with("Edit("))
        .collect()
}

// TASK-2031: a contained resume run from the main checkout is bounded to the
// tree of the lease that minted the session, not to main; a lease naming a
// tree of another repository is ignored.
// trace:TASK-1952 | trace:TASK-2031 | ai:claude
#[cfg(unix)]
#[test]
fn contained_resume_is_bounded_to_the_lease_tree() {
    let (_tmp, main, wt, other) = fixture();
    let _os_wrap = crate::test_env::EnvVarGuard::set("AIDA_OS_WRAP", "0");
    mint_lease(&main, "lease-a", "claude-a", &wt);
    mint_lease(&main, "lease-b", "claude-b", &other);
    let target = |id: &str| crate::session::ResumeTarget {
        agent: "claude".to_string(),
        id: id.to_string(),
        worktree: Some(main.clone()),
    };
    assert_eq!(
        crate::session::resume_lease_tree(&target("claude-a")),
        Some(wt.clone())
    );
    assert_eq!(crate::session::resume_lease_tree(&target("claude-b")), None);
    assert_eq!(
        crate::session::resume_lease_tree(&target("claude-none")),
        None
    );

    // The real contained `aida session resume` path: the target lists main
    // (the store the transcript was found in), the command runs in and is
    // bounded to the lease tree.
    let launcher = main.clone();
    let cmd = aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        crate::session::build_contained_resume_command(&target("claude-a"), &launcher)
    })
    .unwrap();
    assert_eq!(
        cmd.get_current_dir(),
        Some(wt.as_path()),
        "cwd is the lease tree"
    );
    assert_eq!(
        resume_allow_rules(&cmd),
        vec![format!("Edit(/{}/**)", wt.display())],
        "the resume boundary is the lease tree"
    );
    // A lease naming another repository's tree keeps the listed checkout.
    let cmd = aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
        crate::session::build_contained_resume_command(&target("claude-b"), &launcher)
    })
    .unwrap();
    assert_eq!(cmd.get_current_dir(), Some(main.as_path()));
}

// BUG-2016 (r1 minor): a relative gitdir back-pointer (`git worktree add
// --relative-paths`, git 2.48+) resolves against the gitdir, so an intact
// worktree still verifies.
// trace:TASK-1952 | trace:BUG-2016 | ai:claude
#[cfg(unix)]
#[test]
fn relative_gitdir_back_pointer_verifies() {
    let (_tmp, main, wt, _other) = fixture();
    let gitdir = main.join(".git/worktrees/wt");
    std::fs::write(gitdir.join("gitdir"), "../../../../wt/.git\n").unwrap();
    crate::session::verify_linked_worktree_main(&wt, &main).expect("relative back-pointer");
}

// TASK-2111: a tree whose `.git` is a directory holding a `commondir` that
// names another repository, or a symlink to another repository's `.git`,
// redirects `git worktree list` (so the resolved main). Neither is a
// host-recorded seat clone, so the contained launch refuses.
// trace:TASK-1952 | trace:TASK-2111 | ai:claude
#[cfg(unix)]
#[test]
fn task_2111_git_dir_or_symlink_redirect_refuses() {
    // A `.git` directory with a commondir naming the other repository.
    let (_tmp, _main, wt, other) = fixture();
    std::fs::remove_file(wt.join(".git")).unwrap();
    let dot_git = wt.join(".git");
    std::fs::create_dir_all(&dot_git).unwrap();
    std::fs::write(
        dot_git.join("commondir"),
        format!("{}\n", other.join(".git").display()),
    )
    .unwrap();
    std::fs::write(dot_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let resolved = crate::main_worktree_root_from(&wt);
    assert_ne!(
        std::fs::canonicalize(&resolved).ok(),
        std::fs::canonicalize(&wt).ok(),
        "precondition: the redirect moves the resolved main off the tree"
    );
    assert!(crate::session::verify_linked_worktree_main(&wt, &resolved).is_err());

    // A `.git` symlink to the other repository's `.git` directory.
    let (_tmp, _main, wt, other) = fixture();
    std::fs::remove_file(wt.join(".git")).unwrap();
    std::os::unix::fs::symlink(other.join(".git"), wt.join(".git")).unwrap();
    let resolved = crate::main_worktree_root_from(&wt);
    assert!(crate::session::verify_linked_worktree_main(&wt, &resolved).is_err());
    assert!(crate::session::verify_linked_worktree_main(&wt, &other).is_err());

    // A plain repository is its own main and still verifies; a tree with no
    // `.git` at all (a --cwd subdirectory) is not refused either.
    let (_tmp, main, _wt, _other) = fixture();
    crate::session::verify_linked_worktree_main(&main, &main).expect("own main");
    let sub = main.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    crate::session::verify_linked_worktree_main(&sub, &main).expect("subdirectory");
}

// TASK-2112: Claude keys transcripts by cwd, so a contained resume moved to
// the lease tree copies the one transcript listed under main into the lease
// tree's project store: mode 0600, only that file, never over a different
// one and never through a symlink.
// trace:TASK-1952 | trace:TASK-2112 | ai:claude
#[cfg(unix)]
#[test]
fn task_2112_contained_resume_stages_the_transcript_in_the_lease_tree() {
    use std::os::unix::fs::PermissionsExt;
    let (tmp, main, wt, _other) = fixture();
    let home = tmp.path().join("home");
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_TEST_HOME", Some(home.to_str().unwrap())),
        ("AIDA_OS_WRAP", Some("0")),
    ]);
    mint_lease(&main, "lease-a", "claude-a", &wt);
    let main_store = crate::session::claude_project_dir(&main).unwrap();
    let wt_store = crate::session::claude_project_dir(&wt).unwrap();
    std::fs::create_dir_all(&main_store).unwrap();
    std::fs::write(main_store.join("claude-a.jsonl"), "{\"a\":1}\n").unwrap();
    std::fs::write(main_store.join("other-session.jsonl"), "{}\n").unwrap();
    let target = crate::session::ResumeTarget {
        agent: "claude".to_string(),
        id: "claude-a".to_string(),
        worktree: Some(main.clone()),
    };
    let build = || {
        aida_core::git_ops::with_host_git_version("git version 2.43.0", || {
            crate::session::build_contained_resume_command(&target, &main)
        })
    };
    let cmd = build().unwrap();
    assert_eq!(cmd.get_current_dir(), Some(wt.as_path()));
    let copied = wt_store.join("claude-a.jsonl");
    assert_eq!(std::fs::read_to_string(&copied).unwrap(), "{\"a\":1}\n");
    assert_eq!(
        std::fs::metadata(&copied).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !wt_store.join("other-session.jsonl").exists(),
        "only the resumed session's transcript is copied"
    );
    // Idempotent on identical bytes.
    build().expect("an identical copy is reused");
    // A different transcript already there refuses the resume.
    std::fs::write(&copied, "{\"forged\":1}\n").unwrap();
    let err = build().expect_err("a differing destination refuses");
    assert!(
        format!("{err:#}").contains("different transcript"),
        "{err:#}"
    );
    // A symlinked destination refuses.
    std::fs::remove_file(&copied).unwrap();
    std::os::unix::fs::symlink(main_store.join("other-session.jsonl"), &copied).unwrap();
    assert!(build().is_err(), "a symlinked destination refuses");
    std::fs::remove_file(&copied).unwrap();
    // A symlinked source refuses.
    std::fs::rename(
        main_store.join("claude-a.jsonl"),
        main_store.join("real.jsonl"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        main_store.join("real.jsonl"),
        main_store.join("claude-a.jsonl"),
    )
    .unwrap();
    assert!(build().is_err(), "a symlinked source refuses");
    assert!(!copied.exists());
}
