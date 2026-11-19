//! STORY-2905 S1t: one merge hand per PR, claimed on this host.
// trace:STORY-2905 | ai:claude

use super::*;

fn record(pid: u32, start: &str) -> Record {
    Record {
        pr: 7,
        hand: "train".into(),
        pid,
        start: start.into(),
        head: None,
        at: "t".into(),
    }
}

#[test]
fn a_live_holder_blocks_and_only_a_positively_dead_one_is_taken_over() {
    let same = |_: u32| Some("linux-starttime:1".to_string());
    // Nobody holds it, or we already do.
    assert_eq!(blocked_by(None, Hand::Train, 2, "s", &same), None);
    assert_eq!(
        blocked_by(Some(&record(2, "s")), Hand::Train, 2, "s", &same),
        None
    );
    // Another live process holds it.
    let why = blocked_by(
        Some(&record(9, "linux-starttime:1")),
        Hand::Train,
        2,
        "s",
        &same,
    )
    .unwrap();
    assert!(why.contains("held by the train (pid 9)"), "{why}");
    // Its pid now names another process: positively dead.
    let reused = |_: u32| Some("linux-starttime:2".to_string());
    assert_eq!(
        blocked_by(
            Some(&record(9, "linux-starttime:1")),
            Hand::Train,
            2,
            "s",
            &reused
        ),
        None
    );
}

#[test]
fn a_second_hand_is_refused_while_the_first_holds_the_claim() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let first = claim(root, 7, Some("abc"), Hand::Ship).expect("first claim");
    // A claim held by this same process is re-entrant (one process, one
    // hand); a different live process is refused, as the record shows.
    let path = claim_file(root, 7);
    let mut other = read(&path).unwrap().unwrap();
    other.pid = 1; // init: alive, and never this process
    other.start = aida_core::liveness::process_start_identity(1).unwrap_or_default();
    other.hand = "train".into();
    std::fs::write(&path, serde_json::to_vec(&other).unwrap()).unwrap();
    let why = claim(root, 7, Some("abc"), Hand::Ship).unwrap_err();
    assert!(
        why.contains("PR-7 is not merged here") && why.contains("held by the train"),
        "{why}"
    );
    // The first guard no longer owns the file, so dropping it leaves the
    // other hand's claim in place.
    drop(first);
    assert!(path.exists(), "another hand's claim survives our release");
}

#[test]
fn a_released_claim_frees_the_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let guard = claim(root, 8, None, Hand::Drain).expect("claim");
    let path = claim_file(root, 8);
    assert!(path.exists());
    drop(guard);
    assert!(!path.exists(), "released on drop");
    claim(root, 8, None, Hand::Burndown).expect("claimable again");
}

/// The #4270 race: the train has boarded a PR (its claim) while a ship of
/// the same PR reaches the merge boundary: the ship is refused, "held by the
/// train", before anything merges; once the train lets go, the ship's claim
/// is taken.
#[test]
fn a_ship_of_a_pr_the_train_holds_is_refused_at_the_boundary() {
    use crate::landing_merge::{merge, Caller, Entry, MergeError};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let train = claim(root, 7, Some("abc"), Hand::Train).expect("the train boards #7");
    let change = crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "main".into(),
        title: None,
    };
    let opts = crate::forge::MergeOptions {
        method: crate::forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None,
        delete_branch: false,
        match_head: Some("abc".into()),
    };
    let entry = Entry {
        policy: crate::landing_policy::LandingPolicy::default(),
        floor: false,
    };
    let got = merge(
        &crate::forge::fake::RecordingForge::new(),
        root,
        Caller::Ship,
        &entry,
        &change,
        &[],
        &opts,
        &mut crate::network_retry::NoopSink,
    );
    match got {
        Err(MergeError::Refused(kind, why)) => {
            assert_eq!(kind, crate::gate::ship::ShipRefusal::Review);
            assert!(why.contains("held by the train"), "{why}");
        }
        other => panic!("the ship must be refused: {other:?}"),
    }
    drop(train);
    claim(root, 7, Some("abc"), Hand::Ship).expect("free once the train lets go");
}
