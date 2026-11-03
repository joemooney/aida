//! STORY-2326 slice A: LaunchContext resolution, the spawn_agent constructor,
//! seat assignment, and per-path goldens for the converted Claude launches
//! (env ops on the owned AIDA keys and argv equal to before, minus the
//! listed removals).
// trace:STORY-2326 | ai:claude

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

use aida_core::launch_policy::{
    LaunchKind, LaunchVendor, OWNED_KEYS, ROLE_INSTANCE_ENV, VENDOR_PROCESS_ONLY_KEYS,
};

use super::context::{LaunchContext, LaunchRefusal, LaunchRequest};
use super::spawn::{assign_seat, spawn_agent, SeatDecision, AIDA_BOUNDARY_CALLS};
use crate::session::ContainmentPosture;

fn request(posture: ContainmentPosture, kind: LaunchKind, tail: &[&str]) -> LaunchRequest {
    LaunchRequest {
        vendor: LaunchVendor::Claude,
        kind,
        program: "claude".into(),
        args: vec!["-p".to_string(), "hello".to_string()],
        tail: tail.iter().map(|s| s.to_string()).collect(),
        posture,
        cwd: std::env::temp_dir(),
        set_cwd: false,
    }
}

/// The ops a command makes on the owned AIDA keys: `Some(v)` set, `None` removed.
fn owned_ops(cmd: &std::process::Command) -> BTreeMap<String, Option<String>> {
    let owned: Vec<&str> = OWNED_KEYS.iter().map(|(k, _)| *k).collect();
    cmd.get_envs()
        .filter(|(k, _)| owned.iter().any(|o| OsStr::new(o) == *k))
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

fn ops(items: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
        .collect()
}

#[test]
fn resolve_refusal_table() {
    let none = ContainmentPosture::None;
    let aida = ContainmentPosture::Aida;
    let native = ContainmentPosture::Native;
    // (posture, kind, tail, parent contained, expected refusal or None)
    let cases: Vec<(
        ContainmentPosture,
        LaunchKind,
        Vec<&str>,
        bool,
        Option<&str>,
    )> = vec![
        (aida, LaunchKind::Fresh, vec![], false, None),
        (none, LaunchKind::Fresh, vec![], false, None),
        (aida, LaunchKind::Fresh, vec![], true, None),
        (native, LaunchKind::Fresh, vec![], true, None),
        (none, LaunchKind::Fresh, vec![], true, Some("weaker")),
        (none, LaunchKind::Exec, vec![], true, Some("weaker")),
        (none, LaunchKind::Background, vec![], true, Some("weaker")),
        (
            aida,
            LaunchKind::Fresh,
            vec!["--settings", "x"],
            false,
            Some("posture"),
        ),
        (
            none,
            LaunchKind::Fresh,
            vec!["--permission-mode=bypassPermissions"],
            false,
            Some("posture"),
        ),
        (
            none,
            LaunchKind::Fresh,
            vec!["--dangerously-skip-permissions"],
            false,
            Some("posture"),
        ),
        (
            none,
            LaunchKind::Fresh,
            vec!["--model", "opus"],
            false,
            None,
        ),
        (none, LaunchKind::Query, vec![], false, Some("kind")),
        (none, LaunchKind::Control, vec![], false, Some("kind")),
    ];
    for (posture, kind, tail, parent, expected) in cases {
        let got = LaunchContext::resolve_under(request(posture, kind, &tail), parent);
        let label = format!("{posture:?} {kind:?} {tail:?} parent={parent}");
        match (expected, got) {
            (None, Ok(ctx)) => {
                assert_eq!(ctx.posture(), posture, "{label}");
                assert_eq!(ctx.parent_contained(), parent, "{label}");
                let mut argv = vec!["-p".to_string(), "hello".to_string()];
                argv.extend(tail.iter().map(|s| s.to_string()));
                assert_eq!(ctx.argv(), argv.as_slice(), "{label}");
            }
            (Some("weaker"), Err(LaunchRefusal::WeakerThanParent { .. })) => {}
            (Some("posture"), Err(LaunchRefusal::PostureFlag(_))) => {}
            (Some("kind"), Err(LaunchRefusal::UnsupportedKind(_))) => {}
            (expected, got) => panic!("{label}: expected {expected:?}, got {got:?}"),
        }
    }
}

#[test]
fn refusal_messages_are_plain_language() {
    let err = LaunchContext::resolve_under(
        request(ContainmentPosture::None, LaunchKind::Fresh, &[]),
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("contained") && err.contains("not contained"),
        "{err}"
    );
    assert!(!err.contains("STORY-"), "{err}");
}

#[test]
fn spawn_agent_stamps_posture_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let mut req = request(ContainmentPosture::Aida, LaunchKind::Fresh, &[]);
    req.cwd = tmp.path().to_path_buf();
    let ctx = LaunchContext::resolve_under(req, false).unwrap();
    let cmd = spawn_agent(&ctx).unwrap();
    assert_eq!(
        owned_ops(&cmd),
        ops(&[
            ("AIDA_CONTAINED", Some("1")),
            ("AIDA_CONTAINMENT_POSTURE", Some("aida")),
        ])
    );
    assert_eq!(cmd.get_program(), OsStr::new("claude"));
    assert_eq!(
        cmd.get_current_dir(),
        None,
        "set_cwd: false inherits the cwd"
    );
    for (posture, value) in [
        (ContainmentPosture::None, "none"),
        (ContainmentPosture::Native, "native"),
    ] {
        let mut req = request(posture, LaunchKind::Fresh, &[]);
        req.set_cwd = true;
        let ctx = LaunchContext::resolve_under(req, false).unwrap();
        let cmd = spawn_agent(&ctx).unwrap();
        assert_eq!(
            owned_ops(&cmd),
            ops(&[
                ("AIDA_CONTAINED", None),
                ("AIDA_CONTAINMENT_POSTURE", Some(value)),
            ])
        );
        assert_eq!(cmd.get_current_dir(), Some(std::env::temp_dir().as_path()));
    }
}

