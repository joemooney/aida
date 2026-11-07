//! Exact-child metadata transport; real Codex sandbox fixtures make no model call.
// trace:BUG-2398 | ai:codex
#![cfg(target_os = "linux")]

use super::*;
use crate::launch::profile::finalize_codex as prepare_launch;
use std::path::Path;

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
#[test]
fn bug_2398_whitelist_encoding_absent_empty_and_secret_ratchet() {
    for key in TOOL_ENV {
        assert!(key.starts_with("AIDA_"));
        for forbidden in [
            "TOKEN",
            "SECRET",
            "KEY",
            "PASSWORD",
            "GRANT_FILE",
            "CONTAINED",
        ] {
            assert!(!key.contains(forbidden), "{key}");
        }
    }
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_USER", "child's \\\" identity ☃")
        .env("AIDA_SESSION_GRANT", "")
        .env_remove("AIDA_SESSION_SCOPE")
        .env("AIDA_AGENT_REGISTRY_TOKEN", "DO-NOT-TRANSPORT")
        .env("UNRELATED_PARENT", "DO-NOT-TRANSPORT");
    let args = codex_tool_env_overrides(&cmd).unwrap();
    assert_eq!(args.len(), 4);
    assert!(!args.join(" ").contains("DO-NOT-TRANSPORT"));
    for pair in args.chunks_exact(2) {
        assert_eq!(pair[0], "-c");
        let parsed: toml::Value = toml::from_str(&pair[1]).unwrap();
        let table = &parsed["shell_environment_policy"]["set"];
        if let Some(value) = table.get("AIDA_USER") {
            assert_eq!(value.as_str(), Some("child's \\\" identity ☃"));
        } else {
            assert_eq!(table["AIDA_SESSION_GRANT"].as_str(), Some(""));
        }
    }
    for value in ["x\ny", "x\0y", "x\ry"] {
        cmd.env("AIDA_USER", value);
        assert!(codex_tool_env_overrides(&cmd).is_err());
    }
    let mut absent = Command::new("codex");
    absent.env_remove("AIDA_SESSION_GRANT");
    assert!(codex_tool_env_overrides(&absent).unwrap().is_empty());
    absent.env("AIDA_SESSION_GRANT", "not-a-uuid");
    assert!(codex_tool_env_overrides(&absent).is_err());
}

#[test]
fn bug_2398_only_explicit_child_selector_and_sandbox_options() {
    let mut cmd = Command::new("codex");
    let child = uuid::Uuid::new_v4().to_string();
    let parent = uuid::Uuid::new_v4().to_string();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_GRANT", Some(&parent))]);
    assert!(codex_tool_env_overrides(&cmd).unwrap().is_empty());
    cmd.env("AIDA_SESSION_GRANT", &child);
    let args = codex_tool_env_overrides(&cmd).unwrap();
    assert!(args.join(" ").contains(&child));
    assert!(!args.join(" ").contains(&parent));
    cmd.args([
        "exec",
        "--sandbox",
        "workspace-write",
        "--model",
        "unused",
        "--json",
        "prompt",
    ]);
    assert_eq!(
        sandbox_args(&cmd).unwrap(),
        ["sandbox", "-P", ":workspace", "--include-managed-config"]
    );
    let mut sandbox = Command::new("codex");
    sandbox.args([
        "sandbox",
        "-P",
        ":workspace",
        "--",
        "sh",
        "-c",
        "target shell script",
    ]);
    assert_eq!(
        sandbox_args(&sandbox).unwrap(),
        ["sandbox", "-P", ":workspace", "--include-managed-config"]
    );
    // Pure argv builders for other vendors retain their prior behavior.
    assert_eq!(
        crate::session::headless_vendor_args(
            crate::seat_clone::launcher_main().as_deref(),
            crate::session::HeadlessVendor::Agy,
            "fixture",
            "session",
            false,
            None,
            None
        )
        .unwrap(),
        crate::session::agy_headless_args("fixture")
    );
    let claude = crate::session::claude_session_args(
        crate::seat_clone::launcher_main().as_deref(),
        None,
        None,
        Some("hello"),
        None,
        false,
        None,
    )
    .unwrap();
    assert!(!claude.join(" ").contains("shell_environment_policy"));
}

fn output(cmd: &mut Command) -> std::process::Output {
    // All fixture invocations get a short watchdog, including blocked namespaces.
    let mut log = tempfile::tempfile().unwrap();
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log.try_clone().unwrap());
    let status = bounded_status(cmd, std::time::Duration::from_secs(10)).unwrap();
    use std::io::{Read, Seek, SeekFrom};
    log.seek(SeekFrom::Start(0)).unwrap();
    let mut bytes = Vec::new();
    log.take(65536).read_to_end(&mut bytes).unwrap();
    std::process::Output {
        status,
        stdout: bytes,
        stderr: Vec::new(),
    }
}

