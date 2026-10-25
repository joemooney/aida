// trace:TASK-1793 | ai:codex
#![cfg(unix)]
// trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
use super::*;
use aida_core::db::{DatabaseBackend, GitBackend};
use aida_core::models::Requirement;
use std::path::Path;

fn bare_probe(binary: &Path) -> (std::ffi::OsString, Vec<String>) {
    (binary.as_os_str().to_os_string(), vec!["--version".into()])
}

fn probe_version(binary: &Path) -> Option<String> {
    let (program, args) = bare_probe(binary);
    reported_harness_version(&program, &args, None)
}

// Invoked by the fake vendor CLI in a separate, actually launched session.
#[cfg(unix)]
#[test]
fn task1793_launched_write_child() {
    let Some(root) = std::env::var_os("AIDA_TEST_1793_STORE") else {
        return;
    };
    let backend = crate::test_env::home_fixture_backend(Path::new(&root))
        .unwrap()
        .with_auto_commit(false);
    let mut req = Requirement::new("Launched session write".into(), "body".into());
    req.spec_id = Some("TASK-1".into());
    let mut created = backend.add_requirement(req).unwrap();
    created.title = "Launched session edit".into();
    backend.update_requirement(&created).unwrap();
}

#[cfg(unix)]
#[test]
fn task1793_foreground_and_background_versions_reach_canonical_writes() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    std::fs::write(project.join("vendor-version"), "codex-cli 9.8.7\n").unwrap();
    let binary = tmp.path().join("vendor");
    let test_bin = crate::aida_bin::current_executable_path().unwrap();
    let config = AgentLaunchConfig {
        agent_type: "codex",
        binary: "codex",
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let plan = AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: None,
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "version-fixture".into(),
        lease_id: Some("fixture-child-session".into()),
        native_session_id: None,
        resumed_from: None,
    };
    for background in [false, true] {
        for (case, probe, expected) in [
            ("reported", "cat vendor-version", Some("codex-cli 9.8.7")),
            ("failed", "printf 'wrong version\\n'; exit 1", None),
            ("empty", "exit 0", None),
            ("multiline", "printf 'banner\\nversion 1\\n'", None),
        ] {
            let store = tmp.path().join(format!("store-{background}-{case}"));
            let store_string = store.to_str().unwrap();
            let _env = crate::test_env::EnvVarsGuard::apply(&[
                ("AIDA_HARNESS_VERSION", Some("stale-parent-version")),
                ("AIDA_TEST_1793_STORE", Some(store_string)),
                ("AIDA_SESSION_ID", Some("stale-parent-session")),
            ]);
            crate::test_exec::write_executable(
                &binary,
                format!(
                    "#!/bin/sh\nif [ \"${{1:-}}\" = --version ]; then\n{probe}\nexit 0\nfi\nexec '{}' --exact agent_launch::harness_version_tests::task1793_launched_write_child --nocapture\n",
                    test_bin.display()
                ),
            );
            if background {
                let output = build_agent_new_bg_command(&binary, &config, &plan, &[], None)
                    .unwrap()
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            } else {
                run_tracked_agent(&binary, &config, &plan, None, &[], None, false, false).unwrap();
            }
            let backend = crate::test_env::home_fixture_backend(&store)
                .unwrap()
                .with_auto_commit(false);
            let saved = backend.load().unwrap();
            assert_eq!(saved.requirements.len(), 1);
            let req = &saved.requirements[0];
            assert_eq!(req.title, "Launched session edit");
            let filing = req.filed_at.as_ref().unwrap();
            assert_eq!(
                filing.harness_version.as_deref(),
                expected,
                "{background}/{case}"
            );
            assert_eq!(filing.session.as_deref(), Some("fixture-child-session"));
            assert_eq!(
                req.history.last().unwrap().harness_version.as_deref(),
                expected
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn task1793_sleeping_version_probe_is_bounded_and_launch_remains_usable() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("vendor");
    crate::test_exec::write_executable(&binary, "#!/bin/sh\nexec sleep 60\n");
    let mut command = std::process::Command::new("/bin/sh");
    command.args(["-c", "printf '%s' \"${AIDA_HARNESS_VERSION-unset}\""]);
    let start = std::time::Instant::now();
    apply_harness_version(&mut command, bare_probe(&binary));
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"unset");
}

#[cfg(unix)]
#[test]
fn task1793_exited_probe_with_inherited_pipe_is_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("vendor");
    crate::test_exec::write_executable(
        &binary,
        "#!/bin/sh\nsleep 60 &\nprintf 'vendor 1.2\\n'\nexit 0\n",
    );
    let start = std::time::Instant::now();
    assert_eq!(probe_version(&binary), None);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn task1793_invalid_or_oversized_output_is_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("vendor");
    for body in [
        "printf '\\377'",
        "printf 'version\\033[31m'",
        "echo version >&2",
        "i=0; while [ $i -lt 258 ]; do printf x; i=$((i + 1)); done",
        "while :; do printf 'not a version'; done",
    ] {
        crate::test_exec::write_executable(&binary, format!("#!/bin/sh\n{body}\n"));
        let start = std::time::Instant::now();
        assert_eq!(probe_version(&binary), None, "{body}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
    assert_eq!(probe_version(&tmp.path().join("missing")), None);
}

#[cfg(unix)]
#[test]
fn task1793_version_accepts_256_bytes_plus_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("vendor");
    let version = "x".repeat(256);
    crate::test_exec::write_executable(&binary, format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"));
    assert_eq!(probe_version(&binary), Some(version));
}

fn fixture_config(agent_type: &'static str) -> AgentLaunchConfig {
    AgentLaunchConfig {
        agent_type,
        binary: agent_type,
        default_args: vec![],
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    }
}

fn fixture_plan(project: &Path, lease_id: Option<&str>) -> AgentLaunchPlan {
    AgentLaunchPlan {
        project_root: project.to_path_buf(),
        launch_cwd: project.to_path_buf(),
        role: None,
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "version-fixture".into(),
        lease_id: lease_id.map(str::to_string),
        native_session_id: None,
        resumed_from: None,
    }
}

/// The launching seat's grant, role and purpose never reach the `--version`
/// probe — the same grant both launch paths strip from the child itself.
// trace:TASK-1793 | ai:claude
#[cfg(unix)]
#[test]
fn task1793_probe_does_not_inherit_parent_seat_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = tmp.path().join("vendor");
    crate::test_exec::write_executable(
        &binary,
        "#!/bin/sh\nprintf 'grant=%s role=%s purpose=%s\\n' \"${AIDA_SESSION_GRANT-none}\" \"${AIDA_SESSION_ROLE-none}\" \"${AIDA_SESSION_PURPOSE-none}\"\n",
    );
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (seat_authority::GRANT_ENV, Some("parent-seat-grant")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_SESSION_PURPOSE", Some("parent purpose")),
        ("AIDA_OS_WRAP", Some("0")),
    ]);
    let clean = Some("grant=none role=none purpose=none".to_string());
    assert_eq!(probe_version(&binary), clean);
    // Through the real `--bg` launch path: the stamped version is the
    // probe's own env dump, so a leaked grant would appear in it.
    let command = build_agent_new_bg_command(
        &binary,
        &fixture_config("codex"),
        &fixture_plan(&project, None),
        &[],
        None,
    )
    .unwrap();
    let stamped = command
        .get_envs()
        .find(|(key, _)| *key == "AIDA_HARNESS_VERSION")
        .and_then(|(_, value)| value)
        .map(|value| value.to_string_lossy().into_owned());
    assert_eq!(stamped, clean);
}

