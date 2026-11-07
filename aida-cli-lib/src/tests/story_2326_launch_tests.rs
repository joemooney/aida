//! STORY-2326 slice A: LaunchContext resolution, the spawn_agent constructor,
//! seat assignment, and per-path goldens for the converted Claude launches
//! (env ops on the owned AIDA keys and argv equal to before, minus the
//! listed removals).
// trace:STORY-2326 | ai:claude

use std::collections::BTreeMap;
use std::ffi::OsStr;

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
        host_main: None,
    }
}

/// Explicit owned values. Cleared commands discard env_remove entries, so
/// snapshots compare effective assignments; subprocess tests check absence.
fn owned_ops(cmd: &std::process::Command) -> BTreeMap<String, Option<String>> {
    let owned: Vec<&str> = OWNED_KEYS.iter().map(|(k, _)| *k).collect();
    cmd.get_envs()
        .filter(|(k, v)| v.is_some() && owned.iter().any(|o| OsStr::new(o) == *k))
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect()
}

// Assignment-only snapshots are valid only after proving inheritance is off.
// trace:STORY-2326 | ai:codex
fn cleared_owned_ops(cmd: &mut std::process::Command) -> BTreeMap<String, Option<String>> {
    assert!(super::profile::environment_is_cleared(cmd));
    owned_ops(cmd)
}

