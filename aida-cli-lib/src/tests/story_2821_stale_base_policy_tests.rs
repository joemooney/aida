// The reviewer stale-base policy is a project setting, `[review] stale_base`:
// the reviewer pre-flight and the review watcher refuse the heads it names
// and no others, and the watcher never asks the forge to merge the base into
// a head the policy lets through.
// trace:STORY-2821 | ai:claude
use super::*;
use crate::pr_rebase::{
    stale_base_block_message, stale_base_launch, stale_base_prompt_note, stale_base_warn_message,
    ConflictPrediction, StaleBaseLaunch, StaleBaseOutcome, StaleBasePolicy,
};
use crate::review_helpers::{parse_stale_base_policy, read_stale_base_policy};
use std::cell::Cell;

fn overlapping(prediction: ConflictPrediction) -> StaleBaseOutcome {
    StaleBaseOutcome::StaleOverlap {
        behind: 3,
        overlap_files: vec!["src/lib.rs".to_string()],
        prediction,
    }
}

fn conflicting() -> StaleBaseOutcome {
    overlapping(ConflictPrediction::Conflicting {
        files: vec!["src/lib.rs".to_string()],
    })
}

fn block() -> String {
    stale_base_block_message(9, 3, &["src/lib.rs".to_string()])
}

// An unset setting is refuse-on-overlap, and that policy is STORY-281's
// pre-flight unchanged: silent when level, the same warning without overlap,
// the same refusal with it (clean, conflicting or unpredicted).
#[test]
fn story2821_default_policy_refuses_overlap_as_before() {
    assert_eq!(
        parse_stale_base_policy("[review]\nmode = \"local\"\n").unwrap(),
        StaleBasePolicy::RefuseOnOverlap
    );
    assert_eq!(
        read_stale_base_policy(tempfile::tempdir().unwrap().path()).unwrap(),
        StaleBasePolicy::RefuseOnOverlap,
        "no config file is the default"
    );
    let policy = StaleBasePolicy::default();
    assert_eq!(
        stale_base_launch(9, policy, &StaleBaseOutcome::Current),
        StaleBaseLaunch::Silent
    );
    assert_eq!(
        stale_base_launch(9, policy, &StaleBaseOutcome::StaleNoOverlap { behind: 3 }),
        StaleBaseLaunch::Warn(stale_base_warn_message(9, 3))
    );
    for outcome in [
        overlapping(ConflictPrediction::Clean),
        overlapping(ConflictPrediction::Unknown),
        conflicting(),
    ] {
        assert_eq!(
            stale_base_launch(9, policy, &outcome),
            StaleBaseLaunch::Refuse(block())
        );
    }
}

// refuse-on-conflict: a stale, overlapping head that still merges cleanly is
// reviewed, with the staleness in the reviewer prompt; a conflicting one is
// refused with the existing message.
#[test]
fn story2821_refuse_on_conflict_reviews_overlap_and_refuses_conflict() {
    let policy =
        parse_stale_base_policy("[review]\nstale_base = \"refuse-on-conflict\"\n").unwrap();
    assert_eq!(policy, StaleBasePolicy::RefuseOnConflict);
    for outcome in [
        overlapping(ConflictPrediction::Clean),
        overlapping(ConflictPrediction::Unknown),
    ] {
        let StaleBaseLaunch::Warn(warning) = stale_base_launch(9, policy, &outcome) else {
            panic!("a non-conflicting overlapping head is reviewed")
        };
        assert!(warning.contains("3 commits behind"), "{warning}");
        assert!(warning.contains("refuse-on-conflict"), "{warning}");
        let note = stale_base_prompt_note(9, &outcome).expect("the prompt carries the note");
        assert!(note.contains("3 commits behind"), "{note}");
        assert!(note.contains("src/lib.rs"), "{note}");
    }
    assert_eq!(
        stale_base_launch(9, policy, &conflicting()),
        StaleBaseLaunch::Refuse(block())
    );
}

// warn refuses nothing. The prompt note does not depend on the policy: every
// stale head gets one, a level head none.
#[test]
fn story2821_warn_refuses_nothing_and_every_stale_head_is_noted() {
    let policy = parse_stale_base_policy("[review]\nstale_base = \"warn\"\n").unwrap();
    assert_eq!(policy, StaleBasePolicy::Warn);
    for outcome in [
        StaleBaseOutcome::StaleNoOverlap { behind: 1 },
        overlapping(ConflictPrediction::Clean),
        conflicting(),
    ] {
        assert!(
            matches!(
                stale_base_launch(9, policy, &outcome),
                StaleBaseLaunch::Warn(_)
            ),
            "{outcome:?}"
        );
        assert!(stale_base_prompt_note(9, &outcome).is_some(), "{outcome:?}");
    }
    let one = stale_base_prompt_note(9, &StaleBaseOutcome::StaleNoOverlap { behind: 1 }).unwrap();
    assert!(one.contains("1 commit behind"), "{one}");
    assert!(stale_base_prompt_note(9, &StaleBaseOutcome::Current).is_none());
}

// An unknown value is a config error naming the allowed values, not the
// default.
#[test]
fn story2821_unknown_value_is_a_config_error_naming_the_allowed_values() {
    for body in [
        "[review]\nstale_base = \"refuse-on-overlaps\"\n",
        "[review]\nstale_base = \"\"\n",
        "[review]\nstale_base = true\n",
    ] {
        let error = format!("{:#}", parse_stale_base_policy(body).unwrap_err());
        for allowed in StaleBasePolicy::ALLOWED {
            assert!(error.contains(allowed), "{body}: {error}");
        }
        assert!(error.contains("stale_base"), "{error}");
    }
    for value in StaleBasePolicy::ALLOWED {
        let policy = parse_stale_base_policy(&format!("[review]\nstale_base = \"{value}\"\n"));
        assert_eq!(policy.unwrap().as_str(), value);
    }
    // The watcher refuses to act on a mistyped policy.
    let (temp, store) = fixture();
    configure(temp.path(), "strict");
    let error = select(&store, &change(1, true, None), None, 2).unwrap_err();
    assert!(
        format!("{error:#}").contains("refuse-on-conflict"),
        "{error:#}"
    );
}

fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path(), "github:github.com/owner/repo").unwrap();
    (temp, store)
}

fn configure(root: &Path, value: &str) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida").join("config.toml"),
        format!("[review]\nstale_base = \"{value}\"\n"),
    )
    .unwrap();
}

fn change(head: u64, overlap: bool, conflict: Option<bool>) -> forge::Change {
    forge::Change {
        id: 7,
        head: format!("{head:040x}"),
        base: "main".into(),
        author: "writer".into(),
        gitlab: false,
        overlap: Some(overlap),
        conflict,
        behind: Some(u64::from(overlap)),
        head_branch: Some("lane-branch".into()),
    }
}

/// The watcher's Notify arm: deliver the stale-head notice and record it sent.
fn notify(root: &Path, store: &Store, c: &forge::Change) {
    let Action::Notify(notice) = select(store, c, None, 2).unwrap() else {
        panic!("a refused stale head is notified first")
    };
    deliver(root, &notice).unwrap();
    store
        .transaction(&c.key(), |state| {
            state
                .entries
                .get_mut(&c.key())
                .unwrap()
                .notices
                .get_mut(&c.head)
                .unwrap()
                .sent = true;
            Ok(())
        })
        .unwrap();
}

fn dead(_: &forge::Change) -> std::result::Result<Option<String>, String> {
    Ok(None)
}

fn never(_: &forge::Change) -> std::result::Result<(), update::UpdateFailure> {
    panic!("this branch must not be updated")
}

// Under refuse-on-conflict or warn, an overlapping head that was notified
// under the default policy and whose author is gone is not branch-updated:
// it is probed and reviewed.
#[test]
fn story2821_watcher_reviews_an_overlapping_head_without_updating_it() {
    for value in ["refuse-on-conflict", "warn"] {
        let (temp, store) = fixture();
        let root = temp.path();
        let stale = change(1, true, Some(false));
        notify(root, &store, &stale);
        configure(root, value);
        for probed in [false, true] {
            assert_eq!(
                stale_follow_up(root, &store, &stale, probed, &dead, &never).unwrap(),
                None,
                "{value}: no follow-up (probed: {probed})"
            );
        }
        assert_eq!(
            probe_unneeded_with_coverage(&store, &stale, || None).unwrap(),
            None,
            "{value}: the sent notice does not settle the head"
        );
        let action = select(&store, &stale, None, 2).unwrap();
        assert!(
            matches!(&action, Action::Dispatch(attempt) if attempt.head == stale.head),
            "{value}: the overlapping head is reviewed: {action:?}"
        );
    }
}

// The default policy keeps BUG-2426's behaviour: the notified overlapping
// head is settled before the probe, by one branch update.
#[test]
fn story2821_default_policy_watcher_still_updates_a_notified_overlapping_head() {
    let (temp, store) = fixture();
    let root = temp.path();
    let stale = change(1, true, Some(false));
    notify(root, &store, &stale);
    let updates = Cell::new(0);
    let forge_ok = |_: &forge::Change| -> std::result::Result<(), update::UpdateFailure> {
        updates.set(updates.get() + 1);
        Ok(())
    };
    let reason = stale_follow_up(root, &store, &stale, false, &dead, &forge_ok)
        .unwrap()
        .expect("the notified head is settled before the probe");
    assert!(reason.starts_with(update::STALE_UPDATED), "{reason}");
    assert_eq!(updates.get(), 1);
    assert_eq!(
        probe_unneeded_with_coverage(&store, &stale, || None)
            .unwrap()
            .as_deref(),
        Some(update::STALE_NOTIFIED)
    );
}

// refuse-on-conflict still holds a conflicting head back: its author is
// notified, and once the author is gone the forge is asked (and, on a real
// conflict, refuses, which routes the change to the advisor). warn reviews it.
#[test]
fn story2821_watcher_holds_a_conflicting_head_only_under_refuse_on_conflict() {
    let (temp, store) = fixture();
    let root = temp.path();
    configure(root, "refuse-on-conflict");
    let conflicted = change(1, true, Some(true));
    assert_eq!(
        stale_follow_up(root, &store, &conflicted, true, &dead, &never).unwrap(),
        None,
        "nothing to follow up before the notice"
    );
    notify(root, &store, &conflicted);
    assert_eq!(
        stale_follow_up(root, &store, &conflicted, false, &dead, &never).unwrap(),
        None,
        "conflict is not known before the probe"
    );
    let refuse = |_: &forge::Change| -> std::result::Result<(), update::UpdateFailure> {
        Err(update::UpdateFailure::refused(
            "update-branch conflicts: merge conflict",
        ))
    };
    let reason = stale_follow_up(root, &store, &conflicted, true, &dead, &refuse)
        .unwrap()
        .expect("the conflicting head is not reviewed");
    assert!(reason.contains("routed to the advisor"), "{reason}");

    let (temp, store) = fixture();
    configure(temp.path(), "warn");
    assert!(matches!(
        select(&store, &conflicted, None, 2).unwrap(),
        Action::Dispatch(_)
    ));
}
