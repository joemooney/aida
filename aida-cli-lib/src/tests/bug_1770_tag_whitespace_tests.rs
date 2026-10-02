//! Unit coverage for BUG-1770's shared whitespace rule.
//!
//! The black-box acceptance lives in `aida-cli/tests/bug_1770_tag_whitespace.rs`
//! (it drives the shipped CLI and inspects the YAML list). These tests pin the
//! primitive every write path now routes through, so a fourth site splitting a
//! `--tags` string itself cannot quietly reintroduce the blob.
// trace:BUG-1770 | ai:claude

use super::{apply_tag_deltas_report, parse_tag_list, validate_tag_value, TAGS_FLAG};
use std::collections::HashSet;

fn vec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_comma_list_yields_separate_trimmed_tags() {
    assert_eq!(
        parse_tag_list(" alpha , beta ,, severity:major ").unwrap(),
        vec(&["alpha", "beta", "severity:major"])
    );
}

#[test]
fn a_whitespace_value_is_refused_and_the_message_shows_the_comma_repair() {
    let err = parse_tag_list("auto-complete orchestrator")
        .expect_err("a space-separated value must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("auto-complete orchestrator"),
        "must name the offending value: {msg}"
    );
    assert!(
        msg.contains("--tags auto-complete,orchestrator"),
        "must show the pasteable comma form: {msg}"
    );
}

#[test]
fn the_add_tag_refusal_suggests_the_repeatable_flag_not_a_comma_list() {
    let err = validate_tag_value("alpha beta", "--add-tag").expect_err("must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("--add-tag alpha --add-tag beta"),
        "`--add-tag` takes one value per occurrence, so the repair must repeat the flag: {msg}"
    );
    assert!(
        !msg.contains("--add-tag alpha,beta"),
        "a comma list is NOT a valid --add-tag repair: {msg}"
    );
}

#[test]
fn a_tab_or_newline_is_whitespace_too() {
    assert!(validate_tag_value("alpha\tbeta", TAGS_FLAG).is_err());
    assert!(validate_tag_value("alpha\nbeta", TAGS_FLAG).is_err());
}

#[test]
fn a_colon_namespaced_tag_still_round_trips() {
    // The control: this is the case BUG-1542 was filed against and correctly
    // rejected over. It works today and must keep working, so it stops a
    // regression passing by breaking both forms at once.
    assert!(validate_tag_value("severity:major", TAGS_FLAG).is_ok());
    assert_eq!(
        parse_tag_list("severity:major,parent:EPIC-28").unwrap(),
        vec(&["severity:major", "parent:EPIC-28"])
    );
}

#[test]
fn add_tag_refusal_leaves_the_tag_set_completely_unmutated() {
    // The validation runs over the whole `add` list BEFORE anything is
    // inserted, so a caller that saves after an error cannot persist half of
    // the request.
    let mut tags: HashSet<String> = ["existing".to_string()].into_iter().collect();
    let err = apply_tag_deltas_report(&mut tags, &vec(&["fine", "not fine"]), &[])
        .expect_err("the malformed member must refuse the whole call");
    assert!(err.to_string().contains("not fine"));
    assert_eq!(
        tags,
        ["existing".to_string()].into_iter().collect::<HashSet<_>>(),
        "`fine` must NOT have been inserted before the refusal"
    );
}

#[test]
fn remove_tag_is_deliberately_not_validated_so_an_existing_blob_stays_removable() {
    // The 276 objects already carrying a blob can only be named by reproducing
    // the value verbatim. Refusing whitespace on `--remove-tag` would leave
    // them unrepairable by the incremental form.
    let mut tags: HashSet<String> = ["alpha beta".to_string(), "keep".to_string()]
        .into_iter()
        .collect();
    let report = apply_tag_deltas_report(&mut tags, &[], &vec(&["alpha beta"]))
        .expect("removing an existing blob must be allowed");
    assert_eq!(report.removed, vec(&["alpha beta"]));
    assert_eq!(
        tags,
        ["keep".to_string()].into_iter().collect::<HashSet<_>>()
    );
}

#[test]
fn the_report_distinguishes_a_removal_from_a_miss() {
    let mut tags: HashSet<String> = ["alpha".to_string()].into_iter().collect();
    let report = apply_tag_deltas_report(&mut tags, &[], &vec(&["alpha", "nosuchtag"])).unwrap();
    assert_eq!(report.removed, vec(&["alpha"]));
    assert_eq!(report.absent, vec(&["nosuchtag"]));
    let lines = report.summary_lines();
    assert!(
        lines.iter().any(|l| l == "removed 1 tag: alpha"),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == "no matching tag to remove: nosuchtag"),
        "{lines:?}"
    );
}
