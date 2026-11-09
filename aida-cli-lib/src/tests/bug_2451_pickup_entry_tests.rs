//! Exercise session setup, including the queue's resolved-PR handoff.
// trace:BUG-2451 | ai:codex
use super::tests::{fixture, git_ok};
use crate::session_start::{session_start_with_receipt, PathReentry};
use std::path::Path;

fn start(
    scope: &str,
    path: &Path,
    contained: bool,
    resolved: Option<Option<(u64, String)>>,
) -> anyhow::Result<crate::session_start::SessionStartReceipt> {
    session_start_with_receipt(
        scope,
        None,
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
        Some("implementer".into()),
        false,
        Some(false),
        true,
        None,
        contained,
        false,
        PathReentry::None,
        resolved,
    )
}

fn forge_fixture(main: &Path, root: &Path, reply: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(main.join(".aida")).unwrap();
    std::fs::write(
        main.join(".aida/config.toml"),
        "[forge]\nprovider = \"github\"\n",
    )
    .unwrap();
    git_ok(main, &["init", "-q", ".aida-store"]);
    let gh = root.join("fake-gh");
    crate::test_exec::write_executable(&gh, format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'gh test'; exit 0; fi\nif [ \"$1 $2\" = 'pr list' ]; then printf '%s\\n' '{reply}'; exit 0; fi\nexit 1\n"
    ));
    gh
}

#[test]
fn bug_2451_real_session_refuses_a_held_head_in_worktree_and_clone_modes() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(
        &fx.main,
        &fx.root,
        "42\tfix (BUG-2451)\thttps://example.invalid/42\toriginal-pr",
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let holder = fx.root.join("holder");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "original-pr",
            holder.to_str().unwrap(),
        ],
    );
    for contained in [false, true] {
        let error = start("BUG-2451", &fx.root.join("new-seat"), contained, None)
            .err()
            .expect("a held PR head must refuse")
            .to_string();
        assert!(error.contains(&holder.display().to_string()), "{error}");
        assert!(error.contains("held by worktree"), "{error}");
        assert!(!fx.root.join("new-seat").exists());
        assert!(crate::list_leases(&fx.main).is_empty());
        assert!(!super::tests::ref_exists(&fx.main, "refs/heads/bug-2451-2"));
    }
}

#[test]
fn bug_2451_real_session_seats_origin_only_head_after_stale_seat_removal() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(
        &fx.main,
        &fx.root,
        "42\tgeneric fix\thttps://example.invalid/42\toriginal-pr",
    );
    // The hold remains positive association evidence even with generic metadata.
    use crate::gate::merge_hold_core as mh;
    let mut hold = mh::typed_hold(42, mh::HoldReasonKind::Rework, "rework", None);
    hold.spec = Some("BUG-2451".into());
    mh::place_hold(&fx.main, &hold).unwrap();
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let old = fx.root.join("old-seat");
    git_ok(
        &fx.main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "original-pr",
            old.to_str().unwrap(),
        ],
    );
    std::fs::write(old.join("original-work"), "must survive\n").unwrap();
    git_ok(&old, &["add", "original-work"]);
    git_ok(
        &old,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "original fix (BUG-2451)",
        ],
    );
    let head = git_ok(&old, &["rev-parse", "HEAD"]);
    git_ok(
        &old,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "push",
            "-q",
            "origin",
            "HEAD:original-pr",
        ],
    );
    git_ok(&fx.main, &["worktree", "remove", old.to_str().unwrap()]);
    git_ok(&fx.main, &["branch", "-D", "original-pr"]);
    let seated = fx.root.join("new-seat");
    let mut receipt = start("BUG-2451", &seated, false, None).unwrap();
    receipt.disarm();
    assert_eq!(git_ok(&seated, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git_ok(&seated, &["branch", "--show-current"]),
        "original-pr"
    );
    assert_eq!(
        std::fs::read_to_string(seated.join("original-work")).unwrap(),
        "must survive\n"
    );
    assert_eq!(crate::list_leases(&fx.main)[0].branch, "original-pr");
}

#[test]
fn bug_2451_real_session_skips_forge_for_broad_scopes_and_resolved_queue_pickup() {
    for scope in ["feature:x", "src/**", "EPIC-2451", "BUG-2451"] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        crate::test_exec::write_executable(&gh, "#!/bin/sh\nexit 1\n");
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        let resolved = (scope == "BUG-2451").then_some(None);
        let mut receipt = start(scope, &fx.root.join("seat"), false, resolved).unwrap();
        receipt.disarm();
    }
}

