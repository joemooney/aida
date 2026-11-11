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
    adopt_as(owns, branch, path, true)
}

/// [`adopt`], from a contained (`contained`) or uncontained launch.
fn adopt_as(owns: &str, branch: &str, path: &Path, contained: bool) -> anyhow::Result<()> {
    adopt_receipt(owns, branch, path, contained, false).map(|mut receipt| receipt.disarm())
}

fn adopt_receipt(
    owns: &str,
    branch: &str,
    path: &Path,
    contained: bool,
    rollback: bool,
) -> anyhow::Result<crate::session_start::SessionStartReceipt> {
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
        /* contained_seat */ contained,
        /* rollback_failed_setup */ rollback,
        crate::session_start::PathReentry::AdoptOrphan,
        None,
    )
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
    // Model the canonical ownership write independently of the legacy PID
    // fields, so adoption fixtures exercise the production reader.
    // trace:TASK-3420 | ai:codex
    use aida_core::db::DatabaseBackend as _;
    let backend = crate::test_env::home_fixture_backend(&main.join(".aida-store")).unwrap();
    let store = backend.load().unwrap();
    let req = store
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(&lease.scope))
        .unwrap_or_else(|| {
            let mut req = aida_core::Requirement::new("Adoption fixture".into(), String::new());
            req.spec_id = Some(lease.scope.clone());
            req.status = aida_core::RequirementStatus::Approved;
            backend.add_requirement(req).unwrap()
        });
    crate::ownership::reader::seed_lease_heartbeat(
        main,
        &lease,
        crate::ownership::WorkKey::Spec {
            spec: crate::gate::canon::SpecKey(req.id),
        },
        pid == std::process::id(),
    );
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

// trace:STORY-2710 | ai:codex
#[test]
fn story_2710_actual_orphan_adoption_transfers_pending_repair() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let orphan = orphaned_clone(&fx.main, "TASK-2223");
    let tree = &orphan.worktree_path;
    git_ok(tree, &["add", "-A"]);
    git_ok(
        tree,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.com",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "PR edits",
        ],
    );
    let head = git_ok(tree, &["rev-parse", "HEAD"]);
    std::fs::write(fx.main.join("README.md"), "competing main edit\n").unwrap();
    git_ok(
        &fx.main,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qam",
            "main edit",
        ],
    );
    let base = git_ok(&fx.main, &["rev-parse", "HEAD"]);
    git_ok(
        tree,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "fetch",
            "--no-write-fetch-head",
            fx.main.to_str().unwrap(),
            "main",
        ],
    );
    let execute = |args: &[&str]| -> anyhow::Result<String> {
        let out = crate::test_git::git_command()
            .current_dir(tree)
            .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
            .args(args)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "Git refused: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Ok(String::from_utf8(out.stdout)?.trim().to_owned())
    };
    assert!(execute(&["merge", "--no-edit", &base]).is_err());
    assert_eq!(execute(&["rev-parse", "MERGE_HEAD"]).unwrap(), base);
    let key = crate::repair_journal::RepairKey {
        repository: fx.main.clone(),
        pr: 3813,
        original_head: head.clone(),
    };
    let mut journal = crate::repair_journal::RepairGuard::lock(&fx.main).unwrap();
    journal
        .reserve(crate::repair_journal::RepairEntry {
            key: key.clone(),
            dispatched_by: "direct-rework".into(),
            reserved_at: "now".into(),
            lane_id: orphan.id.clone(),
            tree_path: tree.clone(),
            branch: orphan.branch.clone(),
            base: base.clone(),
            spawn: crate::repair_journal::SpawnOutcome::Unknown,
            original_conflict_paths: vec!["README.md".into()],
            preparation_success: Some(true),
            regeneration_success: None,
            validation: None,
            refusal_notice: None,
        })
        .unwrap();
    drop(journal);
    adopt("TASK-2223", &orphan.branch, tree).unwrap();
    let successor = lease_for(&fx.main, "TASK-2223");
    assert_ne!(successor.id, orphan.id);
    let resume = |lane: &str| {
        prepare_host_repair_with(
            &fx.main,
            tree,
            3813,
            lane,
            &execute,
            &orphan.branch,
            true,
            |_| panic!("pending conflicts cannot publish"),
            |_, _| panic!("successful preparation must be retained"),
            || Ok((base.clone(), head.clone())),
        )
    };
    assert!(resume(&orphan.id).is_err());
    assert!(resume("unrelated-lane").is_err());
    assert_eq!(resume(&successor.id).unwrap(), ["README.md"]);
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

    let refusal = crate::leases::lease_adoption_refusal(&fx.main, &orphan);
    assert_eq!(refusal, None);
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &orphan.id, refusal, None, true);
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
    // Legacy production holder without an ownership witness, rather than the
    // dead synthetic heartbeat the initial orphan fixture installed.
    // trace:TASK-3420 | ai:codex
    std::fs::remove_dir_all(fx.main.join(".aida/ownership")).unwrap();
    let refusal = crate::leases::lease_adoption_refusal(&fx.main, &no_pid);
    assert!(
        refusal.is_some_and(|why| why.contains("no owning process")),
        "{refusal:?}"
    );
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &no_pid.id, refusal, None, true);
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
    // Both legacy sessions predate ownership admission. A single canonical
    // record cannot represent two different session holders of the same spec.
    // trace:TASK-3420 | ai:codex
    std::fs::remove_dir_all(fx.main.join(".aida/ownership")).unwrap();

    assert_eq!(
        crate::leases::adoptable_orphan_lease(&fx.main, scope).map(|l| l.id),
        Some(orphan.id.clone()),
        "the newest lease is the orphan"
    );
    let next =
        crate::leases::next_scope_conflict(crate::list_leases(&fx.main), scope, Some(&orphan.id))
            .expect("the live lease is still swept");
    assert_eq!(next.id, live.id);
    assert!(crate::leases::lease_adoption_refusal(&fx.main, &next)
        .is_some_and(|w| w.contains("live process")));

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