#[test]
fn assign_seat_table() {
    let ctx = LaunchContext::resolve_under(
        request(ContainmentPosture::None, LaunchKind::Fresh, &[]),
        false,
    )
    .unwrap();
    let base = |seat: SeatDecision<'_>| {
        let mut cmd = std::process::Command::new("claude");
        assign_seat(&mut cmd, ctx.kind(), seat).unwrap();
        owned_ops(&cmd)
    };
    assert_eq!(
        base(SeatDecision::Child {
            role: "reviewer",
            grant: "g-1"
        }),
        ops(&[
            ("AIDA_SESSION_ROLE", Some("reviewer")),
            ("AIDA_SESSION_GRANT", Some("g-1")),
            ("AIDA_SESSION_PURPOSE", None),
            ("AIDA_STANDALONE_IMPLEMENTER", None),
            ("AIDA_HEADLESS_LANE", None),
        ])
    );
    assert_eq!(
        base(SeatDecision::Lane {
            role: Some("implementer"),
            grant: Some("g-2")
        }),
        ops(&[
            ("AIDA_SESSION_ROLE", Some("implementer")),
            ("AIDA_SESSION_GRANT", Some("g-2")),
            ("AIDA_SESSION_PURPOSE", None),
        ])
    );
    assert_eq!(
        base(SeatDecision::Lane {
            role: None,
            grant: None
        }),
        ops(&[
            ("AIDA_SESSION_ROLE", None),
            ("AIDA_SESSION_GRANT", None),
            ("AIDA_SESSION_PURPOSE", None),
        ])
    );
    assert_eq!(base(SeatDecision::ThisSession { grant: None }), ops(&[]));
    assert_eq!(
        base(SeatDecision::ThisSession { grant: Some("g-3") }),
        ops(&[("AIDA_SESSION_GRANT", Some("g-3"))])
    );
}

