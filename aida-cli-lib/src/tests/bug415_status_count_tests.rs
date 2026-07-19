use super::is_real_requirement_summary;

#[test]
fn meta_rows_are_excluded_case_insensitively() {
    // Cache stores the Debug form ("Meta"); guard against casing drift.
    assert!(!is_real_requirement_summary("Meta"));
    assert!(!is_real_requirement_summary("meta"));
    assert!(!is_real_requirement_summary("META"));
}

#[test]
fn real_requirement_types_are_counted() {
    for t in [
        "Task",
        "Bug",
        "Story",
        "Epic",
        "Functional",
        "NonFunctional",
        "System",
        "User",
        "Spike",
        "Sprint",
        "Folder",
        "Doc",
    ] {
        assert!(
            is_real_requirement_summary(t),
            "{t} should be counted as a real requirement"
        );
    }
}

#[test]
fn fresh_store_counts_match_list() {
    // Reproduces BUG-415: a fresh distributed store seeds 6 Draft META
    // rows (META-001..006) plus the single Approved onboarding TASK.
    // Counting real rows must yield 1 (matching `aida list --all`), not 7.
    let fresh_store = [
        ("Meta", "Draft"),
        ("Meta", "Draft"),
        ("Meta", "Draft"),
        ("Meta", "Draft"),
        ("Meta", "Draft"),
        ("Meta", "Draft"),
        ("Task", "Approved"),
    ];
    let real = fresh_store
        .iter()
        .filter(|(t, _)| is_real_requirement_summary(t))
        .count();
    assert_eq!(real, 1, "fresh store should report 1 real requirement");

    let draft = fresh_store
        .iter()
        .filter(|(t, s)| is_real_requirement_summary(t) && *s == "Draft")
        .count();
    assert_eq!(
        draft, 0,
        "the 6 META Draft rows must not be counted as Draft"
    );
}
