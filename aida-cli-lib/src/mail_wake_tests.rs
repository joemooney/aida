// trace:STORY-HOME-096 | ai:claude
use super::*;
use crate::agent_registry::{self, AgentRegistryEntry, MailWake};
use std::cell::Cell;
use std::path::PathBuf;

const THREAD: &str = "01a185ba-0000-7000-8000-000000000003";

/// Write a registry row (`<agent_type>-<pid>`) and read it back.
fn row(
    project: &Path,
    agent_type: &str,
    pid: u32,
    wake: Option<&str>,
    session: Option<&str>,
) -> AgentRegistryEntry {
    let id = format!("{agent_type}-{pid}");
    let mut table = toml::Table::new();
    for (key, value) in [
        ("id", id.clone()),
        ("agent_type", agent_type.into()),
        ("pid_start", "start-1".into()),
        ("name", "codex-agent-03".into()),
        ("role", "implementer".into()),
        ("started_at", "2026-10-29T00:00:00Z".into()),
        ("last_active_at", "2026-10-29T00:00:00Z".into()),
        ("worktree_path", project.to_string_lossy().into_owned()),
        ("source", "agent-launcher".into()),
    ] {
        table.insert(key.into(), toml::Value::String(value));
    }
    table.insert("pid".into(), toml::Value::Integer(i64::from(pid)));
    if let Some(wake) = wake {
        table.insert("mail_wake".into(), toml::Value::String(wake.into()));
    }
    if let Some(session) = session {
        table.insert(
            "claude_session_id".into(),
            toml::Value::String(session.into()),
        );
    }
    let path = agent_registry::agents_dir(project).join(format!("{id}.toml"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    on_disk(project, &id)
}

fn on_disk(project: &Path, id: &str) -> AgentRegistryEntry {
    let path: PathBuf = agent_registry::registry_path(project, id);
    toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// The flag, then the agents.toml default, then the Codex version decide a
// launch; Antigravity warns once; Claude is untouched.
#[test]
fn story_home_096_launch_decision_honours_flag_config_and_version() {
    let codex = |opt_out, default, version| launch_wake("codex", opt_out, default, version);
    assert_eq!(
        codex(false, None, Some("codex-cli 0.164.0")).state,
        Some(MailWake::Pending)
    );
    assert_eq!(codex(false, None, None).state, Some(MailWake::Pending));
    assert_eq!(
        codex(true, None, Some("codex-cli 0.170.1")).state,
        Some(MailWake::Off)
    );
    assert_eq!(codex(false, Some(false), None).state, Some(MailWake::Off));
    assert_eq!(
        codex(false, Some(true), None).state,
        Some(MailWake::Pending)
    );
    let old = codex(false, None, Some("codex-cli 0.163.9"));
    assert_eq!(old.state, Some(MailWake::Unsupported));
    assert!(old.warning.unwrap().contains("next turn"));
    let agy = launch_wake("antigravity", false, None, None);
    assert_eq!(agy.state, Some(MailWake::Unsupported));
    assert!(agy.warning.unwrap().contains("next turn"));
    assert_eq!(
        launch_wake("claude", false, None, None),
        LaunchWake {
            state: None,
            warning: None
        }
    );
}

#[test]
fn story_home_096_parses_codex_version_lines() {
    assert_eq!(parse_version("codex-cli 0.164.0"), Some((0, 164, 0)));
    assert_eq!(parse_version("codex v1.2.3-beta.1"), Some((1, 2, 3)));
    assert_eq!(parse_version("codex-cli"), None);
    assert_eq!(parse_version("codex 1.2"), None);
}

// The SessionStart hook arms a pending row once it records the thread; a
// failed service install leaves it `failed` with the exact manual commands.
#[test]
fn story_home_096_session_start_arms_pending_row_once_the_thread_is_known() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let calls = Cell::new(0);
    let ok = |_: &Path| {
        calls.set(calls.get() + 1);
        Ok(())
    };

    let waiting = row(project, "codex", 41, Some("pending"), None);
    assert_eq!(arm_pending(project, &waiting, ok), None, "no thread yet");
    let off = row(project, "codex", 42, Some("off"), Some(THREAD));
    assert_eq!(arm_pending(project, &off, ok), None);
    let claude = row(project, "claude", 43, Some("pending"), Some(THREAD));
    assert_eq!(arm_pending(project, &claude, ok), None);
    assert_eq!(calls.get(), 0);

    let pending = row(project, "codex", 44, Some("pending"), Some(THREAD));
    let armed = arm_pending(project, &pending, ok).unwrap();
    assert_eq!(armed.state, MailWake::Armed);
    assert_eq!(calls.get(), 1);
    let stored = on_disk(project, "codex-44");
    assert_eq!(stored.mail_wake, Some(MailWake::Armed));
    assert_eq!(stored.claude_session_id.as_deref(), Some(THREAD));

    let pending = row(project, "codex", 45, Some("pending"), Some(THREAD));
    let failed = arm_pending(project, &pending, |_| anyhow::bail!("no systemd")).unwrap();
    assert_eq!(failed.state, MailWake::Failed);
    let warning = failed.warning.unwrap();
    assert!(
        warning.contains("aida mailbox wake-service install"),
        "{warning}"
    );
    assert!(
        warning.contains("aida agent wake arm codex-agent-03"),
        "{warning}"
    );
    assert_eq!(
        on_disk(project, "codex-45").mail_wake,
        Some(MailWake::Failed)
    );
}

// A row a recycled pid now owns is never stamped.
#[test]
fn story_home_096_stamp_refuses_a_row_another_process_took_over() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let mut stale = row(project, "codex", 46, Some("pending"), Some(THREAD));
    stale.pid_start = Some("start-0".into());
    assert!(agent_registry::set_mail_wake_on_row(project, &stale, MailWake::Armed).is_err());
    assert_eq!(
        on_disk(project, "codex-46").mail_wake,
        Some(MailWake::Pending)
    );
}

// Ending the agent, or the GC finding its process gone, retires the wake;
// a row that never armed keeps its state.
#[test]
fn story_home_096_end_and_gc_retire_the_wake() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    // A pid above the kernel's largest pid_max, so the GC sees it gone.
    let dead = 4_194_399;
    row(project, "codex", 47, Some("armed"), Some(THREAD));
    let ended = agent_registry::mark_agent_ended(project, "codex", 47)
        .unwrap()
        .unwrap();
    assert_eq!(ended.mail_wake, Some(MailWake::Retired));
    assert_eq!(
        on_disk(project, "codex-47").mail_wake,
        Some(MailWake::Retired)
    );

    // Recorded in this process's pid namespace, so the GC can observe it.
    let mut gone = row(project, "codex", dead, Some("pending"), Some(THREAD));
    gone.pid_namespace = agent_registry::current_pid_namespace();
    agent_registry::write_entry(project, &gone).unwrap();
    let report = agent_registry::gc_dead_agents(project, false, None).unwrap();
    assert_eq!(report.marked_ended, 1, "{report:?}");
    assert_eq!(
        on_disk(project, &format!("codex-{dead}")).mail_wake,
        Some(MailWake::Retired)
    );

    let mut off = row(project, "codex", 48, Some("off"), None);
    assert!(!retire(&mut off));
    assert_eq!(off.mail_wake, Some(MailWake::Off));
}