fn ops(items: &[(&str, Option<&str>)]) -> BTreeMap<String, Option<String>> {
    items
        .iter()
        .filter(|(_, value)| value.is_some())
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

    // Advisor ruling 4 (round 5): a parent whose own `AIDA_CONTAINED` is
    // neither on nor off is refused by `resolve` before anything is spawned,
    // for every posture the child asks for.
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_CONTAINED", Some("maybe")),
        ("AIDA_AGENT_CMD", None),
    ]);
    for posture in [aida, none, native] {
        let (got, calls) = boundary_calls(|| {
            LaunchContext::resolve(request(posture, LaunchKind::Fresh, &[]))
                .map(|ctx| spawn_agent(&ctx))
        });
        match got {
            Err(LaunchRefusal::MalformedParentContainment(msg)) => {
                assert!(
                    msg.contains("AIDA_CONTAINED") && msg.contains("maybe"),
                    "{posture:?}: {msg}"
                );
                assert!(!msg.contains("STORY-"), "{msg}");
            }
            other => panic!("{posture:?}: expected a malformed-containment refusal, got {other:?}"),
        }
        assert_eq!(
            calls, 0,
            "{posture:?}: a refused launch reached the boundary"
        );
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
    let mut cmd = spawn_agent(&ctx).unwrap();
    assert_eq!(
        cleared_owned_ops(&mut cmd),
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
        let mut cmd = spawn_agent(&ctx).unwrap();
        assert_eq!(
            cleared_owned_ops(&mut cmd),
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
    let seat_ops = |items: &[(&str, Option<&str>)]| {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
            .collect::<BTreeMap<_, _>>()
    };
    let base = |seat: SeatDecision<'_>| {
        let mut cmd = std::process::Command::new("claude");
        assign_seat(&mut cmd, ctx.kind(), seat).unwrap();
        cmd.get_envs()
            .filter(|(key, _)| {
                OWNED_KEYS
                    .iter()
                    .any(|(owned, _)| *key == OsStr::new(owned))
            })
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(
        base(SeatDecision::Child {
            role: "reviewer",
            grant: "g-1"
        }),
        seat_ops(&[
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
        seat_ops(&[
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
        seat_ops(&[
            ("AIDA_SESSION_ROLE", None),
            ("AIDA_SESSION_GRANT", None),
            ("AIDA_SESSION_PURPOSE", None),
        ])
    );
    assert_eq!(
        base(SeatDecision::ThisSession { grant: None }),
        seat_ops(&[])
    );
    assert_eq!(
        base(SeatDecision::ThisSession { grant: Some("g-3") }),
        seat_ops(&[("AIDA_SESSION_GRANT", Some("g-3"))])
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
        delegate_seats: Vec::new(),
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
        contained: false,
    };
    let mut cmd = crate::agent_launch::build_agent_new_bg_command(
        std::path::Path::new("/bin/echo"),
        &config,
        &plan,
        &["do the work".to_string()],
        None,
    )
    .unwrap();
    let env = cleared_owned_ops(&mut cmd);
    for key in aida_core::launch_policy::BACKGROUND_FORBIDDEN_KEYS {
        assert!(env.get(*key).is_none(), "{key} must be absent: {env:?}");
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
        let env = cleared_owned_ops(&mut cmd);
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

/// Advisor ruling 5: a contained parent carrying the `AIDA_AGENT_CMD`
/// program override is refused, and an Aida-posture child never inherits it.
#[test]
fn agent_cmd_override_never_crosses_the_boundary() {
    let aida = || request(ContainmentPosture::Aida, LaunchKind::Fresh, &[]);
    let err = LaunchContext::resolve_with(aida(), true, true).unwrap_err();
    assert_eq!(err, LaunchRefusal::AgentCmdUnderContainment);
    let msg = err.to_string();
    assert!(
        msg.contains("AIDA_AGENT_CMD") && msg.contains("contained"),
        "{msg}"
    );
    assert!(!msg.contains("STORY-"), "{msg}");
    // The seam stays open for an uncontained parent, contained child or not.
    LaunchContext::resolve_with(aida(), false, true).unwrap();
    LaunchContext::resolve_with(
        request(ContainmentPosture::None, LaunchKind::Fresh, &[]),
        false,
        true,
    )
    .unwrap();
    LaunchContext::resolve_with(aida(), true, false).unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let mut req = aida();
    req.cwd = tmp.path().to_path_buf();
    let cmd = spawn_agent(&LaunchContext::resolve_with(req, false, true).unwrap()).unwrap();
    let env: BTreeMap<_, _> = cmd.get_envs().collect();
    assert!(env
        .get(OsStr::new("AIDA_AGENT_CMD"))
        .and_then(|v| *v)
        .is_none());
    let none = LaunchContext::resolve_under(
        request(ContainmentPosture::None, LaunchKind::Fresh, &[]),
        false,
    )
    .unwrap();
    let cmd = spawn_agent(&none).unwrap();
    assert!(cmd.get_envs().all(|(k, _)| k != "AIDA_AGENT_CMD"));
}

/// The seat and containment env a capture fake records, then its argv.
const CAPTURED_KEYS: &[&str] = &[
    "AIDA_SESSION_ROLE",
    "AIDA_SESSION_GRANT",
    "AIDA_SESSION_PURPOSE",
    "AIDA_CONTAINED",
    "AIDA_CONTAINMENT_POSTURE",
    "AIDA_STANDALONE_IMPLEMENTER",
    "AIDA_HEADLESS_LANE",
    "AIDA_AGENT_CMD",
];

/// A project with a store, and a fake agent that writes the
/// [`CAPTURED_KEYS`] and its argv to `capture`.
struct Launch {
    tmp: tempfile::TempDir,
    project: std::path::PathBuf,
    fake: String,
    capture: std::path::PathBuf,
}

fn launch_fixture() -> Launch {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida-store")).unwrap();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    std::fs::write(
        project.join(".aida/config.toml"),
        "store_path = \".aida-store\"\n",
    )
    .unwrap();
    let fake = tmp.path().join("fake-agent");
    let capture = tmp.path().join("capture");
    let mut body = String::from("#!/bin/sh\n{\n");
    for key in CAPTURED_KEYS {
        body.push_str(&format!(
            "if [ \"${{{key}+set}}\" = set ]; then printf '%s=%s\\n' {key} \"${key}\"; \
             else printf '%s\\n' '{key} unset'; fi\n"
        ));
    }
    body.push_str("printf -- '--\\n'\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n");
    body.push_str(&format!("}} > '{}'\n", capture.display()));
    crate::test_exec::write_executable(&fake, body);
    Launch {
        project,
        fake: fake.to_string_lossy().into_owned(),
        capture,
        tmp,
    }
}

impl Launch {
    fn ambient(&self) -> crate::test_env::AmbientGuard {
        crate::test_env::AmbientGuard::hermetic_with_seat_and(
            &self.project,
            "advisor",
            &["advisor", "implementer", "reviewer"],
            &[
                ("AIDA_AGENT_CMD", Some(self.fake.as_str())),
                ("AIDA_OS_WRAP", Some("0")),
                ("AIDA_HEADLESS_VENDOR", None),
                ("AIDA_CONTAINED", None),
                ("AIDA_STANDALONE_IMPLEMENTER", None),
                ("AIDA_HEADLESS_LANE", None),
                ("AIDA_TEE_HEADLESS", Some("0")),
                // The launcher's own purpose never reaches a child.
                ("AIDA_SESSION_PURPOSE", Some("launcher purpose")),
                ("AIDA_TEST_CAPTURE", Some(self.capture.to_str().unwrap())),
            ],
        )
    }

    /// Replace the per-run parts of a value: the fake's path, the fixture
    /// root, a generated session id.
    fn mask(&self, value: &str) -> String {
        let root = self.tmp.path().to_string_lossy().into_owned();
        let value = value.replace(&self.fake, "<agent>").replace(&root, "<tmp>");
        if uuid::Uuid::parse_str(&value).is_ok() {
            return "<uuid>".to_string();
        }
        value
    }

    /// What the fake recorded: env lines (a minted grant masked) and argv.
    fn captured(&self) -> (Vec<String>, Vec<String>) {
        let raw = std::fs::read_to_string(&self.capture).expect("the fake agent ran");
        let _ = std::fs::remove_file(&self.capture);
        let (env, args) = raw.split_once("--\n").unwrap();
        let env = env
            .lines()
            .map(|line| match line.split_once('=') {
                Some(("AIDA_SESSION_GRANT", g)) if !g.is_empty() => {
                    "AIDA_SESSION_GRANT=<grant>".to_string()
                }
                _ => self.mask(line),
            })
            .collect();
        (env, args.lines().map(|a| self.mask(a)).collect())
    }

    /// A built command's program and argv, masked like [`Self::captured`].
    fn argv_of(&self, cmd: &std::process::Command) -> Vec<String> {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|a| self.mask(&a.to_string_lossy()))
            .collect()
    }
}

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The owned keys a launch decides (the forwarded handle token is per-run).
fn decided_ops(cmd: &mut std::process::Command) -> BTreeMap<String, Option<String>> {
    let mut ops = cleared_owned_ops(cmd);
    ops.retain(|k, _| aida_core::launch_policy::decided_keys().any(|d| d == k));
    ops
}

/// Run `f`, returning how many times it reached the boundary fn.
fn boundary_calls<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let calls = || AIDA_BOUNDARY_CALLS.with(|c| c.get());
    let before = calls();
    let out = f();
    (out, calls() - before)
}

fn agent_new_config(contained: bool) -> crate::agent_launch::AgentLaunchConfig {
    crate::agent_launch::AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "claude",
        binary: "claude",
        default_args: Vec::new(),
        prompt_style: crate::agent_launch::AgentPromptStyle::Positional,
        contained,
    }
}

fn agent_new_plan(project: &std::path::Path) -> crate::agent_launch::AgentLaunchPlan {
    crate::agent_launch::AgentLaunchPlan {
        project_root: project.to_path_buf(),
        launch_cwd: project.to_path_buf(),
        role: None,
        role_instance: crate::role::RoleInstanceKind::Driver,
        current_spec: None,
        name: "lane-2326".to_string(),
        lease_id: None,
        native_session_id: None,
        resumed_from: None,
    }
}

/// Pin a contained interactive session's cwd to a fresh git root, so its
/// write boundary never depends on the tree the tests run in.
fn pinned_git_cwd() -> (
    tempfile::TempDir,
    crate::session::contained_cwd_seam::CwdGuard,
) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".git")).unwrap();
    let guard = crate::session::contained_cwd_seam::pin(tmp.path());
    (tmp, guard)
}

/// Whether this host can hold a contained child's grant key in a session
/// keyring. Inside another agent's sandbox it cannot, and a contained
/// child's grant is refused before the spawn; those cases then skip.
fn keyring_usable() -> bool {
    let usable = crate::gate::seat_authority::preflight_sandboxed_child_key("implementer").is_ok();
    if !usable {
        eprintln!("SKIP contained spawns: the session keyring is unavailable here");
    }
    usable
}

fn no_tee() -> crate::headless_tee::TeeOptions {
    crate::headless_tee::TeeOptions {
        enabled: false,
        label: None,
    }
}

/// F2 ruling (S-i): every Aida-posture spawn of a converted path reaches the
/// one private boundary fn, which STORY-2238 PR3 replaces with
/// `containment::launch`; an uncontained one never does.
#[test]
fn aida_posture_spawn_routes_through_single_boundary_fn() {
    // Reviewer commands inherit the process cwd. Use a one-test child so
    // the golden never resolves the live seat checkout. trace:STORY-2326 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let exec = |c| {
        crate::session::build_claude_exec_command(
            None,
            None,
            None,
            Some("p"),
            Some("sid"),
            c,
            None,
            None,
        )
        .unwrap()
    };
    let run = |c| {
        crate::session::build_run_claude_session_command(
            None,
            None,
            None,
            Some("p"),
            Some("sid"),
            c,
            None,
        )
        .unwrap()
    };
    {
        // Released before the ambient guard below takes the same env lock
        // (and the pinned cwd before the seat grant is looked up).
        let (_git, _cwd) = pinned_git_cwd();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
            ("AIDA_AGENT_CMD", None),
        ]);
        for c in [true, false] {
            let want = usize::from(c);
            let (exec, n) = boundary_calls(|| exec(c));
            assert_eq!(n, want, "exec contained={c}");
            let (run, n) = boundary_calls(|| run(c));
            assert_eq!(n, want, "run session contained={c}");
            for cmd in [&exec, &run] {
                let env: BTreeMap<_, _> = cmd.get_envs().collect();
                let marker = c.then_some(OsStr::new("1"));
                assert_eq!(
                    env.get(OsStr::new("AIDA_CONTAINED")).and_then(|v| *v),
                    marker
                );
            }
        }
    }

    let fx = launch_fixture();
    let _ambient = fx.ambient();
    std::env::set_current_dir(&fx.project).unwrap();
    let claude = crate::session::HeadlessVendor::Claude;
    let log = fx.project.join(".aida/headless.log");
    let fake = std::path::Path::new(&fx.fake);
    let keyring = keyring_usable();
    for c in [true, false] {
        let want = usize::from(c);
        // These two mint a contained child grant, which needs the keyring.
        if !c || keyring {
            let (out, n) = boundary_calls(|| {
                crate::session::spawn_vendor_headless_with_seat_granted(
                    None,
                    claude,
                    aida_core::agents_config::AgentSeat::Implementer,
                    "go",
                    "sid-fork",
                    &log,
                    &no_tee(),
                    c,
                )
            });
            out.unwrap();
            assert_eq!(n, want, "headless forking contained={c}");

            let plan = crate::session::interactive_reviewer_launch_plan(
                None, claude, None, None, "review", "sid-r", c,
            )
            .unwrap();
            let (out, n) = boundary_calls(|| {
                crate::session::spawn_reviewer_launch_plan_granted(&plan, &fx.project)
            });
            out.unwrap();
            assert_eq!(n, want, "reviewer plan contained={c}");
        }

        let (out, n) = boundary_calls(|| {
            crate::session::build_advisor_interactive_command(None, claude, "advise", "g-adv", c)
        });
        out.unwrap();
        assert_eq!(n, want, "advisor interactive contained={c}");

        let (out, n) = boundary_calls(|| {
            crate::session::build_exec_vendor_headless_command(
                None,
                claude,
                "coord",
                "sid-c",
                c,
                None,
                None,
                "implementer",
                "g-c",
            )
        });
        out.unwrap();
        assert_eq!(n, want, "coordinator headless contained={c}");

        let config = agent_new_config(c);
        let plan = agent_new_plan(&fx.project);
        let (out, n) = boundary_calls(|| {
            crate::agent_launch::build_agent_new_bg_command(fake, &config, &plan, &[], None)
        });
        out.unwrap();
        assert_eq!(n, want, "agent new --bg contained={c}");

        let (out, n) = boundary_calls(|| {
            crate::agent_launch::run_agent_generation(
                fake,
                &config,
                &plan,
                None,
                &["work".to_string()],
                None,
                false,
                false,
                None,
                None,
                || {},
            )
        });
        assert!(out.unwrap().status.success());
        assert_eq!(n, want, "agent new foreground contained={c}");
    }
}