/// A linked worktree for `owns` (an uncontained seat) whose worker died,
/// holding uncommitted work.
fn orphaned_worktree(main: &Path, owns: &str) -> crate::SessionLease {
    start(owns, None, None, false, false).unwrap();
    let lease = lease_for(main, owns);
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    std::fs::write(lease.worktree_path.join("wip.txt"), "untracked work\n").unwrap();
    set_owner(main, &lease, dead_pid());
    lease_for(main, owns)
}

/// Finding 5: a linked-worktree (non-clone) orphan is adopted in place through
/// `reuse_existing_worktree_path`, and one git no longer lists is neither
/// offered nor adopted.
// trace:BUG-2223 | ai:claude
#[test]
fn a_linked_worktree_orphan_is_adopted_in_place_unless_git_no_longer_lists_it() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    // The store is its own repository, as in a real checkout: re-entering a
    // worktree excludes `.aida-store` from main's index.
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let scope = "TASK-922704";
    let orphan = orphaned_worktree(&fx.main, scope);
    let tree = orphan.worktree_path.clone();
    assert_eq!(
        crate::leases::lease_adoption_refusal(&fx.main, &orphan),
        None
    );
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &orphan.id, None, None, false);
    assert!(
        hint.contains(&format!("aida queue work {scope} --resume`")),
        "{hint}"
    );

    adopt_as(scope, &orphan.branch, &tree, false).unwrap();
    let lease = lease_for(&fx.main, scope);
    assert_ne!(lease.id, orphan.id);
    assert_eq!(lease.tree_kind, TreeKind::Worktree);
    assert_eq!(lease.worktree_path, tree);
    assert!(!crate::lease_path(&fx.main, &orphan.id).exists());
    assert_eq!(
        std::fs::read_to_string(tree.join("wip.txt")).unwrap(),
        "untracked work\n"
    );

    // Another orphan whose worktree git has forgotten.
    let scope = "TASK-922705";
    let gone = orphaned_worktree(&fx.main, scope);
    let admin = git_ok(&gone.worktree_path, &["rev-parse", "--git-dir"]);
    std::fs::remove_dir_all(gone.worktree_path.join(admin)).unwrap();
    let refusal = crate::leases::lease_adoption_refusal(&fx.main, &gone);
    assert!(
        refusal.is_some_and(|why| why.contains("git worktree list")),
        "{refusal:?}"
    );
    let hint = crate::queue_cmd::scope_conflict_resume_hint(scope, &gone.id, refusal, None, false);
    assert!(!hint.contains("queue work"), "{hint}");
    assert!(crate::leases::adoptable_orphan_lease(&fx.main, scope).is_none());
}

