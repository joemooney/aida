use crate::review_verdict::{build_verdict_object, parse_recorded_verdict, write_verdict_object};
use std::path::Path;

#[test]
fn bug_1799_carry_forward_sweep_skips_inherited_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let file = root.join("verdict.json");

    // 1. Request changes with finding F at sha A
    let obj1 = build_verdict_object(
        root,
        &file,
        Some("request-changes"),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        None,
        None,
        &["F".to_string()],
        "me",
    )
    .unwrap();
    write_verdict_object(&file, &obj1).unwrap();

    // 2. Approve at sha B with no finding flags
    let obj2 = build_verdict_object(
        root,
        &file,
        Some("approved"),
        Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        None,
        None,
        &[],
        "me",
    )
    .unwrap();

    // Convert to JSON and parse via parse_recorded_verdict
    let json2 = serde_json::to_string(&obj2).unwrap();
    let parsed2 = parse_recorded_verdict(&json2).expect("valid verdict");

    // The findings should be empty for successor generation
    let mut already_filed = std::collections::HashSet::new();
    let successors2 = crate::findings_needing_a_successor(&parsed2, &already_filed);
    assert_eq!(
        successors2.len(),
        0,
        "inherited findings should not carry forward"
    );

    // 3. Explicitly re-record at sha C
    let obj3 = build_verdict_object(
        root,
        &file, // read from the same file which has the `request-changes` state in `obj1` + `obj2` changes if we wrote it, but we didn't write obj2
        Some("approved"),
        Some("cccccccccccccccccccccccccccccccccccccccc"),
        None,
        None,
        &["F".to_string()],
        "me",
    )
    .unwrap();

    let json3 = serde_json::to_string(&obj3).unwrap();
    let parsed3 = parse_recorded_verdict(&json3).expect("valid verdict");

    let successors3 = crate::findings_needing_a_successor(&parsed3, &already_filed);
    assert_eq!(
        successors3.len(),
        1,
        "explicitly re-recorded findings carry forward"
    );
    assert_eq!(successors3[0].0, "F");
}
