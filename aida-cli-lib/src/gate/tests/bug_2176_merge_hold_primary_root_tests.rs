//! BUG-2176: merge holds resolve against the primary checkout. A hold
//! written from a linked worktree (a headless reviewer in `aida-pr-N`) must be
//! what `aida merge-hold list` and the `aida pr ship` gate read from the
//! primary checkout, and must survive removing that worktree. M1 slice 2: the
//! hold is an event on the primary's gate ledger; no hold state is written in
//! a worktree. Pure git fixtures, no forge.
// trace:BUG-2176 | ai:claude
use super::*;
use crate::gate::ledger::legacy::hold_marker_path as legacy_marker;
use crate::merge_hold::{
    adopt_worktree_hold, stray_marker_warning, stray_markers_json, stray_worktree_markers,
};

fn git(repo: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A primary checkout with one commit and a linked worktree beside it.
fn primary_with_linked() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let primary = base.join("aida");
    std::fs::create_dir_all(&primary).unwrap();
    git(&primary, &["init", "--initial-branch=main", "--quiet"]);
    git(&primary, &["config", "user.email", "t@example.com"]);
    git(&primary, &["config", "user.name", "Test"]);
    std::fs::write(primary.join("README.md"), "init\n").unwrap();
    git(&primary, &["add", "."]);
    git(&primary, &["commit", "-q", "-m", "chore: init"]);
    let linked = base.join("aida-pr-2726");
    git(
        &primary,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "pr-2726",
            linked.to_str().unwrap(),
        ],
    );
    (tmp, primary, linked)
}

fn rework(pr: u64) -> MergeHoldRecord {
    let mut hold = typed_hold(
        pr,
        HoldReasonKind::Rework,
        "CHANGES REQUESTED for BUG-9 at 1111111",
        Some("1111111aaaa".into()),
    );
    hold.spec = Some("BUG-9".into());
    hold
}

#[test]
fn a_hold_written_from_a_linked_worktree_is_read_from_the_primary_checkout() {
    let (_tmp, primary, linked) = primary_with_linked();
    place_hold(&linked, &rework(2726)).unwrap();

    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let ledger = hold_ledger_file(&linked, 2726);
    assert!(
        ledger.starts_with(&primary) && ledger.is_file(),
        "the hold must land on the primary checkout's gate ledger: {}",
        ledger.display()
    );
    assert!(
        !linked.join(".aida").exists(),
        "no hold state may be left in the reviewer's worktree"
    );
    assert!(!legacy_marker(&primary, 2726).exists(), "no legacy marker");
    // `aida merge-hold list` reads `list_holds` from the primary checkout.
    assert_eq!(
        list_holds(&primary)
            .into_iter()
            .map(|(n, _)| n)
            .collect::<Vec<_>>(),
        vec![2726]
    );
    // `aida pr ship` feeds `read_hold_record(&main_worktree, pr)` to its gate.
    let record = read_hold_record(&primary, 2726).expect("ship sees the marker");
    assert_eq!(record.reason_kind, HoldReasonKind::Rework);
    let refusal = crate::gate::ship::ship_hold_gate(Some(&record), true, 2726, |_| {
        RefusalRelease::Held("no APPROVED verdict at the head".into())
    });
    assert!(refusal.is_some_and(|r| r.contains("rework hold")));

    git(
        &primary,
        &["worktree", "remove", "--force", linked.to_str().unwrap()],
    );
    assert!(!linked.exists());
    assert!(
        read_hold_record(&primary, 2726).is_some(),
        "removing the reviewer worktree must not destroy the hold"
    );
    assert!(read_hold(&primary, 2726).is_some());
}