/// Finding 4a: a seat clone's hint names the contained launch that re-enters
/// it, since an uncontained `--resume` refuses there.
// trace:BUG-2223 | ai:claude
#[test]
fn a_seat_clone_hint_names_the_contained_resume() {
    let hint =
        crate::queue_cmd::scope_conflict_resume_hint("TASK-1", "0123456789", None, None, true);
    assert!(
        hint.contains("`aida queue work TASK-1 --resume --sandbox`"),
        "{hint}"
    );
}

/// BUG-B6B1278890774EF993D2EBC327862470-023: every scope-conflict hint names
/// the FULL lease id. An 8-hex prefix is a timestamp that two sessions often
/// share, so `aida session end <prefix>` it suggested was ambiguous.
// trace:BUG-B6B1278890774EF993D2EBC327862470-023 | ai:claude
#[test]
fn scope_conflict_hints_name_the_full_lease_id() {
    let id = "01a18a18c31c";
    for hint in [
        crate::queue_cmd::scope_conflict_resume_hint("TASK-1", id, None, None, false),
        crate::queue_cmd::scope_conflict_resume_hint("TASK-1", id, Some("why"), None, false),
        crate::queue_cmd::scope_conflict_resume_hint(
            "TASK-1",
            id,
            None,
            Some("01a18a18aaaa"),
            false,
        ),
    ] {
        assert!(
            hint.contains(&format!("aida session end {id} --force")),
            "{hint}"
        );
    }
    let adopted = crate::queue_cmd::scope_conflict_resume_hint(
        "TASK-1",
        id,
        None,
        Some("01a18a18aaaa"),
        false,
    );
    assert!(adopted.contains("(01a18a18aaaa)"), "{adopted}");
}

/// `aida queue work <scope>` as the CLI runs it, with `--no-launch`.
fn queue_work(
    storage: &crate::Storage,
    scope: &str,
    branch: Option<&str>,
    path: Option<&str>,
    resume: Option<&str>,
) -> anyhow::Result<()> {
    queue_work_as(storage, scope, branch, path, resume, false)
}

/// [`queue_work`], with or without `--sandbox` (a contained launch).
fn queue_work_as(
    storage: &crate::Storage,
    scope: &str,
    branch: Option<&str>,
    path: Option<&str>,
    resume: Option<&str>,
    sandbox: bool,
) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some(scope),
        None,
        sandbox,
        /* no_launch */ true,
        crate::queue_cmd::QueueWorkRoleInput::direct(Some("implementer")),
        /* no_pull */ true,
        None,
        branch,
        path,
        false,
        None,
        false,
        false,
        false,
        /* force_claim */ false,
        resume,
        false,
        false,
        None,
        "claude",
        false,
        crate::phase_driver::AutonomyMode::Default,
        None,
        false,
        false,
        false,
        None,
        None,
        None,
        false,
        false,
        false,
        false,
        None,
    )
}

