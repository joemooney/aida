//! STORY-2254: `aida agent new codex --bg` — parsing, early refusals,
//! posture, argv/env construction, exec mapping, helper channel and the
//! strict lease/registry seams. No model, no real Codex, synthetic roots.
// trace:STORY-2254 | ai:claude

use super::*;

fn parse(args: &[&str]) -> std::result::Result<AgentCommand, clap::Error> {
    crate::TEST_CLI.try_parse_from(args).map(|cli| {
        let AgentArgs { inner } = cli.command.registered::<AgentArgs>();
        inner
    })
}

fn codex_bg(args: &[&str]) -> Option<(Option<String>, Option<String>, bool)> {
    let mut argv = vec!["aida", "agent", "new", "codex"];
    argv.extend(args);
    match parse(&argv).ok()? {
        AgentCommand::New {
            command:
                Some(AgentNewCommand::Codex {
                    spec,
                    spec_positional,
                    bg,
                    ..
                }),
            ..
        } => Some((spec, spec_positional, bg)),
        _ => None,
    }
}

fn request(spec: Option<&str>, role: Option<&str>) -> CodexBgRequest {
    CodexBgRequest {
        role: role.map(str::to_string),
        spec: spec.map(str::to_string),
        force: false,
        cwd: None,
        bypass_sandbox: false,
        context: AgentContextOptions::new(true, false),
        noexec: false,
        show_prompt: false,
        verbose: false,
        prompt: AgentPromptOptions::new(None, false),
        resume: AgentResumeOptions::new(false, None, false, true),
        flag_options: AgentDefaultFlagOptions::new(true, Vec::new(), None),
        name: None,
        description: None,
        no_mail_wake: false,
        branch: None,
        base: None,
    }
}