fn run_codex_sandbox(
    home: &Path,
    root: &Path,
    child: &Command,
    argv: &[&str],
    native: bool,
) -> std::process::Output {
    let mut cmd = Command::new("codex");
    cmd.current_dir(root).env("CODEX_HOME", home);
    for (key, value) in child.get_envs() {
        if let Some(value) = value {
            cmd.env(key, value);
        } else {
            cmd.env_remove(key);
        }
    }
    cmd.arg("sandbox");
    if native {
        cmd.args(["-P", ":workspace"]);
    } else {
        cmd.args(["-c", "sandbox_mode=\"danger-full-access\""]);
    }
    cmd.arg("--").args(argv);
    // Exercise the actual finalizer, including requested-seat preflight.
    if child
        .get_envs()
        .any(|(key, value)| key == "BUG2398_INVALID_RECORDER_PROBE" && value.is_some())
    {
        let overrides = codex_tool_env_overrides(child).unwrap();
        let original: Vec<_> = cmd.get_args().map(|arg| arg.to_owned()).collect();
        let mut raw = Command::new(cmd.get_program());
        raw.current_dir(root)
            .env("CODEX_HOME", home)
            .args(overrides)
            .args(original);
        for (key, value) in cmd.get_envs() {
            match value {
                Some(value) => {
                    raw.env(key, value);
                }
                None => {
                    raw.env_remove(key);
                }
            }
        }
        return output(&mut raw);
    }
    prepare_launch(&mut cmd).unwrap();
    output(&mut cmd)
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
#[test]
fn bug_2398_real_codex_core_delegated_roles_and_reviewer_recording() {
    real_codex_roles(false);
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
#[test]
fn bug_2398_real_native_codex_core_delegated_roles_and_reviewer_recording() {
    real_codex_roles(true);
}

fn real_codex_roles(native: bool) {
    let sandbox = |home: &Path, root: &Path, child: &Command, argv: &[&str]| {
        run_codex_sandbox(home, root, child, argv, native)
    };
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    use aida_core::db::DatabaseBackend;
    let codex = Command::new("codex").arg("--version").output();
    if !codex.is_ok_and(|out| out.status.success()) {
        eprintln!("SKIP: codex is not installed");
        return;
    }
    let mut env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_USER", Some("bug2398-parent"))]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let config = root.join("codex-config");
    std::fs::create_dir(&config).unwrap();
    let contents = "[shell_environment_policy]\ninherit = \"core\"\n[shell_environment_policy.set]\nKEEP_ME = \"preserved\"\n";
    std::fs::write(config.join("config.toml"), contents).unwrap();
    let parent = mint_grant_for(
        root,
        "bug2398-parent",
        "advisor",
        &["implementer", "reviewer"],
    );
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    aida_core::git_ops::configure_user(&store, "fixture", "fixture@localhost").unwrap();
    let backend = crate::test_env::home_fixture_cached_backend(
        &store,
        &aida_core::CachedGitBackend::default_cache_path(&store),
    )
    .unwrap();
    let mut req = aida_core::models::Requirement::new("fixture".into(), "desc".into());
    req.spec_id = Some("TASK-1".into());
    backend.add_requirement(req).unwrap();
    crate::test_git::commit_fixture(root);
    let parent = authority::read_grant(&parent).unwrap();
    let implementer =
        authority::issue_child_from(parent.clone(), "implementer", "bug2398-child", true).unwrap();
    let mut child = Command::new("codex");
    child
        .env("AIDA_SESSION_GRANT", &implementer.id)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_USER", "bug2398-child")
        .env("AIDA_PROJECT_ROOT", root)
        .env("AIDA_SESSION_SCOPE", "TASK-1")
        .env(
            "AIDA_CONTAINMENT_POSTURE",
            // trace:TASK-2708 | ai:claude — `aida` is refused at this finalizer.
            if native { "native" } else { "none" },
        )
        .env("AIDA_SESSION_ID", "bug2398-independent-session")
        .env("AIDA_AGENT_TYPE", "codex")
        .env("AIDA_ROLE_INSTANCE", "companion")
        .env("AIDA_AGENT_NAME", "codex-implementer-fixture")
        .env("AIDA_AGENT_REGISTRY_TOKEN", "fixture-excluded-bearer")
        .env("UNRELATED_PARENT", "fixture-excluded-value");
    // The fixture must exercise this implementation's probe, never an older
    // installed CLI that lacks --probe-seat. CI builds the sibling binary.
    let aida_path = crate::test_exec::matching_aida_cli();
    let _aida_probe = crate::kernel::test_aida_exe::set(aida_path.clone());
    let aida = aida_path.to_string_lossy().into_owned();
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.lines().any(|l| l == "implementer"),
        "actual Codex sandbox role probe: {text}"
    );
    assert_effective_child_context(&config, root, &child, &parent, native);
    let out = sandbox(&config, root, &child, &["sh", "-c", "test \"$KEEP_ME\" = preserved && test \"$AIDA_USER\" = bug2398-child && test \"$AIDA_SESSION_SCOPE\" = TASK-1 && test -z \"${AIDA_AGENT_REGISTRY_TOKEN-}\""]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = sandbox(
        &config,
        root,
        &child,
        &["sh", "-c", "test \"$AIDA_ROLE_INSTANCE\" = companion"],
    );
    assert!(
        out.status.success(),
        "companion metadata must survive core policy"
    );
    child.env("AIDA_ROLE_INSTANCE", "driver");
    // Permitted own-scope write and real advisor/wrong-seat refusals.
    for (argv, success) in [
        (
            vec![
                aida.as_str(),
                "comment",
                "add",
                "TASK-1",
                "own-scope fixture",
            ],
            true,
        ),
        (
            vec![aida.as_str(), "edit", "TASK-1", "--status", "approved"],
            false,
        ),
        (
            vec![
                aida.as_str(),
                "review",
                "record",
                "TASK-1",
                "--verdict",
                "approved",
            ],
            false,
        ),
    ] {
        let out = sandbox(&config, root, &child, &argv);
        assert_eq!(
            out.status.success(),
            success,
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let reviewer =
        authority::issue_child_from(parent, "reviewer", "bug2398-reviewer", true).unwrap();
    child
        .env("AIDA_SESSION_GRANT", &reviewer.id)
        .env("AIDA_SESSION_ROLE", "reviewer")
        .env("AIDA_USER", "bug2398-reviewer")
        .env("AIDA_AGENT_NAME", "codex-reviewer-fixture");
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(
        out.status.success(),
        "granted reviewer: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    env.set_key("AIDA_USER", "bug2398-reviewer");
    assert_recorded_authority_boundaries(root, &reviewer.id, true);
    authority::revoke_by_id(&reviewer.id).unwrap();
    // Direct tool probes intentionally present invalid authority; a managed
    // launch would already refuse it in preflight.
    child.env("BUG2398_INVALID_RECORDER_PROBE", "1");
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
    );
    assert!(!out.status.success(), "revoked reviewer must refuse");
    // Presented invalid grants must refuse the recorder, not merely role display.
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(
        !out.status.success(),
        "presented revoked grant must refuse recording"
    );
    child
        .env("AIDA_SESSION_GRANT", "")
        .env_remove("AIDA_SESSION_ROLE")
        .env_remove("BUG2398_INVALID_RECORDER_PROBE");
    let out = sandbox(
        &config,
        root,
        &child,
        &[
            "sh",
            "-c",
            "test \"${AIDA_SESSION_GRANT+x}\" = x && test -z \"$AIDA_SESSION_GRANT\"",
        ],
    );
    assert!(out.status.success());
    let out = sandbox(
        &config,
        root,
        &child,
        &[&aida, "review", "record", "TASK-1", "--verdict", "approved"],
    );
    assert!(
        out.status.success(),
        "named absent-grant compatibility record: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_recorded_authority_boundaries(root, &reviewer.id, false);
    assert_eq!(
        std::fs::read_to_string(config.join("config.toml")).unwrap(),
        contents
    );
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac51a4f1 | ai:claude
#[test]
#[cfg(target_os = "linux")]
fn bug_2398_preflight_refuses_cause_before_model_and_baseline_skips_probe() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("codex");
    crate::test_exec::write_executable(
        &fake,
        "#!/bin/sh\necho 'password=must-never-be-relayed' >&2\necho 'bwrap: No permissions to create new namespace' >&2\nexit 1\n",
    );
    let mut cmd = Command::new(&fake);
    cmd.args(["exec", "--sandbox", "workspace-write", "do work"])
        .env("AIDA_SESSION_GRANT", uuid::Uuid::new_v4().to_string())
        .env("AIDA_SESSION_ROLE", "reviewer");
    let error = prepare_launch(&mut cmd).unwrap_err().to_string();
    assert!(
        error.contains("No permissions to create new namespace"),
        "{error}"
    );
    assert!(!error.contains("must-never-be-relayed"));
    assert!(!cmd.get_args().any(|arg| arg
        .to_string_lossy()
        .starts_with("shell_environment_policy.set.")));
    cmd.env("AIDA_SESSION_GRANT", "")
        .env("AIDA_SESSION_ROLE", "implementer");
    prepare_launch(&mut cmd).unwrap();
    // A stuck namespace helper is bounded too.
    let error = bounded_status(
        Command::new("sh").args(["-c", "sleep 30"]),
        std::time::Duration::from_millis(50),
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
}

#[test]
fn bug_2398_prefix_and_wrapped_prefix_preserve_companion_without_tokens() {
    for wrapped in [false, true] {
        let mut cmd = Command::new(if wrapped { "bwrap" } else { "codex" });
        if wrapped {
            cmd.args(["--ro-bind", "/", "/", "--", "/usr/bin/codex"]);
        }
        cmd.args(["exec", "--", "prompt"])
            .env("AIDA_SESSION_GRANT", "")
            .env("AIDA_ROLE_INSTANCE", "companion")
            .env("AIDA_HANDLE_LAUNCH_TOKEN", "excluded-handle-bearer")
            .env("AIDA_AGENT_REGISTRY_TOKEN", "excluded-registry-bearer");
        prepare_launch(&mut cmd).unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let prefix = if wrapped { 5 } else { 0 };
        assert_eq!(args[prefix], "-c");
        assert!(args[prefix + 1].starts_with("shell_environment_policy.set.AIDA_ROLE_INSTANCE="));
        let exec = args.iter().position(|arg| arg == "exec").unwrap();
        assert!(exec > prefix);
        assert_eq!(&args[exec..], ["exec", "--", "prompt"]);
        assert!(!args.join(" ").contains("excluded-"));
        assert!(cmd
            .get_envs()
            .any(|(key, value)| key == "AIDA_ROLE_INSTANCE"
                && value == Some(std::ffi::OsStr::new("companion"))));
        cmd.env("AIDA_ROLE_INSTANCE", "invalid");
        assert!(codex_tool_env_overrides(&cmd).is_err());
    }
    for grant in [None, Some("")] {
        let mut cmd = Command::new("codex");
        cmd.env("AIDA_SESSION_ROLE", "reviewer");
        if let Some(grant) = grant {
            cmd.env("AIDA_SESSION_GRANT", grant);
        }
        assert!(
            prepare_launch(&mut cmd).is_err(),
            "a requested reviewer cannot launch without its grant"
        );
    }
}

/// How a launch-inventory file finalizes the Codex children it launches.
#[derive(Debug)]
enum Finalization {
    /// Calls `codex_tool_env::prepare_launch` at least this many times.
    PrepareLaunch(usize),
    /// Spawns only through `session::coordinator_status_with_handle_guard`,
    /// which finalizes Codex children itself.
    CoordinatorSpawn,
    /// Assigns a selector but launches no Codex child (reason recorded).
    NotACodexLaunch(&'static str),
}

/// ONE inventory of every production source file that hands a child Command
/// its seat grant selector or spawns a coordinator child, pinned by exact
/// marker count. A file not listed, or a listed file whose count moved, fails:
/// a new launch site must be audited (Codex: finalize through
/// `prepare_launch`) and recorded here. This is how the #3217 coordinator
/// launches would have been caught.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
const LAUNCH_INVENTORY: &[(&str, usize, Finalization)] = &[
    // STORY-2326: the fg/bg seat writes moved into launch::assign_seat; the
    // 2 grant writes plus 5 launcher calls (2 spawn_agent, 3 assign_seat).
    ("agent_launch.rs", 7, Finalization::PrepareLaunch(2)),
    // STORY-2254: the detached Codex worker (one grant assignment, one
    // finalization, after its own grant/headless/identity assignments).
    (
        "agent_codex_background.rs",
        1,
        Finalization::PrepareLaunch(1),
    ),
    ("autopilot_intake.rs", 2, Finalization::CoordinatorSpawn),
    ("burndown_run.rs", 4, Finalization::CoordinatorSpawn),
    ("compete_cmd.rs", 2, Finalization::PrepareLaunch(1)),
    (
        "containment/supervisor.rs",
        1,
        Finalization::NotACodexLaunch(
            "cfg(test) synthetic selector proving the probe's env_clear forwards none",
        ),
    ),
    // trace:STORY-2338 | ai:claude — the script deputy hands each job the
    // operator's own (refreshed) grant; the child is `bash` running the
    // submitted script, never a Codex agent.
    (
        "deputy_shell/runner.rs",
        1,
        Finalization::NotACodexLaunch(
            "bash running a script-deputy job under the operator's grant; no agent vendor",
        ),
    ),
    // trace:STORY-2326 | ai:codex — spawn_agent + assign_seat cover the Claude/Agy
    // implementer builder blanking the grant (Boundary B). A Codex take/impl
    // lane runs `aida agent new codex --bg`, a separate aida process that
    // finalizes in agent_codex_background.rs.
    (
        "dispatch.rs",
        2,
        Finalization::NotACodexLaunch(
            "Claude/Agy implementer lanes blank the grant; Codex lanes launch through \
             `aida agent new codex --bg`, finalized in agent_codex_background.rs",
        ),
    ),
    // trace:STORY-2326 | ai:claude — the one seat writer; its Codex callers
    // finalize at their own (inventoried) sites.
    (
        "launch/spawn.rs",
        3,
        Finalization::NotACodexLaunch(
            "launch::assign_seat, the seat writer every converted site calls",
        ),
    ),
    // PR-C moves the shared advisor seat assignment here; intake/questions
    // retain their coordinator finalization and revoke-on-return boundary.
    // trace:STORY-2326 | ai:codex
    (
        "launch/grant.rs",
        1,
        Finalization::NotACodexLaunch(
            "advisor seat builder only; intake/questions finalize via coordinator spawn",
        ),
    ),
    ("phase_driver.rs", 2, Finalization::PrepareLaunch(1)),
    ("phase_driver_trait.rs", 2, Finalization::PrepareLaunch(1)),
    ("questions.rs", 2, Finalization::CoordinatorSpawn),
    (
        "queue_cmd.rs",
        4,
        Finalization::NotACodexLaunch(
            "explicit Claude exec seats; Codex uses finalized session constructors",
        ),
    ),
    // Claude-only builders/resumes plus the shared seat-env helper; every
    // Codex spawn in this file (incl. the coordinator spawn) finalizes.
    // BUG-2386 adds two Codex resume sites, both finalized: the interactive
    // `build_codex_resume_command` (finalized by `finalize_codex_resume`, which
    // `aida session resume`, `queue work --resume` and the reviewer resume
    // call last) and the headless `spawn_vendor_headless_resume_granted`
    // (punt/bg resume), which finalizes before stdio and revokes on refusal.
    // STORY-2326: exec, headless forking, reviewer, advisor and coordinator
    // seat writes moved into launch::assign_seat: 9 grant/coordinator markers
    // plus 16 launcher calls (8 spawn_agent, 8 assign_seat); PR-B removes
    // two direct grant writes.
    // trace:STORY-2326 | ai:claude
    ("session.rs", 26, Finalization::PrepareLaunch(7)),
    // trace:BUG-2416 | ai:claude — the guided `aida do` session.
    ("zen_drive_root.rs", 2, Finalization::PrepareLaunch(1)),
];

/// Child-launch markers in whitespace-normalized source: a grant selector
/// assigned to a Command, the shared seat-env helper, or a coordinator spawn.
/// The selector counts in every spelling a launch site could use: the
/// fully-qualified or imported `GRANT_ENV` path, an import or `let` alias of
/// it, the literal `"AIDA_SESSION_GRANT"`, each as the key of `.env(..)` or
/// anywhere inside one `.envs(..)` call.
// trace:TASK-2759 | ai:claude
fn launch_markers(normalized: &str) -> usize {
    let aliases = grant_env_aliases(normalized);
    let is_key = |arg: &str| {
        let arg = arg.trim_start_matches('&');
        arg == "\"AIDA_SESSION_GRANT\""
            || arg == "GRANT_ENV"
            || arg.ends_with("::GRANT_ENV")
            || aliases.iter().any(|alias| alias == arg)
    };
    let env_sites = normalized
        .split(".env(")
        .skip(1)
        .filter(|call| is_key(call.split([',', ')']).next().unwrap_or_default()))
        .count();
    let envs_sites = normalized
        .split(".envs(")
        .skip(1)
        .filter(|call| {
            call_args(call)
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':' || c == '"'))
                .any(is_key)
        })
        .count();
    env_sites
        + envs_sites
        + normalized
            .matches("apply_headless_child_seat_env(&mut")
            .count()
        + normalized
            .matches("coordinator_status_with_handle_guard(")
            .count()
        - normalized
            .matches("fncoordinator_status_with_handle_guard(")
            .count()
}

/// The text of one call's arguments: `rest` starts just after the opening
/// parenthesis; stops at its matching close.
fn call_args(rest: &str) -> &str {
    let mut depth = 0usize;
    for (at, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return &rest[..at],
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    rest
}

/// Local names bound to the grant selector: `use ..GRANT_ENV as NAME` and
/// `let NAME = ..GRANT_ENV;` (or the literal) in normalized source.
fn grant_env_aliases(normalized: &str) -> Vec<String> {
    let ident = |s: &str| {
        s.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<String>()
    };
    let mut aliases: Vec<String> = normalized
        .split("GRANT_ENVas")
        .skip(1)
        .map(ident)
        .filter(|name| !name.is_empty())
        .collect();
    for binding in normalized.split("let").skip(1) {
        let binding = binding.strip_prefix("mut").unwrap_or(binding);
        let name = ident(binding);
        let rest = &binding[name.len()..];
        let value = match rest.chars().next() {
            Some('=') => &rest[1..],
            Some(':') => rest.split_once('=').map_or("", |(_, v)| v),
            _ => continue,
        };
        let value = value.split(';').next().unwrap_or_default();
        if !name.is_empty()
            && (value == "GRANT_ENV"
                || value.ends_with("::GRANT_ENV")
                || value == "\"AIDA_SESSION_GRANT\"")
        {
            aliases.push(name);
        }
    }
    aliases
}

/// Calls of the STORY-2326 launcher in raw source. `spawn_agent` builds a
/// child and `assign_seat` writes its role and grant inside
/// `launch/spawn.rs`, so each caller is a launch site the `.env(..)` scans
/// would miss once its writes moved behind the launcher (advisor ruling 2,
/// round 5). A definition (`fn assign_seat(`) is not a call, a longer name
/// (`exec_or_spawn_agent(`) is not either, and an import alias
/// (`use ..assign_seat as NAME`) is followed.
// trace:STORY-2326 | ai:claude
fn launcher_calls(raw: &str) -> usize {
    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }
    let calls = |name: &str| {
        raw.match_indices(name)
            .filter(|(at, _)| {
                let before = &raw[..*at];
                let defines = before
                    .trim_end()
                    .strip_suffix("fn")
                    .is_some_and(|b| !b.ends_with(is_ident));
                !before.ends_with(is_ident)
                    && raw[at + name.len()..].trim_start().starts_with('(')
                    && !defines
            })
            .count()
    };
    [
        "assign_seat",
        "spawn_agent",
        "exec_agent",
        "assign_advisor_child",
    ]
    .iter()
    .map(|&name| {
        let aliases: usize = raw
            .split(&format!("{name} as "))
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .take_while(|c| is_ident(*c))
                    .collect::<String>()
            })
            .filter(|alias| !alias.is_empty())
            .map(|alias| calls(alias.as_str()))
            .sum();
        calls(name) + aliases
    })
    .sum()
}

/// The LAUNCH_INVENTORY count of the production source `src`.
fn launch_sites(src: &str) -> usize {
    launch_markers(&src.split_whitespace().collect::<String>()) + launcher_calls(src)
}

/// Whether the final ratchet would flag `src` as an unlisted launch site if
/// it were the production file `rel`.
fn flagged_as_unlisted(rel: &str, src: &str) -> bool {
    launch_sites(src) > 0 && !LAUNCH_INVENTORY.iter().any(|(f, ..)| *f == rel)
}

/// Advisor ruling 2 (round 5): a file that launches only through
/// `launch::spawn_agent` / `launch::assign_seat` writes no selector itself,
/// yet is a launch site in both inventories.
// trace:STORY-2326 | ai:claude
#[test]
fn story_2326_inventories_count_launcher_callers() {
    let advisor = "fn go() { crate::launch::assign_advisor_child(root, vendor, contained, cmd); }";
    assert_eq!(launcher_calls(advisor), 1);
    assert_eq!(launch_sites(advisor), 1);
    assert_eq!(role_sites(advisor), 1);
    let exec = "fn go(cmd: &mut Command) { crate::launch::spawn::exec_agent(cmd); }";
    assert_eq!(launcher_calls(exec), 1);
    assert!(flagged_as_unlisted("fixture_exec.rs", exec));
    assert!(role_flagged_as_unlisted("fixture_exec.rs", exec));
    let seat = "fn launch(ctx: &LaunchContext, g: &str) -> Result<()> {\n\
                let mut cmd = crate::launch::spawn_agent(ctx)?;\n\
                crate::launch::assign_seat(&mut cmd, ctx.kind(),\n\
                SeatDecision::Child { role: \"implementer\", grant: g })?; Ok(()) }";
    assert_eq!(launcher_calls(seat), 2);
    assert!(flagged_as_unlisted("fixture_launcher.rs", seat));
    assert!(role_flagged_as_unlisted("fixture_launcher.rs", seat));
    let aliased = "use crate::launch::{assign_seat as seat_of, spawn_agent as build};\n\
                   fn launch(ctx: &LaunchContext) -> Result<()> {\n\
                   let mut cmd = build (ctx)?;\n\
                   seat_of(&mut cmd, ctx.kind(), NO_SEAT) }";
    assert_eq!(launcher_calls(aliased), 2);
    assert!(flagged_as_unlisted("fixture_aliased.rs", aliased));
    assert!(role_flagged_as_unlisted("fixture_aliased.rs", aliased));
    let returned = "fn build(ctx: &LaunchContext) -> Result<Command> { return spawn_agent(ctx); }";
    assert_eq!(launcher_calls(returned), 1);
    // A definition, a longer name and a doc link are not calls.
    let clean = "/// Built by [`spawn_agent`].\n\
                 pub(crate) fn spawn_agent(ctx: &LaunchContext) -> Result<Command> { todo!() }\n\
                 fn assign_seat (cmd: &mut Command) {}\n\
                 fn go(cmd: &mut Command) { exec_or_spawn_agent(cmd); my_assign_seat(cmd); }";
    assert_eq!(launcher_calls(clean), 0);
    assert!(!flagged_as_unlisted("fixture_clean_launcher.rs", clean));
    assert!(!role_flagged_as_unlisted(
        "fixture_clean_launcher.rs",
        clean
    ));
}

// One negative test per selector spelling the inventory scan must catch.
// trace:TASK-2759 | ai:claude
#[test]
fn task_2759_ratchet_flags_imported_grant_env_and_aliases() {
    let imported = "use crate::gate::seat_authority::GRANT_ENV;\n\
                    fn launch(cmd: &mut Command, g: &str) { cmd.env(GRANT_ENV, g); }";
    assert!(flagged_as_unlisted("fixture_imported.rs", imported));
    let renamed = "use crate::gate::seat_authority::GRANT_ENV as SELECTOR;\n\
                   fn launch(cmd: &mut Command, g: &str) { cmd.env(SELECTOR, g); }";
    assert!(flagged_as_unlisted("fixture_renamed.rs", renamed));
    let bound = "fn launch(cmd: &mut Command, g: &str) {\n\
                 let key = crate::gate::seat_authority::GRANT_ENV;\n cmd.env(key, g); }";
    assert!(flagged_as_unlisted("fixture_bound.rs", bound));
}

// trace:TASK-2759 | ai:claude
#[test]
fn task_2759_ratchet_flags_literal_grant_selector() {
    let literal = "fn launch(cmd: &mut Command, g: &str) {\n\
                   cmd.env(\"AIDA_SESSION_GRANT\", g); }";
    assert!(flagged_as_unlisted("fixture_literal.rs", literal));
}

// trace:TASK-2759 | ai:claude
#[test]
fn task_2759_ratchet_flags_envs_array_grant_selector() {
    let envs = "fn launch(cmd: &mut Command, g: &str) {\n\
                cmd.envs([(\"AIDA_SESSION_ROLE\", \"reviewer\"),\n\
                (crate::gate::seat_authority::GRANT_ENV, g)]); }";
    assert!(flagged_as_unlisted("fixture_envs.rs", envs));
    let envs_literal = "fn launch(cmd: &mut Command, g: &str) {\n\
                        cmd.envs([(\"AIDA_SESSION_GRANT\", g)]); }";
    assert!(flagged_as_unlisted("fixture_envs_literal.rs", envs_literal));
    // A grant-free `.envs(..)` and a selector that is only removed or read
    // are not launch sites.
    let clean = "fn launch(cmd: &mut Command) {\n\
                 cmd.envs(crate::headless_wait_envs(k, v))\n\
                 .env_remove(crate::gate::seat_authority::GRANT_ENV);\n\
                 let _ = std::env::var(GRANT_ENV); }";
    assert!(!flagged_as_unlisted("fixture_clean.rs", clean));
}

fn production_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                production_sources(&path, out);
            }
        } else if name.ends_with(".rs")
            && !name.ends_with("_tests.rs")
            && !name.starts_with("test_")
        {
            out.push(path);
        }
    }
}

// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
#[test]
fn bug_2398_final_spawn_boundary_ratchet() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    production_sources(&src, &mut files);
    let mut found = std::collections::BTreeMap::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let markers = launch_sites(&text);
        if markers > 0 {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            found.insert(rel, (markers, text));
        }
    }
    for (rel, (markers, _)) in &found {
        let Some((_, expected, _)) = LAUNCH_INVENTORY.iter().find(|(f, ..)| f == rel) else {
            panic!(
                "{rel} launches a seat-granted child but is not in LAUNCH_INVENTORY: a Codex \
                 child must finalize through codex_tool_env::prepare_launch; audit it and record it"
            );
        };
        assert_eq!(
            markers, expected,
            "{rel}: child-launch site count changed; audit the new site for Codex \
             finalization (prepare_launch) and update LAUNCH_INVENTORY"
        );
    }
    for (rel, _, how) in LAUNCH_INVENTORY {
        let (_, text) = found
            .get(*rel)
            .unwrap_or_else(|| panic!("stale LAUNCH_INVENTORY entry {rel}"));
        match how {
            Finalization::PrepareLaunch(n) => assert!(
                text.matches("crate::launch::profile::finalize_codex(")
                    .count()
                    >= *n,
                "{rel}: a managed Codex spawn path lost finalization"
            ),
            Finalization::CoordinatorSpawn => {
                // Every coordinator call site revokes the child grant it
                // issued after the call returns, so a finalization refusal
                // leaves no live grant.
                for call in text
                    .split("session::coordinator_status_with_handle_guard(")
                    .skip(1)
                {
                    let rest_of_fn = call.split("\n}\n").next().unwrap();
                    if matches!(*rel, "autopilot_intake.rs" | "questions.rs") {
                        // PR-C's scoped advisor grant owns revocation, including
                        // assignment and spawn errors. Explicitly drop it before
                        // propagating the coordinator result or exiting.
                        // trace:STORY-2326 | ai:codex
                        assert!(text.contains("crate::launch::assign_advisor_child("));
                        let drop = rest_of_fn.find("drop(child_grant);").unwrap();
                        let propagate = rest_of_fn.find("let status_code = status_code?;").unwrap();
                        assert!(drop < propagate, "{rel}: result escapes grant cleanup");
                        let grant = include_str!("../launch/grant.rs");
                        assert!(grant.contains("impl Drop for AdvisorChildGrant"));
                        assert!(grant
                            .contains("crate::session::revoke_headless_child_grant(&self.0.id)"));
                    } else {
                        assert!(
                            rest_of_fn.contains("revoke_headless_child_grant("),
                            "{rel}: a coordinator launch does not revoke its child grant"
                        );
                    }
                }
                assert!(!text.contains("crate::launch::profile::finalize_codex("));
            }
            Finalization::NotACodexLaunch(reason) => assert!(!reason.is_empty()),
        }
    }
    // The coordinator spawn finalizes BEFORE stdio and spawn: finalization
    // rebuilds the Command and would drop stdio configured earlier.
    let session = include_str!("../session.rs");
    let finalize = session
        .split("fn finalize_coordinator_launch(")
        .nth(1)
        .unwrap();
    assert!(finalize
        .split("\n}\n")
        .next()
        .unwrap()
        .contains("crate::launch::profile::finalize_codex(cmd)"));
    let spawn = session
        .split("fn coordinator_status_with_handle_guard(")
        .nth(1)
        .unwrap()
        .split("\n}\n")
        .next()
        .unwrap();
    let at = |needle: &str| spawn.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(
        at("finalize_coordinator_launch(vendor, cmd, project_root)?") < at("cmd.stdout(stdout)")
    );
    assert!(at("cmd.stdout(stdout)") < at("spawn_retrying_etxtbsy()"));
    let queue = include_str!("../queue_cmd.rs");
    assert!(queue.contains("session::exec_codex_session("));
    // Pure composers stay pure; caller-side customization precedes finalization.
    let session = include_str!("../session.rs");
    let run = session
        .split("fn run_vendor_headless_child(")
        .nth(1)
        .unwrap();
    assert!(
        run.find("customize(&mut child_cmd)").unwrap()
            < run.find("crate::launch::profile::finalize_codex").unwrap()
    );
}