/// An Approved requirement for `scope`, queued for the hermetic test user,
/// in `main`'s store; returns the storage and a setter for its status.
pub(super) fn queued_scope(
    main: &Path,
    scope: &str,
) -> (crate::Storage, impl Fn(aida_core::RequirementStatus)) {
    let store_root = main.join(".aida-store");
    let backend = crate::test_env::home_fixture_backend(&store_root).unwrap();
    // Match allocation authority's file contract under the lane's shared-write umask.
    // trace:BUG-2504 | ai:codex
    use std::os::unix::fs::PermissionsExt;
    let common = aida_core::allocation_authority::common_dir(&store_root).unwrap();
    std::fs::set_permissions(
        common.join(aida_core::allocation_authority::CONFIG_FILE),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let storage = crate::Storage::new(&store_root);
    std::fs::create_dir_all(store_root.join("registry")).unwrap();
    std::fs::write(
        store_root.join("registry/team.toml"),
        format!(
            "[members]\n\"{}\" = [\"implementer\"]\n",
            crate::test_env::HERMETIC_TEST_USER
        ),
    )
    .unwrap();
    use aida_core::db::DatabaseBackend as _;
    let mut req = aida_core::Requirement::new("Orphaned work".to_string(), String::new());
    req.spec_id = Some(scope.to_string());
    req.status = aida_core::RequirementStatus::Approved;
    let req = backend.add_requirement(req).unwrap();
    storage
        .queue_add(aida_core::QueueEntry {
            user_id: crate::test_env::HERMETIC_TEST_USER.to_string(),
            requirement_id: req.id,
            position: 0,
            added_by: crate::test_env::HERMETIC_TEST_USER.to_string(),
            note: None,
            added_at: chrono::Utc::now(),
            for_role: None,
            for_scope: None,
            for_session: None,
            added_by_machine: None,
        })
        .unwrap();
    let set_status = move |status: aida_core::RequirementStatus| {
        let mut store = backend.load().unwrap();
        let r = store
            .requirements
            .iter_mut()
            .find(|r| r.id == req.id)
            .unwrap();
        r.status = status;
        backend.save(&store).unwrap();
    };
    (storage, set_status)
}

/// Finding 5, end to end through `handle_queue_work --resume`: a `--path` or
/// `--branch` naming another tree refuses, the conflict sweep skips the
/// adopted orphan (without `--resume` it refuses the dirty orphan), and the
/// pickup lands in the orphan's tree with its work intact. The status gate
/// lets the orphan's In Progress through, but not NeedsAttention.
// trace:BUG-2223 | ai:claude
#[test]
fn queue_work_resume_adopts_the_orphan_end_to_end() {
    let fx = fixture();
    // Keep synthetic store writes in a separate repository; inheriting the
    // parent code remote would turn config setup into a dirty-store refusal.
    // trace:BUG-2501 | ai:codex
    aida_core::git_ops::init(&fx.main.join(".aida-store")).unwrap();
    aida_core::git_ops::configure_user(&fx.main.join(".aida-store"), "Test", "test@example.com")
        .unwrap();
    let store = fx.main.join(".aida-store");
    crate::test_env::enroll_synthetic_home(&store).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let common = aida_core::allocation_authority::common_dir(&store).unwrap();
        std::fs::set_permissions(
            common.join(aida_core::allocation_authority::CONFIG_FILE),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &["implementer"],
        &[],
    );
    let scope = "TASK-922706";
    let (storage, set_status) = queued_scope(&fx.main, scope);
    let orphan = orphaned_worktree(&fx.main, scope);
    set_status(aida_core::RequirementStatus::InProgress);
    let tree = orphan.worktree_path.clone();
    let elsewhere = fx.root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();

    let results = [
        queue_work(&storage, scope, None, None, None),
        queue_work(&storage, scope, None, elsewhere.to_str(), Some("")),
        queue_work(&storage, scope, Some("another-branch"), None, Some("")),
    ];
    let errs: Vec<String> = results
        .into_iter()
        .map(|r| format!("{:#}", r.unwrap_err()))
        .collect();
    // Without --resume the sweep refuses the orphan (its wording depends on
    // whether this host can see other processes).
    assert!(errs[0].contains(&orphan.id[..8]), "{}", errs[0]);
    assert!(errs[1].contains("drop --path"), "{}", errs[1]);
    assert!(errs[2].contains("drop --branch"), "{}", errs[2]);
    assert_eq!(lease_for(&fx.main, scope).id, orphan.id, "nothing changed");

    set_status(aida_core::RequirementStatus::NeedsAttention);
    let err = format!(
        "{:#}",
        adopt_as(scope, &orphan.branch, &tree, false).unwrap_err()
    );
    assert!(err.contains("NeedsAttention"), "{err}");
    assert_eq!(lease_for(&fx.main, scope).id, orphan.id, "nothing changed");
    set_status(aida_core::RequirementStatus::InProgress);

    queue_work(&storage, scope, None, None, Some("")).unwrap();
    let lease = lease_for(&fx.main, scope);
    assert_ne!(lease.id, orphan.id);
    assert_eq!(lease.worktree_path, tree);
    assert_eq!(lease.branch, orphan.branch);
    assert_eq!(
        std::fs::read_to_string(tree.join("wip.txt")).unwrap(),
        "untracked work\n"
    );
}

/// The PR-3106 incident through `handle_queue_work`: a dead reviewer's lease
/// on the scope whose tree was deleted by hand is released by the conflict
/// sweep without `--force-claim`, and the pickup takes the scope.
// trace:BUG-b6b1278890774ef993d2ebc327862470-024 | ai:claude
#[test]
fn queue_work_releases_a_dead_gone_tree_lease_without_force_claim() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &["implementer"],
        &[],
    );
    let scope = "TASK-922710";
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let (storage, set_status) = queued_scope(&fx.main, scope);
    crate::test_env::write_project_config_fixture(&fx.main, "store_path = \".aida-store\"\n");
    let mut dead = orphaned_worktree(&fx.main, scope);
    // A review lease holds a PR scope, which has no spec status to gate on.
    set_status(aida_core::RequirementStatus::Approved);
    dead.role = Some("reviewer".into());
    write_lease(&fx.main, &dead);
    std::fs::remove_dir_all(&dead.worktree_path).unwrap();
    assert_eq!(
        crate::leases::stale_lease_recovery_for_lease(&dead).verdict,
        crate::StaleLeaseRecovery::ReclaimableClean {
            worktree_missing: true
        }
    );

    // Recordless and live ownership both retain the lease without a journal attempt.
    // Remove the ownership evidence seeded by orphaned_worktree explicitly.
    // trace:TASK-3420 | ai:codex
    std::fs::remove_dir_all(fx.main.join(".aida/ownership")).unwrap();
    let error = queue_work(&storage, scope, Some("task-922710-next"), None, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("aida own migrate --execute"),
        "{error:#}"
    );
    assert!(crate::lease_path(&fx.main, &dead.id).exists());
    let req = storage
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(scope))
        .unwrap();
    let key = crate::ownership::WorkKey::Spec {
        spec: crate::gate::canon::SpecKey(req.id),
    };
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &dead, key.clone(), true);
    let error = queue_work(&storage, scope, Some("task-922710-next"), None, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("ownership holder is alive"),
        "{error:#}"
    );
    assert!(crate::lease_path(&fx.main, &dead.id).exists());
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &dead, key, false);
    // `--branch` steps past the separate leftover-branch guard; the sweep is
    // what is under test.
    queue_work(&storage, scope, Some("task-922710-next"), None, None).unwrap();
    let journal = std::fs::read_to_string(fx.main.join(".aida/ownership/journal.jsonl")).unwrap();
    assert!(journal.contains("reclaim_attempt"), "{journal}");
    assert!(!crate::lease_path(&fx.main, &dead.id).exists());
    let lease = lease_for(&fx.main, scope);
    assert_ne!(lease.id, dead.id);
    assert!(lease.worktree_path.exists());
}

