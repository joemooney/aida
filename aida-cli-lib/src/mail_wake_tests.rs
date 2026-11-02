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
// trace:STORY-HOME-096.ac9979a7 | ai:claude
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
// trace:STORY-HOME-096.aca8bab2 | ai:claude
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
// trace:STORY-HOME-096.acad6a24 | ai:claude
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

// The launcher's stamp reads and writes its row under one lock hold: a hook
// write that lands while the stamp waits is kept, not overwritten by a row
// read before it.
// trace:STORY-HOME-096.aca8bab2 | ai:claude
#[test]
fn story_home_096_stamp_keeps_a_hook_write_that_interleaves() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().to_path_buf();
    let launcher_view = row(&project, "codex", 50, None, None);
    let lock = crate::agent_handle::lock_namespace(&project).unwrap();
    let stamp = {
        let project = project.clone();
        std::thread::spawn(move || {
            agent_registry::set_mail_wake_on_row(&project, &launcher_view, MailWake::Pending)
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(150));
    // The hook's write lands while the stamp waits for the lock.
    let mut hook = on_disk(&project, "codex-50");
    hook.claude_session_id = Some(THREAD.into());
    hook.name = Some("codex-agent-50".into());
    std::fs::write(
        agent_registry::registry_path(&project, "codex-50"),
        toml::to_string(&hook).unwrap(),
    )
    .unwrap();
    drop(lock);
    let stamped = stamp.join().unwrap().unwrap();
    assert_eq!(stamped.mail_wake, Some(MailWake::Pending));
    let stored = on_disk(&project, "codex-50");
    assert_eq!(stored.mail_wake, Some(MailWake::Pending));
    assert_eq!(stored.claude_session_id.as_deref(), Some(THREAD));
    assert_eq!(stored.name.as_deref(), Some("codex-agent-50"));
}

// The hook folds in a launcher register-and-stamp that landed between its
// read and its write: the stamp survives, so the hook arms it.
// trace:STORY-HOME-096.aca8bab2 | ai:claude
#[test]
fn story_home_096_hook_write_keeps_a_launcher_stamp_that_interleaves() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let launcher = row(project, "codex", 51, Some("pending"), None);
    let mut hook = launcher.clone();
    hook.source = "session-start-hook".into();
    hook.name = Some("codex-codex-07".into());
    hook.mail_wake = None;
    hook.claude_session_id = Some(THREAD.into());

    // The hook read no row; the launcher registered and stamped since.
    let mut merged = hook.clone();
    agent_registry::carry_concurrent_launcher_write(&mut merged, None, Some(&launcher));
    assert_eq!(merged.mail_wake, Some(MailWake::Pending));
    assert_eq!(merged.source, "agent-launcher");
    assert_eq!(merged.name.as_deref(), Some("codex-agent-03"));
    assert_eq!(merged.claude_session_id.as_deref(), Some(THREAD));
    let calls = Cell::new(0);
    agent_registry::write_entry(project, &merged).unwrap();
    let armed = arm_pending(project, &merged, |_| {
        calls.set(calls.get() + 1);
        Ok(())
    });
    assert_eq!(armed.map(|a| a.state), Some(MailWake::Armed));
    assert_eq!(calls.get(), 1);

    // The hook read the launcher row already: only the wake is carried.
    let mut seen = hook.clone();
    agent_registry::carry_concurrent_launcher_write(&mut seen, Some(&launcher), Some(&launcher));
    assert_eq!(seen.mail_wake, Some(MailWake::Pending));
    assert_eq!(seen.name.as_deref(), Some("codex-codex-07"));

    // A row a recycled pid owns is never folded in.
    let mut other = launcher.clone();
    other.pid_start = Some("start-0".into());
    let mut kept = hook.clone();
    agent_registry::carry_concurrent_launcher_write(&mut kept, None, Some(&other));
    assert_eq!(kept, hook);
}

