//! STORY-2905 S1t: one merge hand per PR, claimed on this host.
// trace:STORY-2905 | ai:claude

use super::*;

fn record(hand: &str) -> Record {
    Record {
        pr: 7,
        hand: hand.into(),
        holder: aida_core::proc_identity::HolderRecord::refused(
            aida_core::proc_identity::HolderRefusal::EvidenceUnavailable("fixture".into()),
        ),
        head: None,
        at: "t".into(),
    }
}

fn ours() -> aida_core::proc_identity::ProcIdentity {
    aida_core::proc_identity::HolderRecord::capture(std::process::id())
        .validate()
        .expect("this test process has an identity")
        .clone()
}

#[test]
fn a_live_holder_blocks_and_only_a_positively_dead_one_is_taken_over() {
    let me = ours();
    assert_eq!(blocked_by(None, Hand::Ship, &me, &|_| Some(true)), None);
    // Another live holder blocks.
    let why = blocked_by(Some(&record("train")), Hand::Ship, &me, &|_| Some(true)).unwrap();
    assert!(why.contains("held by the train"), "{why}");
    // A positively dead holder is taken over.
    assert_eq!(
        blocked_by(Some(&record("train")), Hand::Ship, &me, &|_| Some(false)),
        None
    );
    // A holder that cannot be judged refuses.
    let why = blocked_by(Some(&record("train")), Hand::Ship, &me, &|_| None).unwrap();
    assert!(why.contains("liveness cannot be checked"), "{why}");
}

#[test]
fn one_process_is_still_two_hands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let first = claim(root, 7, Some("abc"), Hand::Ship).expect("first claim");
    // The same hand again is re-entrant; another hand is refused, even in
    // this same process.
    claim(root, 7, Some("abc"), Hand::Ship).expect("re-entrant for the same hand");
    let why = claim(root, 7, Some("abc"), Hand::Train).unwrap_err();
    assert!(
        why.contains("PR-7 is not merged here") && why.contains("held by the ship"),
        "{why}"
    );
    drop(first);
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