/// The pre-setup branch gate refuses a branch a seat clone holds, except for
/// the orphan's clone `--resume` is re-entering.
// trace:BUG-2223 | ai:claude
#[test]
fn the_branch_preflight_lets_the_adopted_clone_hold_its_branch() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let orphan = orphaned_clone(&fx.main, "TASK-922707");
    let branch = Some(orphan.branch.as_str());
    assert!(crate::queue_cmd::preflight_queue_work_branch(&fx.main, branch).is_err());
    crate::queue_cmd::preflight_queue_work_branch_adopting(
        &fx.main,
        branch,
        Some(&orphan.worktree_path),
    )
    .unwrap();
}

/// The clone counterpart of [`queue_work_resume_adopts_the_orphan_end_to_end`]:
/// a contained `queue work --resume` re-enters an orphaned seat clone in
/// place, its commit and uncommitted work intact.
// trace:TASK-b6b1278890774ef993d2ebc327862470-005 | ai:claude
#[test]
fn queue_work_resume_adopts_an_orphaned_seat_clone_end_to_end() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &["implementer"],
        // A contained pickup exports AIDA_CONTAINED; restore it on drop.
        &[("AIDA_CONTAINED", None)],
    );
    let scope = "TASK-922708";
    let (storage, set_status) = queued_scope(&fx.main, scope);
    let orphan = orphaned_clone(&fx.main, scope);
    set_status(aida_core::RequirementStatus::InProgress);
    let clone = orphan.worktree_path.clone();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);

    queue_work_as(&storage, scope, None, None, Some(""), true).unwrap();
    let lease = lease_for(&fx.main, scope);
    assert_ne!(lease.id, orphan.id);
    assert_eq!(lease.tree_kind, TreeKind::Clone);
    assert_eq!(lease.worktree_path, clone);
    assert_eq!(lease.branch, orphan.branch);
    assert!(!crate::lease_path(&fx.main, &orphan.id).exists());
    assert_work_intact(&clone, &orphan.branch, &head);
}

