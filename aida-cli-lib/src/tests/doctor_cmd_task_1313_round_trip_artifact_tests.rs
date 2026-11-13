// trace:TASK-3780 | ai:codex
use super::*;
use aida_core::models::{Requirement, RequirementsStore};

fn store_with(description: &str) -> RequirementsStore {
    let mut req = Requirement::new("Sample".to_string(), description.to_string());
    req.spec_id = Some("TASK-9001".to_string());
    let mut store = RequirementsStore::new();
    store.requirements.push(req);
    store
}

// 7c: raw-form mis-decode (the parser has already turned the escape into
// the control character) produces exactly one mojibake finding.
#[test]
fn mojibake_raw_form_is_found() {
    let text = "the em-dash reads as \u{00E2}\u{0080}\u{0094} here";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].category, "round-trip-artifacts");
    assert!(findings[0].summary.contains("mojibake"));
}

// 7d: escaped-form mis-decode (the field still carries the literal
// `\xHH` escape text) also produces one finding.
#[test]
fn mojibake_escaped_form_is_found() {
    let text = "the em-dash reads as \u{00E2}\\x80\\x94 here";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].summary.contains("mojibake"));
}

// 7a: a legitimate lone Â/Ã/â (an ordinary accented letter, not a pair)
// produces no mojibake finding.
#[test]
fn lone_latin_letter_is_not_mojibake() {
    let text = "The café menu references the \u{00C2}ge Bracket column.";
    let store = store_with(text);
    assert!(scan_round_trip_artifacts(&store).is_empty());
}

// 7e: a swallowed TOON block with no non-ASCII character at all is still
// found — this is the case the mojibake-only detector would have missed.
#[test]
fn swallowed_toon_block_without_mojibake_is_found() {
    let text = "Closing the description here.\"\nrelationships[1]{rel,id,title}:\n  child,TASK-2,\"x\"\nnext[1]{cmd,to}:\n  aida queue done TASK-2,done";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert_eq!(findings.len(), 2, "expects one finding per anchored header");
    assert!(findings
        .iter()
        .all(|f| f.summary.contains("TOON row header")));
}

// 7b (inline mention, negative case 1): prose that discusses the
// convention mid-line — never at column 0 — must not fire the
// high-confidence anchored check. This is TASK-1313's own defining
// case: its spec text contains the phrase `execution_mode:` and the
// literal pattern `next[1]{` inline, never at the start of a line. An
// ordinary inline mention like this IS expected to surface in the
// separately-labelled, lower-confidence mid-line pass (criterion 7f) —
// that pass is deliberately noisier — so the negative assertion here is
// scoped to the high-confidence category, not to zero findings overall.
#[test]
fn inline_mention_of_execution_mode_is_not_flagged_high_confidence() {
    let text = "It proposes a new execution_mode: value for the advisor to groom.";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert!(
        findings
            .iter()
            .all(|f| f.category != "round-trip-artifacts"),
        "an inline mention must never fire the high-confidence anchored check: {findings:?}"
    );
}

#[test]
fn inline_mention_of_toon_header_is_not_flagged_high_confidence() {
    let text = "aida status already uses the `next[1]{cmd,to}:` row header today.";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert!(
        findings
            .iter()
            .all(|f| f.category != "round-trip-artifacts"),
        "an inline mention must never fire the high-confidence anchored check: {findings:?}"
    );
}

// Criterion 7f: a swallowed TOON row header appended MID-LINE — no
// newline before it, because the capture path stripped (or never had)
// the trailing newline — is still found, just at lower confidence and
// in a separately-labelled category so it does not dilute the
// high-confidence anchored findings. Both anchored forms (column-0 and
// indent-allowing) miss this shape by construction.
#[test]
fn mid_line_toon_header_is_found_as_possible() {
    let text = "Closing the description here.\"execution_mode: drain";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].category, "round-trip-artifacts-possible");
    assert!(findings[0].summary.contains("possible, mid-line"));
    assert!(findings[0].summary.contains("byte offset"));
}

