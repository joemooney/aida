#[test]
fn fast_gate_body_preserves_text_after_previous_annotation() {
    let body = super::fast_gate_body("original", "old", "fail") + "later reviewer notes";
    let updated = super::fast_gate_body(&body, "fixed", "pass");
    assert!(updated.starts_with("original"));
    assert!(updated.ends_with("later reviewer notes"));
    assert!(updated.contains("fast_gate=pass head=fixed"));
    assert!(!updated.contains("head=old"));
}

// trace:BUG-2633 | ai:codex
#[test]
fn bug_2633_body_edit_checks_forge_head_before_mutating() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let response = root.path().join("response.json");
    let mutations = root.path().join("mutations");
    crate::test_exec::write_executable(
        &root.path().join("gh"),
        &format!(
            "#!/bin/sh\nif [ \"$2\" = view ]; then /bin/cat '{}'; else echo edit >> '{}'; fi\n",
            response.display(),
            mutations.display(),
        ),
    );
    let _path = crate::test_env::EnvVarGuard::prepend_path(root.path());
    for (branch, cross, allowed) in [
        ("promote/integration-15", false, false),
        ("integration", false, false),
        ("bug-2", false, false),
        ("bug-1", true, false),
        ("bug-1", false, true),
    ] {
        std::fs::write(
            &response,
            serde_json::json!({
                "headRefOid": "tip", "headRefName": branch,
                "isCrossRepository": cross, "body": "original",
            })
            .to_string(),
        )
        .unwrap();
        let result = super::annotate_fast_gate(root.path(), 15, "bug-1", "tip", "pass");
        assert_eq!(result.is_ok(), allowed, "{result:?}");
        assert_eq!(mutations.exists(), allowed);
    }
    std::fs::remove_file(&mutations).unwrap();
    for incomplete in [
        "{}",
        r#"{"headRefName":"bug-1","headRefOid":"tip","body":"original"}"#,
    ] {
        std::fs::write(&response, incomplete).unwrap();
        assert!(super::annotate_fast_gate(root.path(), 15, "bug-1", "tip", "pass").is_err());
        assert!(!mutations.exists());
    }
}