/// Adoption refuses past another lease on the scope whose owner cannot be
/// proven gone (it records no pid), the same rule the queue-work conflict
/// sweep applies, so a direct `session start --path --force-claim` adoption
/// does not slip past it either.
// trace:TASK-b6b1278890774ef993d2ebc327862470-004 | ai:claude
#[test]
fn adopting_an_orphan_refuses_a_lease_of_unknown_liveness_on_the_scope() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-922709";
    let orphan = orphaned_clone(&fx.main, scope);
    let clone = orphan.worktree_path.clone();
    let head = git_ok(&clone, &["rev-parse", "HEAD"]);

    // An older lease on the same scope that records no owner pid.
    let mut unknown = orphan.clone();
    unknown.id = uuid::Uuid::now_v7().to_string();
    unknown.started_at = orphan.started_at - chrono::Duration::hours(1);
    unknown.worktree_path = fx.root.join("unknown-tree");
    unknown.tree_kind = TreeKind::Worktree;
    unknown.active_pid = None;
    unknown.active_pid_start_time = None;
    unknown.creator_pid = None;
    unknown.creator_pid_start_time = None;
    std::fs::create_dir_all(&unknown.worktree_path).unwrap();
    write_lease(&fx.main, &unknown);
    assert_eq!(
        crate::leases::stale_lease_recovery_for_lease(&unknown).base_verdict,
        crate::StaleLeaseRecovery::UnknownLiveness
    );

    let errs = [
        start(scope, Some(&orphan.branch), Some(&clone), true, true).unwrap_err(),
        adopt(scope, &orphan.branch, &clone).unwrap_err(),
    ];
    for err in errs {
        let err = format!("{err:#}");
        assert!(err.contains(&unknown.id[..8]), "{err}");
        assert!(err.contains("cannot be proven gone"), "{err}");
    }
    let mut ids: Vec<_> = crate::list_leases(&fx.main)
        .into_iter()
        .filter(|l| l.scope == scope)
        .map(|l| l.id)
        .collect();
    ids.sort();
    let mut want = vec![orphan.id.clone(), unknown.id.clone()];
    want.sort();
    assert_eq!(ids, want, "both leases are left as they were");
    assert_work_intact(&clone, &orphan.branch, &head);
}

/// A positive death proof journals only an attempt: ordinary teardown may
/// still refuse a different process holding the tree, retaining the lease.
// trace:STORY-2325 | ai:codex
#[test]
fn queue_work_reclaim_attempt_keeps_lease_when_session_end_refuses() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &["implementer"],
        &[],
    );
    let scope = "TASK-922711";
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let (storage, set_status) = queued_scope(&fx.main, scope);
    crate::test_env::write_project_config_fixture(&fx.main, "store_path = \".aida-store\"\n");
    let dead = orphaned_worktree(&fx.main, scope);
    set_status(aida_core::RequirementStatus::Approved);
    std::fs::remove_file(dead.worktree_path.join("wip.txt")).unwrap();
    git_ok(
        &dead.worktree_path,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "origin",
            &dead.branch,
        ],
    );
    let req = storage
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(scope))
        .unwrap();
    crate::ownership::reader::seed_lease_heartbeat(
        &fx.main,
        &dead,
        crate::ownership::WorkKey::Spec {
            spec: crate::gate::canon::SpecKey(req.id),
        },
        false,
    );
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _child = Child(
        std::process::Command::new("/usr/bin/sleep")
            .arg("60")
            .current_dir(&dead.worktree_path)
            .spawn()
            .unwrap(),
    );
    let error = queue_work(&storage, scope, Some("task-922711-next"), None, None).unwrap_err();
    let error = format!("{error:#}");
    assert!(
        error.contains("ownership reclaim could not end lease"),
        "{error}"
    );
    assert!(crate::lease_path(&fx.main, &dead.id).exists(), "{error}");
    let journal = std::fs::read_to_string(fx.main.join(".aida/ownership/journal.jsonl")).unwrap();
    assert!(journal.contains("reclaim_attempt"), "{error}\n{journal}");
    assert!(!journal.contains("\"op\":\"reclaim\""), "{journal}");
}

