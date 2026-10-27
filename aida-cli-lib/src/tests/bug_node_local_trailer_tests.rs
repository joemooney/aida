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

// --- TASK-b6b1278890774ef993d2ebc327862470-007: one shape rule everywhere ---
// trace:TASK-b6b1278890774ef993d2ebc327862470-007 | ai:claude

#[test]
fn canonical_and_free_text_rules_take_node_local_ids() {
    use crate::{looks_like_canonical_spec_id, looks_like_spec_id_in_text};
    for id in [T3, S5, B6, "FR-1-042", "TASK-398"] {
        assert!(looks_like_canonical_spec_id(id), "canonical: {id}");
        assert!(looks_like_spec_id_in_text(id), "text: {id}");
    }
    // Lower-case prefix: not canonical, but a built-in type prefix in text.
    let lower = T3.to_ascii_lowercase();
    assert!(!looks_like_canonical_spec_id(&lower));
    assert!(looks_like_spec_id_in_text(&lower));
    // Hyphenated prose ending in digits is never an id in free text.
    for prose in ["follow-up-2", "phase-2", "X-1", "story 45", "branch-name"] {
        assert!(!looks_like_spec_id_in_text(prose), "{prose}");
        assert!(!looks_like_canonical_spec_id(prose), "{prose}");
    }
}

#[test]
fn trace_marker_regex_captures_node_local_ids() {
    let re = crate::trace_marker_re();
    let ids =
        |line: &str| -> Vec<String> { re.captures_iter(line).map(|c| c[1].to_string()).collect() };
    assert_eq!(ids(&format!("// trace:{T3} | ai:claude")), owned(&[T3]));
    assert_eq!(ids(&format!("    // trace:{B6} | ai:claude")), owned(&[B6]));
    assert_eq!(ids(&format!("/// doc anchor trace:{S5}")), owned(&[S5]));
    assert_eq!(ids("// trace:FR-1-042 | ai:claude"), owned(&["FR-1-042"]));
    assert_eq!(
        ids("// trace:STORY-499.ac1 | ai:claude"),
        owned(&["STORY-499"])
    );
    // Prose after `trace:` is not an id.
    assert!(ids("trace:follow-up").is_empty());
    assert!(ids("strace:TASK-7").is_empty());
    // The coverage-anchor grammar is the same regex.
    let anchor = crate::trace::coverage_anchor_re();
    assert_eq!(
        anchor
            .captures(&format!("// trace:{T3} | ai:claude"))
            .unwrap()[1]
            .to_string(),
        T3
    );
}

#[test]
fn trace_grep_pattern_matches_node_local_ids() {
    // `SPEC_ID_ERE` is POSIX ERE for `git grep -E`; its one capturing group
    // is also valid Rust regex, so pin the shape here.
    let re = regex::Regex::new(&format!("^trace:{}$", crate::SPEC_ID_ERE)).unwrap();
    for id in [T3, S5, B6, "FR-1-042", "TASK-7"] {
        assert!(re.is_match(&format!("trace:{id}")), "{id}");
    }
    assert!(!re.is_match("trace:follow"));
    assert_eq!(
        crate::parse_trace_id_token(&format!("trace:{T3}")),
        Some(T3.to_ascii_uppercase())
    );
}

#[test]
fn trace_graph_scan_matches_node_local_markers_in_any_case() {
    let tmp = tempfile::TempDir::new().unwrap();
    // The marker spells the node in upper case; the caller wants the stored
    // lower-case spelling. The hit buckets under the caller's spelling.
    std::fs::write(
        tmp.path().join("a.rs"),
        format!(
            "// trace:{} | ai:claude\nfn traced() {{}}\n",
            T3.to_ascii_uppercase()
        ),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("b.rs"),
        format!("// trace:{T3} | ai:claude\nfn also() {{}}\n"),
    )
    .unwrap();
    let wanted: std::collections::HashSet<String> = [T3.to_string()].into_iter().collect();
    let (hits, complete) =
        crate::trace::scan_trace_graph_streaming(tmp.path(), &wanted, &mut || false);
    assert!(complete);
    let mut files: Vec<String> = hits[T3].iter().map(|h| h.file.clone()).collect();
    files.sort();
    assert_eq!(files, vec!["a.rs".to_string(), "b.rs".to_string()]);
    let symbols: Vec<Option<String>> = hits[T3].iter().map(|h| h.symbol.clone()).collect();
    assert!(symbols.contains(&Some("traced".to_string())), "{symbols:?}");
}

#[test]
fn commit_message_ids_include_node_local_trailers_and_traces() {
    let msg = format!("[AI:claude] fix(ids): x ({T3}) (#1)\n\ntrace:{S5} | ai:claude\n(scope)\n");
    assert_eq!(
        crate::plan_intent::spec_ids_from_commit_message(&msg),
        upper(&[T3, S5])
    );
}
