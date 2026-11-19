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
    // The machine-local layer decides whether a bridge will close the gap.
    let aida_home = tempfile::tempdir().unwrap();
    let _home = crate::test_env::EnvVarGuard::set("AIDA_HOME", aida_home.path());
    std::fs::create_dir(aida_home.path().join(".aida")).unwrap();
    let layer = aida_home
        .path()
        .join(".aida")
        .join(crate::store_local::LOCAL_LAYER_FILE);
    let clone_key = crate::store_local::clone_key(observer.path());
    let set_layer = |body: &str| std::fs::write(&layer, body).unwrap();
    let set_bridge = |enabled: bool| {
        set_layer(&format!(
            "[repo.{}]\nbridge = {enabled}\nbridge_every = \"1h\"\n",
            aida_core::toml_quote::toml_key(&clone_key)
        ))
    };
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
    // Without a running bridge nothing will close recent lag, so it is a
    // finding at once: the layer absent, unreadable or `bridge = false`.
    // trace:BUG-2672 | ai:claude
    let assert_reported_now = |case: &str| {
        let unbridged = report();
        assert_eq!(
            unbridged.total, 1,
            "{case}: {:?}",
            unbridged.remote_drift_info
        );
        assert_eq!(
            unbridged.findings[0].id, "remote-drift-aida-store",
            "{case}"
        );
        assert!(
            unbridged.findings[0]
                .summary
                .contains("mirror is 2 commit(s) behind origin"),
            "{case}: {}",
            unbridged.findings[0].summary
        );
        assert!(
            !unbridged
                .remote_drift_info
                .iter()
                .any(|info| info.contains("wait for the bridge")),
            "{case}"
        );
    };
    assert_reported_now("absent layer");
    set_bridge(false);
    assert_reported_now("bridge disabled");
    set_layer("[repo.\"unterminated\nbridge = true\n");
    assert_reported_now("unreadable layer");
    std::fs::remove_file(&layer).unwrap();
    std::fs::create_dir(&layer).unwrap();
    assert_reported_now("layer is a directory");
    std::fs::remove_dir(&layer).unwrap();
    set_bridge(true);
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

    // Fast-forward lag on main earns the same window as the store, while
    // divergence on either branch trips on the first observation.
    publish("origin", "main");
    let main_lag = report();
    assert_eq!(main_lag.total, 0);
    assert!(main_lag.remote_drift_info.iter().any(|info| {
        info.contains("branch `main`")
            && info.contains("mirror is 2 commit(s) behind origin")
            && info.contains("wait for the bridge")
    }));
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
    let assert_diverged = |diverged: &DoctorReport| {
        assert_eq!(diverged.total, 2, "{:?}", diverged.findings);
        assert!(diverged.remote_drift_info.is_empty());
        for (finding, id) in diverged
            .findings
            .iter()
            .zip(["remote-drift-aida-store", "remote-drift-main"])
        {
            assert_eq!(finding.id, id);
            assert!(
                finding
                    .summary
                    .contains("diverged (mirror: 1 unique, origin: 2 unique)"),
                "{}",
                finding.summary
            );
            assert!(finding.action.contains("reconcile by hand"));
            assert!(!finding.safe_heal);
        }
    };
    // Recent two-sided drift is never waited out, inside or past the window.
    assert_diverged(&report());
    set_window(15);
    assert_diverged(&report());

    // A transport configured only in the observer's own repository (here an
    // upload-pack that serves the hub from an unreachable URL) must reach the
    // isolated fetch too, or the divergence becomes unverifiable.
    let wrapper = root.path().join("serve-origin.sh");
    crate::test_exec::write_executable(
        &wrapper,
        format!(
            "#!/bin/sh\nexec git-upload-pack '{}'\n",
            first.to_str().unwrap()
        ),
    );
    let missing = root.path().join("moved-away.git");
    git(
        observer.path(),
        &["remote", "set-url", "origin", missing.to_str().unwrap()],
    );
    git(
        observer.path(),
        &[
            "config",
            "remote.origin.uploadpack",
            wrapper.to_str().unwrap(),
        ],
    );
    assert_diverged(&report());
}

// trace:BUG-2672 | ai:claude
#[test]
fn remote_drift_grace_requires_a_configured_bridge() {
    use crate::store_local::StoreLocal;
    let bridge = |enabled| StoreLocal {
        bridge: enabled,
        bridge_every: Some("30m".to_string()),
        push_hubs: None,
    };
    // Unreadable, absent (the default entry) and disabled layers earn no
    // grace, even with an explicit doctor window.
    assert_eq!(lag_window_for(None, Some(15)), None);
    assert_eq!(lag_window_for(Some(StoreLocal::default()), Some(15)), None);
    assert_eq!(lag_window_for(Some(bridge(false)), None), None);
    assert_eq!(lag_window_for(Some(bridge(true)), None), Some(3600));
    assert_eq!(lag_window_for(Some(bridge(true)), Some(15)), Some(15 * 60));
}

// trace:BUG-2672 | ai:claude
#[test]
fn remote_drift_lag_window_is_twice_the_bridge_interval() {
    assert_eq!(lag_window_from(None, None), 2 * 3600);
    assert_eq!(lag_window_from(None, Some("30m")), 3600);
    assert_eq!(lag_window_from(None, Some("2h")), 4 * 3600);
    assert_eq!(lag_window_from(None, Some("bogus")), 2 * 3600);
    assert_eq!(
        lag_window_from(None, Some("9000000000000000s")),
        18_000_000_000_000_000
    );
    assert_eq!(lag_window_from(Some(15), Some("2h")), 15 * 60);
    assert_eq!(lag_window_from(Some(0), Some("30m")), 3600);
    assert_eq!(lag_window_from(Some(i64::MAX), None), 2 * 3600);
}

// trace:BUG-2672 | ai:claude
#[test]
fn remote_drift_forwards_only_source_transport_config() {
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = crate::test_git::git_command()
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "core.sshCommand", "ssh -i \"/k ey\"\\x"]);
    git(&["config", "credential.helper", ""]);
    git(&["config", "--add", "credential.helper", "store"]);
    git(&["config", "http.https://h.example/.extraHeader", "X: y"]);
    git(&["config", "remote.origin.uploadpack", "up"]);
    git(&["config", "remote.origin.proxy", "http://p"]);
    git(&["config", "remote.origin.fetch", "+refs/*:refs/*"]);
    git(&["config", "remote.Origin.uploadpack", "wrong-case"]);
    git(&["config", "remote.mirror.vcs", "hg"]);
    git(&["config", "user.name", "Not Transport"]);
    let text =
        forwarded_transport_config(repo.path(), &[("origin", "o"), ("mirror", "m")]).unwrap();
    let out = repo.path().join("forwarded");
    std::fs::write(&out, &text).unwrap();
    let listed = crate::test_git::git_command()
        .args(["config", "--file"])
        .arg(&out)
        .args(["--list"])
        .output()
        .unwrap();
    assert!(listed.status.success(), "{text}");
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "core.sshcommand=ssh -i \"/k ey\"\\x\n\
         credential.helper=\n\
         credential.helper=store\n\
         http.https://h.example/.extraheader=X: y\n\
         remote.o.uploadpack=up\n\
         remote.o.proxy=http://p\n\
         remote.m.vcs=hg\n"
    );
}