/// trace:STORY-2325 | ai:codex — BUG-307: a clean lane can die before
/// its first push, with HEAD already covered by origin/main.
#[test]
fn queue_work_reclaims_dead_clean_lane_without_an_origin_branch() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.main,
        "implementer",
        &["implementer"],
        &[],
    );
    let scope = "TASK-922712";
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let (storage, set_status) = queued_scope(&fx.main, scope);
    crate::test_env::write_project_config_fixture(&fx.main, "store_path = \".aida-store\"\n");
    let dead = orphaned_worktree(&fx.main, scope);
    set_status(aida_core::RequirementStatus::Approved);
    std::fs::remove_file(dead.worktree_path.join("wip.txt")).unwrap();
    assert!(!std::process::Command::new("git")
        .arg("-C")
        .arg(&fx.main)
        .args([
            "show-ref",
            "--verify",
            &format!("refs/remotes/origin/{}", dead.branch)
        ])
        .output()
        .unwrap()
        .status
        .success());
    assert_eq!(
        crate::session_end::remote_missing_commits_for_lease(&fx.main, &dead).unwrap(),
        0
    );
    let req = storage
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(scope))
        .unwrap();
    crate::ownership::reader::seed_lease_heartbeat(
        &fx.main,
        &dead,
        crate::ownership::WorkKey::Spec {
            spec: crate::gate::canon::SpecKey(req.id),
        },
        false,
    );
    let result = queue_work(&storage, scope, Some("task-922712-next"), None, None);
    assert!(result.is_ok(), "{result:?}");
    assert!(!crate::lease_path(&fx.main, &dead.id).exists());
    let journal = std::fs::read_to_string(fx.main.join(".aida/ownership/journal.jsonl")).unwrap();
    assert!(journal.contains("reclaim_attempt"));
}

// trace:STORY-2325 | ai:codex
#[test]
fn queue_recover_dry_run_inspects_live_and_unknown_ownership_without_mutation() {
    let fx = fixture();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let scope = "TASK-922713";
    git_ok(&fx.main, &["init", "-q", ".aida-store"]);
    let (storage, _) = queued_scope(&fx.main, scope);
    crate::test_env::write_project_config_fixture(&fx.main, "store_path = \".aida-store\"\n");
    let lease = orphaned_worktree(&fx.main, scope);
    // With no WIP, execution takes EndAndRequeue, which really ends a lease.
    std::fs::remove_file(lease.worktree_path.join("wip.txt")).unwrap();
    let req = storage
        .load()
        .unwrap()
        .requirements
        .into_iter()
        .find(|r| r.spec_id.as_deref() == Some(scope))
        .unwrap();
    let key = crate::ownership::WorkKey::Spec {
        spec: crate::gate::canon::SpecKey(req.id),
    };
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &lease, key, true);
    let before = std::fs::read(crate::lease_path(&fx.main, &lease.id)).unwrap();
    for unknown in [false, true] {
        if unknown {
            std::fs::remove_dir_all(fx.main.join(".aida/ownership")).unwrap();
        }
        crate::queue_cmd::handle_queue_recover(
            &storage,
            crate::test_env::HERMETIC_TEST_USER,
            scope,
            true,
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(crate::lease_path(&fx.main, &lease.id)).unwrap(),
            before
        );
        let error = crate::queue_cmd::handle_queue_recover(
            &storage,
            crate::test_env::HERMETIC_TEST_USER,
            scope,
            false,
            true,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(&lease.id));
    }
}

// trace:STORY-2325 | ai:codex
#[test]
fn queue_recover_parks_wip_with_live_and_unknown_ownership_without_reclaim() {
    for unknown in [false, true] {
        let fx = fixture();
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let scope = "TASK-922714";
        git_ok(&fx.main, &["init", "-q", ".aida-store"]);
        let (storage, _) = queued_scope(&fx.main, scope);
        crate::test_env::write_project_config_fixture(&fx.main, "store_path = \".aida-store\"\n");
        let lease = orphaned_worktree(&fx.main, scope);
        let req = storage
            .load()
            .unwrap()
            .requirements
            .into_iter()
            .find(|r| r.spec_id.as_deref() == Some(scope))
            .unwrap();
        crate::ownership::reader::seed_lease_heartbeat(
            &fx.main,
            &lease,
            crate::ownership::WorkKey::Spec {
                spec: crate::gate::canon::SpecKey(req.id),
            },
            true,
        );
        if unknown {
            std::fs::remove_dir_all(fx.main.join(".aida/ownership")).unwrap();
        }
        let lease_before = std::fs::read(crate::lease_path(&fx.main, &lease.id)).unwrap();
        let journal = fx.main.join(".aida/ownership/journal.jsonl");
        let journal_before = std::fs::read(&journal).ok();
        let head_before = git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]);
        // Execute WipCommitPark with --auto: commit the WIP without ending or
        // relaunching the owner. Neither live nor missing evidence may gate it.
        crate::queue_cmd::handle_queue_recover(
            &storage,
            crate::test_env::HERMETIC_TEST_USER,
            scope,
            false,
            true,
        )
        .unwrap();
        assert_ne!(
            git_ok(&lease.worktree_path, &["rev-parse", "HEAD"]),
            head_before
        );
        assert_eq!(
            git_ok(&lease.worktree_path, &["show", "HEAD:wip.txt"]),
            "untracked work"
        );
        assert_eq!(
            std::fs::read(crate::lease_path(&fx.main, &lease.id)).unwrap(),
            lease_before
        );
        assert_eq!(std::fs::read(&journal).ok(), journal_before);
    }
}