/// A coordinator-launched Codex child (the #3217 burndown path) carries its
/// delegated seat into tool shells through `-c shell_environment_policy.set`
/// overrides, keeps the stdout configured by its caller, and a preflight
/// refusal stops the launch before the model ever runs. A fake `codex`
/// records argv; no model call, CI-runnable.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
#[test]
fn bug_2398_coordinator_codex_launch_keeps_delegated_seat() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    let fake = root.path().join("codex");
    let log = root.path().join("argv.log");
    let refuse = root.path().join("refuse");
    let calls = root.path().join("calls.log");
    crate::test_exec::write_executable(
        &fake,
        &format!(
            "#!/bin/sh\necho \"CALL $*\" >> '{calls}'\ncase \"$*\" in\n*--probe-seat*) [ -e '{refuse}' ] && {{ echo 'seat_grant_valid=false cause=Revoked'; exit 1; }}; echo 'seat_grant_valid=true expected_seat=true'; exit 0;;\n--version) echo 'codex-cli 0.0.0'; exit 0;;\nesac\necho EXEC >> '{log}'\nfor a in \"$@\"; do echo \"$a\" >> '{log}'; done\necho coordinator-stdout\n",
            refuse = refuse.display(),
            log = log.display(),
            calls = calls.display(),
        ),
    );
    let _ambient = crate::test_env::AmbientGuard::hermetic_and(
        root.path(),
        None,
        &[
            ("AIDA_HEADLESS_VENDOR", Some("codex")),
            ("AIDA_AGENT_CMD", Some(fake.to_str().unwrap())),
            ("AIDA_OS_WRAP", Some("0")),
            ("AIDA_CONTAINED", None),
            // Ambient launcher context the child never assigned.
            ("AIDA_SESSION_SCOPE", Some("LAUNCHER-SCOPE")),
        ],
    );
    let grant = uuid::Uuid::new_v4().to_string();
    let build = || {
        crate::burndown_run::build_burndown_quiet_command(
            crate::session::HeadlessVendor::Codex,
            "test prompt",
            "bypassPermissions",
            &grant,
            "CEILING_KEY",
            "100",
            false,
        )
        .unwrap()
    };
    let out = root.path().join("stdout.log");
    let status = crate::session::coordinator_status_with_handle_guard(
        crate::session::HeadlessVendor::Codex,
        &mut build(),
        root.path(),
        Some(std::fs::File::create(&out).unwrap().into()),
        None,
    )
    .unwrap_or_else(|e| {
        let calls = std::fs::read_to_string(&calls).unwrap_or_default();
        panic!("{e:#}\n{calls}")
    });
    assert!(status.success());
    let argv = std::fs::read_to_string(&log).unwrap();
    for expected in [
        format!("shell_environment_policy.set.{GRANT_ENV}=\"{grant}\""),
        "shell_environment_policy.set.AIDA_SESSION_ROLE=\"implementer\"".to_string(),
        format!(
            "shell_environment_policy.set.AIDA_PROJECT_ROOT={}",
            toml::Value::String(root.path().display().to_string())
        ),
    ] {
        assert!(argv.lines().any(|l| l == expected), "{expected}\n{argv}");
    }
    assert!(!argv.contains("LAUNCHER-SCOPE"), "{argv}");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "coordinator-stdout\n",
        "caller stdout must survive finalization"
    );
    // Refusal: the probe fails, nothing execs, the caller gets the cause.
    std::fs::remove_file(&log).unwrap();
    std::fs::write(&refuse, "").unwrap();
    let error = crate::session::coordinator_status_with_handle_guard(
        crate::session::HeadlessVendor::Codex,
        &mut build(),
        root.path(),
        None,
        None,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Revoked"), "{error:#}");
    assert!(!log.exists(), "a refused coordinator child must never exec");
}

// Actual supported recorder output at the PR key, with an older refusal
// retained for chronology coverage. No attestation or verified-authority map
// is fabricated by this matrix.
// trace:BUG-2398 | ai:codex
fn assert_recorded_authority_boundaries(root: &Path, child_grant: &str, authorized: bool) {
    use crate::gate::{review_verdict as rv, ship};
    let body = rv::current_verdict(root, "TASK-1").unwrap().body;
    let v = rv::parse_recorded_verdict(&body).unwrap();
    let head = v.reviewed_sha.as_deref().unwrap();
    let pr_path = rv::verdict_path(root, "PR-2398");
    std::fs::create_dir_all(pr_path.parent().unwrap()).unwrap();
    // trace:TASK-2964 | ai:codex
    // Exercise the real positive-authority view with an older independent
    // refusal at this head. Its retained audit must not veto this approval.
    let mut pr_body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let earlier = chrono::DateTime::parse_from_rfc3339(v.recorded_at.as_deref().unwrap()).unwrap()
        - chrono::Duration::seconds(1);
    pr_body["rounds"] = serde_json::json!([{
        "verdict": "request-changes", "reviewed_sha": head,
        "recorded_by": "earlier-independent-reviewer", "recorded_at": earlier.to_rfc3339()
    }]);
    std::fs::write(&pr_path, pr_body.to_string()).unwrap();
    let mut req = aida_core::Requirement::new("fixture".into(), "desc".into());
    req.spec_id = Some("TASK-1".into());

    // Prose must not restore the same positive claim via an alternate source.
    req.comments.push(aida_core::Comment::new(
        "reviewer".into(),
        "Review: APPROVED".into(),
    ));
    assert_eq!(
        crate::gate::dispatch_authority::verdict_at_head(root, 2398, head),
        authorized.then_some(rv::VerdictKind::Approved)
    );
    assert_eq!(
        crate::human::spec_review_approved_at_head(
            root,
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ),
        authorized
    );
    assert_eq!(
        crate::human::pr_has_approved_verdict(root, 2398),
        authorized
    );
    let action = rv::review_actionability(root, "TASK-1", Some(&v), rv::TipRelation::AtReviewedSha);
    assert_eq!(action == rv::ReviewActionability::Resolved, authorized);
    let (_, display, _) =
        crate::review_verdict_show_body(root, "TASK-1", |_| rv::TipRelation::AtReviewedSha)
            .unwrap();
    assert_eq!(
        display["actionability"],
        if authorized {
            "resolved"
        } else {
            "needs-review"
        }
    );
    assert_eq!(
        crate::review_gates::reviewer_row_actionability(root, &req, &[], |_| None)
            == rv::ReviewActionability::Resolved,
        authorized
    );
    let policy = crate::gate::approval::ship_approval_policy(
        root,
        &["TASK-1".into()],
        Some("main"),
        std::slice::from_ref(&v),
    );
    assert_eq!(
        ship::approval_authority_refusal(std::slice::from_ref(&v), &policy).is_none(),
        authorized
    );
    assert_eq!(
        crate::gate::rework_hold_release::approval_attested(
            std::slice::from_ref(&v),
            &policy,
            head,
            2398
        )
        .is_ok(),
        authorized
    );
    // Collection additionally requires THIS launch's currently live grant.
    let mut checks = crate::queue_cmd::StandaloneVerdictChecks {
        project_root: root,
        pr: 2398,
        head_sha: Some(head.into()),
        launcher_grant: None,
        child_grant: Some(child_grant.into()),
    };
    let collected = rv::collect_standalone_verdict(
        root,
        root,
        &pr_path,
        &["TASK-1".into()],
        std::time::UNIX_EPOCH,
        &checks,
    );
    assert_eq!(collected.is_ok(), authorized, "{collected:?}");
    checks.child_grant = None;
    assert!(rv::collect_standalone_verdict(
        root,
        root,
        &pr_path,
        &["TASK-1".into()],
        std::time::UNIX_EPOCH,
        &checks
    )
    .is_err());
    assert!(crate::gate::dispatch_authority::verdict_at_head(
        root,
        2398,
        "ffffffffffffffffffffffffffffffffffffffff"
    )
    .is_none());
    assert_eq!(
        rv::review_actionability(root, "TASK-1", Some(&v), rv::TipRelation::Rewritten),
        rv::ReviewActionability::NeedsReview
    );
    if authorized {
        let mut closed = v.clone();
        closed.closed_by_merge = Some(head.into());
        assert_eq!(
            rv::review_actionability(root, "TASK-1", Some(&closed), rv::TipRelation::Unknown),
            rv::ReviewActionability::Resolved
        );
        // Persisted authorship must defeat approval labels even after the
        // author's lease is gone. The recorder's actual identity is reused.
        let authorship = crate::gate::review_authority::AuthorshipRecord {
            scope: "TASK-1".into(),
            lease_id: "old-author-lease".into(),
            branch: "main".into(),
            tokens: v.attestation.as_ref().unwrap().identity.clone(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            launch: None,
        };
        let ledger = root.join(".aida/review-authorship.jsonl");
        assert!(!ledger.exists());
        std::fs::write(
            &ledger,
            format!("{}\n", serde_json::to_string(&authorship).unwrap()),
        )
        .unwrap();
        assert!(crate::gate::dispatch_authority::verdict_at_head(root, 2398, head).is_none());
        assert!(!crate::human::spec_review_approved_at_head(
            root,
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ));
        let mut no_branch = v.clone();
        no_branch.reviewed_branch = None;
        assert_eq!(
            rv::review_actionability(
                root,
                "PR-2398",
                Some(&no_branch),
                rv::TipRelation::AtReviewedSha
            ),
            rv::ReviewActionability::NeedsReview
        );
        std::fs::remove_file(ledger).unwrap();
    }
    if !authorized {
        // Persisted compatibility artifacts and the closed-approval shortcut.
        let mut closed = v.clone();
        closed.closed_by_merge = Some(head.into());
        assert_eq!(
            rv::review_actionability(root, "TASK-1", Some(&closed), rv::TipRelation::Unknown),
            rv::ReviewActionability::NeedsReview
        );
        let alias_root = tempfile::tempdir().unwrap();
        req.agreed_id = Some("TASK-2".into());
        let alias = rv::verdict_path(alias_root.path(), "TASK-2");
        std::fs::create_dir_all(alias.parent().unwrap()).unwrap();
        std::fs::write(alias, &body).unwrap();
        assert!(!crate::human::spec_review_approved_at_head(
            alias_root.path(),
            &req,
            "TASK-1",
            2398,
            Some(head),
            Some("main")
        ));
    }
}

/// This command-level matrix runs in CI even when Codex is unavailable.
/// The real-tool fixture above adds the core-policy crossing to the same checks.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.acc739ef | ai:claude
#[test]
fn bug_2398_actual_recorder_artifacts_require_durable_independent_authority() {
    use crate::gate::{review_authority as ra, seat_authority as sa};
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[
            ("AIDA_AGENT_NAME", Some("codex-reviewer-fixture")),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_SESSION_ID", Some("independent-fixture-reviewer")),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_ROLE_INSTANCE", Some("driver")),
        ],
    );
    let head = crate::test_git::commit_fixture(root);
    let record = || {
        crate::handle_review_record_at(
            root.to_path_buf(),
            "TASK-1",
            "approved",
            Some(&head),
            Some("main"),
            Some("actual recorder output"),
            &[],
            &[],
            None,
        )
    };
    let identity = ra::IdentitySource {
        session_ids: vec!["independent-fixture-reviewer".into()],
        ..Default::default()
    };
    ra::with_identity_source(identity.clone(), record).unwrap();
    assert_recorded_authority_boundaries(root, "no-launched-reviewer", false);
    ambient.set_key(sa::GRANT_ENV, uuid::Uuid::new_v4().to_string());
    assert!(
        ra::with_identity_source(identity.clone(), record).is_err(),
        "presented invalid grant refuses recording"
    );
    let grant = sa::test_support::mint_grant_for(
        root,
        crate::test_env::HERMETIC_TEST_USER,
        "reviewer",
        &[],
    );
    ambient.set_key(sa::GRANT_ENV, &grant);
    ra::with_identity_source(identity, record).unwrap();
    assert_recorded_authority_boundaries(root, &grant, true);
    // Persisted legacy refusals carry no positive attestation, yet retain
    // their protective dispatch/actionability meaning.
    for refusal in ["request-changes", "rejected"] {
        crate::gate::review_verdict::record_verdict(
            root,
            "PR-2399",
            Some(refusal),
            Some(&head),
            Some("main"),
            Some("protective compatibility artifact"),
            &[],
            "ungranted-reviewer",
        )
        .unwrap();
        let v = crate::gate::review_verdict::read_recorded_verdict(root, "PR-2399").unwrap();
        assert!(v.attestation.is_none());
        assert_eq!(
            crate::gate::dispatch_authority::verdict_at_head(root, 2399, &head),
            Some(v.kind)
        );
        assert_eq!(
            crate::gate::review_verdict::review_actionability(
                root,
                "PR-2399",
                Some(&v),
                crate::gate::review_verdict::TipRelation::AtReviewedSha
            ),
            crate::gate::review_verdict::ReviewActionability::AwaitingRework
        );
    }
}

