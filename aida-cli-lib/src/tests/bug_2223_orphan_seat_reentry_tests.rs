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

/// `aida queue work --resume`'s session start: adopt the orphan at `path`,
/// with no `--force-claim`.
fn adopt(owns: &str, branch: &str, path: &Path) -> anyhow::Result<()> {
    crate::session_start::session_start_with_receipt(
        owns,
        Some(branch),
        None,
        false,
        path.to_str(),
        None,
        "auto",
        false,
        None,
        false,
        None,
        None,
        false,
        None,
        /* force_claim */ false,
        Some(false),
        /* self_claim */ true,
        /* review_coexistence */ None,
        /* contained_seat */ true,
        /* rollback_failed_setup */ false,
        /* adopt_orphan */ true,
    )
    .map(|mut receipt| receipt.disarm())
}

fn write_lease(main: &Path, lease: &crate::SessionLease) {
    std::fs::write(
        crate::lease_path(main, &lease.id),
        toml::to_string_pretty(lease).unwrap(),
    )
    .unwrap();
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
    write_lease(main, &lease);
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

    adopt("TASK-2223", &orphan.branch, &clone).unwrap();

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

/// Binding 3, at the queue-work conflict sweep: its refusal offers `--resume`
/// exactly when `--resume` adopts the lease. A dirty orphan whose dead owner
/// is recorded is offered and adopted; one that records no owner pid reads as
/// dormant to the sweep but cannot be proven dead, so it is neither offered
/// nor adopted, and `--resume` then names dropping it, never this refusal.
#[test]
fn the_conflict_hint_offers_resume_only_when_resume_adopts() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-922701";
    let resume_cmd = format!("aida queue work {scope} --resume");
    let orphan = orphaned_clone(&fx.main, scope);

    let refusal = crate::leases::lease_adoption_refusal(&orphan);
    assert_eq!(refusal, None);
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &orphan.id, refusal, None);
    assert!(hint.contains(&resume_cmd), "{hint}");
    let adopted = crate::leases::adoptable_orphan_lease(&fx.main, scope).expect("adopted");
    assert_eq!(adopted.id, orphan.id);

    // The same orphan, recording no owning process at all.
    let mut no_pid = orphan.clone();
    no_pid.active_pid = None;
    no_pid.active_pid_start_time = None;
    no_pid.creator_pid = None;
    no_pid.creator_pid_start_time = None;
    write_lease(&fx.main, &no_pid);
    let refusal = crate::leases::lease_adoption_refusal(&no_pid);
    assert!(
        refusal.is_some_and(|why| why.contains("no owning process")),
        "{refusal:?}"
    );
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &no_pid.id, refusal, None);
    assert!(!hint.contains("queue work"), "{hint}");
    assert!(hint.contains("cannot re-enter"), "{hint}");
    assert!(hint.contains("session end"), "{hint}");
    assert!(crate::leases::adoptable_orphan_lease(&fx.main, scope).is_none());
    let err = crate::resolve_queue_work_launch(scope, Some(""), false, None, None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("drop --resume"), "{err}");
}

/// Binding 1, multiplicity: adopting the newest orphan does not skip the
/// other leases on its scope. The sweep still reaches a live one, and session
/// start, given no `--force-claim`, refuses to adopt past it.
#[test]
fn adopting_an_orphan_still_refuses_a_live_lease_on_the_scope() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-922702";
    let orphan = orphaned_clone(&fx.main, scope);
    let clone = orphan.worktree_path.clone();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);

    // An older lease on the same scope whose owner is alive.
    let mut live = orphan.clone();
    live.id = uuid::Uuid::now_v7().to_string();
    live.started_at = orphan.started_at - chrono::Duration::hours(1);
    live.worktree_path = fx.root.join("live-tree");
    live.tree_kind = TreeKind::Worktree;
    std::fs::create_dir_all(&live.worktree_path).unwrap();
    write_lease(&fx.main, &live);
    set_owner(&fx.main, &live, std::process::id());

    assert_eq!(
        crate::leases::adoptable_orphan_lease(&fx.main, scope).map(|l| l.id),
        Some(orphan.id.clone()),
        "the newest lease is the orphan"
    );
    let next =
        crate::leases::next_scope_conflict(crate::list_leases(&fx.main), scope, Some(&orphan.id))
            .expect("the live lease is still swept");
    assert_eq!(next.id, live.id);
    assert!(
        crate::leases::lease_adoption_refusal(&next).is_some_and(|w| w.contains("live process"))
    );

    let err = format!("{:#}", adopt(scope, &orphan.branch, &clone).unwrap_err());
    assert!(err.contains(&live.id[..8]), "{err}");
    let mut ids: Vec<_> = crate::list_leases(&fx.main)
        .into_iter()
        .filter(|l| l.scope == scope)
        .map(|l| l.id)
        .collect();
    ids.sort();
    let mut want = vec![orphan.id.clone(), live.id.clone()];
    want.sort();
    assert_eq!(ids, want, "both leases are left as they were");
    assert_work_intact(&clone, &orphan.branch, &head);
}

/// An adoption at a path no orphaned lease of the scope records refuses
/// instead of starting a session there.
#[test]
fn adopting_a_path_that_is_not_an_orphans_tree_is_refused() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let missing = fx.root.join("seats").join("aida-seat-aida-gone");
    let err = format!("{:#}", adopt("TASK-922703", "x", &missing).unwrap_err());
    assert!(err.contains("no longer the tree of an orphaned"), "{err}");
    assert!(!missing.exists());
}
