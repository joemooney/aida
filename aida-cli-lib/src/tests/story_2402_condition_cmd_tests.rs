//! Real adapter regressions for the S1 checker.
// trace:STORY-2402 | ai:codex
use super::*;

// Snapshot-backed facts must not reload canonical objects during a sweep.
// trace:STORY-2402 | ai:codex
#[test]
fn story2402_snapshot_facts_reuse_rows_for_spec_and_query_conditions() {
    use aida_core::condition::Verdict;

    let fixture = tempfile::tempdir().unwrap();
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
    let snapshot = read_store(fixture.path(), Some(&canonical)).unwrap();
    let facts = CliConditionFacts::from_store(fixture.path(), snapshot);
    std::fs::remove_dir_all(&canonical).unwrap();

    for text in ["spec:TASK-1=completed", "query:status=completed count>=1"] {
        let condition: Condition = text.parse().unwrap();
        assert_eq!(condition.evaluate(&facts).unwrap().verdict, Verdict::Met);
    }
    assert_eq!(facts.store().unwrap().requirements.len(), 1);
    assert!(!canonical.exists());
}

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
        crate::test_exec::write_executable(&binary, format!("#!/bin/sh\n{body}\n"));
        let timeout = if body.contains("sleep") {
            std::time::Duration::from_millis(200)
        } else {
            std::time::Duration::from_secs(5)
        };
        let observation = crate::forge::observe_github_pr_merged_with_binary(
            fixture.path(),
            &binary,
            12,
            timeout,
        );
        assert_eq!(observation.as_known().copied(), want, "{body}");
        if want.is_none() {
            assert!(
                matches!(observation, crate::forge_observed::Observed::Transient(_)),
                "{body}: {observation:?}"
            );
        }
    }
}

#[test]
fn story2402_registered_check_parses_and_global_json_is_available() {
    let cli = crate::TEST_CLI
        .try_parse_from([
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
    assert!(crate::TEST_CLI
        .try_parse_from(["aida", "condition"])
        .is_err());
    assert!(crate::TEST_CLI
        .try_parse_from(["aida", "condition", "run"])
        .is_err());
}

// Exercise the real Git adapter without changing PATH or requiring store data.
// trace:STORY-2402 | ai:codex
#[test]
fn story2402_release_probe_reads_local_numeric_tags_and_reports_missing_repos() {
    use aida_core::condition::Verdict;

    let fixture = tempfile::tempdir().unwrap();
    let facts = CliConditionFacts::new(fixture.path(), None);
    assert!(facts.latest_release().is_err());
    let run_git = |args: &[&str]| {
        let output = crate::test_exec::output(
            crate::test_git::git_command()
                .current_dir(fixture.path())
                .args(args),
        )
        .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    };
    run_git(&["init"]);
    assert_eq!(facts.latest_release().unwrap(), None);
    // No release is not a numeric zero: every comparison remains not-met.
    for op in ["<", "<=", "=", ">=", ">"] {
        let condition: Condition = format!("release:{op}1.0").parse().unwrap();
        assert_eq!(condition.evaluate(&facts).unwrap().verdict, Verdict::NotMet);
    }
    run_git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.test",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-m",
        "fixture",
    ]);
    for tag in ["v0.9", "0.10.2", "v0.10.1", "v99.0-rc1", "notes"] {
        run_git(&["tag", tag]);
    }
    assert_eq!(
        facts.latest_release().unwrap(),
        Some("0.10.2".parse().unwrap())
    );
}
