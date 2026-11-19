//! STORY-2905 S1t: one merge hand per PR, claimed on this host.
// trace:STORY-2905 | ai:claude

use super::*;
use std::path::Path;

fn record(hand: &str) -> Record {
    Record {
        pr: 7,
        hand: hand.into(),
        holder: aida_core::proc_identity::HolderRecord::refused(
            aida_core::proc_identity::HolderRefusal::EvidenceUnavailable("fixture".into()),
        ),
        authority: "none".into(),
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

/// Round 1 finding 5: the claim records the authority it was taken under
/// (here no grant and no terminal: `none`), and a re-entrant claim keeps the
/// first record.
#[test]
fn a_claim_records_its_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _guard = claim(root, 9, Some("abc"), Hand::Ship).expect("claim");
    let text = std::fs::read_to_string(claim_file(root, 9)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let authority = json["authority"].as_str().unwrap();
    assert!(
        authority == "none" || authority == "operator" || authority.starts_with("grant:"),
        "{authority}"
    );
    assert_eq!(json["hand"], "ship");
    assert_eq!(json["head"], "abc");
    let _again = claim(root, 9, Some("abc"), Hand::Ship).expect("re-entrant");
    assert_eq!(std::fs::read_to_string(claim_file(root, 9)).unwrap(), text);
}

/// Write a claim for PR `pr` held by this process's holder record, edited
/// by `edit` (to make the holder dead, foreign, or the file malformed).
fn persisted_claim(root: &Path, pr: u64, hand: &str, edit: impl Fn(&mut serde_json::Value)) {
    let holder = aida_core::proc_identity::HolderRecord::capture(std::process::id());
    let mut value = serde_json::to_value(Record {
        pr,
        hand: hand.into(),
        holder,
        authority: "none".into(),
        head: None,
        at: "earlier".into(),
    })
    .unwrap();
    edit(&mut value);
    std::fs::create_dir_all(claims_dir(root)).unwrap();
    std::fs::write(claim_file(root, pr), value.to_string()).unwrap();
}

/// Round 1 finding 4: persisted claims are judged through the shared
/// observation: a holder whose process is gone is taken over; a holder on
/// another node (unjudgeable here) and a malformed claim both refuse.
#[test]
fn persisted_dead_unknown_and_malformed_claims() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Dead: the recorded pid names no process.
    persisted_claim(root, 11, "drain", |v| {
        v["holder"]["identity"]["pid"] = 4_194_300.into()
    });
    claim(root, 11, None, Hand::Ship).expect("a dead holder's claim is taken over");
    // Unknown: recorded on another node.
    persisted_claim(root, 12, "drain", |v| {
        v["holder"]["node_id"] = "another-node".into()
    });
    let why = claim(root, 12, None, Hand::Ship).unwrap_err();
    assert!(why.contains("liveness cannot be checked"), "{why}");
    // Malformed.
    std::fs::write(claim_file(root, 13), "{not json").unwrap();
    let why = claim(root, 13, None, Hand::Ship).unwrap_err();
    assert!(why.contains("its claim cannot be read"), "{why}");
}

/// A PR a train has boarded (its journal pin) cannot be claimed by any
/// other hand; the train itself may.
#[test]
fn a_train_pin_refuses_other_hands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let journal = root.join(".aida/train");
    std::fs::create_dir_all(&journal).unwrap();
    let entry = serde_json::json!({
        "version": 1, "branch": "train/example", "change_id": 8,
        "step": "Ci", "completed_members": [], "rejected": [], "call": "",
        "train": { "base": "base", "tip": "tip", "cars": [{
            "member": { "pr": 7, "head": "h", "subject": "s", "body": "" },
            "commit": "c", "patch_id": "p"
        }], "dropped": [] }
    });
    std::fs::write(journal.join("example.json"), entry.to_string()).unwrap();
    let why = claim(root, 7, Some("h"), Hand::Ship).unwrap_err();
    assert!(why.contains("boarded on train/example"), "{why}");
    claim(root, 7, Some("h"), Hand::Train).expect("the train holds its own car");
}

/// Round 1 findings 1 and 4, through the functions the train handler calls:
/// boarding leaves off a car another hand holds; recovery refuses the whole
/// retained train when any car is held elsewhere.
#[test]
fn boarding_and_recovery_respect_other_hands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let ship = claim(root, 21, Some("h21"), Hand::Ship).expect("a ship holds #21");
    let (guards, boarded, refused) = board(root, vec![(20u64, "h20"), (21, "h21")], |m| {
        (m.0, m.1.to_string())
    });
    assert_eq!(boarded, vec![(20, "h20")]);
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0].0, 21);
    assert!(
        refused[0].1.contains("held by the ship"),
        "{}",
        refused[0].1
    );
    drop(guards);
    let why = claim_all(root, &[20, 21]).unwrap_err();
    assert!(
        why.contains("PR-21") && why.contains("held by the ship"),
        "{why}"
    );
    drop(ship);
    let held = claim_all(root, &[20, 21]).expect("free once the ship lets go");
    assert_eq!(held.len(), 2);
}

/// The train handler claims through these functions before it drives a
/// fresh train and before it resumes a retained one (removing either call
/// fails this test).
#[test]
fn the_train_handler_claims_before_driving_and_resuming() {
    let src = include_str!("../pr_train.rs");
    let handler = src
        .split("pub(crate) fn pr_train_handler_with_dispatch_ctx(")
        .nth(1)
        .expect("handler");
    let claim_all = handler
        .find(concat!("merge_claim::", "claim_all("))
        .expect("recovery claims its cars");
    let resume = handler.find(".resume_for(").expect("recovery resumes");
    assert!(claim_all < resume, "claims precede the resume");
    let board = handler
        .find(concat!("merge_claim::", "board("))
        .expect("boarding claims its cars");
    let drive_live = handler
        .rfind("drive(&mut rails, members)")
        .expect("the live drive");
    assert!(board < drive_live, "claims precede the live drive");
}
