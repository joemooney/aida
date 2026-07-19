use super::*;

#[test]
fn standing_artifact_types_are_recognized_case_insensitively() {
    // The cache summary's Display form is "Vision", "Principle", etc.
    for t in [
        "Vision",
        "Principle",
        "Term",
        "Constraint",
        "Folder",
        "Meta",
    ] {
        assert!(is_standing_artifact_type(t), "{t} should be standing");
        assert!(
            is_standing_artifact_type(&t.to_lowercase()),
            "{t} lowercase should be standing"
        );
    }
}

#[test]
fn work_types_are_not_standing_artifacts() {
    // Open-work types must NOT be excluded from the default list view.
    for t in [
        "Functional",
        "Bug",
        "Epic",
        "Story",
        "Task",
        "Spike",
        "Sprint",
        "Decision",
        "Doc",
        "User",
        "System",
        "Change Request",
    ] {
        assert!(!is_standing_artifact_type(t), "{t} should be work");
    }
}