/// Amendment 9 goldens for the interactive Claude paths: the owned-key env
/// ops and argv equal the pre-STORY-2326 builders' exactly.
#[test]
fn golden_interactive_claude_paths_are_unchanged() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_CONTAINED", None),
        ("AIDA_AGENT_CMD", None),
        ("AIDA_HEADLESS_LANE", None),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_SESSION_GRANT", None),
        ("AIDA_SESSION_PURPOSE", None),
        ("AIDA_STANDALONE_IMPLEMENTER", None),
    ]);
    let (_git, _cwd) = pinned_git_cwd();
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
            None,
            Some("n"),
            Some("p"),
            Some("sid"),
            c,
            None,
        )
        .unwrap()
    };
    assert_eq!(cleared_owned_ops(&mut exec(true, None)), contained);
    assert_eq!(cleared_owned_ops(&mut exec(false, None)), uncontained);
    assert_eq!(cleared_owned_ops(&mut exec(true, Some("g-9"))), granted);
    assert_eq!(cleared_owned_ops(&mut run(true)), contained);
    assert_eq!(cleared_owned_ops(&mut run(false)), uncontained);
    // The argv recorded from the pre-STORY-2326 builders (os_wrap off; the
    // generated settings JSON masked), not recomputed through their helpers.
    let session = ["--name", "n", "--session-id", "sid", "p"];
    let mut contained_argv = lines(&["--setting-sources", "project", "--settings", "<settings>"]);
    contained_argv.extend(lines(&session));
    for c in [true, false] {
        for cmd in [exec(c, None), run(c)] {
            assert_eq!(cmd.get_program(), OsStr::new("claude"));
            let got = mask_settings(
                cmd.get_args()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            );
            let want = if c {
                contained_argv.clone()
            } else {
                lines(&session)
            };
            assert_eq!(got, want, "contained={c}");
            assert_eq!(cmd.get_current_dir(), None);
        }
    }
}

