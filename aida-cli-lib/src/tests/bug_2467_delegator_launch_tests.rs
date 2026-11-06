// trace:BUG-2467 | ai:codex
use super::*;
use crate::gate::seat_authority as authority;
use crate::test_env::EnvVarsGuard;

#[test]
fn bug_2467_parses_delegation_for_every_vendor() {
    for vendor in ["claude", "codex", "antigravity"] {
        let cli = crate::command_registry::try_parse_from([
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
    let cli = crate::command_registry::try_parse_from([
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
                    authority::issue_launch_child(tmp.path(), worker, "grandchild", false).is_err()
                );
                continue;
            }
            let grandchild =
                authority::issue_launch_child(tmp.path(), worker, "grandchild", false).unwrap();
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
    assert!(authority::issue_launch_child(tmp.path(), "implementer", "worker", false).is_err());
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
    let child = authority::issue_launch_child(tmp.path(), "advisor", "child", false).unwrap();
    env.set_key("AIDA_USER", "child");
    env.set_key(authority::GRANT_ENV, &child.id);
    let worker = authority::issue_launch_child(tmp.path(), "reviewer", "worker", false).unwrap();
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
