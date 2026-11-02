//! Slice-A contract: closed input, typed capability/policy, real fixed lane.
// trace:TASK-2705 | ai:codex
use super::*;

#[cfg(test)]
#[test]
fn task_2705_closed_containment_tracer() {
    let data: Capabilities = toml::from_str(include_str!("vendors.toml")).unwrap();
    assert_eq!(data.version, 1);
    assert!(matches!(
        data.vendor["opencode"].native,
        NativeAdapter::None
    ));
    assert!(supervisor::terminal_preflight(true, Some("1")).is_err());
    assert!(supervisor::terminal_preflight(true, None).is_err());
    assert!(supervisor::terminal_preflight(true, Some("0\n")).is_ok());
    assert!(supervisor::terminal_preflight(false, None).is_ok());
    let tmp = tempfile::tempdir().unwrap();
    let unknown = probe(tmp.path(), "unregistered");
    assert_eq!(unknown.status, ProbeStatus::Unsupported);
    assert!(!unknown.aida_containment_applied);
    assert!(!unknown.ordinary_launch_supported);
    let no_policy = probe(tmp.path(), "codex");
    assert_eq!(no_policy.status, ProbeStatus::Refused);
    assert!(no_policy.cause.contains("trusted"));

    // Required-live runs never turn missing prerequisites into a pass/skip.
    let required = std::env::var("AIDA_REQUIRE_BWRAP_LIVE").as_deref() == Ok("1")
        || std::env::var("AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE").as_deref() == Ok("1");
    // Only nested unshare(CLONE_NEWUSER) failures are host properties; a
    // nested mount/unmount failure is an ordinary (possibly boundary) failure.
    // trace:TASK-2705 | ai:claude
    use supervisor::classify_preflight_failure as classify;
    assert_eq!(
        classify("unshare", libc::EPERM),
        Some(IsolationReason::HostPolicyDenied)
    );
    assert_eq!(
        classify("unshare", libc::EACCES),
        Some(IsolationReason::HostPolicyDenied)
    );
    assert_eq!(
        classify("unshare", libc::ENOSPC),
        Some(IsolationReason::HostResourceLimit)
    );
    assert_eq!(classify("unshare", libc::EINVAL), None);
    for phase in ["mount_positive", "unmount_positive"] {
        for errno in [libc::EPERM, libc::EACCES, libc::ENOSPC] {
            assert_eq!(classify(phase, errno), None, "{phase} {errno}");
        }
    }

    match supervisor::run_probe(|| {}) {
        // Host lacks nested userns: assert the typed result, never a pass.
        // Required-live runs must prove the full lane, so they fail here.
        // trace:TASK-2705 | ai:claude
        Ok(supervisor::TracerOutcome::IsolationUnavailable {
            reason,
            phase,
            errno,
        }) => {
            assert_eq!(phase, "unshare");
            assert_eq!(classify(&phase, errno), Some(reason), "{phase} {errno}");
            // The stalled lane hits the same up-front probe, never a pass.
            assert!(matches!(
                supervisor::timeout_probe(),
                Ok(supervisor::TracerOutcome::IsolationUnavailable { .. })
            ));
            assert!(
                !required,
                "required-live containment tracer: isolation unavailable \
                 ({reason:?}, phase={phase}, errno={errno}); the full lane did not run"
            );
            eprintln!(
                "ISOLATION-UNAVAILABLE live containment tracer (typed result asserted, \
                 not acceptance evidence): {reason:?} phase={phase} errno={errno}"
            );
        }
        Ok(supervisor::TracerOutcome::Passed(checks)) => {
            let timeout = supervisor::timeout_probe().unwrap_err();
            assert!(
                format!("{timeout:#}").contains("owned wrapper and namespace init reaped"),
                "{timeout:#}"
            );
            for name in [
                "write_workspace",
                "write_store",
                "write_cache",
                "write_state",
                "outside_write",
                "protected_modify_.aida",
                "protected_create_.git",
                "protected_rename_.codex",
                "no_new_privs",
                "capabilities_dropped",
                "no_host_pid_visible",
                "host_proc_environ_unreachable",
                "exec_boundary_only_standard_fds_present",
                "only_standard_fds_present",
                "host_controls_unchanged",
                "wrapper_reaped",
                "namespace_init_reaped",
                "nested_userns_cannot_unmount_protected",
                "inherited_fd_write",
                "inherited_file_fd_write",
                "inherited_directory_fd_create",
                "private_namespace_cgroup",
                "no_authority_env_forwarded",
                "reaper_has_no_host_handles",
            ] {
                assert!(
                    checks.iter().any(|c| c == name),
                    "missing {name}: {checks:?}"
                );
            }
        }
        Err(error)
            if !required && {
                let cause = format!("{error:#}");
                cause.contains("bubblewrap unavailable")
                    || cause.contains("Creating new namespace failed")
                    || cause.contains("No permissions to create new namespace")
            } =>
        {
            eprintln!("SKIP live containment tracer (not acceptance evidence): {error:#}")
        }
        Err(error) => panic!("required-live containment tracer refused: {error:#}"),
    }
}