/// BUG-2390: dispatch take/impl hand the lane branch to `--bg` through
/// `--branch`/`--base`; both are `--bg`-only.
// trace:BUG-2390 | ai:claude
#[test]
fn bug_2390_bg_accepts_branch_and_base() {
    let branch_base = |args: &[&str]| {
        let mut argv = vec!["aida", "agent", "new", "codex"];
        argv.extend(args);
        match parse(&argv).ok()? {
            AgentCommand::New {
                command:
                    Some(AgentNewCommand::Codex {
                        branch, base, bg, ..
                    }),
                ..
            } => Some((branch, base, bg)),
            _ => None,
        }
    };
    assert_eq!(
        branch_base(&[
            "--role",
            "implementer",
            "--bg",
            "TASK-12",
            "--branch",
            "feat/x",
            "--base",
            "origin/main"
        ]),
        Some((Some("feat/x".into()), Some("origin/main".into()), true))
    );
    assert_eq!(branch_base(&["--bg", "TASK-12"]), Some((None, None, true)));
    // Without --bg the lane flags refuse at parse time.
    assert!(branch_base(&["--spec", "TASK-12", "--branch", "feat/x"]).is_none());
    assert!(branch_base(&["--spec", "TASK-12", "--base", "origin/main"]).is_none());
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_positional_and_flag_spec() {
    assert_eq!(
        codex_bg(&["--role", "implementer", "--bg", "TASK-12"]),
        Some((None, Some("TASK-12".into()), true))
    );
    assert_eq!(
        codex_bg(&["--bg", "--spec", "TASK-12"]),
        Some((Some("TASK-12".into()), None, true))
    );
    // The positional form exists only for --bg.
    assert!(codex_bg(&["TASK-12"]).is_none());
    // Unsupported combinations refuse at parse time, before any setup.
    for extra in [
        &["--rotate"][..],
        &["--resume", "latest"],
        &["--no-prompt"],
        &["--bypass-sandbox"],
    ] {
        let mut args = vec!["--bg", "TASK-12"];
        args.extend(extra);
        assert!(
            codex_bg(&args).is_none(),
            "{extra:?} must conflict with --bg"
        );
    }
    // Foreground parsing is unchanged.
    assert_eq!(
        codex_bg(&["--spec", "TASK-12"]),
        Some((Some("TASK-12".into()), None, false))
    );
    // Attempt commands.
    let id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        parse(&["aida", "agent", "launch-status", &id]).unwrap(),
        AgentCommand::LaunchStatus { .. }
    ));
    assert!(matches!(
        parse(&["aida", "agent", "recover-launch", &id]).unwrap(),
        AgentCommand::RecoverLaunch { .. }
    ));
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_conflicting_spec_forms_refuse() {
    let cli = crate::TEST_CLI
        .try_parse_from([
            "aida", "agent", "new", "codex", "--bg", "--spec", "TASK-1", "TASK-2",
        ])
        .unwrap();
    let AgentArgs { inner } = cli.command.registered::<AgentArgs>();
    let err = handle_agent_command(&inner).unwrap_err();
    assert!(format!("{err:#}").contains("conflicting specs"), "{err:#}");
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_rejects_unsupported_requests_before_setup() {
    assert!(validate_request(&request(Some("TASK-1"), Some("implementer"))).is_ok());
    assert!(validate_request(&request(Some("TASK-1"), Some("reviewer"))).is_ok());
    assert!(validate_request(&request(Some("TASK-1"), None)).is_ok());
    let err = validate_request(&request(None, Some("implementer"))).unwrap_err();
    assert!(format!("{err}").contains("needs a spec"));
    let err = validate_request(&request(Some("TASK-1"), Some("advisor"))).unwrap_err();
    assert!(format!("{err}").contains("only an implementer or a reviewer"));
    let mut req = request(Some("TASK-1"), None);
    req.prompt = AgentPromptOptions::new(None, true);
    assert!(validate_request(&req).is_err(), "no prompt");
    let mut req = request(Some("TASK-1"), None);
    req.flag_options = req.flag_options.rotating(true);
    assert!(validate_request(&req).is_err(), "rotation");
    let mut req = request(Some("TASK-1"), None);
    req.resume = AgentResumeOptions::new(false, Some("latest".into()), false, true);
    assert!(validate_request(&req).is_err(), "resume");
    let mut req = request(Some("TASK-1"), None);
    req.bypass_sandbox = true;
    assert!(validate_request(&req).is_err(), "bypass");
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.acf38ac0 | ai:claude
#[test]
// trace:TASK-3736 | ai:codex
fn task_3736_story_2254_bg_permission_override_refused() {
    let native: Vec<String> = [
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
    ]
    .map(String::from)
    .to_vec();
    assert!(refuse_posture_overrides(&native).is_ok());
    for bad in [
        vec!["--dangerously-bypass-approvals-and-sandbox"],
        // trace:TASK-3736 | ai:codex
        vec!["--profile", "other"],
        vec!["--profile=other"],
        vec!["-p", "other"],
        vec!["-pother"],
        vec!["-p=other"],
        vec!["-c", "profile=other"],
        vec!["--config", "profiles.other.sandbox_mode=read-only"],
        vec!["--yolo"],
        vec!["--full-auto"],
        vec!["--sandbox", "danger-full-access"],
        vec!["-s", "read-only"],
        vec!["--ask-for-approval", "on-request"],
        vec!["-c", "sandbox_mode=\"danger-full-access\""],
        vec!["--config", "shell_environment_policy.inherit=\"all\""],
        vec!["-capproval_policy=\"never\""],
        vec!["--sandbox=workspace-write"],
    ] {
        let mut args = native.clone();
        args.extend(bad.iter().map(|s| s.to_string()));
        assert!(refuse_posture_overrides(&args).is_err(), "{bad:?}");
    }
    // A model or feature flag is not a posture override.
    let mut args = native.clone();
    args.extend([
        "--model".into(),
        "gpt-x".into(),
        "-c".into(),
        "model_reasoning_effort=high".into(),
    ]);
    assert!(refuse_posture_overrides(&args).is_ok());
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.acf38ac0 | ai:claude
#[test]
fn story_2254_bg_native_posture_requires_trusted_opt_in() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    let config = AgentLaunchConfig {
        delegate_seats: Vec::new(),
        agent_type: "codex",
        binary: "codex",
        default_args: [
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
        ]
        .map(String::from)
        .to_vec(),
        prompt_style: AgentPromptStyle::Positional,
        contained: false,
    };
    // --no-default-flags: refused.
    assert!(resolve_native_posture(&config, tmp.path(), false).is_err());
    // [agents] contained off (no config): refused, never silently `none`,
    // even though the argv itself looks native.
    let err = resolve_native_posture(&config, tmp.path(), true).unwrap_err();
    assert!(format!("{err:#}").contains("contained"), "{err:#}");
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac9b0efd | ai:claude
#[test]
fn story_2254_bg_exec_argv_shape() {
    let defaults: Vec<String> = [
        CODEX_NO_DAEMON_FLAG,
        "--sandbox",
        "workspace-write",
        "--ask-for-approval",
        "never",
        "--model",
        "gpt-x",
    ]
    .map(String::from)
    .to_vec();
    let args = exec_args(&defaults, &["--looks like a flag".into()]);
    assert_eq!(
        args,
        [
            "--sandbox",
            "workspace-write",
            "--ask-for-approval",
            "never",
            "exec",
            "--json",
            "--model",
            "gpt-x",
            "--",
            "--looks like a flag"
        ]
    );
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac9b0efd | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_shim_exec_mapping_proven_or_refused() {
    let tmp = tempfile::tempdir().unwrap();
    // A native executable maps to itself.
    let sh = std::path::Path::new("/bin/sh").canonicalize().unwrap();
    assert_eq!(
        resolve_expected_exec(&sh).unwrap(),
        ExpectedExec::Native { path: sh.clone() }
    );
    assert!(exec_matches(
        &ExpectedExec::Native { path: sh.clone() },
        &sh
    ));
    assert!(!exec_matches(
        &ExpectedExec::Native { path: sh },
        std::path::Path::new("/bin/true")
    ));
    // A non-node script, or a node shim that would spawn a child instead of
    // replacing itself, has no provable worker mapping.
    let script = tmp.path().join("codex");
    crate::test_exec::write_executable(&script, "#!/bin/sh\nexec real-codex \"$@\"\n");
    assert!(resolve_expected_exec(&script).is_err());
    crate::test_exec::write_executable(
        &script,
        "#!/usr/bin/env node\nrequire('child_process').spawn('codex-native')\n",
    );
    let err = resolve_expected_exec(&script).unwrap_err();
    assert!(format!("{err:#}").contains("process.execve"), "{err:#}");
    // The execve shim maps only to a `codex` executable that is neither the
    // interpreter nor aida.
    let node = std::path::PathBuf::from("/usr/bin/node");
    let shim = ExpectedExec::ExecveShim {
        interpreter: node.clone(),
    };
    assert!(exec_matches(
        &shim,
        std::path::Path::new("/opt/vendor/bin/codex")
    ));
    assert!(!exec_matches(&shim, &node));
    assert!(!exec_matches(
        &shim,
        std::path::Path::new("/opt/vendor/bin/node")
    ));
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
#[test]
fn story_2254_bg_parent_env_and_bearer_names_not_forwarded() {
    let parent = [
        ("HOME", "/h"),
        ("PATH", "/bin"),
        ("LC_ALL", "C"),
        ("AIDA_SESSION_GRANT", "parent-grant"),
        ("AIDA_SESSION_GRANT_FILE", "/x"),
        ("AIDA_HANDLE_LAUNCH_TOKEN", "t"),
        ("AIDA_AGENT_REGISTRY_TOKEN", "t"),
        ("AIDA_CONTAINED", "1"),
        ("AIDA_SESSION_ROLE", "advisor"),
        ("CODEX_THREAD_ID", "parent-thread"),
        ("CLAUDE_CODE_SESSION_ID", "s"),
        ("GITHUB_TOKEN", "secret"),
        ("AIDA_HOME", "/ah"),
    ]
    .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v)));
    let kept: Vec<String> =
        ambient_env(parent.clone(), WORKER_AMBIENT_ENV, WORKER_AMBIENT_PREFIXES)
            .into_iter()
            .map(|(k, _)| k.into_string().unwrap())
            .collect();
    assert_eq!(kept, ["HOME", "PATH", "LC_ALL", "AIDA_HOME"]);
    let helper: Vec<String> = ambient_env(parent, HELPER_AMBIENT_ENV, &[])
        .into_iter()
        .map(|(k, _)| k.into_string().unwrap())
        .collect();
    assert_eq!(helper, ["HOME", "PATH", "AIDA_HOME"]);
    for name in WORKER_AMBIENT_ENV.iter().chain(HELPER_AMBIENT_ENV) {
        assert!(
            !name.contains("TOKEN") && !name.contains("GRANT") && !name.starts_with("AIDA_SESSION"),
            "{name}"
        );
    }
}

/// A helper is started with ONLY its explicit environment, the headless
/// wait bounds, its private channel at fd 3, and its own process group.
// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac08e361 | ai:claude
#[test]
fn story_2254_bg_headless_helper_gets_wait_envs_and_private_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        (
            "AIDA_SESSION_GRANT",
            Some("11111111-1111-1111-1111-111111111111"),
        ),
        ("AIDA_AGENT_REGISTRY_TOKEN", Some("leak")),
    ]);
    let log_path = tmp.path().join("helper.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut child = spawn_helper(
        std::path::Path::new("/bin/sh"),
        &[
            "-c",
            "env; read line <&3; echo \"got=$line\" >&3; ps -o pgid= -p $$",
        ],
        tmp.path(),
        tmp.path(),
        &log,
        &theirs,
        false,
    )
    .unwrap();
    drop(theirs);
    let mut writer = ours.try_clone().unwrap();
    writeln!(writer, "hello").unwrap();
    let mut reader = BufReader::new(ours);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "got=hello");
    assert!(child.wait().unwrap().success());
    let out = std::fs::read_to_string(&log_path).unwrap();
    assert!(out.contains("AIDA_HEADLESS=1"), "{out}");
    assert!(
        out.contains(&format!("{}=", crate::burndown::BG_WAIT_CEILING_ENV)),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "{}=1",
            crate::session::DISABLE_BACKGROUND_TASKS_ENV
        )),
        "{out}"
    );
    assert!(!out.contains("AIDA_SESSION_GRANT"), "{out}");
    assert!(!out.contains("AIDA_AGENT_REGISTRY_TOKEN"), "{out}");
    let pgid: u32 = out.lines().last().unwrap().trim().parse().unwrap();
    assert_eq!(
        pgid,
        child.id(),
        "the gated worker leads its own process group"
    );
}

