//! Store-free doctor checks. All fixtures live under temporary HOME/project roots.
// trace:TASK-1544 | ai:codex
use super::*;

fn bad_store() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".aida-store"), "not a store").unwrap();
    assert!(Storage::new(&tmp.path().join(".aida-store"))
        .load()
        .is_err());
    tmp
}

#[test]
fn performance_injected_inputs_keep_findings_and_audits() {
    let root = bad_store();
    let now = chrono::Utc::now();
    let budgets = [PerformanceBudget {
        cmd: "show".into(),
        budget_ms: 100,
        ceiling_ms: None,
    }];
    let events = [crate::usage::UsageEvent {
        ts: now.to_rfc3339(),
        cmd: "show".into(),
        args_count: 0,
        exit_code: 0,
        duration_ms: 200,
        binary_sha: None,
        role: None,
        scope: None,
        schedule_source: None,
    }];
    let policy = PerformancePolicy::default();
    let lineage = BinaryLineage::unscoped();
    let report = performance_light_report_with(&events, &budgets, &policy, now, &lineage);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.total, 1);
    assert_eq!(report.findings[0].category, "performance");
    assert_eq!(report.performance_audits.len(), 1);
    assert_eq!(report.performance_audits[0].denominator, 1);
    assert!(serde_json::to_value(&report)
        .unwrap()
        .get("performance_audits")
        .is_some());
    assert!(root.path().join(".aida-store").is_file());
}

#[test]
fn remote_drift_injected_scanners_merge_and_sort_without_store() {
    let root = bad_store();
    let finding = |id: &str| DoctorFinding {
        category: "remote-drift".into(),
        id: id.into(),
        summary: id.into(),
        action: "inspect".into(),
        safe_heal: false,
    };
    let report =
        remote_drift_light_report_with(root.path(), |_| vec![finding("z")], |_| vec![finding("a")]);
    assert_eq!(report.total, 2);
    assert_eq!(
        report
            .findings
            .iter()
            .map(|f| f.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert!(report.performance_audits.is_empty());
}

#[test]
fn performance_real_probe_reads_fixture_config_and_home() {
    let root = bad_store();
    std::fs::create_dir_all(root.path().join(".aida")).unwrap();
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[performance.budgets]\nshow = 100\n",
    )
    .unwrap();
    // The production report reads telemetry through AIDA_HOME. Serialize this
    // environment override so the test cannot touch the user's live usage log.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _lock = ENV_LOCK.lock().unwrap();
    let old = std::env::var_os("AIDA_HOME");
    unsafe {
        std::env::set_var("AIDA_HOME", root.path());
    }
    let report = performance_light_report(root.path());
    match old {
        Some(value) => unsafe {
            std::env::set_var("AIDA_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("AIDA_HOME");
        },
    }
    assert_eq!(
        report.total, 1,
        "configured shape without fixture telemetry is unobserved"
    );
    assert_eq!(report.performance_audits.len(), 1);
    assert_eq!(report.performance_audits[0].denominator, 0);
}

#[test]
fn remote_drift_real_probe_accepts_fixture_git_repository() {
    let root = bad_store();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("HOME", root.path())
            .env("GIT_CONFIG_GLOBAL", root.path().join("gitconfig"))
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(root.path(), &["init", "-q", "-b", "main"]);
    let first = root.path().join("first.git");
    let second = root.path().join("second.git");
    git(
        root.path(),
        &["init", "-q", "--bare", first.to_str().unwrap()],
    );
    git(
        root.path(),
        &["init", "-q", "--bare", second.to_str().unwrap()],
    );
    git(
        root.path(),
        &["remote", "add", "origin", first.to_str().unwrap()],
    );
    git(
        root.path(),
        &["remote", "add", "mirror", second.to_str().unwrap()],
    );
    std::fs::write(root.path().join("fixture.txt"), "first").unwrap();
    git(root.path(), &["add", "fixture.txt"]);
    git(root.path(), &["commit", "-qm", "first"]);
    git(root.path(), &["push", "-q", "origin", "main"]);
    std::fs::write(root.path().join("fixture.txt"), "second").unwrap();
    git(root.path(), &["commit", "-qam", "second"]);
    git(root.path(), &["push", "-q", "mirror", "main"]);
    let report = remote_drift_light_report(root.path());
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.id == "remote-drift-main"));
}