/// Replace the generated settings JSON after `--settings` (it embeds the
/// per-run write roots) with `<settings>`.
fn mask_settings(mut args: Vec<String>) -> Vec<String> {
    if let Some(at) = args.iter().position(|a| a == "--settings") {
        if let Some(json) = args.get_mut(at + 1) {
            assert!(json.starts_with('{'), "inline settings JSON: {json}");
            *json = "<settings>".to_string();
        }
    }
    args
}

/// Amendment 9 goldens for the remaining converted Claude paths: the seat
/// and containment env each child actually receives, and its argv, recorded
/// from the pre-STORY-2326 builders. The argv composers are untouched by the
/// conversion; only where each child's env is decided moved.
#[test]
fn golden_headless_and_lane_claude_paths_are_unchanged() {
    // Reviewer commands inherit the process cwd. Use a one-test child so
    // the golden never resolves the live seat checkout. trace:STORY-2326 | ai:codex
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = launch_fixture();
    let mut ambient = fx.ambient();
    std::env::set_current_dir(&fx.project).unwrap();
    let claude = crate::session::HeadlessVendor::Claude;
    let log = fx.project.join(".aida/headless.log");
    let posture = |c: bool| {
        if c {
            lines(&["AIDA_CONTAINED=1", "AIDA_CONTAINMENT_POSTURE=aida"])
        } else {
            lines(&["AIDA_CONTAINED unset", "AIDA_CONTAINMENT_POSTURE=none"])
        }
    };
    // Ruling 5: the program override never reaches a contained child.
    let agent_cmd = |c: bool| {
        if c {
            "AIDA_AGENT_CMD unset".to_string()
        } else {
            "AIDA_AGENT_CMD=<agent>".to_string()
        }
    };
    let child_env = |role: &str, c: bool| {
        let mut env = vec![
            format!("AIDA_SESSION_ROLE={role}"),
            "AIDA_SESSION_GRANT=<grant>".to_string(),
            "AIDA_SESSION_PURPOSE unset".to_string(),
        ];
        env.extend(posture(c));
        env.extend(lines(&[
            "AIDA_STANDALONE_IMPLEMENTER unset",
            "AIDA_HEADLESS_LANE unset",
        ]));
        env.push(agent_cmd(c));
        env
    };
    let child_ops = |role: &str, grant: &str, c: bool| {
        let mut want = ops(&[
            ("AIDA_SESSION_ROLE", Some(role)),
            ("AIDA_SESSION_GRANT", Some(grant)),
            ("AIDA_SESSION_PURPOSE", None),
            ("AIDA_STANDALONE_IMPLEMENTER", None),
            ("AIDA_HEADLESS_LANE", None),
        ]);
        want.extend(if c {
            ops(&[
                ("AIDA_CONTAINED", Some("1")),
                ("AIDA_CONTAINMENT_POSTURE", Some("aida")),
            ])
        } else {
            ops(&[
                ("AIDA_CONTAINED", None),
                ("AIDA_CONTAINMENT_POSTURE", Some("none")),
            ])
        });
        want
    };

    let keyring = keyring_usable();
    let mut miss = Mismatches::default();
    for c in [true, false] {
        // These two mint a contained child grant, which needs the keyring.
        if !c || keyring {
            // spawn_vendor_headless_forking: a fresh headless phase.
            crate::session::spawn_vendor_headless_with_seat_granted(
                None,
                claude,
                aida_core::agents_config::AgentSeat::Implementer,
                "go",
                "sid-fork",
                &log,
                &no_tee(),
                c,
            )
            .unwrap();
            let (env, args) = fx.captured();
            assert_eq!(env, child_env("implementer", c), "forking contained={c}");
            miss.check("forking", c, mask_settings(args), golden::headless_fork(c));

            // spawn_reviewer_launch_plan_granted.
            let plan = crate::session::interactive_reviewer_launch_plan(
                None, claude, None, None, "review", "sid-r", c,
            )
            .unwrap();
            crate::session::spawn_reviewer_launch_plan_granted(&plan, &fx.project).unwrap();
            let (env, args) = fx.captured();
            assert_eq!(env, child_env("reviewer", c), "reviewer contained={c}");
            miss.check("reviewer", c, mask_settings(args), golden::reviewer(c));
        }

        // Command-only snapshots exercise the production projection. The
        // surrounding spawn fixtures deliberately use the uncontained override
        // seam, whose full-parent inheritance is separately covered.
        // AmbientGuard owns the environment lock in this isolated test.
        ambient.unset_key("AIDA_AGENT_CMD");
        // build_advisor_interactive_command.
        let mut cmd =
            crate::session::build_advisor_interactive_command(None, claude, "advise", "g-adv", c)
                .unwrap();
        assert_eq!(
            decided_ops(&mut cmd),
            child_ops("advisor", "g-adv", c),
            "advisor contained={c}"
        );
        miss.check(
            "advisor",
            c,
            mask_settings(fx.argv_of(&cmd)),
            golden::advisor(c),
        );

        // build_coordinator_headless_command_inner, via the exec phase.
        let mut cmd = crate::session::build_exec_vendor_headless_command(
            None,
            claude,
            "coord",
            "sid-c",
            c,
            None,
            None,
            "implementer",
            "g-c",
        )
        .unwrap();
        assert_eq!(
            decided_ops(&mut cmd),
            child_ops("implementer", "g-c", c),
            "coordinator contained={c}"
        );
        miss.check(
            "coordinator",
            c,
            mask_settings(fx.argv_of(&cmd)),
            golden::coordinator(c),
        );

        ambient.set_key("AIDA_AGENT_CMD", &fx.fake);

        // run_agent_generation (`aida agent new` foreground) without a role:
        // the seat keys are removed, and so (the listed change) is the
        // launcher's purpose.
        let config = agent_new_config(c);
        let plan = agent_new_plan(&fx.project);
        let exit = crate::agent_launch::run_agent_generation(
            std::path::Path::new(&fx.fake),
            &config,
            &plan,
            None,
            &["work".to_string()],
            None,
            false,
            false,
            None,
            None,
            || {},
        )
        .unwrap();
        assert!(exit.status.success());
        let (env, args) = fx.captured();
        let mut lane = lines(&[
            "AIDA_SESSION_ROLE unset",
            "AIDA_SESSION_GRANT unset",
            "AIDA_SESSION_PURPOSE unset",
        ]);
        lane.extend(posture(c));
        lane.extend(lines(&[
            "AIDA_STANDALONE_IMPLEMENTER unset",
            "AIDA_HEADLESS_LANE unset",
        ]));
        lane.push(agent_cmd(c));
        assert_eq!(env, lane, "agent new contained={c}");
        miss.check("agent new", c, mask_settings(args), golden::agent_new_fg(c));
    }
    assert!(
        miss.0.is_empty(),
        "argv goldens differ:\n{}",
        miss.0.join("\n")
    );
}