#[test]
fn bug_2451_queue_workspace_dates_confirmed_merged_retained_branch() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    crate::test_exec::write_executable(
        &gh,
        r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'gh test'; exit 0; fi
case "$*" in
  *'pr list'*'--state merged'*) printf '88\tmerged fix\thttps://example.invalid/88\tbug-2451\n';;
esac
exit 0
"#,
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    git_ok(&fx.main, &["branch", "bug-2451"]);
    // The ordinary queue-work resolver must allocate a dated branch without
    // operator flags, while leaving the retained branch intact.
    let (_, planned) =
        crate::session_start::resolve_pickup_workspace(&fx.main, "BUG-2451", "auto").unwrap();
    assert!(planned.starts_with("bug-2451-"), "{planned}");
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, false).unwrap();
    let leases = crate::list_leases(&fx.main);
    assert_eq!(leases.len(), 1);
    let seated = leases[0].worktree_path.clone();
    assert_eq!(git_ok(&seated, &["branch", "--show-current"]), planned);
    assert!(super::tests::ref_exists(&fx.main, "refs/heads/bug-2451"));
    use crate::auto_complete::PhaseDriver;
    let driver = crate::RealPhaseDriver::new(
        fx.main.clone(),
        "BUG-2451".into(),
        "test".into(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "test-token".into(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::none(),
        crate::auto_complete::AutoCompleteVariant::ThroughCi,
    );
    let (path, branch) = driver.implementer_workspace().unwrap().unwrap();
    assert_eq!(
        Path::new(&path).canonicalize().unwrap(),
        seated.canonicalize().unwrap()
    );
    assert!(branch.starts_with("bug-2451-"), "{branch}");
}

// trace:BUG-2451 | ai:codex
fn queue_work(storage: &crate::Storage, list_sessions: bool) -> anyhow::Result<()> {
    crate::queue_cmd::handle_queue_work(
        storage,
        None,
        crate::test_env::HERMETIC_TEST_USER,
        Some("BUG-2451"),
        None,
        false,
        /* no_launch */ true,
        crate::queue_cmd::QueueWorkRoleInput::direct(Some("implementer")),
        /* no_pull */ true,
        None,
        None,
        None,
        false,
        None,
        false,
        false,
        false,
        /* force_claim */ false,
        None,
        false,
        list_sessions,
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

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_queue_list_sessions_never_contacts_offline_forge() {
    let fx = fixture();
    let mut ambient =
        crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
    let gh = forge_fixture(&fx.main, &fx.root, "");
    crate::test_exec::write_executable(
        &gh,
        "#!/bin/sh\necho contacted > \"${0%/*}/contacted\"\nexit 1\n",
    );
    ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
    let (storage, _) = super::orphan_reentry_tests::queued_scope(&fx.main, "BUG-2451");
    queue_work(&storage, true).unwrap();
    assert!(!fx.root.join("contacted").exists());
    assert!(crate::list_leases(&fx.main).is_empty());
}

// trace:BUG-2451 | ai:codex
#[test]
fn bug_2451_retained_branch_requires_confirmed_absence_of_open_pr() {
    for open_reply in [
        "echo offline >&2; exit 1",
        "printf '99\\topen fix\\thttps://example.invalid/99\\tbug-2451\\n'",
    ] {
        let fx = fixture();
        let mut ambient =
            crate::test_env::AmbientGuard::hermetic_with_seat(&fx.main, "implementer", &[]);
        let gh = forge_fixture(&fx.main, &fx.root, "");
        crate::test_exec::write_executable(
            &gh,
            format!(
                r#"#!/bin/sh
if [ "$1" = --version ]; then echo 'gh test'; exit 0; fi
case "$*" in
  *'--state merged'*) printf '88\tmerged fix\thttps://example.invalid/88\tbug-2451\n';;
  *) {open_reply} ;;
esac
"#
            ),
        );
        ambient.set_key("AIDA_TEST_GH_BINARY", &gh);
        git_ok(&fx.main, &["branch", "bug-2451"]);
        let error = crate::session_start::resolve_session_branch(&fx.main, "bug-2451", "auto")
            .unwrap_err()
            .to_string();
        assert!(error.contains("pass --branch explicitly"), "{error}");
        assert!(!error.contains("--branch-style"), "{error}");
    }
}