// Real installed Codex policy merging, including later custom overrides. The
// second grant is valid for the same seat: ordinary seat validity alone must
// not accept its different selector/subject as this intended child.
// trace:BUG-2398 | ai:codex
fn assert_effective_child_context(
    config: &Path,
    root: &Path,
    child: &Command,
    parent: &crate::gate::seat_authority::SeatGrant,
    native: bool,
) {
    let other = crate::gate::seat_authority::issue_child_from(
        parent.clone(),
        "implementer",
        "bug2398-other-child",
        true,
    )
    .unwrap();
    let mut other_child = Command::new("codex");
    for (key, value) in child.get_envs() {
        if let Some(value) = value {
            other_child.env(key, value);
        }
    }
    other_child
        .env(crate::gate::seat_authority::GRANT_ENV, &other.id)
        .env("AIDA_USER", "bug2398-other-child");
    let aida = crate::test_exec::matching_aida_cli();
    let out = run_codex_sandbox(
        config,
        root,
        &other_child,
        &[aida.to_str().unwrap(), "role", "current", "--check"],
        native,
    );
    assert!(
        out.status.success(),
        "the alternative same-seat child must itself be valid"
    );
    let invoke = |extra: &[String]| {
        let mut cmd = Command::new("codex");
        cmd.current_dir(root).env("CODEX_HOME", config);
        for (key, value) in child.get_envs() {
            match value {
                Some(v) => {
                    cmd.env(key, v);
                }
                None => {
                    cmd.env_remove(key);
                }
            }
        }
        cmd.arg("exec");
        if native {
            cmd.args(["--sandbox", "workspace-write"]);
        } else {
            cmd.args(["-c", "sandbox_mode=\"danger-full-access\""]);
        }
        cmd.args(extra).arg("unused model prompt");
        prepare_launch(&mut cmd)
    };
    let original = std::fs::read_to_string(config.join("config.toml")).unwrap();
    std::fs::write(config.join("fixture.config.toml"), "[shell_environment_policy]\ninherit = \"core\"\n[shell_environment_policy.set]\nAIDA_SESSION_SCOPE = \"TASK-profile\"\n").unwrap();
    for profile in [false, true] {
        let base = if profile {
            vec!["--profile".into(), "fixture".into()]
        } else {
            vec![]
        };
        invoke(&base).expect("profile-only control preserves exact prefix overrides");
        for assignment in [
            "shell_environment_policy.set.AIDA_SESSION_SCOPE=\"TASK-other\"".to_string(),
            "shell_environment_policy.set={AIDA_SESSION_SCOPE=\"TASK-table\"}".to_string(),
            "shell_environment_policy={inherit=\"core\",set={AIDA_SESSION_SCOPE=\"TASK-full\"}}"
                .to_string(),
            "shell_environment_policy.set.AIDA_ROLE_INSTANCE=\"driver\"".to_string(),
            "shell_environment_policy.set.AIDA_CONTAINMENT_POSTURE=\"different\"".to_string(),
            "shell_environment_policy.set.AIDA_SESSION_ROLE=\"reviewer\"".to_string(),
            format!(
                "shell_environment_policy.set.{}=\"{}\"",
                crate::gate::seat_authority::GRANT_ENV,
                other.id
            ),
            "shell_environment_policy.set.AIDA_USER=\"bug2398-other-child\"".to_string(),
        ] {
            let mut args = base.clone();
            args.extend(["-c".into(), assignment]);
            let err = invoke(&args).unwrap_err().to_string();
            assert!(err.contains("ChildContextMismatch"), "{err}");
            assert!(
                !err.contains(&other.id),
                "selector must not enter diagnostics"
            );
        }
        let mut args = base.clone();
        args.extend([
            "-c".into(),
            format!(
                "shell_environment_policy.set.{}=\"{}\"",
                crate::gate::seat_authority::GRANT_ENV,
                other.id
            ),
            "-c".into(),
            "shell_environment_policy.set.AIDA_USER=\"bug2398-other-child\"".into(),
        ]);
        assert!(invoke(&args)
            .unwrap_err()
            .to_string()
            .contains("ChildContextMismatch"));
        let mut args = base;
        args.extend([
            "-c".into(),
            "model_reasoning_effort=\"low\"".into(),
            "-c".into(),
            "shell_environment_policy.set.CUSTOM_OK=\"preserved\"".into(),
        ]);
        invoke(&args).expect("nonprotected custom configuration remains usable");
    }
    std::fs::write(config.join("config.toml"), original).unwrap();
}