#[test]
fn background_handoff_env_has_no_seat_keys() {
    let mut cmd = std::process::Command::new("claude");
    for seat in [
        SeatDecision::Child {
            role: "implementer",
            grant: "g",
        },
        SeatDecision::Lane {
            role: Some("implementer"),
            grant: None,
        },
        SeatDecision::ThisSession { grant: None },
    ] {
        assert!(
            assign_seat(&mut cmd, LaunchKind::Background, seat).is_err(),
            "{seat:?}"
        );
    }
    // The real `--bg` builder, with a role on the plan: no seat key is set.
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().to_path_buf();
    std::fs::create_dir_all(project.join(".aida-store")).unwrap();
    let plan = crate::agent_launch::AgentLaunchPlan {
        project_root: project.clone(),
        launch_cwd: project.clone(),
        role: Some("implementer".to_string()),
        role_instance: crate::role::RoleInstanceKind::Driver,
        current_spec: None,
        name: "bg-lane".to_string(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    };
    let config = crate::agent_launch::AgentLaunchConfig {
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
        contained: false,
    };
    let cmd = crate::agent_launch::build_agent_new_bg_command(
        std::path::Path::new("/bin/echo"),
        &config,
        &plan,
        &["do the work".to_string()],
        None,
    )
    .unwrap();
    let env = owned_ops(&cmd);
    for key in aida_core::launch_policy::BACKGROUND_FORBIDDEN_KEYS {
        assert_eq!(env.get(*key), Some(&None), "{key} must be removed: {env:?}");
    }
    // Golden: the owned-key ops equal the pre-STORY-2326 builder's, minus the
    // listed removals (the role, previously set from the plan, and the purpose).
    assert_eq!(
        env,
        ops(&[
            ("AIDA_CONTAINED", None),
            ("AIDA_CONTAINMENT_POSTURE", Some("none")),
            ("AIDA_SESSION_ROLE", None),
            ("AIDA_SESSION_GRANT", None),
            ("AIDA_SESSION_PURPOSE", None),
            ("AIDA_HANDLE_LAUNCH_TOKEN", None),
        ])
    );
    let args: Vec<&OsStr> = cmd.get_args().collect();
    assert_eq!(args, [OsStr::new("do the work")]);
}

#[test]
fn companion_marker_survives_spawn_agent() {
    // Neither the constructor nor any seat decision touches the forwarded
    // keys, so an ambient AIDA_ROLE_INSTANCE (and the vendor tokens) pass.
    let ctx = LaunchContext::resolve_under(
        request(ContainmentPosture::None, LaunchKind::Fresh, &[]),
        false,
    )
    .unwrap();
    for seat in [
        SeatDecision::Child {
            role: "r",
            grant: "",
        },
        SeatDecision::Lane {
            role: None,
            grant: None,
        },
        SeatDecision::ThisSession { grant: Some("g") },
    ] {
        let mut cmd = spawn_agent(&ctx).unwrap();
        assign_seat(&mut cmd, ctx.kind(), seat).unwrap();
        let env = owned_ops(&cmd);
        assert!(!env.contains_key(ROLE_INSTANCE_ENV), "{seat:?}: {env:?}");
        for key in VENDOR_PROCESS_ONLY_KEYS {
            assert!(!env.contains_key(*key), "{seat:?}: {env:?}");
        }
    }
}

#[test]
fn registry_and_launch_tokens_absent_in_tool_shell() {
    for key in VENDOR_PROCESS_ONLY_KEYS {
        assert!(
            !crate::codex_tool_env::TOOL_ENV.contains(key),
            "{key} belongs on the vendor process only, never in a tool shell"
        );
    }
}

/// F2 ruling (S-i): every Aida-posture spawn of a converted path reaches the
/// one private boundary fn, which STORY-2238 PR3 replaces with
/// `containment::launch`.
#[test]
fn aida_posture_spawn_routes_through_single_boundary_fn() {
    let calls = || AIDA_BOUNDARY_CALLS.with(|c| c.get());
    let before = calls();
    let exec = crate::session::build_claude_exec_command(
        None,
        None,
        Some("p"),
        Some("sid"),
        true,
        None,
        None,
    )
    .unwrap();
    assert_eq!(calls(), before + 1, "contained exec");
    let run = crate::session::build_run_claude_session_command(
        None,
        None,
        Some("p"),
        Some("sid"),
        true,
        None,
    )
    .unwrap();
    assert_eq!(calls(), before + 2, "contained run session");
    // Uncontained launches never reach it.
    crate::session::build_claude_exec_command(
        None,
        None,
        Some("p"),
        Some("sid"),
        false,
        None,
        None,
    )
    .unwrap();
    crate::session::build_run_claude_session_command(
        None,
        None,
        Some("p"),
        Some("sid"),
        false,
        None,
    )
    .unwrap();
    assert_eq!(
        calls(),
        before + 2,
        "uncontained launches skip the boundary"
    );
    for cmd in [&exec, &run] {
        let env: BTreeMap<_, _> = cmd.get_envs().collect();
        assert_eq!(
            env.get(OsStr::new("AIDA_CONTAINED")),
            Some(&Some(OsStr::new("1")))
        );
    }
}

/// Amendment 9 goldens for the interactive Claude paths: the owned-key env
/// ops and argv equal the pre-STORY-2326 builders' exactly.
#[test]
fn golden_interactive_claude_paths_are_unchanged() {
    let contained = ops(&[
        ("AIDA_CONTAINED", Some("1")),
        ("AIDA_CONTAINMENT_POSTURE", Some("aida")),
    ]);
    let uncontained = ops(&[
        ("AIDA_CONTAINED", None),
        ("AIDA_CONTAINMENT_POSTURE", Some("none")),
    ]);
    let mut granted = contained.clone();
    granted.insert("AIDA_SESSION_GRANT".to_string(), Some("g-9".to_string()));
    let exec = |c: bool, grant: Option<&str>| {
        crate::session::build_claude_exec_command(
            None,
            Some("n"),
            Some("p"),
            Some("sid"),
            c,
            None,
            grant,
        )
        .unwrap()
    };
    let run = |c: bool| {
        crate::session::build_run_claude_session_command(
            None,
            Some("n"),
            Some("p"),
            Some("sid"),
            c,
            None,
        )
        .unwrap()
    };
    assert_eq!(owned_ops(&exec(true, None)), contained);
    assert_eq!(owned_ops(&exec(false, None)), uncontained);
    assert_eq!(owned_ops(&exec(true, Some("g-9"))), granted);
    assert_eq!(owned_ops(&run(true)), contained);
    assert_eq!(owned_ops(&run(false)), uncontained);
    for c in [true, false] {
        let (program, args) = crate::session::interactive_claude_program_and_args(
            c,
            crate::session::claude_session_args(None, Some("n"), Some("p"), Some("sid"), c, None)
                .unwrap(),
        )
        .unwrap();
        for cmd in [exec(c, None), run(c)] {
            assert_eq!(cmd.get_program(), OsString::from(&program).as_os_str());
            let got: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(got, args);
            assert_eq!(cmd.get_current_dir(), None);
        }
    }
}