// trace:TASK-3420 | ai:codex
#[test]
fn task_3420_orphan_adoption_requires_matching_owner_death() {
    let fx = fixture();
    clone_config(&fx.main, &fx.root);
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let mut lease = orphaned_clone(&fx.main, "TASK-3420");
    // A change scope has a stable ownership key without a spec-store fixture.
    lease.scope = "PR-3420".into();
    write_lease(&fx.main, &lease);
    let key = crate::ownership::WorkKey::Change {
        change: crate::ownership::ChangeId {
            forge: "origin".into(),
            n: 3420,
        },
    };
    let adopt = || {
        crate::session_start::orphan_lease_to_adopt(
            &fx.main,
            &lease.worktree_path,
            &lease.scope,
            &lease.branch,
            true,
        )
    };
    assert_eq!(
        adopt().unwrap().unwrap().id,
        lease.id,
        "crashed legacy holder remains adoptable"
    );
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &lease, key.clone(), true);
    assert!(adopt().is_err(), "live ownership must retain the clone");
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &lease, key.clone(), false);
    assert_eq!(adopt().unwrap().unwrap().id, lease.id);
    let mut replacement = lease.clone();
    replacement.id = "replacement".into();
    crate::ownership::reader::seed_lease_heartbeat(&fx.main, &replacement, key, false);
    assert!(
        adopt().is_err(),
        "a replacement's death cannot release this clone"
    );
}

// trace:BUG-2504 | ai:codex
#[test]
fn bug_2504_refused_adopted_dirty_launch_is_retryable() {
    for orphan_still_present in [false, true] {
        let fx = fixture();
        clone_config(&fx.main, &fx.root);
        let _ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let orphan = orphaned_clone(&fx.main, "TASK-2504");
        let head = git_ok(&orphan.worktree_path, &["rev-parse", "HEAD"]);
        let receipt = adopt_receipt(
            "TASK-2504",
            &orphan.branch,
            &orphan.worktree_path,
            true,
            true,
        )
        .unwrap();
        assert!(!crate::lease_path(&fx.main, &orphan.id).exists());
        // Simulate adoption's warned removal failure: rollback must recognize the
        // original record and still remove its successor and release its claim.
        if orphan_still_present {
            std::fs::write(
                crate::lease_path(&fx.main, &orphan.id),
                format!(
                    "# older writer formatting\n{}",
                    toml::to_string(&orphan).unwrap()
                ),
            )
            .unwrap();
        }
        let (result, rolled_back) = crate::queue_cmd::run_pickup_launch(&fx.main, receipt, |_| {
            anyhow::bail!("dirty resume merge admission refused before vendor launch")
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("admission refused"));
        assert!(rolled_back);
        let remaining = crate::list_leases(&fx.main);
        assert_eq!(remaining.len(), 1, "successor lease must be removed");
        assert_eq!(remaining[0].id, orphan.id);
        assert_eq!(
            crate::leases::adoptable_orphan_lease(&fx.main, "TASK-2504").map(|l| l.id),
            Some(orphan.id.clone())
        );
        assert_work_intact(&orphan.worktree_path, &orphan.branch, &head);
        adopt("TASK-2504", &orphan.branch, &orphan.worktree_path).unwrap();
        assert_work_intact(&orphan.worktree_path, &orphan.branch, &head);
    }
}