#[test]
fn bug_2398_baseline_cannot_be_replaced_by_later_child_context() {
    for override_value in [
        "shell_environment_policy.set.AIDA_SESSION_GRANT=\"11111111-1111-1111-1111-111111111111\"",
        "shell_environment_policy.set={KEEP_ME=\"preserved\"}",
        "shell_environment_policy={inherit=\"all\",set={KEEP_ME=\"preserved\"}}",
    ] {
        let mut child = Command::new("codex");
        child
            .env(crate::gate::seat_authority::GRANT_ENV, "")
            .args(["exec", "-c", override_value]);
        assert!(prepare_launch(&mut child)
            .unwrap_err()
            .to_string()
            .contains("baseline context conflict"));
    }
    let mut child = Command::new("codex");
    child.env(crate::gate::seat_authority::GRANT_ENV, "").args([
        "exec",
        "-c",
        "shell_environment_policy.set.KEEP_ME=\"preserved\"",
    ]);
    prepare_launch(&mut child).unwrap();
}

/// Codex's DEFAULT `inherit = "all"` passes the launcher's own environment to
/// tool shells. A whitelisted key the launcher has but the child never
/// assigned must not reach the child (nor fail its context preflight).
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac7d6a3b | ai:claude
#[test]
fn bug_2398_real_codex_inherit_all_drops_unassigned_launcher_context() {
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    let codex = Command::new("codex").arg("--version").output();
    if !codex.is_ok_and(|out| out.status.success()) {
        eprintln!("SKIP: codex is not installed");
        return;
    }
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_USER", Some("bug2398-parent")),
        ("AIDA_SESSION_SCOPE", Some("PARENT-SCOPE")),
        (
            "AIDA_AGENT_CONTEXT_FILE",
            Some("/parent/launcher-context.md"),
        ),
        ("AIDA_ROLE_INSTANCE", Some("driver")),
    ]);
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let config = root.join("codex-config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("config.toml"),
        "[shell_environment_policy]\ninherit = \"all\"\n",
    )
    .unwrap();
    let parent = mint_grant_for(root, "bug2398-parent", "advisor", &["implementer"]);
    let store = crate::detect_distributed_store_from(root).unwrap();
    aida_core::git_ops::init(&store).unwrap();
    crate::test_git::commit_fixture(root);
    let parent = authority::read_grant(&parent).unwrap();
    let implementer =
        authority::issue_child_from(parent, "implementer", "bug2398-child", true).unwrap();
    // Control: the fixture really inherits the launcher's ambient value.
    let mut raw = Command::new("codex");
    raw.current_dir(root)
        .env("CODEX_HOME", &config)
        .args(["sandbox", "-c", "sandbox_mode=\"danger-full-access\"", "--"])
        .args(["sh", "-c", "test \"$AIDA_SESSION_SCOPE\" = PARENT-SCOPE"]);
    assert!(
        output(&mut raw).status.success(),
        "inherit=all fixture must expose ambient launcher keys"
    );
    let mut child = Command::new("codex");
    child
        .env("AIDA_SESSION_GRANT", &implementer.id)
        .env("AIDA_SESSION_ROLE", "implementer")
        .env("AIDA_USER", "bug2398-child")
        .env("AIDA_PROJECT_ROOT", root)
        // trace:TASK-2708 | ai:claude — an `aida` stamp is refused by name at
        // this finalizer; the whitelisted key still carries a public value.
        .env("AIDA_CONTAINMENT_POSTURE", "native")
        .env("AIDA_AGENT_TYPE", "codex");
    let aida_path = crate::test_exec::matching_aida_cli();
    let _aida_probe = crate::kernel::test_aida_exe::set(aida_path.clone());
    let aida = aida_path.to_string_lossy().into_owned();
    // run_codex_sandbox unwraps prepare_launch: the preflight must pass.
    let out = run_codex_sandbox(
        &config,
        root,
        &child,
        &[&aida, "role", "current", "--check"],
        false,
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && text.lines().any(|l| l == "implementer"),
        "inherit=all launch with ambient launcher context: {text}"
    );
    let out = run_codex_sandbox(
        &config,
        root,
        &child,
        &[
            "sh",
            "-c",
            "test -z \"${AIDA_SESSION_SCOPE+x}\" && test -z \"${AIDA_AGENT_CONTEXT_FILE+x}\" && test -z \"${AIDA_ROLE_INSTANCE+x}\" && test \"$AIDA_USER\" = bug2398-child",
        ],
        false,
    );
    assert!(
        out.status.success(),
        "unassigned launcher keys must be absent: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// The Codex advisor-tier consult mints its own delegated `advisor` child,
/// revokes it after exit, and refuses before launch when the launcher's
/// grant does not delegate `advisor`.
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac022c22 | ai:claude
// trace:BUG-b6b1278890774ef993d2ebc327862470-172.ac51a4f1 | ai:claude
#[test]
fn bug_2398_codex_advisor_consult_delegates_own_child_and_refuses_without_delegation() {
    use crate::gate::seat_authority::{self as authority, test_support::mint_grant_for};
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(home.join(".aida")).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    // trace:TASK-2708 | ai:claude — a real project (a resolvable default
    // branch): the finalizer refuses a root whose trusted sha cannot resolve.
    crate::test_git::commit_fixture(&project);
    let capture = tmp.path().join("advisor-grant.txt");
    let fake = tmp.path().join("fake-codex");
    crate::test_exec::write_executable(
        &fake,
        crate::test_exec::codex_probe_script(format!(
            "#!/bin/sh\nprintf '%s' \"$AIDA_SESSION_GRANT\" > '{}'\nexit 0\n",
            capture.display()
        )),
    );
    let fake = fake.to_string_lossy().into_owned();
    let mut env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_HOME", Some(home.to_str().unwrap())),
        ("HOME", Some(home.to_str().unwrap())),
        ("AIDA_HEADLESS_VENDOR", Some("codex")),
        ("AIDA_AGENT_CMD", Some(&fake)),
        ("AIDA_OS_WRAP", Some("0")),
        ("AIDA_TEE_HEADLESS", Some("0")),
        ("AIDA_CONTAINED", None),
        (authority::GRANT_ENV, None),
    ]);
    let _aida_probe = crate::kernel::test_aida_exe::set(crate::test_exec::matching_aida_cli());
    crate::session::set_headless_vendor_override(None);
    let driver = crate::phase_driver::RealPhaseDriver::new(
        project.clone(),
        "BUG-2398".to_string(),
        "test-queue".to_string(),
        None,
        true,
        None,
        crate::AutonomyMode::Default,
        "run-token".to_string(),
        false,
        false,
        false,
        false,
        crate::auto_complete::LifecycleSkip::default(),
        crate::auto_complete::AutoCompleteVariant::Full,
    );
    let consult = || {
        driver.spawn_advisor_session(
            crate::advisor_cmd_root::AdvisorPass::ColdBoot,
            &crate::advisor::AdvisorConfig::default(),
            &tmp.path().join("response.json"),
            &tmp.path().join("request.json"),
        )
    };
    let user = crate::current_user_id(None);

    // Refusal: the launcher grant does not delegate `advisor`.
    let narrow = mint_grant_for(&project, &user, "advisor", &["implementer"]);
    env.set_key(authority::GRANT_ENV, &narrow);
    let failure = consult().expect_err("undelegated advisor consult must refuse");
    assert!(
        failure.reason.contains("advisor delegation")
            && failure.reason.contains("does not delegate `advisor`")
            && failure.reason.contains("--delegate-seat advisor"),
        "{}",
        failure.reason
    );
    assert!(!capture.exists(), "refusal must precede any model launch");

    // Success: a fresh advisor child, never the launcher's selector, revoked
    // once the consult exits.
    let launcher = mint_grant_for(&project, &user, "advisor", &["implementer", "advisor"]);
    env.set_key(authority::GRANT_ENV, &launcher);
    let failure = consult().expect_err("the fake advisor writes no response");
    assert!(
        failure.reason.contains("no usable response"),
        "the consult must fail only for the missing response: {}",
        failure.reason
    );
    let child = std::fs::read_to_string(&capture).unwrap();
    assert!(!child.is_empty() && child != launcher && child != narrow);
    let record = authority::read_grant(&child).unwrap();
    assert_eq!(record.seat, "advisor");
    assert_eq!(record.parent_grant_id.as_deref(), Some(launcher.as_str()));
    assert!(
        record.revoked_at.is_some(),
        "consult child must be revoked after exit"
    );
    crate::session::set_headless_vendor_override(None);
}

