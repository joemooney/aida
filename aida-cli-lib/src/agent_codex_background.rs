//! Detached Codex exec launches reuse foreground preflight, grants and registry tracking.
// trace:TASK-2631 | ai:codex
use super::*;

pub(super) fn validate(
    config: &AgentLaunchConfig,
    prompt: &AgentPromptOptions,
    resume: &AgentResumeOptions,
    rotate: bool,
) -> Result<()> {
    if config.agent_type != "codex"
        || rotate
        || resume.direct_target.is_some()
        || (!prompt.auto_prompt && prompt.explicit_prompt.is_none())
    {
        anyhow::bail!("Codex --bg requires a fresh prompted session; --rotate, --resume and --no-prompt cannot be combined with it");
    }
    Ok(())
}

// trace:TASK-2631 | ai:codex
pub(super) fn configure_args(config: &mut AgentLaunchConfig) -> Result<()> {
    if config.agent_type != "codex" {
        anyhow::bail!("native background exec is only supported for codex");
    }
    config.default_args.retain(|a| a != CODEX_NO_DAEMON_FLAG);
    // Normalize the alias before the ordinary bypass consent gate sees the argv.
    for arg in &mut config.default_args {
        if arg == "--yolo" {
            *arg = "--dangerously-bypass-approvals-and-sandbox".into();
        }
    }
    let bypass = config
        .default_args
        .iter()
        .any(|a| a == "--dangerously-bypass-approvals-and-sandbox" || a == "--yolo");
    if !bypass {
        let sandbox = resolved_flag_value(&config.default_args, "--sandbox")
            .unwrap_or("workspace-write")
            .to_string();
        config.default_args =
            split_posture_flags("codex", std::mem::take(&mut config.default_args)).0;
        config.default_args.extend([
            "--sandbox".into(),
            sandbox,
            "--ask-for-approval".into(),
            "never".into(),
        ]);
    }
    config
        .default_args
        .extend(["exec".into(), "--json".into(), "--".into()]);
    Ok(())
}

// trace:TASK-2631 | ai:codex
pub(super) fn is_headless(config: &AgentLaunchConfig) -> bool {
    config.agent_type == "codex"
        && config
            .default_args
            .ends_with(&["exec".into(), "--json".into(), "--".into()])
}

// trace:TASK-2631 | ai:codex
#[cfg(unix)]
pub(super) fn configure_command(
    command: &mut std::process::Command,
    root: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    let directory = root.join(".aida/headless-logs");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("codex-bg-{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    command
        .stdin(std::process::Stdio::null())
        .stderr(log.try_clone()?)
        .stdout(log)
        .env("AIDA_HEADLESS", "1");
    // SAFETY: setsid is async-signal-safe; the forked child owns the new session.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(path)
}

// trace:TASK-2631 | ai:codex
#[cfg(not(unix))]
pub(super) fn configure_command(
    _command: &mut std::process::Command,
    _root: &std::path::Path,
) -> Result<std::path::PathBuf> {
    anyhow::bail!("Codex background launches require Linux")
}

/// A failed acceptance must not leave an untracked detached process running.
// trace:TASK-2631 | ai:codex
pub(super) fn accept_child(
    child: &mut std::process::Child,
    accept: impl FnOnce(u32) -> Result<()>,
) -> Result<()> {
    if let Err(error) = accept(child.id()) {
        // The leader is our unreaped child, so its process-group id cannot be recycled.
        crate::kill_process_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.context("could not record detached Codex child; stopped the child"));
    }
    Ok(())
}

