//! BUG-b6b1278890774ef993d2ebc327862470-006: since the WORK migration every
//! new spec gets a node-local id (`TASK-<32-hex node>-NNN`). The shared
//! spec-id shape rule capped tokens at 40 chars, so a 41+ char node-local
//! trailer was dropped and `aida pr auto-queue-review` reported "carries no
//! `(REQ-ID)` trailers". These pin every trailer surface on real long ids.
// trace:BUG-b6b1278890774ef993d2ebc327862470-006 | ai:claude

use crate::pr_ship::{
    derive_squash_subject_spec_ids, extract_spec_ids_from_text,
    extract_trailing_refs_spec_ids_from_subject, extract_trailing_spec_ids_from_subject,
};
use crate::review_gates::review_coverage_from_commits;
use crate::{
    extract_all_spec_ids, extract_spec_ids_from_commit, extract_trace_line_spec_ids,
    looks_like_spec_id, zen_drive,
};

const T3: &str = "TASK-b6b1278890774ef993d2ebc327862470-003";
const T4: &str = "TASK-b6b1278890774ef993d2ebc327862470-004";
const S5: &str = "STORY-b6b1278890774ef993d2ebc327862470-005";
const B6: &str = "BUG-B6B1278890774EF993D2EBC327862470-006";

fn upper(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_ascii_uppercase()).collect()
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn shape_rule_accepts_node_local_ids() {
    for id in [T3, T4, S5, B6, "FR-1-042", "TASK-7-1500", "BUG-12"] {
        assert!(looks_like_spec_id(id), "{id} should be a spec id");
        assert!(zen_drive::looks_like_spec_id(id), "zen: {id}");
    }
    // Still rejects prose, a non-numeric sequence, and runaway tokens.
    let runaway = format!("TASK-{}-001", "a".repeat(64));
    for junk in [
        "TASK-b6b1278890774ef993d2ebc327862470",
        "TASK-b6b1-12a",
        "-003",
        runaway.as_str(),
    ] {
        assert!(!looks_like_spec_id(junk), "{junk} should be rejected");
    }
}

#[test]
fn subject_trailer_comma_list_is_delivered() {
    let msg = format!("[AI:claude] feat(mailbox): thing ({T3}, {T4}, {S5}) (#2955)\n\nbody\n");
    assert_eq!(extract_spec_ids_from_commit(&msg), owned(&[T3, T4, S5]));
    let subject = msg.lines().next().unwrap();
    assert_eq!(
        extract_trailing_spec_ids_from_subject(subject),
        upper(&[T3, T4, S5])
    );
}

#[test]
fn auto_queue_coverage_counts_node_local_trailers() {
    let messages = vec![
        format!("[AI:claude] fix(review): one ({B6})"),
        format!("[AI:claude] feat(x): slice (Refs {S5})\n\ntrace:{T3} | ai:claude\n"),
    ];
    let cov = review_coverage_from_commits(&messages);
    assert_eq!(cov.delivered, owned(&[B6]));
    assert_eq!(cov.slice_refs, upper(&[S5]));
    assert!(crate::review_gates::auto_queue_skip_reason(
        true,
        Some(2955),
        Some(cov.delivered.len())
    )
    .is_none());
}

#[test]
fn refs_trailer_with_and_without_pr_suffix() {
    assert_eq!(
        extract_trailing_refs_spec_ids_from_subject(&format!("x: y (Refs {T3}, {T4})")),
        upper(&[T3, T4])
    );
    assert_eq!(
        extract_trailing_refs_spec_ids_from_subject(&format!("x: y (Refs {S5}) (#12)")),
        upper(&[S5])
    );
}

#[test]
fn ship_squash_trailer_and_resolution_surfaces() {
    let title = format!("[AI:claude] fix(review): recognise ids ({B6})");
    assert_eq!(
        derive_squash_subject_spec_ids(&title, "bug-trailer-node-ids", ""),
        upper(&[B6])
    );
    assert_eq!(extract_spec_ids_from_text(&title), upper(&[B6]));
    assert_eq!(
        extract_all_spec_ids(&format!("Review PR-2955: thing ({T3})")),
        upper(&[T3])
    );
    assert_eq!(
        extract_trace_line_spec_ids(&format!("// trace:{T3},{S5} | ai:claude")),
        owned(&[T3, S5])
    );
}
