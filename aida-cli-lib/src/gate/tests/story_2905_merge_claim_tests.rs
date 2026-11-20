//! STORY-2905 S1t: one merge hand per PR, claimed on this host.
// trace:STORY-2905 | ai:claude

use super::*;
use std::path::Path;

/// No train has boarded anything in these fixtures; the journal-pin case,
/// which reads the real train journal, is mounted from `pr_train`.
fn no_pins(_: &Path, _: u64) -> anyhow::Result<Vec<(String, String)>> {
    Ok(Vec::new())
}

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
    let first = claim(root, 7, Some("abc"), Hand::Ship, Some(&no_pins)).expect("first claim");
    // The same hand again is re-entrant; another hand is refused, even in
    // this same process.
    claim(root, 7, Some("abc"), Hand::Ship, Some(&no_pins)).expect("re-entrant for the same hand");
    let why = claim(root, 7, Some("abc"), Hand::Train, None).unwrap_err();
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
    let guard = claim(root, 8, None, Hand::Drain, Some(&no_pins)).expect("claim");
    let path = claim_file(root, 8);
    assert!(path.exists());
    drop(guard);
    assert!(!path.exists(), "released on drop");
    claim(root, 8, None, Hand::Burndown, Some(&no_pins)).expect("claimable again");
}

/// The #4270 race: the train has boarded a PR (its claim) while a ship of
/// the same PR reaches the merge boundary: the ship is refused, "held by the
/// train", before anything merges; once the train lets go, the ship's claim
/// is taken.
#[test]
fn a_ship_of_a_pr_the_train_holds_is_refused_at_the_boundary() {
    use crate::gate::landing_merge::{merge, Caller, Entry, MergeError};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let train = claim(root, 7, Some("abc"), Hand::Train, None).expect("the train boards #7");
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
        &no_pins,
    );
    match got {
        Err(MergeError::Refused(kind, why)) => {
            assert_eq!(kind, crate::gate::ship::ShipRefusal::Review);
            assert!(why.contains("held by the train"), "{why}");
        }
        other => panic!("the ship must be refused: {other:?}"),
    }
    drop(train);
    claim(root, 7, Some("abc"), Hand::Ship, Some(&no_pins)).expect("free once the train lets go");
}

/// Round 1 finding 5: the claim records the authority it was taken under
/// (here no grant and no terminal: `none`), and a re-entrant claim keeps the
/// first record.
#[test]
fn a_claim_records_its_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let _guard = claim(root, 9, Some("abc"), Hand::Ship, Some(&no_pins)).expect("claim");
    let text = std::fs::read_to_string(claim_file(root, 9)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let authority = json["authority"].as_str().unwrap();
    assert!(
        authority == "none" || authority == "operator" || authority.starts_with("grant:"),
        "{authority}"
    );
    assert_eq!(json["hand"], "ship");
    assert_eq!(json["head"], "abc");
    let _again = claim(root, 9, Some("abc"), Hand::Ship, Some(&no_pins)).expect("re-entrant");
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
    claim(root, 11, None, Hand::Ship, Some(&no_pins)).expect("a dead holder's claim is taken over");
    // Unknown: recorded on another node.
    persisted_claim(root, 12, "drain", |v| {
        v["holder"]["node_id"] = "another-node".into()
    });
    let why = claim(root, 12, None, Hand::Ship, Some(&no_pins)).unwrap_err();
    assert!(why.contains("liveness cannot be checked"), "{why}");
    // Malformed.
    std::fs::write(claim_file(root, 13), "{not json").unwrap();
    let why = claim(root, 13, None, Hand::Ship, Some(&no_pins)).unwrap_err();
    assert!(why.contains("its claim cannot be read"), "{why}");
}

/// Round 1 findings 1 and 4, through the functions the train handler calls:
/// boarding leaves off a car another hand holds; recovery refuses the whole
/// retained train when any car is held elsewhere.
#[test]
fn boarding_and_recovery_respect_other_hands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let ship = claim(root, 21, Some("h21"), Hand::Ship, Some(&no_pins)).expect("a ship holds #21");
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
    let src = include_str!("../../pr_train.rs");
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

fn git_in(dir: &Path, args: &[&str]) {
    let out = crate::test_git::git_command()
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A ci-gated repo with `feat` served from a local origin.
fn served_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q", "-b", "main"]);
    git_in(&repo, &["config", "user.email", "t@example.com"]);
    git_in(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "base"]);
    git_in(&repo, &["branch", "feat"]);
    let origin = root.join("origin.git");
    git_in(
        &root,
        &["clone", "-q", "--bare", "repo", origin.to_str().unwrap()],
    );
    git_in(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    (temp, repo)
}