// Arming only moves a row that is still pending: a stale view never
// overwrites a row the other side of the launch armed, or one that retired.
// trace:STORY-HOME-096.aca8bab2 | ai:claude
#[test]
fn story_home_096_arming_a_stale_view_leaves_a_row_that_moved_on() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    for (pid, now) in [(52, "armed"), (53, "retired")] {
        let stale = row(project, "codex", pid, Some("pending"), Some(THREAD));
        row(project, "codex", pid, Some(now), Some(THREAD));
        assert_eq!(
            arm_pending(project, &stale, |_| anyhow::bail!("no systemd")),
            None
        );
        let stored = on_disk(project, &format!("codex-{pid}")).mail_wake;
        assert_eq!(stored.map(MailWake::as_str), Some(now));
    }
}

// One side of the launch failed to install the unit and the other then
// installed it: the success wins, so the row does not read `failed` while
// the wake is live.
// trace:STORY-HOME-096.aca8bab2 | ai:claude
#[test]
fn story_home_096_a_later_successful_arm_replaces_the_other_sides_failure() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let stale = row(project, "codex", 54, Some("pending"), Some(THREAD));
    let failed = arm_pending(project, &stale, |_| anyhow::bail!("no systemd")).unwrap();
    assert_eq!(failed.state, MailWake::Failed);
    let armed = arm_pending(project, &stale, |_| Ok(())).unwrap();
    assert_eq!(armed.state, MailWake::Armed);
    assert_eq!(
        on_disk(project, "codex-54").mail_wake,
        Some(MailWake::Armed)
    );
}

// A wedged `codex --version` reads as an unknown version instead of
// hanging the launch.
// trace:STORY-HOME-096.ac9979a7 | ai:claude
#[cfg(unix)]
#[test]
fn story_home_096_codex_version_is_bounded() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let wedged = script("wedged", "exec sleep 30");
    let started = std::time::Instant::now();
    assert_eq!(
        version_within(&wedged, std::time::Duration::from_millis(300)),
        None
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    let answers = script("answers", "echo codex-cli 0.164.0");
    assert_eq!(
        version_within(&answers, std::time::Duration::from_secs(5)).as_deref(),
        Some("codex-cli 0.164.0")
    );
}

// The launcher's row keeps a thread the child's hook recorded first.
// trace:STORY-HOME-096.aca8bab2 | ai:claude
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

// trace:STORY-HOME-096.ac316a6a | ai:claude
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

// `[agents.codex] mail_wake` is the launch default; a non-boolean is an
// error the launcher turns into a warning and the default.
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

// A row a hand-started Codex session's SessionStart hook wrote (no launcher,
// no decision) adopts the default and arms on its exact thread; the opt-outs
// still hold; other vendors and decided rows are left alone.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_hook_arms_a_hand_started_codex_row() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let calls = Cell::new(0);
    let ok = |_: &Path| {
        calls.set(calls.get() + 1);
        Ok(())
    };

    let hand = row(project, "codex", 60, None, Some(THREAD));
    let armed = arm_session(project, &hand, None, || None, ok).unwrap();
    assert_eq!(armed.state, MailWake::Armed);
    assert_eq!(armed.warning, None);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        on_disk(project, "codex-60").mail_wake,
        Some(MailWake::Armed)
    );

    // `[agents.codex] mail_wake = false` opts a hand-started seat out.
    let opted = row(project, "codex", 61, None, Some(THREAD));
    assert_eq!(arm_session(project, &opted, None, || Some(false), ok), None);
    assert_eq!(on_disk(project, "codex-61").mail_wake, Some(MailWake::Off));

    // `agent new --no-mail-wake` names `off` to its child, which wins even
    // when the hook writes the row before the launcher stamps it.
    let flagged = row(project, "codex", 62, None, Some(THREAD));
    assert_eq!(
        arm_session(project, &flagged, Some("off"), || Some(true), ok),
        None
    );
    assert_eq!(on_disk(project, "codex-62").mail_wake, Some(MailWake::Off));

    // An already-decided row, or a non-Codex row, is not re-decided.
    let decided = row(project, "codex", 63, Some("off"), Some(THREAD));
    assert_eq!(arm_session(project, &decided, None, || None, ok), None);
    let claude = row(project, "claude", 64, None, Some(THREAD));
    assert_eq!(arm_session(project, &claude, None, || None, ok), None);
    assert_eq!(on_disk(project, "claude-64").mail_wake, None);
    assert_eq!(calls.get(), 1, "only the first row installed the service");

    // No thread yet: the row waits as pending, never guessed.
    let early = row(project, "codex", 65, None, None);
    assert_eq!(arm_session(project, &early, None, || None, ok), None);
    assert_eq!(
        on_disk(project, "codex-65").mail_wake,
        Some(MailWake::Pending)
    );
}