/// Every argv golden that differs, reported together.
#[derive(Default)]
struct Mismatches(Vec<String>);

impl Mismatches {
    fn check(&mut self, path: &str, contained: bool, got: Vec<String>, want: Vec<String>) {
        if got != want {
            self.0.push(format!(
                "{path} contained={contained}: got {got:?}, want {want:?}"
            ));
        }
    }
}

/// The argv each path recorded before the conversion (masked: `<agent>` the
/// fake vendor, `<tmp>` the fixture root, `<uuid>` a generated session id,
/// `<settings>` the generated settings JSON).
mod golden {
    use super::lines;

    /// The headless `-p` argv: contained runs `dontAsk` under the project's
    /// settings sources, uncontained runs `bypassPermissions`.
    fn headless(contained: bool, session: &str, prompt: &str) -> Vec<String> {
        let mut argv = lines(&["-p", "--permission-mode"]);
        argv.extend(if contained {
            lines(&["dontAsk", "--setting-sources", "project"])
        } else {
            lines(&["bypassPermissions"])
        });
        argv.extend(lines(&[
            "--settings",
            "<settings>",
            "--disallowed-tools",
            "AskUserQuestion",
            "Monitor",
            "--output-format",
            "stream-json",
            "--verbose",
            "--session-id",
            session,
            prompt,
        ]));
        argv
    }

    /// The interactive reviewer-plan argv (no name, no permission mode).
    fn interactive(contained: bool, session: &str, prompt: &str) -> Vec<String> {
        let mut argv = if contained {
            lines(&["--setting-sources", "project", "--settings", "<settings>"])
        } else {
            Vec::new()
        };
        argv.extend(lines(&["--session-id", session, prompt]));
        argv
    }

    pub(super) fn headless_fork(contained: bool) -> Vec<String> {
        headless(contained, "sid-fork", "go")
    }

    pub(super) fn reviewer(contained: bool) -> Vec<String> {
        interactive(contained, "sid-r", "review")
    }

    pub(super) fn advisor(contained: bool) -> Vec<String> {
        let mut argv = lines(&["claude"]);
        argv.extend(interactive(contained, "<uuid>", "advise"));
        argv
    }

    pub(super) fn coordinator(contained: bool) -> Vec<String> {
        let mut argv = lines(&["claude"]);
        argv.extend(headless(contained, "sid-c", "coord"));
        argv
    }