// Same fixture never reads the live repo/config or executes config commands.
#[cfg(test)]
fn trusted_fixture(root: &Path, policy: &str) {
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(root.join(".aida/config.toml"), policy).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.name", "probe-test"],
        vec!["config", "user.email", "probe@example.invalid"],
        vec!["add", ".aida/config.toml"],
        vec!["commit", "-q", "-m", "trusted"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    ] {
        let out = crate::test_git::git_command()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(test)]
#[test]
fn task_2705_policy_and_partial_backend_remain_closed() {
    let root = tempfile::tempdir().unwrap();
    trusted_fixture(root.path(), "[contained.vendors]\ncodex = \"invalid\"\n");
    let report = probe(root.path(), "codex");
    assert_eq!(report.status, ProbeStatus::Refused);
    assert!(report.cause.contains("aida or native"));
    // An uncommitted self-widening edit cannot replace the pinned policy.
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[contained.vendors]\ncodex = \"aida\"\ncommand = \"touch NEVER\"\n",
    )
    .unwrap();
    let report = probe(root.path(), "codex");
    assert_eq!(report.status, ProbeStatus::Refused);
    assert!(report.cause.contains("aida or native"));
    assert!(!root.path().join("NEVER").exists());
    assert!(!report.aida_containment_applied);
    assert!(!report.ordinary_launch_supported);
}

/// Full isolation path when the host allows it; otherwise the typed
/// `isolation=unavailable` refusal (`host_policy_denied` or
/// `host_resource_limit`) with phase and errno, closed with no checks or
/// credit. Never a silent pass, and never a pass at all when `required`.
// trace:TASK-2705 | ai:claude
#[cfg(test)]
fn assert_ready_or_isolation_unavailable(report: &ProbeReport, required: bool) {
    match report.isolation.state {
        IsolationState::Unavailable => {
            assert_eq!(report.status, ProbeStatus::Refused, "{}", report.cause);
            let reason = report.isolation.reason.expect("typed reason");
            assert_eq!(report.isolation.phase.as_deref(), Some("unshare"));
            assert!(report.isolation.errno.is_some_and(|e| e != 0));
            let (text, wire) = match reason {
                IsolationReason::HostPolicyDenied => ("host policy denied", "host_policy_denied"),
                IsolationReason::HostResourceLimit => {
                    ("host resource limit", "host_resource_limit")
                }
            };
            assert!(report.cause.contains(text), "{}", report.cause);
            assert!(report.checks.is_empty());
            assert!(!report.aida_containment_applied);
            let json = serde_json::to_value(report).unwrap();
            assert_eq!(json["isolation"]["state"], "unavailable");
            assert_eq!(json["isolation"]["reason"], wire);
            assert!(
                !required,
                "required-live probe: isolation unavailable, full lane not run: {}",
                report.cause
            );
        }
        IsolationState::Available => {
            assert_eq!(report.status, ProbeStatus::Ready, "{}", report.cause);
        }
        IsolationState::Unknown => assert!(
            !required,
            "required-live probe refused before isolation: {}",
            report.cause
        ),
    }
}

