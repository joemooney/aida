//! trace:TASK-75 | ai:claude
use super::*;

#[test]
fn classify_commit_source_pr_merge() {
    assert_eq!(
        classify_commit_source(
            "[AI:claude] feat(session): polish (STORY-98 STORY-90 BUG-74) (#10)"
        ),
        "PR-10 merge"
    );
    assert_eq!(
        classify_commit_source("[AI:claude] fix(x): y (BUG-77) (#42)"),
        "PR-42 merge"
    );
}

#[test]
fn classify_commit_source_explicit_ids() {
    assert_eq!(
        classify_commit_source("[AI:claude] fix(cache): foo (BUG-77)"),
        "commit (BUG-77)"
    );
    assert_eq!(
        classify_commit_source("[AI:claude] fix(s): foo (BUG-75 BUG-76)"),
        "commit (BUG-75, BUG-76)"
    );
}

#[test]
fn classify_commit_source_manual() {
    assert_eq!(classify_commit_source("chore: bump dep"), "manual");
    assert_eq!(classify_commit_source("release: v1.2.3 (1.2.3)"), "manual");
}

#[test]
fn diff_parser_extracts_single_status_change() {
    let diff = r#"diff --git a/objects/BUG/000/BUG-77.yaml b/objects/BUG/000/BUG-77.yaml
index abc..def 100644
--- a/objects/BUG/000/BUG-77.yaml
+++ b/objects/BUG/000/BUG-77.yaml
@@ -3 +3 @@ title: foo
-status: Approved
+status: Completed
"#;
    let out = parse_status_transitions_from_diff(diff);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0],
        ("BUG-77".into(), "Approved".into(), "Completed".into())
    );
}

#[test]
fn diff_parser_handles_multiple_files() {
    let diff = r#"diff --git a/objects/BUG/000/BUG-77.yaml b/objects/BUG/000/BUG-77.yaml
--- a/objects/BUG/000/BUG-77.yaml
+++ b/objects/BUG/000/BUG-77.yaml
@@ -3 +3 @@
-status: Approved
+status: Completed
diff --git a/objects/STORY/000/STORY-1.yaml b/objects/STORY/000/STORY-1.yaml
--- a/objects/STORY/000/STORY-1.yaml
+++ b/objects/STORY/000/STORY-1.yaml
@@ -3 +3 @@
-status: InProgress
+status: Completed
"#;
    let out = parse_status_transitions_from_diff(diff);
    assert_eq!(out.len(), 2);
    assert!(out
        .iter()
        .any(|t| t.0 == "BUG-77" && t.1 == "Approved" && t.2 == "Completed"));
    assert!(out
        .iter()
        .any(|t| t.0 == "STORY-1" && t.1 == "InProgress" && t.2 == "Completed"));
}

#[test]
fn diff_parser_skips_unchanged_status() {
    // If status is in the diff context but didn't actually change,
    // we shouldn't produce a transition.
    let diff = r#"diff --git a/objects/BUG/000/BUG-77.yaml b/objects/BUG/000/BUG-77.yaml
--- a/objects/BUG/000/BUG-77.yaml
+++ b/objects/BUG/000/BUG-77.yaml
@@ -2,3 +2,3 @@
 title: foo
-status: Approved
+status: Approved
"#;
    let out = parse_status_transitions_from_diff(diff);
    assert!(out.is_empty(), "unchanged status should not transition");
}