// trace:TASK-2631 | ai:codex
#[cfg(all(test, target_os = "linux"))]
mod task_2631_background_tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn config() -> AgentLaunchConfig {
        AgentLaunchConfig {
            agent_type: "codex",
            binary: "codex",
            default_args: vec![CODEX_NO_DAEMON_FLAG.into()],
            prompt_style: AgentPromptStyle::Positional,
            contained: false,
        }
    }

    // AC2': the actual CLI accepts --bg and refuses incompatible attached-session modes.
    #[test]
    fn codex_bg_flag_and_conflicts() {
        let parsed = crate::command_registry::try_parse_from([
            "aida",
            "agent",
            "new",
            "codex",
            "--bg",
            "--no-exec",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command.registered::<AgentArgs>().inner,
            AgentCommand::New {
                command: Some(AgentNewCommand::Codex { bg: true, .. })
            }
        ));
        for option in [
            vec!["--rotate"],
            vec!["--resume", "latest"],
            vec!["--no-prompt"],
        ] {
            let mut args = vec!["aida", "agent", "new", "codex", "--bg"];
            args.extend(option);
            assert!(crate::command_registry::try_parse_from(args).is_err());
        }
    }

    // AC2': normal preflight still refuses missing opt-in before any log/process exists.
    #[test]
    fn background_no_key_refuses_and_native_builds_exec() {
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOME", home.path().to_str()),
            ("AIDA_CONTAINED", None),
        ]);
        std::fs::create_dir_all(root.path().join(".aida")).unwrap();
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[contained]\nenable = true\n",
        )
        .unwrap();
        let mut launch = config();
        assert!(crate::apply_agent_default_flags(
            &mut launch,
            root.path(),
            AgentDefaultFlagOptions::new(true, Vec::new(), None).background(true),
            false
        )
        .is_err());
        assert!(!root.path().join(".aida/headless-logs").exists());
        std::fs::write(
            root.path().join(".aida/config.toml"),
            "[contained]\nenable = true\n[contained.vendors]\ncodex = \"native\"\n",
        )
        .unwrap();
        let mut launch = config();
        crate::apply_agent_default_flags(
            &mut launch,
            root.path(),
            AgentDefaultFlagOptions::new(true, Vec::new(), None).background(true),
            false,
        )
        .unwrap();
        configure_args(&mut launch).unwrap();
        assert_eq!(
            launch.default_args,
            [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "exec",
                "--json",
                "--"
            ]
        );
        assert!(!is_contained_launch(&launch));
        assert_eq!(agent_containment_posture(&launch), "native");
    }

    // This process owns the unreaped fixture child: even a failed assertion cleans it up.
    struct ChildGuard(u32);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            crate::kill_process_group(self.0);
            unsafe {
                libc::waitpid(self.0 as i32, std::ptr::null_mut(), 0);
            }
        }
    }

    // AC2': start a real mock child, return while it runs, and retain its PID/lease/log.
    #[test]
    fn native_background_child_detaches_and_is_tracked() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let _env = crate::test_env::EnvVarsGuard::apply(&[
            ("AIDA_HOME", home.path().to_str()),
            ("AIDA_CONTAINED", None),
            ("AIDA_OS_WRAP", None),
        ]);
        let fake = root.path().join("fake-codex");
        let release = root.path().join("release");
        crate::test_exec::write_executable(&fake, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'codex fake'; exit 0; fi\nprintf '%s\\n' \"posture=$AIDA_CONTAINMENT_POSTURE\" \"contained=${{AIDA_CONTAINED-unset}}\" \"headless=$AIDA_HEADLESS\" \"$@\"\nwhile [ ! -f '{}' ]; do sleep 0.02; done\n", release.display()));
        std::fs::create_dir_all(root.path().join(".aida/sessions")).unwrap();
        std::fs::write(
            root.path().join(".aida/sessions/bg-lease.toml"),
            "id = \"bg-lease\"\n",
        )
        .unwrap();
        let plan = AgentLaunchPlan {
            project_root: root.path().to_path_buf(),
            launch_cwd: root.path().to_path_buf(),
            role: None,
            role_instance: RoleInstanceKind::Driver,
            current_spec: None,
            name: "codex-bg-test".into(),
            lease_id: Some("bg-lease".into()),
            native_session_id: None,
            resumed_from: None,
        };
        let mut launch = config();
        configure_args(&mut launch).unwrap();
        assert!(
            agent_launch_env_inputs(&launch, &plan, None).contains(&("AIDA_HEADLESS", "1".into()))
        );
        let exit = run_agent_generation(
            &fake,
            &launch,
            &plan,
            None,
            &["--literal prompt".into()],
            None,
            false,
            false,
            None,
            None,
            true,
            || {},
        )
        .unwrap();
        assert!(exit.status.success());
        let entries = agent_registry::load_entries(root.path());
        assert_eq!(entries.len(), 1);
        let entry = &entries[0].1;
        let _child = ChildGuard(entry.pid);
        assert_eq!(unsafe { libc::getsid(entry.pid as i32) }, entry.pid as i32);
        assert!(entry.ended_at.is_none());
        let lease: toml::Value =
            std::fs::read_to_string(root.path().join(".aida/sessions/bg-lease.toml"))
                .unwrap()
                .parse()
                .unwrap();
        assert_eq!(lease["active_pid"].as_integer(), Some(i64::from(entry.pid)));
        let log = std::fs::read_dir(root.path().join(".aida/headless-logs"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(entry
            .description
            .as_deref()
            .unwrap()
            .contains(&log.display().to_string()));
        let start = Instant::now();
        let output = loop {
            let output = std::fs::read_to_string(&log).unwrap();
            if output.contains("--literal prompt") {
                break output;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "mock child did not write its launch contract"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            output.contains("posture=native\ncontained=unset\nheadless=1"),
            "{output}"
        );
        assert!(
            output.contains("never\nexec\n--json\n--\n--literal prompt"),
            "{output}"
        );
        std::fs::write(release, "finish").unwrap();
    }

    // AC2': registration/binding errors stop and reap the detached child before refusal.
    #[test]
    fn failed_background_acceptance_stops_child() {
        let root = tempfile::tempdir().unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "sleep 60"]);
        configure_command(&mut command, root.path()).unwrap();
        let mut child = command.spawn().unwrap();
        let error = accept_child(&mut child, |_| {
            anyhow::bail!("fixture registration failure")
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("stopped the child"));
        assert!(child.try_wait().unwrap().is_some());
    }
}
