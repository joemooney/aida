//! BUG-2223: `aida queue work --resume` re-enters an orphaned seat clone in
//! place. Session start adopts the clone an orphaned lease records (dead
//! owner, same scope, same path and branch) without touching its commits or
//! uncommitted work, and refuses a live lease or any mismatch.
//! trace:BUG-2223 | ai:claude

use super::tests::{fixture, git_ok};
use super::*;
use std::path::{Path, PathBuf};

/// Point contained seats at private clones under `<root>/seats`.
fn clone_config(main: &Path, root: &Path) -> PathBuf {
    let seats = root.join("seats");
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        format!(
            "[contained]\nseat_tree = \"clone\"\n\n[worktree_pool]\nworktree_parent = {}\n",
            aida_core::toml_quote::toml_string(&seats.display().to_string())
        ),
    )
    .unwrap();
    seats
}

fn start(
    owns: &str,
    branch: Option<&str>,
    path: Option<&Path>,
    force_claim: bool,
    contained: bool,
) -> anyhow::Result<()> {
    crate::session_start::session_start(
        owns,
        branch,
        None,
        false,
        path.map(|p| p.to_str().unwrap()),
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        force_claim,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ contained,
    )
}

fn lease_for(main: &Path, scope: &str) -> crate::SessionLease {
    crate::list_leases(main)
        .into_iter()
        .find(|l| l.scope == scope)
        .unwrap_or_else(|| panic!("no lease for {scope}"))
}

/// Rewrite `lease`'s owner as `pid`, the way a headless launch records its
/// vendor child.
fn set_owner(main: &Path, lease: &crate::SessionLease, pid: u32) {
    let mut lease = lease.clone();
    lease.active_pid = Some(pid);
    lease.active_pid_start_time = crate::process_probe::process_start_identity(pid);
    std::fs::write(
        crate::lease_path(main, &lease.id),
        toml::to_string_pretty(&lease).unwrap(),
    )
    .unwrap();
}

/// A pid that has exited: the orphaned worker.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// A seat clone for `owns` whose worker died, holding a commit and
/// uncommitted work.
fn orphaned_clone(main: &Path, owns: &str) -> crate::SessionLease {
    start(owns, None, None, false, true).unwrap();
    let lease = lease_for(main, owns);
    assert_eq!(lease.tree_kind, TreeKind::Clone);
    let clone = &lease.worktree_path;
    std::fs::write(clone.join("committed.txt"), "kept\n").unwrap();
    git_ok(clone, &["add", "committed.txt"]);
    git_ok(
        clone,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=T",
            "commit",
            "-qm",
            "seat work",
        ],
    );
    std::fs::write(clone.join("README.md"), "seed\nuncommitted edit\n").unwrap();
    std::fs::write(clone.join("wip.txt"), "untracked work\n").unwrap();
    set_owner(main, &lease, dead_pid());
    lease_for(main, owns)
}

fn assert_work_intact(clone: &Path, branch: &str, head: &str) {
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md")).unwrap(),
        "seed\nuncommitted edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("wip.txt")).unwrap(),
        "untracked work\n"
    );
    assert_eq!(git_ok(clone, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        crate::seat_clone::checked_out_branch(clone).as_deref(),
        Some(branch)
    );
}

/// Binding 5a: the orphaned clone is re-entered in place, its work intact,
/// and the new lease replaces the orphan's.
#[test]
fn an_orphaned_clone_with_uncommitted_work_is_re_entered_in_place() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let orphan = orphaned_clone(&fx.main, "TASK-2223");
    let clone = orphan.worktree_path.clone();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);
    let pin = pin_ref(&clone).unwrap();
    assert_eq!(
        crate::leases::adoptable_orphan_lease(&fx.main, "TASK-2223").map(|l| l.id),
        Some(orphan.id.clone()),
        "queue work --resume adopts this lease"
    );

    start(
        "TASK-2223",
        Some(&orphan.branch),
        Some(&clone),
        /* force_claim */ true,
        /* contained */ true,
    )
    .unwrap();

    let leases: Vec<_> = crate::list_leases(&fx.main)
        .into_iter()
        .filter(|l| l.scope == "TASK-2223")
        .collect();
    assert_eq!(
        leases.len(),
        1,
        "the orphan's lease is replaced: {leases:?}"
    );
    let lease = &leases[0];
    assert_ne!(lease.id, orphan.id);
    assert_eq!(lease.tree_kind, TreeKind::Clone);
    assert_eq!(lease.worktree_path, clone);
    assert_eq!(lease.branch, orphan.branch);
    assert_ne!(lease.active_pid, orphan.active_pid);
    assert!(!crate::lease_path(&fx.main, &orphan.id).exists());
    assert_work_intact(&clone, &orphan.branch, &head);
    assert!(
        super::tests::ref_exists(&fx.main, &pin),
        "the base pin stays"
    );
    assert_eq!(
        crate::worktree_gc::main_worktree_root_from(&clone),
        fx.main,
        "the re-entered clone still verifies against main"
    );
}