fn write_lease(root: &std::path::Path, id: &str, body: &str) -> std::path::PathBuf {
    let dir = root.join(".aida/sessions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.toml"));
    std::fs::write(&path, body).unwrap();
    path
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_strict_lease_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let grant = "22222222-2222-2222-2222-222222222222";
    let me = std::process::id();
    let start = aida_core::liveness::process_start_identity(me).unwrap();
    // Without this launch's grant on the lease: refused, nothing written.
    let path = write_lease(root, "L1", "id = \"L1\"\n");
    assert!(session::stamp_lease_detached_worker_at(root, "L1", grant, me, &start).is_err());
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("active_pid"));
    // A different LIVE process already holds the lease: refused.
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let other_start = aida_core::liveness::process_start_identity(other.id()).unwrap();
    write_lease(
        root,
        "L1",
        &format!(
            "id = \"L1\"\nchild_grant_id = \"{grant}\"\nactive_pid = {}\nactive_pid_start_time = \"{other_start}\"\n",
            other.id()
        ),
    );
    assert!(session::stamp_lease_detached_worker_at(root, "L1", grant, me, &start).is_err());
    let _ = other.kill();
    let _ = other.wait();
    // A stale (dead) holder is replaced, pid and start together.
    session::stamp_lease_detached_worker_at(root, "L1", grant, me, &start).unwrap();
    let table: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(table["active_pid"].as_integer(), Some(i64::from(me)));
    assert_eq!(
        table["active_pid_start_time"].as_str(),
        Some(start.as_str())
    );
    // A missing start identity is refused outright.
    assert!(session::stamp_lease_detached_worker_at(root, "L1", grant, me, "").is_err());
}

// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac0b1daf | ai:claude
#[test]
fn story_2254_bg_registers_actual_worker_pid_and_required_start() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut worker = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = worker.id();
    let start = aida_core::liveness::process_start_identity(pid).unwrap();
    let row = || agent_registry::SpawnedRow {
        delegate_seats: Vec::new(),
        role: Some("implementer".into()),
        current_spec: Some("TASK-1".into()),
        worktree_path: root.to_path_buf(),
        binary: None,
        name: Some("codex-implementer-77".into()),
        native_session_id: None,
        resumed_from: None,
        description: None,
    };
    // A stale start identity (reused pid) is refused.
    assert!(agent_registry::register_detached_worker(
        root,
        "codex",
        pid,
        "linux-starttime:1",
        row()
    )
    .is_err());
    let entry =
        agent_registry::register_detached_worker(root, "codex", pid, &start, row()).unwrap();
    assert_eq!(entry.pid, pid);
    assert_eq!(
        entry
            .process_identity()
            .map(|id| format!("linux-starttime:{}", id.starttime))
            .as_deref(),
        Some(start.as_str())
    );
    assert_eq!(
        entry.pid_namespace,
        agent_registry::process_pid_namespace(pid)
    );
    // An old generation's identity never ends a successor's row.
    assert!(
        agent_registry::mark_detached_worker_ended(root, "codex", pid, "linux-starttime:1")
            .unwrap()
            .is_none()
    );
    let ended = agent_registry::mark_detached_worker_ended(root, "codex", pid, &start).unwrap();
    assert!(ended.unwrap().ended_at.is_some());
    let _ = worker.kill();
    let _ = worker.wait();
}