    pub(super) fn agent_new_fg(_contained: bool) -> Vec<String> {
        lines(&["work"])
    }
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_pr_b_posture_flags_are_refused_for_each_vendor() {
    for vendor in [LaunchVendor::Claude, LaunchVendor::Codex, LaunchVendor::Agy] {
        for tail in [
            vec!["--settings", "host.json"],
            vec!["--permission-mode=bypassPermissions"],
            vec!["--dangerously-bypass-approvals-and-sandbox"],
            vec!["--sandbox", "danger-full-access"],
            vec!["--add-dir", "/"],
            vec!["--full-auto"],
            vec!["-c", "sandbox_workspace_write.writable_roots=[\"/\"]"],
            vec!["-c", "approval_policy=never"],
            vec!["-c", "\"sandbox_mode\"=danger-full-access"],
            vec!["--profile", "unrestricted"],
        ] {
            let mut req = request(ContainmentPosture::None, LaunchKind::Fresh, &tail);
            req.vendor = vendor;
            assert!(
                matches!(
                    LaunchContext::resolve_under(req, false),
                    Err(LaunchRefusal::PostureFlag(_))
                ),
                "{vendor:?} {tail:?}"
            );
        }
    }
}

#[test]
fn story_2326_pr_b_phase_builders_preserve_none_argv_and_seat() {
    let _guard = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let tmp = tempfile::tempdir().unwrap();
    for vendor in [
        crate::session::HeadlessVendor::Codex,
        crate::session::HeadlessVendor::Agy,
    ] {
        let argv = vec!["--prompt".to_owned(), "fix this".to_owned()];
        let advisor = crate::phase_driver::build_phase_driver_advisor_command(
            tmp.path(),
            vendor.program(),
            &argv,
            &tmp.path().join("request"),
            &tmp.path().join("response"),
            "CEILING",
            "10",
            vendor,
            false,
        )
        .unwrap();
        let ci_fix = crate::phase_driver_trait::build_phase_driver_ci_fix_command(
            tmp.path(),
            vendor.program(),
            &argv,
            "CEILING",
            "10",
            vendor,
            false,
            "child-grant",
        )
        .unwrap();
        for cmd in [&advisor, &ci_fix] {
            assert_eq!(
                cmd.get_args().collect::<Vec<_>>(),
                argv.iter().map(OsStr::new).collect::<Vec<_>>()
            );
            assert_eq!(cmd.get_current_dir(), Some(tmp.path()));
            let env = cmd.get_envs().collect::<BTreeMap<_, _>>();
            assert!(env
                .get(OsStr::new("AIDA_CONTAINED"))
                .and_then(|v| *v)
                .is_none());
            assert_eq!(
                env.get(OsStr::new("AIDA_CONTAINMENT_POSTURE")),
                Some(&Some(OsStr::new("none")))
            );
        }
        assert_eq!(
            owned_ops(&ci_fix).get("AIDA_SESSION_GRANT"),
            Some(&Some("child-grant".into()))
        );
        assert_eq!(
            owned_ops(&advisor).get("AIDA_SESSION_ROLE"),
            Some(&Some("advisor".into()))
        );
    }
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_pr_b_dispatch_background_env_and_refusal() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".aida")).unwrap();
    std::fs::write(tmp.path().join(".aida/agents.toml"), "contained = false\n").unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        tmp.path(),
        "advisor",
        &["implementer"],
        &[("AIDA_CONTAINED", None)],
    );
    let build = || {
        crate::dispatch::build_implementer_launch_command(
            std::path::Path::new("claude"),
            tmp.path(),
            tmp.path(),
            "STORY-2326",
            "--settings is prompt text",
            "lane-test",
            crate::dispatch::DispatchVendor::Claude,
        )
    };
    let cmd = build().unwrap();
    let env = cmd.get_envs().collect::<BTreeMap<_, _>>();
    for key in [
        "AIDA_SESSION_ROLE",
        "AIDA_SESSION_GRANT",
        "AIDA_SESSION_PURPOSE",
    ] {
        assert!(env.get(OsStr::new(key)).and_then(|v| *v).is_none(), "{key}");
    }
    for (key, value) in [
        ("AIDA_SESSION_SCOPE", "STORY-2326"),
        ("AIDA_AGENT_NAME", "lane-test"),
        ("AIDA_USER", "lane-test"),
    ] {
        assert_eq!(env.get(OsStr::new(key)), Some(&Some(OsStr::new(value))));
    }
    assert_eq!(
        cmd.get_args().collect::<Vec<_>>(),
        ["--bg", "-n", "lane-test", "--settings is prompt text"].map(OsStr::new)
    );
    drop(_ambient);
    let _parent = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        tmp.path(),
        "advisor",
        &["implementer"],
        &[("AIDA_CONTAINED", Some("1"))],
    );
    assert!(build()
        .unwrap_err()
        .to_string()
        .contains("requires the supervised agent launcher"));
}

