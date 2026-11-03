//! STORY-2324 PR-A rework item 3: `--compare` models today's merge gate
//! exactly — the TASK-1960 carry-over is applied, and an open refusal
//! outranks a stale approval. One fixture per case.
// trace:STORY-2324 | ai:claude

use super::*;
use crate::gate::review_verdict::{self as rv, JsonObj, RecordedVerdict};
use crate::gate::ship::ApprovalHeadRefusal;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OLD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn verdict(word: &str, sha: &str, recorded_at: &str) -> RecordedVerdict {
    let mut r = JsonObj::new();
    r.insert("verdict".into(), word.into());
    r.insert("reviewed_sha".into(), sha.into());
    r.insert("recorded_at".into(), recorded_at.into());
    r.insert("recorded_by".into(), "reviewer seat".into());
    rv::parse_ledger_record(&r).expect("fixture parses")
}

fn approved_at(sha: &str) -> RecordedVerdict {
    verdict("approved", sha, "2026-11-02T10:00:00+00:00")
}

/// A carry-over read that must not be consulted.
fn no_carry(_: &ApprovalHeadRefusal) -> CarryAnswer {
    panic!("the carry-over is consulted only for a head refusal")
}

#[test]
fn an_approval_at_head_is_approved_without_a_carry_over() {
    assert_eq!(
        today_verdict(&[approved_at(HEAD)], HEAD, no_carry),
        "approved"
    );
}

#[test]
fn a_moved_head_with_a_carry_over_proof_is_approved() {
    let mut asked = None;
    let v = today_verdict(&[approved_at(OLD)], HEAD, |r| {
        asked = Some(r.clone());
        Some(true)
    });
    assert_eq!(v, "approved");
    assert!(matches!(
        asked,
        Some(ApprovalHeadRefusal::Moved { ref reviewed_sha, ref head_sha })
            if reviewed_sha == OLD && head_sha == HEAD
    ));
}

#[test]
fn a_moved_head_whose_carry_over_is_refused_is_stale() {
    assert_eq!(
        today_verdict(&[approved_at(OLD)], HEAD, |_| Some(false)),
        "stale"
    );
}

#[test]
fn a_moved_head_with_no_carry_over_answer_is_stale() {
    assert_eq!(today_verdict(&[approved_at(OLD)], HEAD, |_| None), "stale");
}

#[test]
fn an_open_refusal_outranks_a_stale_approval() {
    let candidates = [
        approved_at(OLD),
        verdict("changes_requested", OLD, "2026-11-02T11:00:00+00:00"),
    ];
    // Even a carry-over that would apply does not answer an open refusal.
    assert_eq!(
        today_verdict(&candidates, HEAD, no_carry),
        "changes-requested"
    );
}

#[test]
fn an_open_refusal_outranks_an_approval_at_head() {
    let candidates = [
        approved_at(HEAD),
        verdict("rejected", HEAD, "2026-11-02T11:00:00+00:00"),
    ];
    assert_eq!(
        today_verdict(&candidates, HEAD, no_carry),
        "changes-requested"
    );
}

#[test]
fn no_candidate_is_absent() {
    assert_eq!(today_verdict(&[], HEAD, no_carry), "absent");
}

// Round-2 residue (A2 instrument): an implementer-seat approval is today's
// authority refusal, its own category, not `stale`; draft-only compares as
// a set, not by emptiness.
// trace:STORY-2324 | ai:claude

#[test]
fn an_implementer_seat_approval_is_unauthorized_not_stale() {
    let mut r = JsonObj::new();
    r.insert("verdict".into(), "approved".into());
    r.insert("reviewed_sha".into(), HEAD.into());
    r.insert("recorded_at".into(), "2026-11-02T10:00:00+00:00".into());
    r.insert("recorded_by".into(), "claude implementer seat".into());
    let approval = rv::parse_ledger_record(&r).expect("fixture parses");
    assert_eq!(today_verdict(&[approval], HEAD, no_carry), "unauthorized");
}

fn state_with_draft_only(draft_only: &[&str]) -> GateState {
    GateState {
        head: HEAD.into(),
        members: Vec::new(),
        change_verdict: Verdict::Absent,
        verdict: Verdict::Absent,
        hold: Hold::None,
        mode: Mode::Drain,
        draft_only: draft_only.iter().map(|s| s.to_string()).collect(),
        foreign_labels: Vec::new(),
        foreign_verdict: Verdict::Absent,
        decision: Decision::Refused {
            refusals: Vec::new(),
        },
    }
}

fn today_with_draft_only(draft_only: &[&str]) -> Today {
    Today {
        verdict: "absent".into(),
        hold_open: false,
        supervised: Vec::new(),
        draft_only: draft_only.iter().map(|s| s.to_string()).collect(),
    }
}

fn draft_only_differs(today: &[&str], canonical: &[&str]) -> bool {
    compare(
        &today_with_draft_only(today),
        &state_with_draft_only(canonical),
    )
    .iter()
    .find(|c| c.column == "draft-only")
    .expect("draft-only column")
    .differs
}

#[test]
fn draft_only_compares_the_member_set_not_its_emptiness() {
    assert!(draft_only_differs(&["STORY-1"], &["STORY-2"]));
    assert!(draft_only_differs(&["STORY-1"], &["STORY-1", "STORY-2"]));
    assert!(!draft_only_differs(
        &["story-1", "STORY-2"],
        &["STORY-2", "STORY-1"]
    ));
    assert!(!draft_only_differs(&[], &[]));
}