#[test]
fn a_clearance_from_a_linked_worktree_is_recorded_in_the_primary_checkout() {
    let (_tmp, primary, linked) = primary_with_linked();
    place_hold(&primary, &rework(7)).unwrap();
    assert!(read_hold_record(&linked, 7).is_some());
    release_hold(&linked, &rework(7), &PrincipalIdentity::human("joe"), None).unwrap();
    assert!(read_hold_record(&primary, 7).is_none());
    assert!(recorded_clearance(&primary, 7).is_some());
    assert_eq!(hold_ledger_file(&linked, 7), hold_ledger_file(&primary, 7));
    assert!(!linked.join(".aida").exists());
}

#[test]
fn a_contained_writer_is_refused_and_leaves_nothing_in_its_worktree() {
    // trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
    let (_tmp, primary, linked) = primary_with_linked();
    {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", Some("1"))]);
        let err = place_hold(&linked, &rework(2726)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }
    assert!(!linked.join(".aida").exists(), "no local fallback marker");
    assert!(read_hold_record(&primary, 2726).is_none());
}

#[test]
fn a_legacy_worktree_marker_is_reported_then_collected_by_the_launcher() {
    let (_tmp, primary, linked) = primary_with_linked();
    let since = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
    // A marker a pre-ledger reviewer left in its worktree.
    let local = legacy_marker(&linked, 2726);
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, serde_json::to_vec_pretty(&rework(2726)).unwrap()).unwrap();

    // The stray is reported where the gates cannot see it.
    let strays = stray_worktree_markers(&primary);
    assert_eq!(strays.len(), 1);
    assert_eq!(strays[0].0, 2726);
    let line = stray_marker_warning(2726, &strays[0].1, &primary, true);
    assert!(line.contains("held by the label alone"), "{line}");

    // The launcher, unconstrained, collects it onto the primary's ledger.
    assert!(adopt_worktree_hold(&primary, &linked, 2726, since).unwrap());
    assert!(!local.exists());
    let adopted = read_hold_record(&primary, 2726).expect("adopted hold");
    assert_eq!(adopted.reason_kind, HoldReasonKind::Rework);
    assert_eq!(adopted.spec.as_deref(), Some("BUG-9"));
    assert!(stray_worktree_markers(&primary).is_empty());
}

#[test]
fn adoption_skips_a_marker_older_than_the_review_run() {
    let (_tmp, primary, linked) = primary_with_linked();
    std::fs::create_dir_all(legacy_marker(&linked, 5).parent().unwrap()).unwrap();
    std::fs::write(
        legacy_marker(&linked, 5),
        serde_json::to_vec_pretty(&rework(5)).unwrap(),
    )
    .unwrap();
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    assert!(!adopt_worktree_hold(&primary, &linked, 5, later).unwrap());
    assert!(read_hold_record(&primary, 5).is_none());
    assert!(legacy_marker(&linked, 5).is_file());
}

#[test]
fn outside_a_repository_the_hold_stays_under_the_given_root() {
    let dir = tempfile::tempdir().unwrap();
    place_hold(dir.path(), &rework(3)).unwrap();
    assert!(hold_ledger_file(dir.path(), 3).starts_with(dir.path()));
    assert!(hold_ledger_file(dir.path(), 3).is_file());
    assert_eq!(hold_store_root(dir.path()), dir.path());
}

#[test]
fn many_hold_path_calls_resolve_the_primary_checkout_at_most_once() {
    let (_tmp, primary, linked) = primary_with_linked();
    STORE_RESOLUTIONS.with(|n| n.set(0));
    for pr in 0..50 {
        assert_eq!(hold_path(&linked, pr), legacy_marker(&primary, pr));
        assert_eq!(
            clearance_path(&linked, pr),
            crate::gate::ledger::legacy::clearance_file_path(&primary, pr)
        );
    }
    let misses: Vec<_> = (0..20).map(|pr| read_hold(&linked, pr)).collect();
    assert!(misses.iter().all(Option::is_none));
    assert!(
        STORE_RESOLUTIONS.with(|n| n.get()) <= 1,
        "the store root must be memoised, not re-resolved per call"
    );
}