/// BUG-2403 (signoff amendment 8): BUG-2398's recorder-artifact floor holds
/// through the REMAPPED selectors too. An older friendly-label refusal and a
/// newer native-label approval of the same requirement: the cross-label
/// selection answers with the native approval, and that approval earns
/// positive actionability only when it carries durable independent grant
/// authority — the absent-grant artifact stays recordable but never
/// resolves; an invalid presented grant still refuses recording; the
/// protective refusal is never lost to an unselectable read.
// trace:BUG-2403 | ai:claude
#[test]
fn bug_2403_bug_2398_recorder_artifacts_hold_through_remapped_selectors() {
    use crate::gate::{review_authority as ra, review_verdict as rv, seat_authority as sa};
    const FRIENDLY: &str = "TASK-24039";
    const NATIVE: &str = "TASK-B6B1278890774EF993D2EBC327862470-399";
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut ambient = crate::test_env::AmbientGuard::hermetic_and(
        root,
        None,
        &[
            ("AIDA_AGENT_NAME", Some("codex-reviewer-fixture")),
            ("AIDA_AGENT_TYPE", Some("codex")),
            ("AIDA_SESSION_ID", Some("independent-fixture-reviewer")),
            ("AIDA_REVIEW_VERDICT_FILE", None),
            ("AIDA_ROLE_INSTANCE", Some("driver")),
            ("AIDA_CONTAINED", None),
        ],
    );
    let head = crate::test_git::commit_fixture(root);
    rv::record_verdict(
        root,
        FRIENDLY,
        Some("request-changes"),
        Some(&head),
        Some("main"),
        Some("older friendly refusal"),
        &["obsolete finding".to_string()],
        "ungranted-reviewer",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    let record = || {
        crate::handle_review_record_at(
            root.to_path_buf(),
            NATIVE,
            "approved",
            Some(&head),
            Some("main"),
            Some("actual recorder output"),
            &[],
            &[],
            None,
        )
    };
    let identity = ra::IdentitySource {
        session_ids: vec!["independent-fixture-reviewer".into()],
        ..Default::default()
    };
    let check = |authorized: bool| {
        for ids in [[FRIENDLY, NATIVE], [NATIVE, FRIENDLY]] {
            let v = rv::read_recorded_verdict_any(root, &ids).expect("a selection");
            assert_eq!(v.kind, rv::VerdictKind::Approved, "{ids:?}");
            let action =
                rv::review_actionability(root, NATIVE, Some(&v), rv::TipRelation::AtReviewedSha);
            assert_eq!(
                action == rv::ReviewActionability::Resolved,
                authorized,
                "{ids:?}: {action:?}"
            );
        }
        assert_eq!(
            crate::queue_cmd::rework_findings_block_for_spec(root, FRIENDLY, NATIVE),
            None,
            "the obsolete friendly refusal's findings are not replayed"
        );
    };
    // The absent-grant named-agent artifact: recordable, never positive.
    ra::with_identity_source(identity.clone(), record).unwrap();
    check(false);
    // An invalid presented grant refuses recording.
    ambient.set_key(sa::GRANT_ENV, uuid::Uuid::new_v4().to_string());
    assert!(ra::with_identity_source(identity.clone(), record).is_err());
    // A durable independent grant: positive through either label order.
    let grant = sa::test_support::mint_grant_for(
        root,
        crate::test_env::HERMETIC_TEST_USER,
        "reviewer",
        &[],
    );
    ambient.set_key(sa::GRANT_ENV, &grant);
    std::thread::sleep(std::time::Duration::from_millis(15));
    ra::with_identity_source(identity, record).unwrap();
    check(true);
}

/// How a production file that assigns `AIDA_SESSION_ROLE` to a child Command
/// gets that role to a Codex child's hooks and tool shells.
// trace:BUG-2395 | ai:claude
#[derive(Debug)]
enum RoleRoute {
    /// The file is in `LAUNCH_INVENTORY`, whose ratchet pins its Codex
    /// finalization (prepare_launch or the coordinator spawn).
    LaunchInventory,
    /// The child is another `aida` process whose own Codex spawn finalizes.
    AidaChild(&'static str),
    /// A Codex launch that does not finalize yet; the reason names the
    /// follow-up. Moving an entry out of here is the fix, not editing it.
    KnownGap(&'static str),
    /// `launch::assign_seat`, the one seat writer: it launches nothing
    /// itself, and every caller is counted at its own site.
    // trace:STORY-2326 | ai:claude
    SeatWriter,
}

/// The ROLE_LAUNCH_SITES count of the production source `src`: each child
/// role write (`.env(` keyed by the literal or `ROLE_ENV`) plus each call of
/// the launcher, which writes the role behind the caller's back.
// trace:BUG-2395 STORY-2326 | ai:claude
fn role_sites(src: &str) -> usize {
    let normalized: String = src.split_whitespace().collect();
    let writes = normalized
        .split(".env(")
        .skip(1)
        .filter(|call| {
            let key = call.split([',', ')']).next().unwrap_or_default();
            let key = key.trim_start_matches('&');
            key == "\"AIDA_SESSION_ROLE\"" || key == "ROLE_ENV" || key.ends_with("::ROLE_ENV")
        })
        .count();
    writes + launcher_calls(src)
}

/// Whether the role ratchet would flag `src` as an unlisted role-launch site
/// if it were the production file `rel`.
fn role_flagged_as_unlisted(rel: &str, src: &str) -> bool {
    role_sites(src) > 0 && !ROLE_LAUNCH_SITES.iter().any(|(f, ..)| *f == rel)
}

/// BUG-2395 (verify-only, advisor ruling): every production file that hands a
/// child its lane role, pinned by exact count. A new role-assigning site fails
/// here until it is audited for Codex finalization and recorded.
// trace:BUG-2395 | ai:claude
const ROLE_LAUNCH_SITES: &[(&str, usize, RoleRoute)] = &[
    // STORY-2254: the detached Codex worker finalizes via prepare_launch.
    ("agent_codex_background.rs", 1, RoleRoute::LaunchInventory),
    // STORY-2326: the fg/bg lane seats, through 2 spawn_agent and 3
    // assign_seat calls. trace:STORY-2326 | ai:claude
    ("agent_launch.rs", 5, RoleRoute::LaunchInventory),
    ("autopilot_intake.rs", 1, RoleRoute::LaunchInventory),
    // PR-B launcher calls are seat routes, even without a literal role write.
    // trace:STORY-2326 | ai:codex
    ("compete_cmd.rs", 1, RoleRoute::LaunchInventory),
    ("phase_driver_trait.rs", 1, RoleRoute::LaunchInventory),
    // trace:BUG-2390 | ai:claude
    (
        "dispatch.rs",
        2,
        RoleRoute::AidaChild(
            "the role site is the Claude/Agy implementer builder; Codex take/impl run \
             `aida agent new codex --bg`, which finalizes in agent_codex_background.rs",
        ),
    ),
    // The two `.env(ROLE_ENV, ..)` writes of assign_seat (Child, Lane).
    ("launch/spawn.rs", 2, RoleRoute::SeatWriter),
    // Shared advisor builder only; callers finalize via coordinator spawn.
    ("launch/grant.rs", 1, RoleRoute::SeatWriter),
    ("phase_driver.rs", 2, RoleRoute::LaunchInventory),
    ("questions.rs", 1, RoleRoute::LaunchInventory),
    (
        "queue_cmd.rs",
        4,
        RoleRoute::AidaChild("Claude exec seats; Codex finalizes in session.rs"),
    ),
    // BUG-2386 adds two: `finalize_codex_resume` (interactive resume, seat
    // from the delegated grant) and `spawn_vendor_headless_resume_granted`
    // (punt/bg resume); both finalize through `prepare_launch`.
    // trace:BUG-2386 | ai:claude
    // STORY-2326: the headless forking and reviewer writes moved into
    // launch::assign_seat; 6 literal writes plus 16 launcher calls (8
    // spawn_agent, 8 assign_seat). trace:STORY-2326 | ai:codex
    ("session.rs", 25, RoleRoute::LaunchInventory),
    ("zen_drive_root.rs", 2, RoleRoute::LaunchInventory),
];

// trace:BUG-2395 | ai:claude
#[test]
fn bug_2395_every_role_launch_site_is_inventoried() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    production_sources(&src, &mut files);
    let mut found = std::collections::BTreeMap::new();
    for path in files {
        let n = role_sites(&std::fs::read_to_string(&path).unwrap());
        if n > 0 {
            let rel = path
                .strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            found.insert(rel, n);
        }
    }
    for (rel, n) in &found {
        let Some((_, expected, _)) = ROLE_LAUNCH_SITES.iter().find(|(f, ..)| f == rel) else {
            panic!(
                "{rel} assigns a child AIDA_SESSION_ROLE but is not in ROLE_LAUNCH_SITES: a \
                 Codex child must finalize through codex_tool_env::prepare_launch; audit and record it"
            );
        };
        assert_eq!(
            n, expected,
            "{rel}: role-launch site count changed; re-audit it"
        );
    }
    for (rel, _, route) in ROLE_LAUNCH_SITES {
        assert!(
            found.contains_key(*rel),
            "stale ROLE_LAUNCH_SITES entry {rel}"
        );
        match route {
            RoleRoute::LaunchInventory => assert!(
                LAUNCH_INVENTORY.iter().any(|(f, _, how)| f == rel
                    && matches!(
                        how,
                        Finalization::PrepareLaunch(_) | Finalization::CoordinatorSpawn
                    )),
                "{rel} claims LAUNCH_INVENTORY finalization but is not finalized there"
            ),
            RoleRoute::AidaChild(why) | RoleRoute::KnownGap(why) => assert!(!why.is_empty()),
            // The seat writer's callers are the launch sites: at least one
            // other inventoried file must call the launcher.
            RoleRoute::SeatWriter => assert!(
                ROLE_LAUNCH_SITES.iter().any(|(f, _, r)| f != rel
                    && matches!(r, RoleRoute::LaunchInventory)
                    && LAUNCH_INVENTORY.iter().any(|(l, ..)| l == f)),
                "{rel}: the seat writer's callers are not inventoried"
            ),
        }
    }
}

/// BUG-2395 (verify-only): hooks inherit the Codex process env, so a hook run
/// under a prepared Codex launch env sees the seat role, and the tool-shell
/// transport carries the same role.
// trace:BUG-2395 | ai:claude
#[cfg(target_os = "linux")]
#[test]
fn bug_2395_hook_under_prepared_codex_launch_sees_seat_role() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(project.join(".aida/roles")).unwrap();
    std::fs::write(
        project.join(".aida/roles/implementer.toml"),
        "purpose = \"Lane fixture purpose\"\n",
    )
    .unwrap();
    // trace:TASK-2708 | ai:claude — a real project: the finalizer refuses a
    // root whose trusted default branch cannot be resolved.
    crate::test_git::commit_fixture(&project);

    let mut cmd = Command::new("codex");
    cmd.args(["exec", "--", "prompt"])
        .current_dir(&project)
        .env("AIDA_SESSION_GRANT", "")
        .env("AIDA_SESSION_ROLE", "implementer");
    prepare_launch(&mut cmd).unwrap();
    assert!(
        cmd.get_args().any(|a| a.to_string_lossy()
            == "shell_environment_policy.set.AIDA_SESSION_ROLE=\"implementer\""),
        "tool shells get the seat role: {:?}",
        cmd.get_args().collect::<Vec<_>>()
    );

    let hook = tmp.path().join("aida-role-context.sh");
    std::fs::write(
        &hook,
        aida_core::templates::EMBEDDED_TEMPLATES
            .get("hooks/aida-role-context.sh")
            .unwrap(),
    )
    .unwrap();
    let mut run = Command::new("bash");
    run.arg(&hook)
        .current_dir(&project)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tmp.path())
        .env("CODEX_PROJECT_DIR", &project)
        .stdin(std::process::Stdio::null());
    for (k, v) in cmd.get_envs() {
        if let Some(v) = v {
            run.env(k, v);
        }
    }
    let out = run.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("AIDA active role: implementer") && stdout.contains("Lane fixture purpose"),
        "hook did not see the prepared seat role: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// BUG-2416: the guided `aida do` Codex launch makes an explicit baseline
/// grant decision and takes its argv and env from `prepare_launch`; the
/// Claude launch never enters the Codex adapter.
// trace:BUG-2416 | ai:claude
#[test]
fn bug_2416_guided_codex_launch_is_finalized_by_prepare_launch() {
    let tmp = tempfile::TempDir::new().unwrap();
    // trace:TASK-2708 | ai:claude — a real project: the finalizer refuses a
    // root whose trusted default branch cannot be resolved.
    crate::test_git::commit_fixture(tmp.path());
    let launch = crate::session::GuidedSessionLaunch {
        program: "codex",
        prompt: "guided prompt".into(),
    };
    let cmd = crate::zen_drive_root::guided_launch_command(&launch, tmp.path()).unwrap();

    let mut expected = Command::new("codex");
    expected
        .arg("guided prompt")
        .current_dir(tmp.path())
        .env(GRANT_ENV, "")
        .env(crate::gate::seat_authority::GRANT_FILE_ENV, "")
        .env("AIDA_SESSION_ROLE", "implementer")
        .env_remove("AIDA_CONTAINED")
        .env("AIDA_CONTAINMENT_POSTURE", "none")
        .env_remove("AIDA_SESSION_PURPOSE")
        .env_remove("AIDA_STANDALONE_IMPLEMENTER")
        .env_remove("AIDA_HEADLESS_LANE");
    prepare_launch(&mut expected).unwrap();
    let args = |c: &Command| c.get_args().map(|a| a.to_owned()).collect::<Vec<_>>();
    let envs = |c: &Command| {
        c.get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned())))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(
        args(&cmd),
        args(&expected),
        "argv comes from prepare_launch"
    );
    assert_eq!(envs(&cmd), envs(&expected), "env comes from prepare_launch");
    assert!(
        cmd.get_args().any(|a| a.to_string_lossy()
            == "shell_environment_policy.set.AIDA_SESSION_ROLE=\"implementer\""),
        "tool shells get the implementer seat: {:?}",
        args(&cmd)
    );
    assert_eq!(
        cmd.get_envs()
            .find(|(k, _)| *k == GRANT_ENV)
            .map(|(_, v)| v),
        Some(Some(std::ffi::OsStr::new(""))),
        "explicit empty baseline, never the launcher's grant"
    );
    assert_eq!(args(&cmd).last().unwrap(), "guided prompt");

    let claude = crate::session::GuidedSessionLaunch {
        program: "claude",
        prompt: "/aida-guided-implement TASK-9".into(),
    };
    let cmd = crate::zen_drive_root::guided_launch_command(&claude, tmp.path()).unwrap();
    assert_eq!(
        args(&cmd),
        vec![std::ffi::OsString::from("/aida-guided-implement TASK-9")]
    );
}

