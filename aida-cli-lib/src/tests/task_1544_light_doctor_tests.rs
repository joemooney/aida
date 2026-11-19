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
        phases: Vec::new(),
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
    let report = remote_drift_light_report_with(
        root.path(),
        |_| RemoteDriftScan {
            findings: vec![finding("z")],
            ..Default::default()
        },
        |_| vec![finding("a")],
    );
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
    // The production report reads telemetry through AIDA_HOME. Route through
    // EnvVarGuard so this override serialises against every other test.
    // trace:TASK-1532 | ai:agy
    let _env = crate::test_env::EnvVarGuard::set("AIDA_HOME", root.path());
    let report = performance_light_report(root.path());
    assert_eq!(
        report.total, 1,
        "configured shape without fixture telemetry is unobserved"
    );
    assert_eq!(report.performance_audits.len(), 1);
    assert_eq!(report.performance_audits[0].denominator, 0);
}

// trace:BUG-2672 | ai:codex
#[test]
fn remote_drift_bridge_cadence_rejects_duration_overflow() {
    let parse = crate::maintenance_schedule::parse_duration;
    assert_eq!(parse("30m").unwrap().num_seconds(), 1800);
    assert!(parse("9223372036854775807w").is_err());
}

#[test]
fn remote_drift_real_probe_classifies_fixture_hubs() {
    let root = bad_store();
    let git_at = |dir: &std::path::Path, args: &[&str], epoch: i64| {
        let date = format!("@{epoch} +0000");
        let output = crate::test_git::git_command()
            .args(args)
            .current_dir(dir)
            .env("HOME", root.path())
            .env("GIT_CONFIG_GLOBAL", root.path().join("gitconfig"))
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
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
    let now = chrono::Utc::now().timestamp();
    let git = |dir: &std::path::Path, args: &[&str]| git_at(dir, args, now);
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
    let observer = tempfile::tempdir().unwrap();
    git(observer.path(), &["init", "-q", "-b", "main"]);
    for (name, remote) in [("origin", &first), ("mirror", &second)] {
        git(
            observer.path(),
            &["remote", "add", name, remote.to_str().unwrap()],
        );
    }
    let report = || remote_drift_light_report(observer.path());
    let publish = |remote, branch| {
        git(
            root.path(),
            &["push", "-q", remote, &format!("HEAD:refs/heads/{branch}")],
        );
    };

    git(root.path(), &["commit", "--allow-empty", "-qm", "base"]);
    git(root.path(), &["branch", "base"]);
    for remote in ["origin", "mirror"] {
        publish(remote, "main");
        publish(remote, "aida-store");
    }
    let equal = report();
    assert_eq!(equal.total, 0);
    assert_eq!(equal.remote_drift_info.len(), 2);
    assert!(equal
        .remote_drift_info
        .iter()
        .all(|info| info.contains("in-sync")));

    // The observer has never fetched either branch. One old missing commit
    // must determine the age even when the newer tip was committed just now.
    git_at(
        root.path(),
        &["commit", "--allow-empty", "-qm", "lag"],
        now - 20 * 60,
    );
    git(root.path(), &["commit", "--allow-empty", "-qm", "recent"]);
    publish("origin", "aida-store");
    let recent = report();
    assert_eq!(recent.total, 0);
    assert!(recent.remote_drift_info.iter().any(|info| {
        info.contains("mirror is 2 commit(s) behind origin") && info.contains("wait for the bridge")
    }));
    assert!(!observer.path().join(".git/FETCH_HEAD").exists());
    assert!(!observer.path().join(".git/refs/remotes").exists());

    std::fs::create_dir(observer.path().join(".aida")).unwrap();
    let config_path = observer.path().join(".aida/config.toml");
    let set_window = |minutes| {
        std::fs::write(
            &config_path,
            format!("[doctor.remote_drift]\nstore_lag_minutes = {minutes} # minutes\n"),
        )
        .unwrap();
    };
    set_window(15);
    let stale = report();
    assert_eq!(stale.total, 1);
    let finding = &stale.findings[0];
    assert_eq!(finding.id, "remote-drift-aida-store");
    assert!(
        finding.summary.contains("2 commit(s) behind"),
        "{}",
        finding.summary
    );
    assert!(finding.action.contains("aida remote mirror-sync"));
    set_window(120);

    // BUG-2693: the hourly bridge can union recent two-sided store drift,
    // while main divergence trips on the first observation.
    publish("origin", "main");
    let main_lag = report();
    assert_eq!(main_lag.total, 1);
    assert_eq!(main_lag.findings[0].id, "remote-drift-main");
    assert!(main_lag.findings[0].summary.contains("2 commit(s) behind"));
    git(root.path(), &["checkout", "-q", "base"]);
    git(
        root.path(),
        &["commit", "--allow-empty", "-qm", "other side"],
    );
    publish("mirror", "main");
    let main_only = report();
    assert_eq!(main_only.total, 1);
    assert_eq!(main_only.findings[0].id, "remote-drift-main");
    assert!(main_only.findings[0].summary.contains("diverged"));
    publish("mirror", "aida-store");
    let recent_union = report();
    assert_eq!(recent_union.total, 1);
    assert!(recent_union.remote_drift_info.iter().any(|info| {
        info.contains("aida-store")
            && info.contains("diverged")
            && info.contains("wait for the bridge")
    }));

    set_window(15);
    let diverged = report();
    assert_eq!(diverged.total, 2);
    for (finding, id) in diverged
        .findings
        .iter()
        .zip(["remote-drift-aida-store", "remote-drift-main"])
    {
        assert_eq!(finding.id, id);
        assert!(finding
            .summary
            .contains("diverged (mirror: 1 unique, origin: 2 unique)"));
        assert!(finding.action.contains("reconcile by hand"));
        assert!(!finding.safe_heal);
    }
}