#[test]
fn story_2326_pr_b_session_and_phase_refuse_weaker_parent() {
    let _guard = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_CONTAINED", Some("1")),
        ("AIDA_AGENT_CMD", None),
    ]);
    let err =
        crate::session::build_codex_session_command("hello", false, false, None, None).unwrap_err();
    assert!(matches!(
        err.downcast_ref::<LaunchRefusal>(),
        Some(LaunchRefusal::WeakerThanParent { .. })
    ));
    let tmp = tempfile::tempdir().unwrap();
    for vendor in [
        crate::session::HeadlessVendor::Codex,
        crate::session::HeadlessVendor::Agy,
    ] {
        let advisor = crate::phase_driver::build_phase_driver_advisor_command(
            tmp.path(),
            vendor.program(),
            &[],
            &tmp.path().join("request"),
            &tmp.path().join("response"),
            "LIMIT",
            "1",
            vendor,
            false,
        )
        .unwrap_err();
        let fix = crate::phase_driver_trait::build_phase_driver_ci_fix_command(
            tmp.path(),
            vendor.program(),
            &[],
            "LIMIT",
            "1",
            vendor,
            false,
            "grant",
        )
        .unwrap_err();
        for err in [advisor, fix] {
            assert!(
                matches!(
                    err.downcast_ref::<LaunchRefusal>(),
                    Some(LaunchRefusal::WeakerThanParent { .. })
                ),
                "{err:#}"
            );
        }
    }
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_pr_b_compete_builds_real_templates_without_scanning_prompts() {
    use crate::compete::{self, JudgeVendor};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::write(
        root.join(".aida/config.toml"),
        "[contained]\nenable = false\n[contained.vendors]\ncodex = \"native\"\n",
    )
    .unwrap();
    // Native Codex policy must be committed on the trusted default branch.
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", ".aida/config.toml"],
        vec!["commit", "-qm", "trusted native policy"],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
        vec![
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    ] {
        let output = crate::test_git::git_command()
            .current_dir(root)
            .args(&args)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
    }
    let mut ambient = crate::test_env::AmbientGuard::hermetic_with_seat_and(
        root,
        "implementer",
        &[],
        &[("AIDA_CONTAINED", Some("0")), ("AIDA_OS_WRAP", Some("0"))],
    );
    let _cwd = crate::session::contained_cwd_seam::pin(root);
    for contained in [false, true] {
        ambient.set_key("AIDA_CONTAINED", if contained { "1" } else { "0" });
        for (vendor, judge_vendor) in [
            ("claude", JudgeVendor::Claude),
            ("codex", JudgeVendor::Codex),
        ] {
            for prompt in [
                "ordinary brief",
                "--dangerously-example brief",
                "--permission-mode",
                "--sandbox",
            ] {
                let adapter = compete::vendor_adapter(vendor).unwrap();
                let argv = compete::headless_argv(&adapter, prompt).unwrap();
                let arm = crate::compete_cmd::build_compete_arm_command(
                    root, root, vendor, &argv, vendor,
                )
                .unwrap();
                let (bin, argv) = compete::judge_command(judge_vendor, None, prompt);
                let judge = crate::compete_cmd::build_compete_judge_command(
                    root,
                    &bin,
                    &argv,
                    judge_vendor,
                )
                .unwrap();
                for (kind, cmd) in [("arm", arm), ("judge", judge)] {
                    let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
                    assert_eq!(
                        args.last().copied(),
                        Some(prompt),
                        "{vendor} {kind}, contained={contained}"
                    );
                    let flags = &args[..args.len() - 1];
                    if prompt.starts_with('-') {
                        assert_eq!(flags.last().copied(), Some("--"));
                    }
                    if contained {
                        assert!(!flags.iter().any(|a| matches!(
                            *a,
                            "bypassPermissions" | "--dangerously-bypass-approvals-and-sandbox"
                        )));
                        if vendor == "claude" {
                            assert!(flags
                                .windows(2)
                                .any(|a| a == ["--permission-mode", "dontAsk"]));
                        }
                    }
                    if vendor == "codex" && kind == "judge" {
                        assert!(flags.windows(2).any(|a| a == ["--sandbox", "read-only"]));
                    }
                }
            }
        }
    }
}

/// Queue selection must never change this process's authority. Both Claude
/// exec arms set the selected seat on their command before the shared exec.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_queue_exec_assigns_child_seat_without_parent_role_export() {
    let src = include_str!("../queue_cmd.rs");
    let body = src
        .split("pub(crate) fn handle_queue_work(")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(!body.contains("set_var(\"AIDA_SESSION_ROLE\""));
    for builder in [
        "session::build_claude_exec_command(",
        "session::build_claude_resume_exec_command(",
    ] {
        let arm = body.split(builder).nth(1).unwrap();
        let seat = arm.find("crate::launch::assign_seat(").unwrap();
        let exec = arm.find("crate::launch::spawn::exec_agent(").unwrap();
        assert!(seat < exec);
        assert!(arm[seat..exec].contains("child_grant_id.as_deref().unwrap_or(\"\")"));
    }
}