// A launched Codex reports its thread through its SessionStart hook.
#[test]
fn story_home_096_row_thread_falls_back_to_the_hook_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut entry = row(dir.path(), "codex", 49, None, Some(THREAD));
    assert_eq!(
        crate::mailbox_wake_run::row_thread(&entry).as_deref(),
        Some(THREAD)
    );
    entry.claude_session_id = Some("not-a-uuid".into());
    assert_eq!(crate::mailbox_wake_run::row_thread(&entry), None);
}

// The launcher's row keeps a thread the child's hook recorded first.
#[test]
fn story_home_096_launcher_row_keeps_a_hook_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let pid = std::process::id();
    let start = crate::process_probe::process_start_identity(pid);
    let mut hook = row(project, "codex", pid, None, Some(THREAD));
    hook.pid_start = start.clone();
    agent_registry::write_entry(project, &hook).unwrap();
    let launched = agent_registry::register_spawned_agent(
        project,
        "codex",
        pid,
        Some("implementer".into()),
        None,
        project.to_path_buf(),
        None,
        Some("codex-agent-03".into()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(launched.claude_session_id.as_deref(), Some(THREAD));
}

#[test]
fn story_home_096_wake_column_names_every_state() {
    assert_eq!(wake_cell("codex", Some(MailWake::Armed)), "armed");
    assert_eq!(
        wake_cell("codex", Some(MailWake::Pending)),
        "pending-thread"
    );
    assert_eq!(wake_cell("codex", Some(MailWake::Failed)), "failed");
    assert_eq!(wake_cell("codex", Some(MailWake::Off)), "off");
    assert_eq!(wake_cell("codex", Some(MailWake::Retired)), "retired");
    assert_eq!(
        wake_cell("codex", Some(MailWake::Unsupported)),
        "unsupported"
    );
    assert_eq!(wake_cell("antigravity", None), "unsupported");
    assert_eq!(wake_cell("claude", None), "-");
    assert_eq!(wake_cell("codex", None), "-");
}

// `[agents.codex] mail_wake` is the launch default; a non-boolean refuses.
#[test]
fn story_home_096_agents_toml_mail_wake_default() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join(".aida")).unwrap();
    let file = project.join(".aida/agents.toml");
    std::fs::write(&file, "[agents.codex]\nmail_wake = false\n").unwrap();
    assert_eq!(
        crate::load_agents_tool_bool(project, "codex", "mail_wake").unwrap(),
        Some(false)
    );
    std::fs::write(&file, "[agents.codex]\nmail_wake = \"no\"\n").unwrap();
    assert!(crate::load_agents_tool_bool(project, "codex", "mail_wake").is_err());
}
