// trace:BUG-2570 | ai:claude
//! Two open changes that ship one spec each keep their own verdict.
use crate::gate::review_verdict as rv;
use crate::gate::{ledger, ship};
use std::path::Path;

const SPEC: &str = "STORY-9";
const FIRST: u64 = 71;
const SECOND: u64 = 72;

fn head_of(pr: u64) -> String {
    if pr == FIRST { "a" } else { "b" }.repeat(40)
}

/// Record `verdict` for `pr` at its head under the shared spec, as `aida
/// review record STORY-9 --pr <pr>` does, and leave the spec's file holding
/// that verdict: the file keeps only the last one written.
fn record(root: &Path, pr: u64, verdict: &str, minute: u32) {
    let record = serde_json::json!({"verdict": verdict, "reviewed_sha": head_of(pr),
        "recorded_by": "reviewer-a",
        "recorded_at": format!("2026-11-13T01:{minute:02}:00Z")});
    ledger::record_verdict_at(
        &rv::verdict_path(root, &format!("PR-{pr}")),
        record.as_object().unwrap(),
        Some(&crate::gate::canon::GateSpec::with_key(SPEC, None)),
    )
    .unwrap();
    let file = rv::verdict_path(root, SPEC);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, record.to_string()).unwrap();
}

fn head_refusal(root: &Path, pr: u64) -> Option<ship::ApprovalHeadRefusal> {
    let head = head_of(pr);
    let candidates =
        ship::merge_gate_verdict_candidates(&[root], pr, &[SPEC.to_string()], Some(&head));
    assert!(!candidates.is_empty(), "PR-{pr} has a recorded approval");
    ship::approval_head_refusal(&candidates, Some(&head))
}

// #3972 and #3973 (both STORY-2768): the second approval replaced the first
// in the spec's file, and the first change then read as approved at the
// other change's head.
#[test]
fn each_change_is_approved_at_its_own_head_whichever_was_approved_last() {
    for order in [[FIRST, SECOND], [SECOND, FIRST]] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (minute, pr) in order.into_iter().enumerate() {
            record(root, pr, "approved", minute as u32);
        }
        for pr in order {
            assert_eq!(
                head_refusal(root, pr),
                None,
                "PR-{pr}, approved in the order {order:?}"
            );
            let verdict =
                rv::read_recorded_verdict_for_change(root, SPEC, pr, Some(&head_of(pr))).unwrap();
            assert_eq!(verdict.reviewed_sha.as_deref(), Some(head_of(pr).as_str()));
        }
    }
}

// One change refused and the other approved afterwards: the refused change
// is not read as approved, and the approved one is not held by the refusal.
#[test]
fn a_refusal_stays_with_its_own_change() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    record(root, FIRST, "request-changes", 0);
    record(root, SECOND, "approved", 1);
    assert_eq!(head_refusal(root, SECOND), None);
    let first = rv::read_recorded_verdict_for_change(root, SPEC, FIRST, Some(&head_of(FIRST)));
    assert!(!first.unwrap().kind.approves());
    let candidates = ship::merge_gate_verdict_candidates(
        &[root],
        FIRST,
        &[SPEC.to_string()],
        Some(&head_of(FIRST)),
    );
    assert!(
        candidates.iter().all(|verdict| !verdict.kind.approves()),
        "the other change's approval is not this change's: {candidates:?}"
    );
}

// A change with no verdict of its own still reads the spec's record, and a
// verdict another change recorded at this very head still counts.
#[test]
fn the_spec_record_answers_for_a_change_with_no_verdict_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    record(root, FIRST, "request-changes", 0);
    let unreviewed = 73;
    let at_first_head = ship::merge_gate_verdict_candidates(
        &[root],
        unreviewed,
        &[SPEC.to_string()],
        Some(&head_of(FIRST)),
    );
    assert!(
        at_first_head.iter().any(|v| v.kind.requires_changes()),
        "{at_first_head:?}"
    );
}
