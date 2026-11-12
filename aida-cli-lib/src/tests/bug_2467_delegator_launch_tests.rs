// trace:BUG-2467 | ai:codex
use super::*;
use crate::gate::seat_authority as authority;
use crate::test_env::EnvVarsGuard;

#[test]
fn bug_2467_parses_delegation_for_every_vendor() {
    for vendor in ["claude", "codex", "antigravity"] {
        let cli = crate::TEST_CLI
            .try_parse_from([
                "aida",
                "agent",
                "new",
                vendor,
                "--role",
                "advisor",
                "--delegate-seat",
                "reviewer",
                "--delegate-seat",
                "implementer",
            ])
            .unwrap();
        let AgentArgs {
            inner: AgentCommand::New {
                command: Some(cmd), ..
            },
        } = cli.command.registered::<AgentArgs>()
        else {
            panic!("expected launch")
        };
        let (AgentNewCommand::Claude { delegate_seats, .. }
        | AgentNewCommand::Codex { delegate_seats, .. }
        | AgentNewCommand::Antigravity { delegate_seats, .. }) = cmd;
        assert_eq!(delegate_seats, &["reviewer", "implementer"]);
    }
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "agent",
            "new",
            "--role",
            "advisor",
            "--delegate-seat",
            "reviewer",
        ])
        .unwrap();
    let AgentArgs {
        inner:
            AgentCommand::New {
                delegate_seats,
                command: None,
                ..
            },
    } = cli.command.registered::<AgentArgs>()
    else {
        panic!("expected default-vendor launch")
    };
    assert_eq!(delegate_seats, &["reviewer"]);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-233.ac70d987 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-233.acb17271 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-233.ac32bf6b | ai:codex
#[test]
fn bug_2467_launched_delegators_mint_workers_without_widening() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", authority::GRANT_ENV, "AIDA_CONTAINED"]);
    env.set_key("AIDA_USER", "delegator-operator");
    env.unset_key("AIDA_CONTAINED");
    let seats = ["advisor", "integrator", "reviewer", "implementer"];
    let parent = authority::test_support::mint_grant_for(
        tmp.path(),
        "delegator-operator",
        "advisor",
        &seats,
    );
    env.set_key(authority::GRANT_ENV, &parent);
    let fake = tmp.path().join("claude");
    crate::test_exec::write_executable(
        &fake,
        r#"#!/bin/sh
case "$1" in --version) echo "2.0.0 (Claude Code)"; exit 0;; esac
printf '%s' "$AIDA_SESSION_GRANT" > "$AIDA_PROJECT_ROOT/child-grant"
"#,
    );
    let mut config = AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    for (role, selected) in [
        ("advisor", Vec::new()),
        ("integrator", Vec::new()),
        ("advisor", vec!["reviewer".to_string()]),
    ] {
        apply_agent_default_flags(
            &mut config,
            tmp.path(),
            AgentDefaultFlagOptions::new(false, Vec::new(), None).delegating(selected.clone()),
            false,
        )
        .unwrap();
        env.set_key("AIDA_USER", "delegator-operator");
        env.set_key(authority::GRANT_ENV, &parent);
        let plan = AgentLaunchPlan {
            project_root: tmp.path().into(),
            launch_cwd: tmp.path().into(),
            role: Some(role.into()),
            role_instance: RoleInstanceKind::Driver,
            current_spec: None,
            name: "delegator-child".into(),
            lease_id: None,
            native_session_id: None,
            resumed_from: None,
        };
        run_tracked_agent(&fake, &config, &plan, None, &[], None, false, false).unwrap();
        let id = std::fs::read_to_string(tmp.path().join("child-grant")).unwrap();
        let child = authority::read_grant(&id).unwrap();
        let expected: Vec<String> = if selected.is_empty() {
            seats.iter().map(|s| s.to_string()).collect()
        } else {
            selected
        };
        assert_eq!(child.delegable_seats, expected);
        assert_eq!(
            child.expires_at,
            authority::read_grant(&parent).unwrap().expires_at
        );
        env.set_key("AIDA_USER", "delegator-child");
        env.set_key(authority::GRANT_ENV, &id);
        // This is review watch's admission gate, followed by its worker issuer.
        authority::validate_child_delegation(tmp.path(), "reviewer").unwrap();
        for worker in ["reviewer", "implementer"] {
            if !expected.iter().any(|s| s == worker) {
                assert!(
                    crate::launch::issue_launch_child(tmp.path(), worker, "grandchild", false)
                        .is_err()
                );
                continue;
            }
            let grandchild =
                crate::launch::issue_launch_child(tmp.path(), worker, "grandchild", false).unwrap();
            assert!(grandchild.delegable_seats.is_empty());
            assert_eq!(grandchild.parent_grant_id.as_deref(), Some(id.as_str()));
        }
    }
    env.set_key("AIDA_USER", "delegator-operator");
    env.set_key(authority::GRANT_ENV, &parent);
    config.delegate_seats = vec!["reviewer".into()];
    let narrowed = authority::issue_launch_child_with_delegation(
        tmp.path(),
        "advisor",
        "delegator-child",
        false,
        &config.delegate_seats,
    )
    .unwrap();
    assert_eq!(narrowed.delegable_seats, ["reviewer"]);
    env.set_key("AIDA_USER", "delegator-child");
    env.set_key(authority::GRANT_ENV, &narrowed.id);
    let refusal = authority::issue_launch_child_with_delegation(
        tmp.path(),
        "reviewer",
        "worker",
        false,
        &["implementer".into()],
    )
    .unwrap_err();
    assert!(format!("{refusal:#}").contains("requires an advisor or integrator"));
    assert!(crate::launch::issue_launch_child(tmp.path(), "implementer", "worker", false).is_err());
    env.set_key("AIDA_USER", "delegator-operator");
    env.set_key(authority::GRANT_ENV, &parent);
    let refusal = authority::issue_launch_child_with_delegation(
        tmp.path(),
        "advisor",
        "child",
        false,
        &["product".into()],
    )
    .unwrap_err();
    let message = format!("{refusal:#}");
    assert!(message.contains("does not delegate `product`"), "{message}");
    assert!(message.contains("aida role enter advisor"), "{message}");
}

