//! BUG-2704: a store version committed with git conflict markers (store
//! commit 7124c0f: a contained reviewer session re-applied a stash in the
//! store worktree and its launcher committed the result) no longer makes the
//! mandate cover treat the spec as unreadable. Through the entry `aida pr
//! ship` and the promotion train call ([`mandate_ship_cover_for_ship`]) on
//! real fixture store commits: both sides of the conflicted version are
//! judged, so a conflict whose sides differ only in `modified_at` is covered,
//! one whose stashed side carries an exclusion is refused, and a version
//! unparseable for any other reason still refuses as unreadable history.
// trace:BUG-2704 | ai:claude
use super::bug_2702_promotion_cover_tests::{
    active_mandate, git, plant, Fixture, MANDATE_ID, PROMOTION_TITLE,
};
use super::MandateCover;
use crate::gate::mandate::MandateRefusal;

/// Commit everything staged-or-not in the fixture store, without git's
/// auto-maintenance (the test-git ratchet's guard pair).
fn commit_all(store: &std::path::Path, message: &str) {
    git(store, &["add", "-A"]);
    git(
        store,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "--allow-empty",
            "-qm",
            message,
        ],
    );
}

/// Commit, on the fixture store, a version of STORY-1 whose `modified_at`
/// line is wrapped in the 7124c0f conflict hunk with `stashed` as the
/// "Stashed changes" side, then a clean current version on top (the store
/// as it is now: the conflicted version only in history).
fn commit_conflicted_history(fx: &Fixture, stashed: &str) {
    let store = fx.root.join(".aida-store");
    let rel = aida_core::object_store::relative_object_path("STORY-1").unwrap();
    let path = store.join(&rel);
    let clean = std::fs::read_to_string(&path).unwrap();
    let mut wrapped = false;
    let conflicted: String = clean
        .lines()
        .map(|line| {
            if line.starts_with("modified_at:") && !wrapped {
                wrapped = true;
                format!(
                    "<<<<<<< Updated upstream\n{line}\n=======\n{stashed}\n\
                     >>>>>>> Stashed changes\n"
                )
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    assert!(wrapped, "fixture object has a modified_at line");
    std::fs::write(&path, &conflicted).unwrap();
    commit_all(
        &store,
        "reviewer: commit 2 store changes from the contained session",
    );
    std::fs::write(&path, &clean).unwrap();
    commit_all(&store, "repair");
}

/// Commit a STORY-1 version that is not YAML for a reason other than
/// conflict markers, then the clean current version.
fn commit_unparseable_history(fx: &Fixture) {
    let store = fx.root.join(".aida-store");
    let rel = aida_core::object_store::relative_object_path("STORY-1").unwrap();
    let path = store.join(&rel);
    let clean = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, "title: [never closed\n").unwrap();
    commit_all(&store, "broken");
    std::fs::write(&path, &clean).unwrap();
    commit_all(&store, "repair");
}

/// A fresh promotion fixture (approved head, typed releasable hold) whose
/// STORY-1 history is shaped by `history`, then the cover for promotion 23.
fn promotion_cover(history: impl FnOnce(&Fixture)) -> Result<MandateCover, MandateRefusal> {
    let fx = Fixture::new();
    let store = fx.root.join(".aida-store");
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        &fx.root,
        "advisor",
        &[],
        &[
            ("USER", Some(crate::test_env::HERMETIC_TEST_USER)),
            ("AIDA_TEST_GH_BINARY", Some(fx_gh(&fx).as_str())),
            ("AIDA_CONTAINED", None),
            ("AIDA_SESSION_ID", Some("advisor-2704")),
        ],
    );
    commit_all(&store, "seed fixture seat roster");
    plant(&active_mandate());
    history(&fx);
    fx.approve();
    let hold = crate::gate::merge_hold_core::typed_hold(
        42,
        crate::gate::merge_hold_core::HoldReasonKind::Supervision,
        "operator review",
        None,
    );
    crate::gate::merge_hold_core::place_hold(&fx.root, &hold).unwrap();
    fx.ship_cover("promote/integration-23", PROMOTION_TITLE)
}

fn fx_gh(fx: &Fixture) -> String {
    fx.root.join("gh-stub").to_str().unwrap().to_owned()
}

#[cfg(unix)]
#[test]
fn bug_2704_conflicted_store_version_is_judged_on_both_sides() {
    // The production forge transport and a fresh mandate/exclusion home: run
    // in a PATH-isolated child process.
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let _transport = crate::review_pr_transport_fixture();

    // (a) AC1/AC2: the 7124c0f shape — the sides differ only in
    // `modified_at` — is read, both sides judged, and covered.
    let covered = promotion_cover(|fx| {
        commit_conflicted_history(fx, "modified_at: 2026-11-07T23:59:59Z");
    });
    assert_eq!(
        covered.as_ref().map(|c| c.id.as_str()),
        Ok(MANDATE_ID),
        "{covered:?}"
    );

    // (b) AC1/AC2: the stashed side alone carries `operator-only`; the spec
    // is refused as that side says (never silently included).
    let refused = promotion_cover(|fx| {
        commit_conflicted_history(
            fx,
            "modified_at: 2026-11-07T23:59:59Z\ntags:\n- operator-only",
        );
    });
    assert_eq!(
        refused.as_ref().map(|c| c.id.as_str()),
        Err(&MandateRefusal::AlwaysHuman {
            spec: "STORY-1".into(),
            class: "operator-only".into(),
        }),
    );

    // (c) AC1: a version unparseable for any other reason still refuses as
    // unreadable history (fail closed).
    let unreadable = promotion_cover(commit_unparseable_history);
    assert!(
        matches!(
            &unreadable,
            Err(MandateRefusal::HistoryUnreadable { spec, why })
                if spec == "STORY-1" && why.contains("unparseable version")
        ),
        "{unreadable:?}"
    );
}
