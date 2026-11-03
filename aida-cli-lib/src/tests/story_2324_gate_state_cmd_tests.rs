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