// trace:BUG-2467 | ai:codex
#[test]
fn bug_2467_revoking_root_invalidates_grandchild_live_and_recorded_authority() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", authority::GRANT_ENV]);
    env.set_key("AIDA_USER", "operator");
    let root = authority::test_support::mint_grant_for(
        tmp.path(),
        "operator",
        "advisor",
        &["advisor", "reviewer"],
    );
    env.set_key(authority::GRANT_ENV, &root);
    let child = crate::launch::issue_launch_child(tmp.path(), "advisor", "child", false).unwrap();
    env.set_key("AIDA_USER", "child");
    env.set_key(authority::GRANT_ENV, &child.id);
    let worker =
        crate::launch::issue_launch_child(tmp.path(), "reviewer", "worker", false).unwrap();
    env.set_key("AIDA_USER", "worker");
    env.set_key(authority::GRANT_ENV, &worker.id);
    assert!(authority::current_grant(tmp.path()).is_some());
    let before = chrono::Utc::now();
    assert!(authority::grant_authorized_at(
        tmp.path(),
        &worker.id,
        "reviewer",
        before,
        &worker.binding
    ));
    authority::revoke_by_id(&root).unwrap();
    assert!(authority::current_grant(tmp.path()).is_none());
    assert!(!authority::grant_authorized_at(
        tmp.path(),
        &worker.id,
        "reviewer",
        chrono::Utc::now(),
        &worker.binding
    ));
    assert!(authority::grant_authorized_at(
        tmp.path(),
        &worker.id,
        "reviewer",
        before,
        &worker.binding
    ));
}

// Detached Codex uses this issuer, including its one-use launch capability.
#[test]
fn bug_2467_detached_delegators_preserve_narrowing_and_refuse_widening() {
    let tmp = tempfile::tempdir().unwrap();
    let mut env = EnvVarsGuard::snapshot(&["AIDA_USER", authority::GRANT_ENV]);
    env.set_key("AIDA_USER", "detached-operator");
    let parent = authority::test_support::mint_grant_for(
        tmp.path(),
        "detached-operator",
        "advisor",
        &["advisor", "integrator", "reviewer", "implementer"],
    );
    env.set_key(authority::GRANT_ENV, &parent);
    for role in ["advisor", "integrator"] {
        let (grant, capability) = authority::issue_detached_launch_child(
            tmp.path(),
            role,
            "detached-child",
            false,
            "attempt",
            "scope",
            &["reviewer".into()],
        )
        .unwrap();
        assert_eq!(capability.grant_id(), grant.id);
        assert_eq!(
            authority::read_grant(&grant.id).unwrap().delegable_seats,
            ["reviewer"]
        );
        assert_eq!(
            grant.expires_at,
            authority::read_grant(&parent).unwrap().expires_at
        );
        let err = authority::issue_detached_launch_child(
            tmp.path(),
            role,
            "detached-child",
            false,
            "attempt",
            "scope",
            &["product".into()],
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("does not delegate `product`"));
    }
}