/// The detached lane is one more Codex launch site: it finalizes through
/// the BUG-2398 adapter after its own grant and headless assignments, and
/// binds only through the gate.
// trace:STORY-2254 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac3769a0 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac08e361 | ai:claude
// trace:STORY-b6b1278890774ef993d2ebc327862470-277.ac9b0efd | ai:claude
#[test]
fn story_2254_bg_source_contract() {
    let src = include_str!("../agent_codex_background.rs");
    let build = src.split("fn build_worker_exec(").nth(1).unwrap();
    let at = |needle: &str| build.find(needle).unwrap_or_else(|| panic!("{needle}"));
    assert!(at(".env_clear()") < at("crate::launch::profile::finalize_codex(&mut command)"));
    assert!(
        at("seat_authority::GRANT_ENV, grant_id)")
            < at("crate::launch::profile::finalize_codex(&mut command)")
    );
    assert!(at("headless_wait_envs(") < at("crate::launch::profile::finalize_codex(&mut command)"));
    assert_eq!(
        src.matches("crate::launch::profile::finalize_codex(")
            .count(),
        1
    );
    assert!(!src.contains("bind_grant_to_pid"));
    assert!(!src.contains("issue_launch_child("));
    assert!(src.contains("seat_authority::accept_detached_launch_child("));
}
