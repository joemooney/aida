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
    git_ok(&old, &["commit", "-qm", "original fix (BUG-2451)"]);
    let head = git_ok(&old, &["rev-parse", "HEAD"]);
    git_ok(&old, &["push", "-q", "origin", "HEAD:original-pr"]);
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
fn bug_2451_merged_branch_refusal_names_pr_and_explicit_recovery() {
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
    let error = start("BUG-2451", &fx.root.join("new-seat"), false, None)
        .err()
        .expect("retained branch requires explicit naming")
        .to_string();
    assert!(error.contains("merged PR #88"), "{error}");
    assert!(error.contains("--branch-style date"), "{error}");
    assert!(error.contains("--branch explicitly"), "{error}");
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
    let error = driver.implementer_workspace().unwrap_err();
    assert!(error.contains("merged PR #88"), "{error}");
    assert!(
        error.contains("aida session start --owns BUG-2451 --branch-style date"),
        "{error}"
    );
    // Phase 1 honors the deliberate recovery lease instead of resolving auto again.
    let seated = fx.root.join("dated-seat");
    let mut receipt = session_start_with_receipt(
        "BUG-2451",
        None,
        None,
        false,
        seated.to_str(),
        None,
        "date",
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
        false,
        false,
        PathReentry::None,
        None,
    )
    .unwrap();
    receipt.disarm();
    let (path, branch) = driver.implementer_workspace().unwrap().unwrap();
    assert_eq!(
        Path::new(&path).canonicalize().unwrap(),
        seated.canonicalize().unwrap()
    );
    assert!(branch.starts_with("bug-2451-"), "{branch}");
}
