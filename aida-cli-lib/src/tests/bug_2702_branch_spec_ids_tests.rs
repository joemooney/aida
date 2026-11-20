//! BUG-2702: a branch name yields a spec ID only under AIDA's branch
//! conventions (a built-in spec-type prefix), so a promotion or train branch
//! never names a spec.
// trace:BUG-2702 | ai:claude
use super::*;

#[test]
fn non_spec_type_branches_yield_no_spec_id() {
    for branch in [
        "promote/integration-23",
        "promote/integration-3a-7",
        "train/3fa85f64-5717-4562-b3fc-2c963f66afa6-1",
        "release-2",
        "worktree-agent-1",
        "integration-23",
        "",
    ] {
        assert!(
            extract_branch_spec_ids(branch).is_empty(),
            "{branch:?} -> {:?}",
            extract_branch_spec_ids(branch)
        );
    }
}

#[test]
fn spec_type_branches_keep_their_spec_id() {
    let cases: &[(&str, &[&str])] = &[
        ("bug-2702-mandate-cover", &["BUG-2702"]),
        ("BUG-2702", &["BUG-2702"]),
        ("story-1698-x", &["STORY-1698"]),
        ("feature/task-688-aida-release", &["TASK-688"]),
        ("adr-12", &["ADR-12"]),
        ("vis-3-north-star", &["VIS-3"]),
        (
            "bug-b6b1278890774ef993d2ebc327862470-464",
            &["BUG-B6B1278890774EF993D2EBC327862470-464"],
        ),
    ];
    for (branch, want) in cases {
        assert_eq!(&extract_branch_spec_ids(branch), want, "{branch}");
    }
}

#[test]
fn pr_spec_ids_take_the_branch_by_convention_and_the_title_as_written() {
    // The promotion title's ids survive; the promotion branch adds nothing.
    assert_eq!(
        extract_pr_spec_ids(
            "promote/integration-23",
            "[AI:claude] chore(integration): batch [promotion 23] (Refs STORY-1 STORY-2)"
        ),
        vec!["STORY-1", "STORY-2"]
    );
    // Branch first, then title, deduplicated by numeric identity.
    assert_eq!(
        extract_pr_spec_ids("bug-0042-x", "fix (BUG-42) (Refs TASK-7)"),
        vec!["BUG-0042", "TASK-7"]
    );
    // A title keeps any ID-shaped token: fail closed on what a human wrote.
    assert_eq!(extract_pr_spec_ids("", "(WIDGET-9)"), vec!["WIDGET-9"]);
}