/// Intake and clarify use the same mint/assign lifetime, including failures
/// before spawn. Refused key installs must not persist a child grant.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_intake_clarify_keyring_refusal_and_grant_cleanup() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = launch_fixture();
    let _env = fx.ambient();
    std::env::set_current_dir(&fx.project).unwrap();
    for clarify in [false, true] {
        for contained in [true, false] {
            let build = || {
                if clarify {
                    crate::questions::build_questions_clarify_command(
                        crate::session::HeadlessVendor::Claude,
                        "clarify",
                        "",
                        contained,
                    )
                    .unwrap()
                } else {
                    crate::autopilot_intake::build_autopilot_intake_command(
                        crate::session::HeadlessVendor::Claude,
                        "intake",
                        if contained {
                            "dontAsk"
                        } else {
                            "bypassPermissions"
                        },
                        "",
                        false,
                        &[],
                        &[],
                        &crate::intake::IntakeConfig::default(),
                        "queue",
                        crate::backlog::RiskLevel::Low,
                        None,
                        contained,
                    )
                    .unwrap()
                }
            };
            let mut command = build();
            // Quota refusal is unconditional, including kernels where
            // keyrings are disabled. No child record or process may appear.
            let grants = crate::gate::seat_authority::grant_dir().unwrap();
            let before = std::fs::read_dir(&grants).unwrap().count();
            crate::grant_keyring::FORCE_INSTALL_ERRNO.with(|errno| errno.set(Some(libc::EDQUOT)));
            let result = super::assign_advisor_child(
                &fx.project,
                crate::session::HeadlessVendor::Claude,
                contained,
                &mut command,
            );
            {
                let error = result
                    .err()
                    .expect("contained child must refuse a missing key");
                assert!(format!("{error:#}").contains("key"), "{error:#}");
                assert!(!fx.capture.exists(), "refusal must precede child execution");
                assert_eq!(std::fs::read_dir(&grants).unwrap().count(), before);
            }
            if contained {
                // Unlike quota failure, EPERM only refuses a sandboxed
                // grant. This catches accidentally passing `false` here.
                crate::grant_keyring::FORCE_AVAILABLE.with(|available| available.set(Some(true)));
                crate::grant_keyring::FORCE_INSTALL_ERRNO
                    .with(|errno| errno.set(Some(libc::EPERM)));
                let refusal = super::assign_advisor_child(
                    &fx.project,
                    crate::session::HeadlessVendor::Claude,
                    contained,
                    &mut command,
                );
                crate::grant_keyring::FORCE_AVAILABLE.with(|available| available.set(None));
                let error = refusal
                    .err()
                    .expect("contained advisor must require the key");
                assert!(
                    format!("{error:#}").contains("contained `advisor` worker"),
                    "{error:#}"
                );
                assert_eq!(std::fs::read_dir(&grants).unwrap().count(), before);
            }
            // The scoped grant is live until spawn returns, and is revoked
            // on success and spawn error, in both containment postures.
            let grant = super::assign_advisor_child(
                &fx.project,
                crate::session::HeadlessVendor::Claude,
                contained,
                &mut command,
            )
            .unwrap();
            let id = command
                .get_envs()
                .find(|(key, _)| *key == "AIDA_SESSION_GRANT")
                .unwrap()
                .1
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            assert!(crate::gate::seat_authority::read_grant(&id)
                .unwrap()
                .revoked_at
                .is_none());
            if clarify {
                command.current_dir(fx.tmp.path().join("missing"));
                assert!(command.status().is_err());
            } else {
                assert!(command.status().unwrap().success());
            }
            drop(grant);
            assert!(crate::gate::seat_authority::read_grant(&id)
                .unwrap()
                .revoked_at
                .is_some());
            let _ = std::fs::remove_file(&fx.capture);
        }
    }
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_spawn_agent_clears_env() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("STORY2326_PARENT_SENTINEL", Some("secret")),
        ("AIDA_HOME", Some("/fixture/config")),
        ("AIDA_TEST_UNFORWARDED", Some("fixture")),
        ("AIDA_GITHUB_TOKEN", Some("credential")),
        ("AIDA_SESSION_PURPOSE", Some("parent purpose")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_SESSION_GRANT", Some("explicit-session-selector")),
    ]);
    let mut req = request(ContainmentPosture::None, LaunchKind::Fresh, &[]);
    req.program = "/usr/bin/env".into();
    req.args.clear();
    let ctx = LaunchContext::resolve_under(req.clone(), false).unwrap();
    let output = spawn_agent(&ctx).unwrap().output().unwrap();
    assert!(output.status.success());
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(env.lines().any(|line| line == "AIDA_HOME=/fixture/config"));
    assert!(env.lines().any(|line| line.starts_with("PATH=")));
    for name in [
        "STORY2326_PARENT_SENTINEL",
        "AIDA_TEST_UNFORWARDED",
        "AIDA_GITHUB_TOKEN",
    ] {
        assert!(
            !env.lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name}"
        );
    }
    for seat in [
        SeatDecision::Child {
            role: "reviewer",
            grant: "child-selector",
        },
        SeatDecision::Lane {
            role: None,
            grant: None,
        },
    ] {
        let mut command = spawn_agent(&ctx).unwrap();
        assign_seat(&mut command, LaunchKind::Fresh, seat).unwrap();
        let output = command.output().unwrap();
        assert!(output.status.success());
        let env = String::from_utf8(output.stdout).unwrap();
        assert!(!env.contains("AIDA_SESSION_PURPOSE="));
        assert!(!env.contains("AIDA_SESSION_ROLE=advisor"));
        assert!(!env.contains("AIDA_SESSION_GRANT=explicit-session-selector"));
    }
    let mut command = spawn_agent(&ctx).unwrap();
    assign_seat(
        &mut command,
        LaunchKind::Fresh,
        SeatDecision::ThisSession { grant: None },
    )
    .unwrap();
    let output = command.output().unwrap();
    assert!(output.status.success());
    let env = String::from_utf8(output.stdout).unwrap();
    assert!(env.lines().any(|line| line == "AIDA_SESSION_ROLE=advisor"));
    assert!(env
        .lines()
        .any(|line| line == "AIDA_SESSION_GRANT=explicit-session-selector"));
    assert!(!env.contains("STORY2326_PARENT_SENTINEL="));
    let ctx = LaunchContext::resolve_with(req, false, true).unwrap();
    let output = spawn_agent(&ctx).unwrap().output().unwrap();
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("STORY2326_PARENT_SENTINEL=secret"));
}

// Exercise the real cleared process boundary without AIDA_AGENT_CMD inheritance.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_reviewer_verdict_is_explicit_after_clear() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[(
        "AIDA_REVIEW_VERDICT_FILE",
        Some("/review/verdict.json"),
    )]);
    let mut req = request(ContainmentPosture::None, LaunchKind::Fresh, &[]);
    req.program = "/usr/bin/env".into();
    req.args.clear();
    let ctx = LaunchContext::resolve_under(req, false).unwrap();
    for role in ["reviewer", "implementer"] {
        let mut cmd = spawn_agent(&ctx).unwrap();
        assign_seat(
            &mut cmd,
            ctx.kind(),
            SeatDecision::Child { role, grant: "" },
        )
        .unwrap();
        let output = cmd.output().unwrap();
        assert!(output.status.success());
        let env = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            env.lines()
                .any(|l| l == "AIDA_REVIEW_VERDICT_FILE=/review/verdict.json"),
            role == "reviewer"
        );
    }
    let mut cmd = spawn_agent(&ctx).unwrap();
    cmd.env("AIDA_REVIEW_VERDICT_FILE", "/site/verdict.json");
    assign_seat(
        &mut cmd,
        ctx.kind(),
        SeatDecision::Child {
            role: "reviewer",
            grant: "",
        },
    )
    .unwrap();
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .any(|line| line == "AIDA_REVIEW_VERDICT_FILE=/site/verdict.json"));
}
