//! Real adapter regressions for the S1 checker.
// trace:STORY-2402 | ai:codex
use super::*;

#[test]
fn story2402_store_reads_are_strict_and_do_not_create_missing_stores() {
    let fixture = tempfile::tempdir().unwrap();
    let missing = fixture.path().join("missing.yaml");
    assert!(read_store(fixture.path(), Some(&missing)).is_err());
    assert!(!missing.exists());

    let canonical = fixture.path().join("canonical");
    let objects = canonical.join("objects/TASK/00");
    std::fs::create_dir_all(&objects).unwrap();
    let mut req = aida_core::Requirement::new("fixture".into(), "".into());
    req.spec_id = Some("TASK-1".into());
    req.status = RequirementStatus::Completed;
    std::fs::write(
        objects.join("TASK-1.yaml"),
        serde_yaml::to_string(&req).unwrap(),
    )
    .unwrap();
    let facts = CliConditionFacts::new(fixture.path(), canonical.to_str());
    assert_eq!(
        facts.spec_status("TASK-1").unwrap(),
        Some(RequirementStatus::Completed)
    );
    assert_eq!(facts.count(&[]).unwrap(), 1);

    // A condition must not choose one side of a conflicting agreed ID.
    req.agreed_id = Some("TASK-77".into());
    std::fs::write(
        objects.join("TASK-1.yaml"),
        serde_yaml::to_string(&req).unwrap(),
    )
    .unwrap();
    let mut other = req.clone();
    other.id = uuid::Uuid::new_v4();
    other.spec_id = Some("TASK-2".into());
    other.status = RequirementStatus::Draft;
    std::fs::write(
        objects.join("TASK-2.yaml"),
        serde_yaml::to_string(&other).unwrap(),
    )
    .unwrap();
    let facts = CliConditionFacts::new(fixture.path(), canonical.to_str());
    assert!(facts.spec_status("TASK-77").is_err());

    std::fs::write(objects.join("TASK-2.yaml"), "[broken").unwrap();
    let facts = CliConditionFacts::new(fixture.path(), canonical.to_str());
    assert!(facts.count(&[]).is_err());
    assert!(facts.spec_status("TASK-1").is_err());
}

#[cfg(unix)]
#[test]
fn story2402_pr_probe_requires_a_known_state_and_bounds_the_process() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = tempfile::tempdir().unwrap();
    let binary = fixture.path().join("gh-fixture");
    for (body, want) in [
        ("printf '%s' '{\"state\":\"MERGED\"}'", Some(true)),
        ("printf '%s' '{\"state\":\"OPEN\"}'", Some(false)),
        ("printf '%s' '{\"state\":\"CLOSED\"}'", Some(false)),
        ("printf '%s' '{\"state\":\"SURPRISE\"}'", None),
        ("printf '%s' '{}'", None),
        ("printf '%s' 'not-json'", None),
        ("exit 1", None),
        ("exec /bin/sleep 5", None),
    ] {
        std::fs::write(&binary, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let timeout = if body.contains("sleep") {
            std::time::Duration::from_millis(200)
        } else {
            std::time::Duration::from_secs(5)
        };
        assert_eq!(
            read_pr_state(fixture.path(), &binary, 12, timeout),
            want,
            "{body}"
        );
    }
}

#[test]
fn story2402_registered_check_parses_and_global_json_is_available() {
    let cli = crate::command_registry::try_parse_from([
        "aida",
        "--format",
        "json",
        "condition",
        "check",
        "manual:joe",
    ])
    .unwrap();
    assert!(matches!(cli.command.registered::<ConditionArgs>().inner,
        ConditionCommand::Check { ref condition, json: false } if condition == "manual:joe"));
    assert!(crate::command_registry::try_parse_from(["aida", "condition"]).is_err());
    assert!(crate::command_registry::try_parse_from(["aida", "condition", "run"]).is_err());
}
