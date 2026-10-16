// trace:TASK-1793 | ai:codex
#![cfg(unix)]
use super::*;
use aida_core::db::{DatabaseBackend, GitBackend};
use aida_core::models::Requirement;
use std::path::Path;

// Invoked by the fake vendor CLI in a separate, actually launched session.
#[cfg(unix)]
#[test]
fn task1793_launched_write_child() {
    let Some(root) = std::env::var_os("AIDA_TEST_1793_STORE") else {
        return;
    };
    let backend = GitBackend::new(Path::new(&root))
        .unwrap()
        .with_auto_commit(false);
    let mut req = Requirement::new("Launched session write".into(), "body".into());
    req.spec_id = Some("TASK-1".into());
    backend.add_requirement(req).unwrap();
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
        lease_id: None,
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
                ("AIDA_SESSION_ID", Some("launched-version-session")),
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
            let backend = GitBackend::new(&store).unwrap().with_auto_commit(false);
            let saved = backend.load().unwrap();
            assert_eq!(saved.requirements.len(), 1);
            let req = &saved.requirements[0];
            let filing = req.filed_at.as_ref().unwrap();
            assert_eq!(
                filing.harness_version.as_deref(),
                expected,
                "{background}/{case}"
            );
            assert_eq!(filing.session.as_deref(), Some("launched-version-session"));
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
    apply_harness_version(&mut command, &binary);
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
    assert_eq!(reported_harness_version(&binary, None), None);
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
        assert_eq!(reported_harness_version(&binary, None), None, "{body}");
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }
    assert_eq!(
        reported_harness_version(&tmp.path().join("missing"), None),
        None
    );
}

#[cfg(unix)]
#[test]
fn task1793_version_accepts_256_bytes_plus_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let binary = tmp.path().join("vendor");
    let version = "x".repeat(256);
    crate::test_exec::write_executable(&binary, format!("#!/bin/sh\nprintf '%s\\n' '{version}'\n"));
    assert_eq!(reported_harness_version(&binary, None), Some(version));
}