#[test]
fn the_store_resolver_never_spawns_git() {
    let src = include_str!("../merge_hold_core.rs");
    let start = src.find("pub(crate) fn hold_store_root(").unwrap();
    let end = start + src[start..].find("\npub(crate) fn hold_is_open(").unwrap();
    let body = &src[start..end];
    assert!(
        !body.contains("Command::new"),
        "no subprocess on the hold path"
    );
    assert!(
        !body.contains("main_worktree_root_from"),
        "that resolver spawns git"
    );
}

#[test]
fn list_json_reports_stray_markers_with_their_label_only_state() {
    let strays = vec![
        (
            2726,
            PathBuf::from("/w/aida-pr-2726/.aida/merge-holds/PR-2726"),
        ),
        (
            2727,
            PathBuf::from("/w/aida-pr-2727/.aida/merge-holds/PR-2727"),
        ),
    ];
    let json = stray_markers_json(&strays, &[2726]);
    assert_eq!(
        json,
        serde_json::json!([
            {"pr": 2726, "path": "/w/aida-pr-2726/.aida/merge-holds/PR-2726", "label_only": true},
            {"pr": 2727, "path": "/w/aida-pr-2727/.aida/merge-holds/PR-2727", "label_only": false},
        ])
    );
    // The list command emits exactly these rows, in text and JSON alike.
    let cmd = include_str!("../../merge_hold_cmd.rs");
    assert!(cmd.contains("let strays = merge_hold::stray_worktree_markers(&root);"));
    assert!(cmd.contains(
        "\"stray_worktree_markers\": merge_hold::stray_markers_json(&strays, &label_only)"
    ));
    assert!(
        cmd.contains("merge_hold::stray_marker_warning(*pr, path, &root, label_only.contains(pr))")
    );
}

#[test]
fn the_standalone_reviewer_adopts_its_marker_after_collecting_the_verdict() {
    let src = include_str!("../../queue_cmd.rs");
    let collect = src
        .find("crate::gate::review_verdict::collect_standalone_verdict(")
        .expect("verdict collection");
    let adopt = src
        .find("crate::merge_hold::adopt_worktree_hold(project_root, worktree, pr, launched_at)")
        .expect("the launcher must adopt the reviewer's hold marker");
    let exit = src[adopt..]
        .find("std::process::exit(code)")
        .map(|i| adopt + i)
        .expect("exit propagation");
    assert!(
        collect < adopt && adopt < exit,
        "adoption must run after collection and before the exit-code early return"
    );
}

/// TASK-1406: a hold placed from a linked worktree records its placement
/// event in the primary's events log, so deleting the primary's marker by
/// hand is detected there as an unrecorded removal (BUG-1693).
// trace:TASK-1951 trace:TASK-1406 | ai:claude
#[test]
fn a_linked_worktree_placement_is_audited_from_the_primary_checkout() {
    let (_tmp, primary, linked) = primary_with_linked();
    place_hold(&linked, &rework(2727)).unwrap();
    std::fs::remove_file(hold_ledger_file(&primary, 2727)).unwrap();
    assert_eq!(unrecorded_marker_removals(&primary), vec![2727]);
    assert_eq!(unrecorded_marker_removals(&linked), vec![2727]);
}

/// TASK-2153 / M1 slice 2: a collected legacy worktree marker is placed on
/// the primary's ledger with a placement event in the primary's log, so a
/// hand-deleted ledger is detected from the primary.
// trace:TASK-1951 trace:TASK-2153 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude
#[test]
fn a_collected_legacy_marker_is_audited_from_the_primary() {
    let (_tmp, primary, linked) = primary_with_linked();
    let since = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
    let local = legacy_marker(&linked, 2728);
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, serde_json::to_vec_pretty(&rework(2728)).unwrap()).unwrap();
    assert!(adopt_worktree_hold(&primary, &linked, 2728, since).unwrap());
    std::fs::remove_file(hold_ledger_file(&primary, 2728)).unwrap();
    assert_eq!(unrecorded_marker_removals(&primary), vec![2728]);
}