fn change7() -> crate::forge::ChangeRef {
    crate::forge::ChangeRef {
        id: 7,
        url: String::new(),
        branch: "feat".into(),
        base: "main".into(),
        title: None,
    }
}

fn opts7() -> crate::forge::MergeOptions {
    crate::forge::MergeOptions {
        method: crate::forge::MergeMethod::Squash,
        squash_subject: None,
        squash_body: None,
        delete_branch: false,
        match_head: None,
    }
}

/// Round 2 finding 3, the #4270 race through the production merge
/// boundary: two merge hands (a ship and a drain) reach the boundary for
/// the same PR at once, against a forge that holds each merge open.
/// Exactly one merge reaches the forge; the other is refused with the
/// review exit category (23). Without the claim both would merge.
#[test]
fn two_hands_racing_for_one_pr_merge_it_exactly_once() {
    use crate::gate::landing_merge::{merge, Caller, Entry, MergeError};
    let (_t, repo) = served_repo();
    let merges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let forge = crate::forge::fake::RecordingForge {
        merges: Some(merges.clone()),
        merge_hold: std::time::Duration::from_millis(400),
        ..crate::forge::fake::RecordingForge::new()
    };
    let entry = Entry {
        policy: crate::landing_policy::LandingPolicy::default(),
        floor: false,
    };
    let start = std::sync::Barrier::new(2);
    let results: Vec<Result<(), MergeError>> = std::thread::scope(|s| {
        let racers: Vec<_> = [Caller::Ship, Caller::Drain]
            .into_iter()
            .map(|caller| {
                let (forge, repo, entry, start) = (&forge, &repo, &entry, &start);
                s.spawn(move || {
                    start.wait();
                    merge(
                        forge,
                        repo,
                        caller,
                        entry,
                        &change7(),
                        &[],
                        &opts7(),
                        &mut crate::network_retry::NoopSink,
                        &no_pins,
                    )
                    .map(|_| ())
                })
            })
            .collect();
        racers.into_iter().map(|r| r.join().unwrap()).collect()
    });
    assert_eq!(
        merges.lock().unwrap().as_slice(),
        &[7],
        "exactly one merge reached the forge"
    );
    let refused: Vec<_> = results.into_iter().filter_map(Result::err).collect();
    assert_eq!(refused.len(), 1, "exactly one hand lost: {refused:?}");
    let loser = refused.into_iter().next().unwrap();
    assert!(loser.to_string().contains("held by the"), "{loser}");
    assert_eq!(crate::exit_code_for_error(&loser.into_anyhow()), 23);
}

/// The same race against the train: a car the train boarded (through the
/// handler's own `board`) is not merged by a ship until the train lets go.
#[test]
fn a_ship_waits_out_a_train_that_boarded_the_pr() {
    use crate::gate::landing_merge::{merge, Caller, Entry, MergeError};
    let (_t, repo) = served_repo();
    let merges = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let forge = crate::forge::fake::RecordingForge {
        merges: Some(merges.clone()),
        ..crate::forge::fake::RecordingForge::new()
    };
    let entry = Entry {
        policy: crate::landing_policy::LandingPolicy::default(),
        floor: false,
    };
    let (guards, boarded, _) = board(&repo, vec![(7u64, "h")], |m| (m.0, m.1.to_string()));
    assert_eq!(boarded.len(), 1);
    let ship = |forge: &crate::forge::fake::RecordingForge| {
        merge(
            forge,
            &repo,
            Caller::Ship,
            &entry,
            &change7(),
            &[],
            &opts7(),
            &mut crate::network_retry::NoopSink,
            &no_pins,
        )
    };
    match ship(&forge) {
        Err(MergeError::Refused(kind, why)) => {
            assert_eq!(kind, crate::gate::ship::ShipRefusal::Review);
            assert!(why.contains("held by the train"), "{why}");
        }
        other => panic!("the ship must be refused: {other:?}"),
    }
    assert!(
        merges.lock().unwrap().is_empty(),
        "nothing merged while the train held it"
    );
    drop(guards);
    ship(&forge).expect("the ship merges once the train lets go");
    assert_eq!(merges.lock().unwrap().as_slice(), &[7]);
}