#[cfg(test)]
#[test]
fn task_2705_trusted_snapshot_cannot_execute_stored_shell() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("NEVER");
    trusted_fixture(
        root.path(),
        &format!(
            "[contained.vendors]\ncodex = \"native\"\ncommand = \"touch {}\"\n",
            marker.display()
        ),
    );
    std::fs::write(
        root.path().join(".aida/config.toml"),
        "[contained.vendors]\ncodex = \"aida\"\n",
    )
    .unwrap();
    let report = probe(root.path(), "codex");
    assert_eq!(report.configured_posture, Some(ConfiguredPosture::Native));
    assert!(report.policy_sha.is_some());
    assert!(!report.ordinary_launch_supported);
    assert!(!report.aida_containment_applied);
    assert!(!marker.exists());
    let required = std::env::var("AIDA_REQUIRE_BWRAP_LIVE").as_deref() == Ok("1")
        || std::env::var("AIDA_REQUIRE_CODEX_CONTAINMENT_LIVE").as_deref() == Ok("1");
    // Any host-policy denial must be the typed report, whether or not required.
    assert_ready_or_isolation_unavailable(&report, required);
    let other = probe(root.path(), "opencode");
    assert!(matches!(
        other.capability.as_ref().unwrap().native,
        NativeAdapter::None
    ));
    assert!(!other.ordinary_launch_supported);
    assert!(!other.aida_containment_applied);
    assert_ready_or_isolation_unavailable(&other, required);
    if other.status == ProbeStatus::Ready {
        assert!(other.checks.iter().any(|c| c == "write_workspace"));
    }
    // Production selector is deliberately still refused for partial AIDA Codex.
    // This snapshot is native and retains its existing successful selection.
    assert_eq!(
        crate::session::containment_posture_in(root.path(), "codex", true).unwrap(),
        crate::session::ContainmentPosture::Native
    );
}

#[cfg(test)]
#[test]
fn task_2705_readiness_channels_are_bounded() {
    // Exercise the production nonblocking collector's hard cap, without a
    // subprocess or arbitrary command hook for testing.
    assert!(supervisor::channel_limit_fixture());
}

#[cfg(test)]
#[test]
fn task_2705_public_probe_has_no_command_override() {
    use clap::Parser;
    assert!(crate::config_cmd::ConfigArgs::try_parse_from([
        "config",
        "permissions",
        "probe",
        "--vendor",
        "codex",
        "--json"
    ])
    .is_ok());
    for argument in ["--command", "--executable", "--state-path", "--"] {
        assert!(crate::config_cmd::ConfigArgs::try_parse_from([
            "config",
            "permissions",
            "probe",
            "--vendor",
            "codex",
            argument,
            "/bin/sh"
        ])
        .is_err());
    }
    assert!(toml::from_str::<Capability>(
        "native = \"shell\"\nipc = \"private_runtime\"\ncredentials = \"unverified\""
    )
    .is_err());
}

#[cfg(test)]
#[test]
fn task_2705_policy_errors_are_redacted_and_aida_selector_stays_closed() {
    let malformed = tempfile::tempdir().unwrap();
    trusted_fixture(
        malformed.path(),
        "[contained]\nvalue = \"SYNTHETIC_MUST_NOT_PRINT\n",
    );
    let report = probe(malformed.path(), "codex");
    assert_eq!(report.status, ProbeStatus::Refused);
    assert!(report.cause.contains("malformed TOML"));
    assert!(!report.cause.contains("SYNTHETIC_MUST_NOT_PRINT"));
    let partial = tempfile::tempdir().unwrap();
    trusted_fixture(partial.path(), "[contained.vendors]\ncodex = \"aida\"\n");
    assert!(crate::session::containment_posture_in(partial.path(), "codex", true).is_err());
    assert_eq!(
        crate::session::containment_posture_in(partial.path(), "claude", true).unwrap(),
        crate::session::ContainmentPosture::Aida
    );
}