// Follow-up (criterion 7b): TASK-1313's own defining sentence and the
// raw regex source line, verbatim, as a fixture. The spec argues this is
// the strongest negative case: the `^execution_mode:` clause of the
// regex's own SOURCE TEXT matches the pattern mid-line (never at column
// 0), so a fix that special-cased this spec by id would still have to
// reckon with the pattern re-appearing in its own defining prose.
#[test]
fn task_1313_own_defining_text_is_not_flagged_high_confidence() {
    let text = "THE PRIMARY PREDICATE IS THE CAUSE, NOT THE SYMPTOM, AND IT IS LINE-ANCHORED AT COLUMN 0. A description containing a TOON row header AT THE START OF A LINE is a round-trip artifact:\n\n       ^(?:relationships|next)\\[\\d+\\]\\{      or      ^execution_mode:\n";
    let store = store_with(text);
    let findings = scan_round_trip_artifacts(&store);
    assert!(
            findings.iter().all(|f| f.category != "round-trip-artifacts"),
            "TASK-1313's own defining text must never fire the high-confidence anchored check: {findings:?}"
        );
}

// 7b (negative case 2): an INDENTED code block quoting a TOON sample —
// the column-0 anchor (not a leading-indent-allowing anchor) must not
// match an indented line.
#[test]
fn indented_toon_sample_is_not_flagged() {
    let text =
        "See the example below:\n\n    relationships[1]{rel,id,title}:\n      child,TASK-2,\"x\"\n";
    let store = store_with(text);
    assert!(scan_round_trip_artifacts(&store).is_empty());
}

// Coverage: title and comment bodies are scanned too, not only
// description (criterion 6).
#[test]
fn title_and_comment_fields_are_scanned() {
    let mut req = Requirement::new(
        "execution_mode:\nswallowed into the title field".to_string(),
        "clean description".to_string(),
    );
    req.spec_id = Some("TASK-9002".to_string());
    req.comments.push(aida_core::models::Comment::new(
        "joe".to_string(),
        "comment carries \u{00C3}\u{00A2} mojibake".to_string(),
    ));
    let mut store = RequirementsStore::new();
    store.requirements.push(req);
    let findings = scan_round_trip_artifacts(&store);
    assert!(findings.iter().any(|f| f.id.contains("/title/")));
    assert!(findings.iter().any(|f| f.id.contains("/comment[0]/")));
}

// Criterion 6: coverage is every text field, including a NESTED reply
// (`Comment.replies: Vec<Comment>`), not just top-level comment bodies.
// Mojibake buried two levels deep (a reply's reply) must still surface.
#[test]
fn mojibake_in_nested_reply_is_found() {
    let mut req = Requirement::new("Sample".to_string(), "clean description".to_string());
    req.spec_id = Some("TASK-9003".to_string());
    let top =
        aida_core::models::Comment::new("joe".to_string(), "clean top-level comment".to_string());
    let top_id = top.id;
    let mut reply = aida_core::models::Comment::new_reply(
        "advisor".to_string(),
        "clean first-level reply".to_string(),
        top_id,
    );
    let reply_id = reply.id;
    let nested_reply = aida_core::models::Comment::new_reply(
        "advisor".to_string(),
        "the em-dash reads as \u{00E2}\u{0080}\u{0094} here, two levels deep".to_string(),
        reply_id,
    );
    reply.replies.push(nested_reply);
    let mut top = top;
    top.replies.push(reply);
    req.comments.push(top);

    let mut store = RequirementsStore::new();
    store.requirements.push(req);
    let findings = scan_round_trip_artifacts(&store);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert!(findings[0].summary.contains("mojibake"));
    assert!(
        findings[0].id.contains("/comment[0]/reply[0]/reply[0]/"),
        "expected a doubly-nested reply field label, got {}",
        findings[0].id
    );
}