#[test]
fn bug_2467_resume_restores_persisted_narrowing_and_revalidates_parent() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut env = EnvVarsGuard::snapshot(&[
        "AIDA_USER",
        authority::GRANT_ENV,
        "AIDA_CONTAINED",
        "AIDA_TEST_HOME",
        "PATH",
    ]);
    env.set_key("AIDA_USER", "resume-operator");
    env.unset_key("AIDA_CONTAINED");
    env.set_key("AIDA_TEST_HOME", tmp.path());
    let parent = authority::test_support::mint_grant_for(
        tmp.path(),
        "resume-operator",
        "advisor",
        &["advisor", "reviewer", "implementer"],
    );
    env.set_key(authority::GRANT_ENV, &parent);
    let fake = tmp.path().join("claude");
    crate::test_exec::write_executable(
        &fake,
        r#"#!/bin/sh
case "$1" in --version) echo "2.0.0 (Claude Code)"; exit 0;; esac
printf '%s' "$AIDA_SESSION_GRANT" > "$AIDA_PROJECT_ROOT/child-grant"
"#,
    );
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    env.set_key(
        "PATH",
        std::env::join_paths(
            std::iter::once(tmp.path().to_path_buf()).chain(std::env::split_paths(&old_path)),
        )
        .unwrap(),
    );
    let transcript = session::claude_project_dir(tmp.path())
        .unwrap()
        .join("resume-session.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(transcript, "").unwrap();
    let config = AgentLaunchConfig {
        delegate_seats: vec!["reviewer".into()],
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    let plan = AgentLaunchPlan {
        project_root: tmp.path().into(),
        launch_cwd: tmp.path().into(),
        role: Some("advisor".into()),
        role_instance: RoleInstanceKind::Driver,
        current_spec: None,
        name: "resumed-advisor".into(),
        lease_id: None,
        native_session_id: Some("resume-session".into()),
        resumed_from: None,
    };
    run_tracked_agent(&fake, &config, &plan, None, &[], None, false, false).unwrap();
    let (_, entry) = agent_registry::load_entries(tmp.path())
        .into_iter()
        .find(|(_, row)| row.name.as_deref() == Some("resumed-advisor"))
        .unwrap();
    assert_eq!(entry.delegate_seats, ["reviewer"]);
    assert!(entry.ended_at.is_some());
    agent_resume_ended(tmp.path(), entry.clone()).unwrap();
    let id = std::fs::read_to_string(tmp.path().join("child-grant")).unwrap();
    assert_eq!(
        authority::read_grant(&id).unwrap().delegable_seats,
        ["reviewer"]
    );
    // Resume cannot restore authority the current parent no longer delegates.
    let less = authority::test_support::mint_grant_for(
        tmp.path(),
        "resume-operator",
        "advisor",
        &["advisor", "implementer"],
    );
    env.set_key(authority::GRANT_ENV, &less);
    std::fs::remove_file(tmp.path().join("child-grant")).unwrap();
    let err = agent_resume_ended(tmp.path(), entry).unwrap_err();
    assert!(format!("{err:#}").contains("does not delegate `reviewer`"));
    assert!(!tmp.path().join("child-grant").exists());
}

// trace:BUG-2467 | ai:codex
// trace:BUG-b6b1278890774ef993d2ebc327862470-233.ac63c1c5 | ai:codex
#[test]
fn bug_2467_standing_orders_dispatch_inherits_parent_delegation() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let home = tmp.path().join("home");
    let bin = tmp.path().join("bin");
    for dir in [&root, &home, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", bin.display());
    let mut env = EnvVarsGuard::apply(&[
        ("HOME", home.to_str()),
        ("AIDA_HOME", home.join(".aida").to_str()),
        ("AIDA_TEST_HOME", home.to_str()),
        ("PATH", Some(&path)),
        ("AIDA_USER", Some("standing-orders-operator")),
        ("AIDA_CONTAINED", None),
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_SESSION_ID", None),
        ("AIDA_SESSION_SCOPE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_BUILD_PROFILE", None),
        ("AIDA_BIN", None),
        (authority::GRANT_ENV, None),
    ]);
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    crate::test_env::home_fixture_storage(root.join(".aida-store"))
        .save(&aida_core::models::RequirementsStore::new())
        .unwrap();
    let seats = ["advisor", "integrator", "reviewer", "implementer"];
    let parent = authority::test_support::mint_grant_for(
        &root,
        "standing-orders-operator",
        "advisor",
        &seats,
    );
    env.set_key(authority::GRANT_ENV, &parent);
    crate::test_exec::write_executable(
        &bin.join("claude"),
        r#"#!/bin/sh
case "$1" in --version) echo "2.0.0 (Claude Code)"; exit 0;; esac
printf '%s' "$AIDA_SESSION_GRANT" > "$AIDA_PROJECT_ROOT/child-grant"
printf '%s\n' "$@" > "$AIDA_PROJECT_ROOT/child-argv"
"#,
    );
    crate::test_exec::write_executable(
        &bin.join("aida"),
        "#!/bin/sh\necho 'aida 0.16.0 fixture'\n",
    );
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&root).unwrap();
    // The exact documented line: no extra flags or direct launcher calls.
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida",
            "agent",
            "new",
            "claude",
            "--role",
            "advisor",
            "--permission-mode",
            "auto",
        ])
        .unwrap();
    let AgentArgs { inner } = cli.command.registered::<AgentArgs>();
    let result = handle_agent_command(&inner);
    std::env::set_current_dir(original_cwd).unwrap();
    result.unwrap();
    let id = std::fs::read_to_string(root.join("child-grant")).unwrap();
    let child = authority::read_grant(&id).unwrap();
    assert_ne!(id, parent);
    assert_eq!(child.seat, "advisor");
    assert_eq!(child.parent_grant_id.as_deref(), Some(parent.as_str()));
    assert_eq!(child.delegable_seats, seats);
    let argv = std::fs::read_to_string(root.join("child-argv")).unwrap();
    assert!(argv.contains("--permission-mode\nauto\n"), "{argv}");
    assert!(!argv.contains("dangerously-skip-permissions"), "{argv}");
}