/// Binding 5b: a lease whose owner is alive is never adopted.
#[test]
fn a_live_lease_is_refused() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let orphan = orphaned_clone(&fx.main, "TASK-2224");
    set_owner(&fx.main, &orphan, std::process::id());
    let clone = orphan.worktree_path.clone();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);
    assert!(crate::leases::adoptable_orphan_lease(&fx.main, "TASK-2224").is_none());

    let err = format!(
        "{:#}",
        start("TASK-2224", Some(&orphan.branch), Some(&clone), true, true).unwrap_err()
    );
    assert!(err.contains("is not orphaned"), "{err}");
    assert!(err.contains("live process"), "{err}");
    assert_eq!(lease_for(&fx.main, "TASK-2224").id, orphan.id);
    assert_work_intact(&clone, &orphan.branch, &head);
}

/// Binding 5c: a clone the scope's orphaned lease does not record, a branch
/// the lease does not record, and an uncontained launch are all refused, and
/// nothing changes in either clone.
#[test]
fn a_mismatched_path_branch_or_uncontained_launch_is_refused() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let mine = orphaned_clone(&fx.main, "TASK-2225");
    let other = orphaned_clone(&fx.main, "TASK-2226");
    let mine_head = git_ok(&mine.worktree_path, &["rev-parse", "HEAD"]);
    let other_head = git_ok(&other.worktree_path, &["rev-parse", "HEAD"]);

    for (path, branch, contained, expected) in [
        // The other scope's clone.
        (
            &other.worktree_path,
            other.branch.as_str(),
            true,
            "not `TASK-2225`",
        ),
        // The right clone, the wrong branch.
        (
            &mine.worktree_path,
            "some-other-branch",
            true,
            "records branch",
        ),
        // The right clone and branch, but uncontained.
        (
            &mine.worktree_path,
            mine.branch.as_str(),
            false,
            "only a contained launch",
        ),
    ] {
        let err = format!(
            "{:#}",
            start("TASK-2225", Some(branch), Some(path), true, contained).unwrap_err()
        );
        assert!(err.contains(expected), "{expected}: {err}");
    }
    // A seat-clone directory no lease records at all.
    let unleased = fx.root.join("seats").join("aida-seat-aida-unleased");
    std::fs::create_dir_all(unleased.join(".git")).unwrap();
    std::fs::write(unleased.join(".git/HEAD"), "ref: refs/heads/x\n").unwrap();
    let err = format!(
        "{:#}",
        start("TASK-2225", Some("x"), Some(&unleased), true, true).unwrap_err()
    );
    assert!(err.contains("no session lease records it"), "{err}");

    assert_eq!(lease_for(&fx.main, "TASK-2225").id, mine.id);
    assert_eq!(lease_for(&fx.main, "TASK-2226").id, other.id);
    assert_work_intact(&mine.worktree_path, &mine.branch, &mine_head);
    assert_work_intact(&other.worktree_path, &other.branch, &other_head);
}

/// Binding 3: bare `--resume` with no recorded session cold-starts in the
/// orphan's tree; without one it names a command that works (dropping
/// --resume), never the orphaned-lease error that points back at --resume.
#[test]
fn bare_resume_without_a_recorded_session_cold_starts_in_the_orphan_tree() {
    let scope = "BUG-2223-NO-RECORDED-SESSION-ZZ";
    let tree = Path::new("/nonexistent/aida-seat-aida-bug-2223");
    let launch =
        crate::resolve_queue_work_launch(scope, Some(""), false, None, Some(tree)).unwrap();
    assert!(
        matches!(launch, crate::QueueWorkLaunch::Fresh(_)),
        "{launch:?}"
    );
    let err = crate::resolve_queue_work_launch(scope, Some(""), false, None, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("drop --resume"), "{err}");
    assert!(err.contains("no orphaned session tree"), "{err}");
}

/// Binding 4: the containment refusal leads with the vendor fix.
#[test]
fn the_containment_refusal_names_the_vendor_fix_in_its_first_line() {
    use crate::session::HeadlessVendor;
    let errors = [
        crate::session::require_vendor_containment(HeadlessVendor::Codex, true, "x").unwrap_err(),
        crate::session::child_containment_preflight(HeadlessVendor::Codex, true, true, "x")
            .unwrap_err(),
    ];
    for err in errors {
        let msg = format!("{err:#}");
        let first = msg.lines().next().unwrap();
        assert!(first.contains("AIDA_HEADLESS_VENDOR=claude"), "{first}");
        assert!(first.contains("--vendor claude"), "{first}");
        assert!(first.contains("`codex`"), "{first}");
    }
}