/// With os_wrap on, the foreground child's probe resolves through the same
/// bwrap wrapper as the child: identical sandbox argv, only the vendor args
/// differ. A wrap that cannot be set up fails closed for the probe too.
// trace:TASK-1793 | ai:claude
#[cfg(unix)]
#[test]
fn task1793_probe_uses_the_childs_os_wrap_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(project.join(".aida/config.toml"), "").unwrap();
    let binary = project.join("vendor");
    let outside = tmp.path().join("probe-ran-unconfined");
    crate::test_exec::write_executable(
        &binary,
        format!(
            "#!/bin/sh\n: > '{}' 2>/dev/null\nprintf 'claude 1.0\\n'\n",
            outside.display()
        ),
    );
    let config = fixture_config("claude");
    let plan = fixture_plan(&project, None);
    for wrap in ["1", "0"] {
        let _ = std::fs::remove_file(&outside);
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_OS_WRAP", Some(wrap))]);
        let child = resolved_agent_program_and_args(&binary, &config, &plan, &[], true);
        let probe = harness_version_probe_program_and_args(&binary, &config, &plan, true);
        let (child, probe) = match (child, probe) {
            (Ok(child), Ok(probe)) => (child, probe),
            // bwrap unusable on this host: both refuse; never an unconfined probe.
            (Err(_), Err(_)) => continue,
            other => panic!("probe and child disagree on os_wrap: {other:?}"),
        };
        assert_eq!(probe.0, child.0, "os_wrap={wrap}");
        assert_eq!(probe.1.last().map(String::as_str), Some("--version"));
        assert_eq!(
            &probe.1[..probe.1.len() - 1],
            &child.1[..],
            "os_wrap={wrap}"
        );
        assert_eq!(
            reported_harness_version(&probe.0, &probe.1, Some(&project)),
            Some("claude 1.0".to_string()),
            "os_wrap={wrap}"
        );
        // Inside bwrap the host filesystem outside the worktree is read-only
        // (and /tmp is a private tmpfs), so the probe's marker never lands.
        assert_eq!(outside.exists(), wrap == "0", "os_wrap={wrap}");
    }
}