// A refused service install (containment, no user manager) leaves the
// hand-started row `failed`, not `pending`, with the one command to run.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_refused_install_marks_a_hand_started_row_failed() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let hand = row(project, "codex", 66, None, Some(THREAD));
    let failed = arm_session(
        project,
        &hand,
        None,
        || None,
        |_| anyhow::bail!("systemctl --user refused"),
    )
    .unwrap();
    assert_eq!(failed.state, MailWake::Failed);
    let warning = failed.warning.unwrap();
    assert!(
        warning.contains("aida mailbox wake-service install")
            && warning.contains("aida agent wake arm codex-agent-03"),
        "{warning}"
    );
    assert_eq!(
        on_disk(project, "codex-66").mail_wake,
        Some(MailWake::Failed)
    );
}

// The launcher's later `pending` stamp never downgrades a row the child's
// hook already armed.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_launcher_stamp_keeps_a_hook_armed_row() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let armed = row(project, "codex", 67, Some("armed"), Some(THREAD));
    let after = agent_registry::set_mail_wake_on_row(project, &armed, MailWake::Pending).unwrap();
    assert_eq!(after.mail_wake, Some(MailWake::Armed));
    let after = agent_registry::set_mail_wake_on_row(project, &armed, MailWake::Off).unwrap();
    assert_eq!(
        after.mail_wake,
        Some(MailWake::Off),
        "an opt-out still wins"
    );
}

// `aida session start` on a Codex project says whether the wake will arm.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_session_start_names_the_wake() {
    let root = Path::new("/work/aida");
    let installed = session_start_wake_line("codex", None, true, root).unwrap();
    assert!(installed.contains("arms when codex starts"), "{installed}");
    let missing = session_start_wake_line("codex", None, false, root).unwrap();
    assert!(
        missing.contains("(cd /work/aida && aida mailbox wake-service install)"),
        "{missing}"
    );
    let off = session_start_wake_line("codex", Some(false), false, root).unwrap();
    assert!(off.contains("off"), "{off}");
    assert_eq!(session_start_wake_line("claude", None, false, root), None);
}

// Default `aida doctor`: a live Codex agent with no wake service installed
// is a finding with the install command; an installed service, or only
// opted-out / non-Codex agents, is quiet.
// trace:STORY-2256 | ai:claude
#[test]
fn story_2256_doctor_flags_live_codex_without_the_service() {
    use crate::codex_mail_health::service_findings_in;
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    let codex = row(project, "codex", 70, None, Some(THREAD));
    let off = row(project, "codex", 71, Some("off"), Some(THREAD));
    let claude = row(project, "claude", 72, None, Some(THREAD));

    let findings = service_findings_in(project, &[codex.clone(), off.clone()], false);
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(findings[0].id, "codex-mail-wake:service-not-installed");
    assert!(findings[0].summary.contains("codex-agent-03"));
    assert!(findings[0]
        .action
        .contains("aida mailbox wake-service install"));
    assert!(service_findings_in(project, &[codex], true).is_empty());
    assert!(service_findings_in(project, &[off, claude], false).is_empty());
}