// trace:BUG-2440 | ai:codex
#[test]
fn bug_2440_wrapped_codex_prepares_with_explicit_command_separator() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join(".aida")).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // Stub only the host namespace availability probe: no model or real
    // sandbox launch is needed to verify argv construction and adaptation.
    crate::test_exec::write_executable(&bin.join("bwrap"), "#!/bin/sh\nexit 0\n");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    for (os_wrap, config) in [
        (Some("1"), "[contained]\nos_wrap = false\n"),
        (None, "[contained]\nos_wrap = true\n"),
    ] {
        std::fs::write(root.join(".aida/config.toml"), config).unwrap();
        let _ambient = crate::test_env::AmbientGuard::hermetic_and(
            &root,
            Some("advisor"),
            &[("PATH", Some(path.as_str())), ("AIDA_OS_WRAP", os_wrap)],
        );
        for vendor in ["codex", "claude", "agy"] {
            let executable = format!("/opt/vendor/{vendor}");
            let args = vec!["exec".into(), "--".into(), "prompt mentioning codex".into()];
            let (program, wrapped) =
                crate::session::os_wrapped_program_and_args(None, &root, &executable, args.clone())
                    .unwrap();
            assert_eq!(program, "bwrap");
            let separator = wrapped.iter().position(|arg| arg == "--").unwrap();
            assert_eq!(wrapped[separator + 1], executable);
            assert_eq!(&wrapped[separator + 2..], &args);
            assert!(wrapped[..separator]
                .iter()
                .any(|arg| arg == "--die-with-parent"));
            if vendor == "codex" {
                let mut child = std::process::Command::new(&program);
                child
                    .args(&wrapped)
                    .current_dir(&root)
                    // The launcher stamps the posture it resolved. This
                    // fixture root is no git repo, so without the stamp the
                    // TASK-2708 fail-closed check would (rightly) refuse.
                    // trace:TASK-2708 | ai:claude
                    .env("AIDA_CONTAINMENT_POSTURE", "native")
                    .env("AIDA_SESSION_GRANT", "")
                    .env("AIDA_USER", "wrapped-child");
                prepare_launch(&mut child).unwrap();
                let prepared: Vec<_> = child
                    .get_args()
                    .map(|arg| arg.to_str().unwrap().to_string())
                    .collect();
                assert_eq!(&prepared[..separator + 2], &wrapped[..separator + 2]);
                assert_eq!(&prepared[prepared.len() - args.len()..], &args);
                assert!(prepared[separator + 2..].windows(2).any(|pair| {
                    pair[0] == "-c"
                        && pair[1] == "shell_environment_policy.set.AIDA_USER=\"wrapped-child\""
                }));
                assert_eq!(child.get_current_dir(), Some(root.as_path()));
            }
        }
    }
}
