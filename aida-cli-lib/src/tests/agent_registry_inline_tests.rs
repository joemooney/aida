use super::*;
use chrono::Duration;
use tempfile::TempDir;

// trace:TASK-3366 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn proc_identity_s4_legacy_heartbeat_upgrades_in_place() {
    let root = TempDir::new().unwrap();
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_AGENT_TYPE", Some("codex")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_SESSION_SCOPE", None),
        ("AIDA_AGENT_NAME", None),
    ]);
    let pid = std::process::id();
    let id = ProcIdentity::capture(pid).unwrap();
    let binary = AgentBinaryIdentity::new("test".into(), "abc".into());
    let mut old =
        touch_session_start_agent_for(root.path(), root.path(), Some("sid"), &binary, pid).unwrap();
    old.source = "agent-launcher".into();
    old.name = Some("stable-seat".into());
    old.role = Some("reviewer".into());
    old.native_session_id = Some("native-thread".into());
    old.mail_wake = Some(MailWake::Armed);
    old.delegate_seats = vec!["reviewer".into()];
    old.resumed_from = Some("old-thread".into());
    old.pid_start = Some(id.legacy_ticks_start());
    for namespace in [None, Some(id.pidns_inode)] {
        old.pid_namespace = namespace;
        write_entry(root.path(), &old).unwrap();
        assert_eq!(
            heartbeat_session_agent_for(root.path(), root.path(), Some("sid"), &binary, pid)
                .unwrap(),
            Heartbeat::Refreshed
        );
        let body = std::fs::read_to_string(registry_path(root.path(), &old.id)).unwrap();
        let upgraded: AgentRegistryEntry = toml::from_str(&body).unwrap();
        let mut expected = old.clone();
        expected.pid_start = Some(id.legacy().start);
        expected.pid_namespace = Some(id.pidns_inode);
        expected.last_active_at = upgraded.last_active_at;
        assert_eq!(upgraded, expected);
    }
    assert!(!registry_path(root.path(), &old.id)
        .parent()
        .unwrap()
        .join("retired")
        .exists());
}

// trace:TASK-3366 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn proc_identity_s4_legacy_rows_prove_death_without_authorizing_liveness() {
    let pid = std::process::id();
    let identity = ProcIdentity::capture(pid).unwrap();
    let root = TempDir::new().unwrap();
    let mut row = entry_with(pid, Utc::now());
    row.pid_namespace = None;
    for start in [
        None,
        Some(identity.legacy_ticks_start()),
        Some(identity.legacy().start),
    ] {
        row.pid_start = start;
        assert!(matches!(
            entry_process_verdict(&row),
            Verdict::ForeignUnknown(_)
        ));
        assert_eq!(
            observed_liveness_from(root.path(), &row, PidObserver::current()),
            row.pid_start.as_ref().map(|_| true),
        );
    }
    row.pid_namespace = Some(identity.pidns_inode);
    row.pid_start = Some(format!("linux-starttime:{}", identity.starttime + 1));
    assert_eq!(
        entry_process_verdict(&row),
        Verdict::Dead(proc_identity::DeadReason::Reused)
    );
    row.pid_namespace = Some(identity.pidns_inode ^ 1);
    assert!(matches!(
        entry_process_verdict(&row),
        Verdict::ForeignUnknown(_)
    ));
    row.pid_namespace = None;
    row.pid = i32::MAX as u32;
    row.pid_start = None;
    assert_eq!(
        entry_process_verdict(&row),
        Verdict::Dead(proc_identity::DeadReason::Absent)
    );
}

// trace:TASK-3366 | ai:codex
#[test]
#[cfg(target_os = "linux")]
fn proc_identity_s4_registry_matrix_and_launcher_race() {
    let root = TempDir::new().unwrap();
    let pid = std::process::id();
    let identity = ProcIdentity::capture(pid).unwrap();
    let mut row = entry_with(pid, Utc::now());
    row.pid_start = Some(identity.legacy().start);
    row.pid_namespace = Some(identity.pidns_inode);
    assert_eq!(entry_process_verdict(&row), Verdict::Alive);
    assert!(same_registry_process(&row, &row));
    let body = registry_body(&row).unwrap();
    let old_reader: toml::Value = toml::from_str(&body).unwrap();
    assert_ne!(
        old_reader["id"].as_str(),
        Some(row.id.as_str()),
        "old strict filename checks must refuse the new row version"
    );
    let decoded: AgentRegistryEntry = toml::from_str(&body).unwrap();
    assert_eq!(decoded, row);
    assert_eq!(
        serde_json::to_value(&row).unwrap()["id"].as_str(),
        Some(row.id.as_str()),
        "display/API IDs remain stable"
    );
    let future: AgentRegistryEntry =
        toml::from_str(&body.replace("proc-identity-v1:", "proc-identity-v2:")).unwrap();
    assert!(matches!(
        entry_process_verdict(&future),
        Verdict::ForeignUnknown(_)
    ));
    assert!(
        registry_entry_is_live(root.path(), &future),
        "unsupported records retain claims"
    );
    for changed in ["namespace", "ticks", "boot", "legacy", "missing"] {
        let mut other = row.clone();
        match changed {
            "namespace" => other.pid_namespace = Some(identity.pidns_inode ^ 1),
            "ticks" => {
                let mut id = identity.clone();
                id.starttime += 1;
                other.pid_start = Some(id.legacy().start);
            }
            "boot" => {
                let mut id = identity.clone();
                id.boot_id = "00000000-0000-4000-8000-000000000001".into();
                other.pid_start = Some(id.legacy().start);
            }
            "legacy" => other.pid_start = Some(identity.legacy_ticks_start()),
            _ => other.pid_namespace = None,
        }
        assert!(!same_registry_process(&row, &other), "{changed}");
        row.mail_wake = None;
        other.mail_wake = Some(MailWake::Armed);
        carry_concurrent_launcher_write(&mut row, None, Some(&other));
        assert_eq!(
            row.mail_wake,
            if changed == "legacy" {
                Some(MailWake::Armed)
            } else {
                None
            },
            "{changed}: inherited successor's wake"
        );
    }
    let mut reused = row.clone();
    let mut id = identity.clone();
    id.starttime += 1;
    reused.pid_start = Some(id.legacy().start);
    assert!(matches!(
        entry_process_verdict(&reused),
        Verdict::Dead(proc_identity::DeadReason::Reused)
    ));
    let mut foreign = row.clone();
    foreign.pid_namespace = Some(identity.pidns_inode ^ 1);
    assert!(matches!(
        entry_process_verdict(&foreign),
        Verdict::ForeignUnknown(_)
    ));
    assert!(ensure_local_replacement(&foreign, Some(&identity)).is_err());
    let mut legacy = row.clone();
    legacy.pid_start = None;
    assert_eq!(
        observed_liveness(root.path(), &legacy, Some(identity.pidns_inode)),
        None
    );
    assert!(
        registry_entry_is_live(root.path(), &legacy),
        "unknown retains claims"
    );
    write_entry(root.path(), &foreign).unwrap();
    assert!(end_entry(root.path(), row.clone()).is_err());
    assert!(set_mail_wake_on_row(root.path(), &row, MailWake::Armed).is_err());
    write_entry(root.path(), &row).unwrap();
    assert_eq!(
        set_mail_wake_on_row(root.path(), &row, MailWake::Armed)
            .unwrap()
            .mail_wake,
        Some(MailWake::Armed)
    );
}

fn entry_with(pid: u32, last_active_at: DateTime<Utc>) -> AgentRegistryEntry {
    let identity = ProcIdentity::capture(pid).unwrap_or_else(|_| {
        let mut identity = ProcIdentity::capture(std::process::id()).unwrap();
        identity.pid = pid;
        identity.starttime = 0;
        identity
    });
    AgentRegistryEntry {
        id: agent_id("codex", pid),
        agent_type: "codex".to_string(),
        pid,
        pid_start: Some(identity.legacy().start),
        pid_namespace: Some(identity.pidns_inode),
        name: None,
        description: None,
        tty: Some("/dev/pts/1".to_string()),
        terminal: None,
        started_at: last_active_at,
        last_active_at,
        role: Some("implementer".to_string()),
        seat: None,
        current_spec: None,
        worktree_path: PathBuf::from("/tmp/aida-story"),
        source: "mcp".to_string(),
        binary_version: Some("0.9.1".to_string()),
        build_sha: Some("abc123".to_string()),
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        grant_key: None,
        delegate_seats: Vec::new(),
        resumed_from: None,
    }
}

// STORY-1420: a named seat is live only while an un-ended entry with a
// live pid carries its name; a dead pid or an `ended_at` reads as exited;
// a name the registry never saw is unknown.
// trace:STORY-1420 | ai:claude
#[test]
fn named_agent_liveness_reads_dead_pid_and_ended_at_as_exited() {
    let tmp = TempDir::new().unwrap();
    let mut live = entry_with(std::process::id(), Utc::now());
    live.id = "live".to_string();
    live.name = Some("Reviewer-Live".to_string());
    write_entry(tmp.path(), &live).unwrap();

    let mut dead = entry_with((i32::MAX as u32) - 1, Utc::now());
    dead.id = "dead".to_string();
    dead.name = Some("reviewer-dead".to_string());
    write_entry(tmp.path(), &dead).unwrap();

    let mut ended = entry_with(std::process::id(), Utc::now());
    ended.id = "ended".to_string();
    ended.name = Some("reviewer-ended".to_string());
    ended.ended_at = Some(Utc::now());
    write_entry(tmp.path(), &ended).unwrap();

    assert_eq!(
        named_agent_liveness(tmp.path(), "reviewer-live"),
        Some(true)
    );
    assert_eq!(
        named_agent_liveness(tmp.path(), "reviewer-dead"),
        Some(false)
    );
    assert_eq!(
        named_agent_liveness(tmp.path(), "reviewer-ended"),
        Some(false)
    );
    assert_eq!(named_agent_liveness(tmp.path(), "never-registered"), None);
}

#[test]
fn terminal_identity_captures_known_emulator_env() {
    let terminator = terminal_identity_from_env(
        Some("/dev/pts/4".to_string()),
        vec![("TERMINATOR_UUID", "term-1")],
    )
    .unwrap();
    assert_eq!(terminator.emulator.as_deref(), Some("terminator"));
    assert_eq!(terminator.terminator_uuid.as_deref(), Some("term-1"));

    let tmux = terminal_identity_from_env(
        Some("/dev/pts/4".to_string()),
        vec![("TMUX_PANE", "%3"), ("TMUX", "/tmp/tmux.sock,1,0")],
    )
    .unwrap();
    assert_eq!(tmux.emulator.as_deref(), Some("tmux"));
    assert_eq!(tmux.tmux_pane.as_deref(), Some("%3"));
    assert_eq!(tmux.tmux_socket.as_deref(), Some("/tmp/tmux.sock"));

    for (vars, emulator) in [
        (vec![("WEZTERM_PANE", "12")], "wezterm"),
        (vec![("KITTY_WINDOW_ID", "9")], "kitty"),
        (vec![("WINDOWID", "123")], "gnome-terminal"),
    ] {
        let ident = terminal_identity_from_env(Some("/dev/pts/4".to_string()), vars).unwrap();
        assert_eq!(ident.tty.as_deref(), Some("/dev/pts/4"));
        assert_eq!(ident.emulator.as_deref(), Some(emulator));
        match emulator {
            "wezterm" => assert_eq!(ident.wezterm_pane.as_deref(), Some("12")),
            "kitty" => assert_eq!(ident.kitty_window_id.as_deref(), Some("9")),
            "gnome-terminal" => assert_eq!(ident.windowid.as_deref(), Some("123")),
            _ => unreachable!(),
        }
    }
}

#[test]
fn terminal_identity_with_missing_env_is_only_tty() {
    let ident =
        terminal_identity_from_env(Some("/dev/pts/9".to_string()), Vec::<(&str, &str)>::new())
            .unwrap();
    assert_eq!(ident.tty.as_deref(), Some("/dev/pts/9"));
    assert_eq!(ident.emulator, None);
    assert_eq!(ident.terminator_uuid, None);
    assert_eq!(ident.tmux_pane, None);
    assert_eq!(ident.wezterm_pane, None);
    assert_eq!(ident.windowid, None);
    assert_eq!(ident.kitty_window_id, None);
}

#[test]
fn launch_title_includes_role_optional_spec_and_short_id() {
    assert_eq!(
        launch_title("implementer", Some("STORY-993"), "01a079fe-dead-beef"),
        "aida · implementer · STORY-993 · 01a079fe"
    );
    assert_eq!(
        launch_title("advisor", None, "abcdef012345"),
        "aida · advisor · abcdef01"
    );
}

#[test]
fn native_restore_title_command_restores_supported_emulators() {
    let previous = Some("previous title".to_string());

    let tmux = terminal_identity_from_env(
        Some("/dev/pts/4".to_string()),
        vec![("TMUX_PANE", "%3"), ("TMUX", "/tmp/tmux.sock,1,0")],
    )
    .unwrap();
    assert_eq!(
        native_restore_title_command(&tmux, &previous),
        Some((
            "tmux".to_string(),
            vec!["rename-window".to_string(), "previous title".to_string()]
        ))
    );

    let wezterm =
        terminal_identity_from_env(Some("/dev/pts/4".to_string()), vec![("WEZTERM_PANE", "12")])
            .unwrap();
    assert_eq!(
        native_restore_title_command(&wezterm, &previous),
        Some((
            "wezterm".to_string(),
            vec![
                "cli".to_string(),
                "set-tab-title".to_string(),
                "previous title".to_string(),
                "--pane-id".to_string(),
                "12".to_string()
            ]
        ))
    );

    let terminator = terminal_identity_from_env(
        Some("/dev/pts/4".to_string()),
        vec![("TERMINATOR_UUID", "term-994")],
    )
    .unwrap();
    assert_eq!(
        native_restore_title_command(&terminator, &previous),
        Some((
            "remotinator".to_string(),
            vec![
                "set_tab_title".to_string(),
                "term-994".to_string(),
                "previous title".to_string()
            ]
        ))
    );

    assert_eq!(native_restore_title_command(&tmux, &None), None);
}

// `cmdline_matches_agent` reads /proc-style cmdlines and is unix-gated.
#[cfg(unix)]
#[test]
fn session_start_cmdline_match_detects_agent_binary() {
    assert!(cmdline_matches_agent(
        "/home/joe/.local/bin/codex --cd /repo",
        "codex"
    ));
    assert!(cmdline_matches_agent(
        "/usr/bin/node /usr/local/bin/claude",
        "claude"
    ));
    assert!(cmdline_matches_agent(
        "node /usr/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        "claude"
    ));
    assert!(cmdline_matches_agent("gemini --model pro", "antigravity"));
    // trace:TASK-2272 | ai:claude
    assert!(cmdline_matches_agent(
        "/home/joe/.local/bin/agy --resume",
        "antigravity"
    ));
    assert!(!cmdline_matches_agent(
        "aida awaiting --notice",
        "antigravity"
    ));
    assert!(!cmdline_matches_agent(
        "bash .claude/hooks/aida-mail-notice.sh",
        "claude"
    ));
}

// trace:STORY-1843.ac0eef56 | ai:codex
#[cfg(unix)]
#[test]
fn story_1843_pr3_hook_arguments_do_not_identify_a_client() {
    let tree: std::collections::HashMap<u32, (u32, &str)> = [
            (400, (300, "aida awaiting --notice")),
            (300, (200, "sh /repo/.agents/hooks/aida-mail-notice.sh --client agy --event SessionStart")),
            (200, (100, "sh -c h=/repo/.agents/hooks/aida-mail-notice.sh; $h --client agy --event SessionStart")),
            (100, (1, "/home/joe/.local/bin/agy --prompt claude")),
            (1, (0, "/sbin/init")),
        ].into_iter().collect();
    assert_eq!(
        session_start_agent_pid_in(
            400,
            "antigravity",
            |pid| tree.get(&pid).map(|(p, _)| *p),
            |pid| tree.get(&pid).map(|(_, command)| command.to_string())
        ),
        Some(100)
    );
    assert!(!cmdline_matches_agent("agy --prompt claude", "claude"));
    assert!(!cmdline_matches_agent(
        "python3 /fixture/claude agy",
        "antigravity"
    ));
    assert!(cmdline_matches_agent("python3 /fixture/agy", "antigravity"));
}

// TASK-2272 (TASK-2282, TASK-1-463): the Antigravity SessionStart hook
// runs as `aida awaiting --notice` under a hook shell under `agy`. The
// registry row must name the `agy` process, not the short-lived hook.
// trace:TASK-2272 | ai:claude
#[cfg(unix)]
#[test]
fn session_start_agent_pid_finds_the_agy_ancestor_of_the_hook() {
    let tree: std::collections::HashMap<u32, (u32, &str)> = [
        (300, (200, "aida awaiting --notice")),
        (200, (100, "sh -c aida awaiting --notice")),
        (100, (1, "/home/joe/.local/bin/agy")),
        (1, (0, "/sbin/init")),
    ]
    .into_iter()
    .collect();
    let parent_of = |pid: u32| tree.get(&pid).map(|(p, _)| *p);
    let cmdline_of = |pid: u32| tree.get(&pid).map(|(_, c)| c.to_string());
    assert_eq!(
        session_start_agent_pid_in(300, "antigravity", parent_of, cmdline_of),
        Some(100)
    );
    // No agent ancestor: None, so the caller falls back to the hook pid.
    assert_eq!(
        session_start_agent_pid_in(300, "codex", parent_of, cmdline_of),
        None
    );
}

#[test]
fn session_start_hook_touch_writes_terminal_block() {
    let tmp = TempDir::new().unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_AGENT_TYPE", "codex"),
        ("AIDA_SESSION_ROLE", "advisor"),
        ("AIDA_SESSION_SCOPE", "STORY-994"),
        ("TERMINATOR_UUID", "term-994"),
    ]);

    let binary = AgentBinaryIdentity::new("0.14.0".into(), "abc123".into());
    let entry =
        touch_session_start_agent(tmp.path(), tmp.path(), Some("claude-sid"), &binary).unwrap();

    assert_eq!(entry.agent_type, "codex");
    assert_eq!(entry.role.as_deref(), Some("advisor"));
    assert_eq!(entry.current_spec.as_deref(), Some("STORY-994"));
    assert_eq!(entry.claude_session_id.as_deref(), Some("claude-sid"));
    let terminal = entry.terminal.as_ref().expect("terminal block");
    assert_eq!(terminal.emulator.as_deref(), Some("terminator"));
    assert_eq!(terminal.terminator_uuid.as_deref(), Some("term-994"));
}

/// Write lease `id` (scope `scope`, worktree `worktree`) and its
/// manifest linking Claude session `sid`, as `agent new --bg` does.
// trace:TASK-1955 | ai:claude (BUG-2167)
fn bug_2167_linked_lease(root: &Path, id: &str, scope: &str, worktree: &Path, sid: &str) {
    let dir = root.join(".aida").join("sessions");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
            dir.join(format!("{id}.toml")),
            format!(
                "id = \"{id}\"\nscope = \"{scope}\"\nslug = \"s\"\nowner = \"t\"\nworktree_path = '{}'\nbranch = \"b\"\nstarted_at = \"2026-10-20T00:00:00Z\"\nhostname = \"h\"\nrole = \"implementer\"\n",
                worktree.display()
            ),
        )
        .unwrap();
    std::fs::write(
            dir.join(format!("{id}.manifest.toml")),
            format!(
                "session_id = \"{id}\"\nplanned_at = \"2026-10-20T00:00:00Z\"\nplan_source = \"agent new --bg\"\nclaude_session_id = \"{sid}\"\nitems = []\n"
            ),
        )
        .unwrap();
}

/// BUG-2167: a `--bg` session in a recycled spare carrying an earlier
/// launch's env is registered from the lease linked to its own session
/// id (spec, role, worktree), and the stale env name is not used.
// trace:TASK-1955 | ai:claude (BUG-2167)
#[test]
fn bg_session_start_uses_the_linked_lease_not_the_spares_stale_env() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("pool-7");
    bug_2167_linked_lease(tmp.path(), "leaseb00000001", "TASK-2000", &pool, "abcd1234");
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_AGENT_TYPE", "claude"),
        ("AIDA_AGENT_NAME", "fix-task-1784"),
        ("AIDA_SESSION_SCOPE", "TASK-1784"),
    ]);
    let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
    let entry = touch_session_start_agent(
        tmp.path(),
        tmp.path(),
        Some("abcd1234-0000-4000-8000-000000000000"),
        &binary,
    )
    .unwrap();
    assert_eq!(entry.current_spec.as_deref(), Some("TASK-2000"));
    assert_eq!(entry.worktree_path, pool);
    assert_eq!(entry.role.as_deref(), Some("implementer"));
    assert_ne!(entry.name.as_deref(), Some("fix-task-1784"));

    // The launcher then names it from its own plan.
    let n = adopt_bg_session_identity(
        tmp.path(),
        "abcd1234",
        "rebase-2673",
        Some("TASK-2000"),
        Some("implementer"),
        None,
        &pool,
    );
    assert_eq!(n, 1);
    let views: Vec<_> = load_entries(tmp.path())
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].name.as_deref(), Some("rebase-2673"));
}

/// TASK-2102: a SessionStart hook that fires AFTER `link_bg_session`
/// saved the manifest names the row from the launch, even when the
/// spare's env contradicts the lease and its pid already had an entry
/// with this start identity carrying a stale name.
// trace:TASK-1955 | ai:claude (TASK-2102)
#[test]
fn bg_session_start_after_the_link_takes_the_launch_name() {
    let tmp = TempDir::new().unwrap();
    let pool = tmp.path().join("pool-8");
    bug_2167_linked_lease(tmp.path(), "leaseb00000002", "TASK-2001", &pool, "feed5678");
    let manifest = tmp
        .path()
        .join(".aida")
        .join("sessions")
        .join("leaseb00000002.manifest.toml");
    let body = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        body.replace("items = []", "agent_name = \"rebase-2678\"\nitems = []"),
    )
    .unwrap();
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_AGENT_TYPE", "claude"),
        ("AIDA_AGENT_NAME", "fix-task-1784"),
        ("AIDA_SESSION_SCOPE", "TASK-1784"),
    ]);
    let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
    let sid = "feed5678-0000-4000-8000-000000000000";
    // The spare's first session already left a same-start entry named
    // from its stale env.
    let first = touch_session_start_agent(tmp.path(), tmp.path(), Some(sid), &binary).unwrap();
    let mut stale = first.clone();
    stale.name = Some("fix-task-1784".into());
    write_entry(tmp.path(), &stale).unwrap();

    let entry = touch_session_start_agent(tmp.path(), tmp.path(), Some(sid), &binary).unwrap();
    assert_eq!(entry.name.as_deref(), Some("rebase-2678"));
    assert_eq!(entry.current_spec.as_deref(), Some("TASK-2001"));
    assert_eq!(first.name.as_deref(), Some("rebase-2678"));
}

/// BUG-2167: a pid's entry left by an earlier session never lends its
/// name to the next session on that pid; an unlinked hook records its
/// own worktree, not the main checkout.
// trace:TASK-1955 | ai:claude (BUG-2167)
#[test]
fn recycled_pid_session_start_does_not_inherit_the_earlier_name() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("wt");
    let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
    {
        let _env = crate::test_env::EnvVarsGuard::set(&[
            ("AIDA_AGENT_TYPE", "codex"),
            ("AIDA_AGENT_NAME", "first-seat"),
        ]);
        let mut old =
            touch_session_start_agent(tmp.path(), &worktree, Some("sid-one"), &binary).unwrap();
        // The entry an earlier process with this pid left behind.
        old.pid_start = Some("linux-starttime:1".into());
        write_entry(tmp.path(), &old).unwrap();
    }
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_AGENT_TYPE", "codex"),
        ("AIDA_AGENT_NAME", "second-seat"),
    ]);
    let entry = touch_session_start_agent(tmp.path(), &worktree, Some("sid-two"), &binary).unwrap();
    assert_eq!(entry.name.as_deref(), Some("second-seat"));
    assert_eq!(entry.worktree_path, worktree);
}

/// PR-2809 r1: the same live process starting a new session (`/clear`,
/// a new conversation) keeps its `agent new` launcher entry.
// trace:TASK-1955 | ai:claude (BUG-2167)
#[test]
fn same_process_new_session_keeps_its_launcher_entry() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("wt");
    let binary = AgentBinaryIdentity::new("0.16.0".into(), "abc".into());
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_AGENT_TYPE", "codex"),
        ("AIDA_AGENT_NAME", "lane-7"),
    ]);
    let mut launched =
        touch_session_start_agent(tmp.path(), &worktree, Some("sid-one"), &binary).unwrap();
    launched.source = "agent-launcher".into();
    launched.description = Some("lane seven".into());
    write_entry(tmp.path(), &launched).unwrap();

    let entry = touch_session_start_agent(tmp.path(), &worktree, Some("sid-two"), &binary).unwrap();
    assert_eq!(entry.source, "agent-launcher");
    assert_eq!(entry.name.as_deref(), Some("lane-7"));
    assert_eq!(entry.description.as_deref(), Some("lane seven"));
    assert_eq!(entry.claude_session_id.as_deref(), Some("sid-two"));
}

// trace:STORY-994 | ai:codex
#[test]
fn role_enter_registration_writes_terminal_block() {
    let tmp = TempDir::new().unwrap();
    let worktree = tmp.path().join("worktree");
    let _env = crate::test_env::EnvVarsGuard::set(&[
        ("AIDA_SESSION_SCOPE", "STORY-994"),
        ("TERMINATOR_UUID", "role-term-994"),
    ]);

    let entry = register_role_enter_agent(tmp.path(), "advisor", worktree.clone()).unwrap();

    assert_eq!(entry.agent_type, "shell");
    assert_eq!(entry.source, "role-enter");
    assert_eq!(entry.role.as_deref(), Some("advisor"));
    assert_eq!(entry.current_spec.as_deref(), Some("STORY-994"));
    assert_eq!(entry.worktree_path, worktree);
    let terminal = entry.terminal.as_ref().expect("terminal block");
    assert_eq!(terminal.emulator.as_deref(), Some("terminator"));
    assert_eq!(terminal.terminator_uuid.as_deref(), Some("role-term-994"));

    let persisted = std::fs::read_to_string(registry_path(tmp.path(), &entry.id)).unwrap();
    assert!(
        persisted.contains("[terminal]"),
        "role-enter registry entry must persist a [terminal] block: {persisted}"
    );
    assert!(persisted.contains("terminator_uuid = \"role-term-994\""));
    assert!(persisted.contains("emulator = \"terminator\""));
}

// trace:BUG-1156 | ai:codex
#[cfg(unix)]
#[test]
fn role_enter_shell_liveness_requires_live_descendant_session() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let worktree = tmp.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();

    let shell_pid = std::process::id();
    let mut role_entry = entry_with(shell_pid, now);
    role_entry.id = agent_id("shell", shell_pid);
    role_entry.agent_type = "shell".to_string();
    role_entry.source = "role-enter".to_string();
    role_entry.name = Some("role:implementer".to_string());
    role_entry.worktree_path = worktree.clone();
    write_entry(tmp.path(), &role_entry).unwrap();

    let mut child = std::process::Command::new("sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("spawn child session stand-in");
    let child_pid = child.id();

    let mut session_entry = entry_with(child_pid, now);
    session_entry.id = agent_id("codex", child_pid);
    session_entry.source = "session-start-hook".to_string();
    session_entry.worktree_path = worktree;
    write_entry(tmp.path(), &session_entry).unwrap();

    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
    let shell = views
        .iter()
        .find(|view| view.id == role_entry.id)
        .expect("role-enter view");
    assert_ne!(shell.status, AgentStatus::Stale);

    child.kill().expect("kill child session stand-in");
    child.wait().expect("reap child session stand-in");

    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
    let shell = views
        .iter()
        .find(|view| view.id == role_entry.id)
        .expect("role-enter view");
    assert_eq!(shell.status, AgentStatus::Stale);
}

fn ctx(now: DateTime<Utc>, threshold_secs: u64, leases: Vec<PathBuf>) -> AgentClassifyContext {
    AgentClassifyContext::new(now, threshold_secs, leases)
}

#[test]
fn registry_path_lives_under_aida_agents() {
    let root = Path::new("/tmp/project");
    assert_eq!(
        registry_path(root, "codex-123"),
        PathBuf::from("/tmp/project/.aida/agents/codex-123.toml")
    );
}

// STORY-435: dead pid wins over every other signal.
#[test]
fn classify_status_dead_pid_is_stale() {
    let now = Utc::now();
    let e = entry_with(42, now);
    let c = ctx(now, 30, vec![PathBuf::from("/tmp/aida-story")]);
    assert_eq!(classify_status(&e, false, &c), AgentStatus::Stale);
}

// STORY-435: fresh activity (within threshold) → Busy.
/// BUG-1701 AC5: the CPU figure that distinguishes a finished seat from a working one.
// trace:BUG-1701 | ai:claude
#[test]
fn parse_ps_cpu_time_covers_every_ps_shape() {
    assert_eq!(parse_ps_cpu_time("00:00"), Some(0));
    // The observed stranded seats: 12s and 46s of CPU across hours.
    assert_eq!(parse_ps_cpu_time("00:12"), Some(12));
    assert_eq!(parse_ps_cpu_time("00:00:46"), Some(46));
    assert_eq!(parse_ps_cpu_time("01:02:03"), Some(3723));
    assert_eq!(parse_ps_cpu_time(" 2-03:04:05 "), Some(183_845));
    // Unavailable or unparseable stays None rather than guessing zero, which would read as
    // "definitely idle".
    assert_eq!(parse_ps_cpu_time(""), None);
    assert_eq!(parse_ps_cpu_time("   "), None);
    assert_eq!(parse_ps_cpu_time("not-a-time"), None);
    assert_eq!(parse_ps_cpu_time("1:2:3:4"), None);
}

#[test]
fn classify_status_fresh_activity_is_busy() {
    let now = Utc::now();
    let e = entry_with(42, now - Duration::seconds(5));
    let c = ctx(now, 30, vec![]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
}

// STORY-435: stale activity + no lease → Idle.
#[test]
fn classify_status_stale_activity_is_idle() {
    let now = Utc::now();
    let e = entry_with(42, now - Duration::minutes(5));
    let c = ctx(now, 30, vec![]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Idle);
}

// trace:STORY-790 | ai:codex
#[test]
fn ended_resumable_agent_resolves_by_name_or_spec() {
    let tmp = TempDir::new().unwrap();
    let dead_pid = 4_294_967_294u32;
    let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    register_spawned_agent(
        tmp.path(),
        "claude",
        dead_pid,
        Some("advisor".into()),
        Some("STORY-790".into()),
        tmp.path().into(),
        Some(&binary),
        Some("claude-advisor-1".into()),
        Some("018f-session".into()),
        None,
        None,
    )
    .unwrap();
    mark_agent_ended(tmp.path(), "claude", dead_pid).unwrap();

    let by_name = resolve_resumable_ended_agent(tmp.path(), "claude-advisor-1").unwrap();
    assert_eq!(by_name.native_session_id.as_deref(), Some("018f-session"));
    assert!(by_name.ended_at.is_some());

    let by_spec = resolve_resumable_ended_agent(tmp.path(), "STORY-790").unwrap();
    assert_eq!(by_spec.id, by_name.id);

    let views =
        ended_resumable_agent_views(tmp.path(), &ctx(Utc::now(), 30, vec![tmp.path().into()]));
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].native_session_id.as_deref(), Some("018f-session"));
}

/// TASK-1750: an entry whose live pid carries a different recorded start
/// time is a recycled pid and reads dead; the matching start time (and a
/// legacy entry with none) reads live.
// trace:TASK-1750 | ai:claude
#[test]
fn recycled_pid_entry_is_not_live() {
    let tmp = tempfile::tempdir().unwrap();
    let mut e = entry_with(std::process::id(), Utc::now());
    assert!(registry_entry_is_live(tmp.path(), &e));
    let mut changed = e.process_identity().unwrap();
    changed.starttime += 1;
    e.pid_start = Some(changed.legacy().start);
    assert!(!registry_entry_is_live(tmp.path(), &e));
    e.pid_start = Some("2000-01-01T00:00:00+00:00".into());
    assert!(
        registry_entry_is_live(tmp.path(), &e),
        "legacy identity remains unknown"
    );
    assert_eq!(
        observed_liveness(tmp.path(), &e, current_pid_namespace()),
        None
    );
}

// trace:STORY-790 | ai:codex
#[test]
fn ended_marker_wins_over_live_pid_probe() {
    let now = Utc::now();
    let mut e = entry_with(std::process::id(), now);
    e.ended_at = Some(now);
    e.native_session_id = Some("session-id".to_string());
    let c = ctx(now, 30, vec![]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Stale);
}

// STORY-435: stale activity but a Live lease covers the worktree → Busy.
//
// BUG-1727 AC5: pinned in the shape production uses. The only caller that passes a
// non-empty lease list also attaches a work probe (`lib.rs` builds the context with
// `.with_work_probe(..)`), so a `proc_tree: None` context can never reach the lease arm
// in the CLI: every other call site passes `Vec::new()` and `covers` is false for an
// empty list. Asserting STORY-435 through `Some(..)` keeps this test on the live path.
// The `None` fallback arm is covered by `classify_status_live_lease_without_proc_tree_is_busy`.
// trace:STORY-435 | ai:claude
#[test]
fn classify_status_live_lease_is_busy_despite_stale_activity() {
    let now = Utc::now();
    let e = entry_with(42, now - Duration::minutes(5));
    // The lease path comes from the entry itself, so no tmp-path literal is duplicated.
    let c = ctx(now, 30, vec![e.worktree_path.clone()]).with_work_probe(bug1704_tree(120), 10);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
}

// trace:BUG-1704 | ai:codex
#[test]
fn ages_show_work_counts_a_descendant_inside_the_window() {
    assert!(ages_show_work(&[120], 600, 10));
}

// trace:BUG-1704 | ai:codex
#[test]
fn ages_show_work_ignores_session_long_service_children() {
    assert!(!ages_show_work(&[3600], 600, 10));
}

// trace:BUG-1704 | ai:codex
#[test]
fn ages_show_work_ignores_momentary_hook_children() {
    assert!(!ages_show_work(&[2], 600, 10));
}

// trace:BUG-1704 | ai:codex
#[test]
fn ages_show_work_is_false_when_grace_window_is_empty() {
    assert!(!ages_show_work(&[7], 10, 10));
    assert!(!ages_show_work(&[7], 5, 10));
}

// trace:BUG-1704 | ai:codex
#[test]
fn ages_show_work_is_time_invariant_for_a_child_started_after_the_heartbeat() {
    for (secs_since_heartbeat, age) in [
        (120, 110),
        (1_200, 1_190),
        (36_000, 35_990),
        (360_000, 359_990),
    ] {
        assert!(ages_show_work(&[age], secs_since_heartbeat, 10));
    }

    // This is BUG-1726's shape: heartbeat after child start excludes it, intentionally.
    for (secs_since_heartbeat, age) in [(600, 601), (36_000, 36_001)] {
        assert!(!ages_show_work(&[age], secs_since_heartbeat, 10));
    }
}

// trace:BUG-1704 | ai:codex
#[test]
fn seat_has_own_work_is_true_when_seat_pid_is_absent_from_snapshot() {
    assert!(seat_has_own_work(
        &ProcTree::from_nodes(vec![]),
        42,
        600,
        10
    ));
}

/// BUG-1727 AC3: the grace floor's cost, pinned rather than assumed. A child younger
/// than `grace_secs` is not counted, so the seat reads Idle while that child runs —
/// and the window closes on its own. Both halves use the SAME heartbeat staleness and
/// the SAME child; only its age advances, which is what makes this a self-heal and not
/// two unrelated cases.
// trace:BUG-1727 | ai:claude
#[test]
fn ages_show_work_self_heals_when_a_young_child_reaches_the_grace_floor() {
    assert!(!ages_show_work(&[5], 31, 10));
    assert!(ages_show_work(&[10], 31, 10));
}

/// BUG-1727 AC1 + AC2: a live descendant the probe could not age reads Busy, the same
/// way a missing root does. Without the unsampled count this tree yields an empty age
/// list, indistinguishable from a seat with no children at all, and reads Idle.
// trace:BUG-1727 | ai:claude
#[test]
fn seat_has_own_work_is_true_when_a_descendant_cannot_be_aged() {
    let tree = ProcTree::from_nodes(vec![
        aida_core::liveness::ProcNode {
            pid: 42,
            ppid: None,
            age_secs: Some(700),
        },
        aida_core::liveness::ProcNode {
            pid: 50,
            ppid: Some(42),
            age_secs: None,
        },
    ]);
    assert!(seat_has_own_work(&tree, 42, 600, 10));
}

/// …and the fail-safe stays narrow: a fully sampled subtree with nothing in the work
/// window still reads Idle, so AC1 did not turn every covered seat Busy.
// trace:BUG-1727 | ai:claude
#[test]
fn seat_has_own_work_is_false_when_a_sampled_subtree_shows_no_work() {
    assert!(!seat_has_own_work(&bug1704_tree(3600), 42, 600, 10));
}

// trace:BUG-1704 | ai:codex
fn bug1704_tree(child_age: i64) -> ProcTree {
    ProcTree::from_nodes(vec![
        aida_core::liveness::ProcNode {
            pid: 42,
            ppid: None,
            age_secs: Some(700),
        },
        aida_core::liveness::ProcNode {
            pid: 50,
            ppid: Some(42),
            age_secs: Some(700),
        },
        aida_core::liveness::ProcNode {
            pid: 51,
            ppid: Some(50),
            age_secs: Some(child_age),
        },
    ])
}

// trace:BUG-1704 | ai:codex
#[test]
fn classify_status_live_lease_without_proc_tree_is_busy() {
    let now = Utc::now();
    let entry = entry_with(42, now - Duration::minutes(5));
    // The lease path comes from the entry itself: the rule under test is "a live lease
    // covers this seat's worktree", so duplicating a literal here would add a non-portable
    // tmp path for no gain. trace:BUG-1704 | ai:claude
    let ctx = ctx(now, 30, vec![entry.worktree_path.clone()]);
    assert_eq!(classify_status(&entry, true, &ctx), AgentStatus::Busy);
}

// trace:BUG-1704 | ai:codex
#[test]
fn classify_status_live_lease_with_quiet_subtree_is_idle() {
    let now = Utc::now();
    let entry = entry_with(42, now - Duration::minutes(5));
    let ctx =
        ctx(now, 30, vec![entry.worktree_path.clone()]).with_work_probe(bug1704_tree(3600), 10);
    assert_eq!(classify_status(&entry, true, &ctx), AgentStatus::Idle);
}

// STORY-435: lease worktree that's an ancestor of the agent worktree
// also counts (operator working in a subdir of the lease's root).
#[test]
fn classify_status_live_lease_covers_ancestor() {
    let now = Utc::now();
    let mut e = entry_with(42, now - Duration::minutes(5));
    e.worktree_path = PathBuf::from("/tmp/aida-story/subdir");
    let c = ctx(now, 30, vec![PathBuf::from("/tmp/aida-story")]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
}

// STORY-435: empty lease worktree (advisory lock) must NOT match every
// agent — mirrors TASK-474's lease_covers_cwd short-circuit.
#[test]
fn classify_status_ignores_empty_lease_worktree() {
    let now = Utc::now();
    let e = entry_with(42, now - Duration::minutes(5));
    let c = ctx(now, 30, vec![PathBuf::from("")]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Idle);
}

// STORY-435: a clock that ticked backwards (NTP step, suspend/resume)
// must not prematurely flip a fresh entry to Idle.
#[test]
fn classify_status_clamps_negative_elapsed() {
    let now = Utc::now();
    let e = entry_with(42, now + Duration::seconds(1));
    let c = ctx(now, 30, vec![]);
    assert_eq!(classify_status(&e, true, &c), AgentStatus::Busy);
}

// STORY-435: spec-mandated regression — three agents with different
// activity patterns each classify correctly off the same context.
#[test]
fn three_agents_with_different_activity_patterns_classify_correctly() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();

    // (1) live pid + fresh activity → Busy.
    let mut busy = entry_with(std::process::id(), now - Duration::seconds(2));
    busy.id = agent_id("codex", busy.pid);
    write_entry(tmp.path(), &busy).unwrap();

    // (2) live pid + stale activity + no lease → Idle.
    let mut idle = entry_with(std::process::id(), now - Duration::minutes(5));
    idle.agent_type = "claude".to_string();
    idle.id = agent_id("claude", idle.pid);
    write_entry(tmp.path(), &idle).unwrap();

    // (3) dead pid → Stale (regardless of last_active_at).
    let mut stale = entry_with((i32::MAX as u32) - 1, now - Duration::seconds(1));
    stale.agent_type = "antigravity".to_string();
    stale.id = agent_id("antigravity", stale.pid);
    write_entry(tmp.path(), &stale).unwrap();

    let c = ctx(now, 30, vec![]);
    let views = list_agent_views(tmp.path(), &c);
    assert_eq!(views.len(), 3);

    // Sorted alphabetically by agent_type: antigravity, claude, codex.
    let by_type: std::collections::HashMap<_, _> = views
        .iter()
        .map(|v| (v.agent_type.as_str(), v.status))
        .collect();
    assert_eq!(by_type["codex"], AgentStatus::Busy);
    assert_eq!(by_type["claude"], AgentStatus::Idle);
    assert_eq!(by_type["antigravity"], AgentStatus::Stale);
}

#[test]
fn list_agent_views_reads_toml_and_computes_stale_status() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let record = entry_with((i32::MAX as u32) - 1, now);
    write_entry(tmp.path(), &record).unwrap();

    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].agent_type, "codex");
    assert_eq!(views[0].status, AgentStatus::Stale);
}

// STORY-791: descriptions are durable registry metadata and render in the
// agent-status row instead of forcing operators to infer purpose from role.
#[test]
fn spawned_agent_description_roundtrips_to_view_and_status_line() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    let entry = register_spawned_agent(
        tmp.path(),
        "codex",
        std::process::id(),
        Some("advisor".to_string()),
        None,
        tmp.path().to_path_buf(),
        Some(&binary),
        Some("codex-advisor-1".to_string()),
        None,
        None,
        Some("repo triage".to_string()),
    )
    .unwrap();
    assert_eq!(entry.description.as_deref(), Some("repo triage"));

    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
    assert_eq!(views[0].description.as_deref(), Some("repo triage"));
    let lines = format_agent_status_lines(&views);
    assert!(lines[0].contains("process"));
    assert!(lines[0].contains("repo triage"));
}

// STORY-791: the singleton role set has a safe default but remains
// configurable under [agent_registry].
#[test]
fn config_parses_singleton_roles() {
    let cfg = Config::from_toml_str(
        "[agent_registry]\nsingleton_roles = \"advisor, product, integrator\"\n",
    );
    assert!(cfg.role_is_singleton("advisor"));
    assert!(cfg.role_is_singleton("product"));
    assert!(!cfg.role_is_singleton("implementer"));
}

// STORY-791: launches are singleton per role per derived scope. A repo-wide
// advisor conflicts with another advisor, while a different role does not.
#[test]
fn same_scope_conflict_detects_live_same_role_scope() {
    let tmp = TempDir::new().unwrap();
    let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    register_spawned_agent(
        tmp.path(),
        "codex",
        std::process::id(),
        Some("advisor".to_string()),
        None,
        tmp.path().to_path_buf(),
        Some(&binary),
        Some("codex-advisor-1".to_string()),
        None,
        None,
        Some("repo master advisor".to_string()),
    )
    .unwrap();
    let cfg = Config::default();

    let conflict = same_scope_conflict(tmp.path(), &cfg, Some("advisor"), None, tmp.path());
    assert!(conflict.is_some());
    assert!(same_scope_conflict(tmp.path(), &cfg, Some("reviewer"), None, tmp.path()).is_none());
}

// STORY-791: spec-scoped doer roles conflict only on the same spec scope.
#[test]
fn same_scope_conflict_is_spec_scoped_for_implementers() {
    let tmp = TempDir::new().unwrap();
    let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    register_spawned_agent(
        tmp.path(),
        "codex",
        std::process::id(),
        Some("implementer".to_string()),
        Some("TASK-1".to_string()),
        tmp.path().join("task-1"),
        Some(&binary),
        Some("codex-implementer-1".to_string()),
        None,
        None,
        None,
    )
    .unwrap();
    let cfg = Config::default();

    assert!(same_scope_conflict(
        tmp.path(),
        &cfg,
        Some("implementer"),
        Some("TASK-1"),
        &tmp.path().join("other")
    )
    .is_some());
    assert!(same_scope_conflict(
        tmp.path(),
        &cfg,
        Some("implementer"),
        Some("TASK-2"),
        &tmp.path().join("task-2")
    )
    .is_none());
}

// TASK-121: generic implementers form a pool — several repo-scope members
// (any vendor) coexist, while a spec stays exclusive and singular roles
// keep their one-per-scope rule.
// trace:TASK-b6b1278890774ef993d2ebc327862470-121 | ai:claude
#[test]
fn same_scope_conflict_allows_a_pool_of_generic_implementers() {
    let tmp = TempDir::new().unwrap();
    let binary = AgentBinaryIdentity::new("0.9.1".into(), "abc123".into());
    for (vendor, role) in [("codex", "implementer"), ("claude", "reviewer")] {
        register_spawned_agent(
            tmp.path(),
            vendor,
            std::process::id(),
            Some(role.to_string()),
            None,
            tmp.path().to_path_buf(),
            Some(&binary),
            Some(format!("{vendor}-{role}-1")),
            None,
            None,
            None,
        )
        .unwrap();
    }
    let cfg = Config::default();

    // Another generic implementer (any vendor) launches beside the live one.
    assert!(same_scope_conflict(tmp.path(), &cfg, Some("implementer"), None, tmp.path()).is_none());
    assert!(
        same_scope_conflict(tmp.path(), &cfg, Some("Implementer"), Some(" "), tmp.path()).is_none()
    );
    // A singular role on the same scope is still refused.
    assert!(same_scope_conflict(tmp.path(), &cfg, Some("reviewer"), None, tmp.path()).is_some());
    assert!(is_pool_launch(Some("implementer"), None));
    assert!(!is_pool_launch(Some("implementer"), Some("TASK-1")));
    assert!(!is_pool_launch(Some("advisor"), None));
}

// TASK-1184: dead rows without a resumable native session are pruned.
#[test]
fn gc_dead_agents_removes_dead_non_resumable_entries() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let record = entry_with((i32::MAX as u32) - 1, now);
    let path = registry_path(tmp.path(), &record.id);
    write_entry(tmp.path(), &record).unwrap();

    let report = gc_dead_agents(tmp.path(), false, None).unwrap();

    assert_eq!(report.removed_count(), 1);
    assert!(report.retained.is_empty());
    assert!(!path.exists());
}

// TASK-1184: resumable rows are retained by default, but an explicit
// age threshold can prune old tombstones.
#[test]
fn gc_dead_agents_retains_resumable_until_older_than_matches() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut record = entry_with((i32::MAX as u32) - 1, now - Duration::days(10));
    record.native_session_id = Some("native-session-1".to_string());
    let path = registry_path(tmp.path(), &record.id);
    write_entry(tmp.path(), &record).unwrap();

    let retained = gc_dead_agents(tmp.path(), false, None).unwrap();
    assert!(retained.removed.is_empty());
    assert_eq!(retained.retained.len(), 1);
    assert!(path.exists());

    let removed = gc_dead_agents(tmp.path(), false, Some(7)).unwrap();
    assert_eq!(removed.removed_count(), 1);
    assert!(removed.retained.is_empty());
    assert!(!path.exists());
}

// STORY-1843 (TASK-2177): a dead resumable row stays for resume but is
// stamped ended, so a sandboxed mail sender stops counting it as a reader.
// trace:STORY-1843 | ai:claude
// trace:STORY-1843.ac97b025 | ai:claude
#[test]
fn gc_dead_agents_marks_dead_resumable_rows_ended_and_keeps_them() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut record = entry_with((i32::MAX as u32) - 1, now - Duration::minutes(5));
    record.native_session_id = Some("native-session-1".to_string());
    let path = registry_path(tmp.path(), &record.id);
    write_entry(tmp.path(), &record).unwrap();

    let preview = gc_dead_agents(tmp.path(), true, None).unwrap();
    assert_eq!(preview.marked_ended, 1);
    assert!(load_entries(tmp.path())[0].1.ended_at.is_none());

    let report = gc_dead_agents(tmp.path(), false, None).unwrap();
    assert_eq!(report.marked_ended, 1);
    assert_eq!(report.retained.len(), 1);
    assert!(path.exists());
    let ended = load_entries(tmp.path()).remove(0).1;
    assert!(ended.ended_at.is_some());
    assert_eq!(ended.native_session_id.as_deref(), Some("native-session-1"));
    assert_eq!(ended.last_active_at, record.last_active_at);

    assert_eq!(ended.ended_at, Some(record.last_active_at));

    let again = gc_dead_agents(tmp.path(), false, None).unwrap();
    assert_eq!(again.marked_ended, 0, "ending is idempotent");
}

// STORY-1843: a crash gc discovers late keeps its last-heartbeat end, so a
// session that ended recently stays first for resume-latest and the picker.
// trace:STORY-1843 | ai:claude
#[test]
fn gc_ended_old_crash_does_not_outrank_a_recent_end_for_resume() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut crashed = entry_with((i32::MAX as u32) - 1, now - Duration::days(3));
    crashed.native_session_id = Some("old-crash".to_string());
    write_entry(tmp.path(), &crashed).unwrap();
    let mut recent = entry_with((i32::MAX as u32) - 2, now - Duration::minutes(10));
    recent.native_session_id = Some("recent-end".to_string());
    recent.ended_at = Some(now - Duration::minutes(10));
    write_entry(tmp.path(), &recent).unwrap();

    let report = gc_dead_agents(tmp.path(), false, None).unwrap();
    assert_eq!(report.marked_ended, 1);

    let views =
        ended_resumable_agent_views(tmp.path(), &ctx(Utc::now(), 30, vec![tmp.path().into()]));
    let order: Vec<_> = views
        .iter()
        .map(|v| v.native_session_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(order, vec!["recent-end", "old-crash"]);
    assert_eq!(views[1].ended_at, Some(crashed.last_active_at));
}

// STORY-1843: gc in another PID namespace cannot see the row's process, so
// it must neither prune nor end it.
// trace:STORY-1843 | ai:claude
#[test]
fn gc_dead_agents_leaves_rows_from_unobservable_namespaces_untouched() {
    let Some(observer) = current_pid_namespace() else {
        return;
    };
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut resumable = entry_with((i32::MAX as u32) - 1, now);
    resumable.pid_namespace = Some(observer ^ 1);
    resumable.native_session_id = Some("native-session-1".to_string());
    write_entry(tmp.path(), &resumable).unwrap();
    let mut plain = entry_with((i32::MAX as u32) - 2, now);
    plain.pid_namespace = Some(observer ^ 1);
    write_entry(tmp.path(), &plain).unwrap();
    let before: Vec<_> = load_entries(tmp.path())
        .into_iter()
        .map(|(p, _)| std::fs::read(p).unwrap())
        .collect();

    let report = gc_dead_agents(tmp.path(), false, None).unwrap();

    assert_eq!(report, AgentGcReport::default());
    let after: Vec<_> = load_entries(tmp.path())
        .into_iter()
        .map(|(p, _)| std::fs::read(p).unwrap())
        .collect();
    assert_eq!(before, after);
}

fn probe(self_ns: u64, pid1_ns: Option<u64>, nspid: usize, comm: &str) -> PidNamespaceProbe {
    PidNamespaceProbe {
        self_ns: Some(self_ns),
        pid1_ns,
        nspid_levels: nspid,
        pid1_comm: Some(comm.to_string()),
    }
}

// BUG-2212: each signal alone marks the caller isolated; a host shell
// with systemd as pid 1 in its own namespace is not.
// trace:BUG-2212 | ai:claude
#[test]
fn pid_namespace_probe_detects_isolated_callers() {
    let host = INITIAL_PID_NAMESPACE;
    assert!(!probe(host, Some(host), 1, "systemd").is_isolated());
    assert!(!probe(host, None, 1, "init").is_isolated());
    assert!(probe(7, Some(host), 1, "systemd").is_isolated());
    assert!(probe(7, None, 2, "systemd").is_isolated());
    // bubblewrap: pid 1 unreadable from the sandbox user, comm is not init.
    assert!(probe(7, None, 1, "3").is_isolated());
    assert!(!PidNamespaceProbe::default().is_isolated());
}

// TASK-2272 (TASK-2281): a host whose pid 1 is runit/openrc/s6 shares its
// namespace with pid 1, so the inode comparison settles it and the name is
// never consulted. The name decides only when /proc/1/ns/pid is unreadable.
// trace:TASK-2272 | ai:claude
#[test]
fn pid_namespace_probe_prefers_the_namespace_inode_over_pid1_comm() {
    let host = INITIAL_PID_NAMESPACE;
    for comm in ["runit", "openrc-init", "s6-svscan", "my-custom-init"] {
        assert!(!probe(host, Some(host), 1, comm).is_isolated(), "{comm}");
    }
    for comm in ["runit", "openrc-init", "s6-svscan", "dumb-init", "tini"] {
        assert!(!probe(host, None, 1, comm).is_isolated(), "{comm}");
    }
    assert!(probe(host, None, 1, "bwrap").is_isolated());
    assert!(probe(7, Some(host), 1, "runit").is_isolated());
}

// BUG-2425: the mandate gate keeps a floor copy of the probe; it must
// judge isolation exactly as the shared aida-core judge does.
// trace:BUG-2425 | ai:claude
#[test]
fn gate_floor_probe_matches_the_core_isolation_judge() {
    let host = INITIAL_PID_NAMESPACE;
    for (self_ns, pid1_ns, nspid, comm) in [
        (host, Some(host), 1, "systemd"),
        (host, None, 1, "init"),
        (7, Some(host), 1, "systemd"),
        (7, None, 2, "systemd"),
        (7, None, 1, "3"),
        (host, None, 1, "bwrap"),
        (host, None, 1, "tini"),
        (7, Some(host), 1, "runit"),
    ] {
        let core = probe(self_ns, pid1_ns, nspid, comm);
        let gate = crate::gate::dispatch_deps::PidNamespaceProbe {
            self_ns: core.self_ns,
            pid1_ns: core.pid1_ns,
            nspid_levels: core.nspid_levels,
            pid1_comm: core.pid1_comm.clone(),
        };
        assert_eq!(gate.is_isolated(), core.is_isolated(), "{core:?}");
    }
}

// trace:BUG-2212 | ai:claude
#[test]
fn pid_observer_sees_only_its_own_namespace_and_unstamped_rows_from_the_host() {
    let host = PidObserver::from_probe(&probe(INITIAL_PID_NAMESPACE, None, 1, "systemd"));
    let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));
    assert!(host.can_observe(Some(INITIAL_PID_NAMESPACE)));
    assert!(host.can_observe(None));
    assert!(!host.can_observe(Some(7)));
    assert!(sandbox.can_observe(Some(7)));
    assert!(!sandbox.can_observe(Some(INITIAL_PID_NAMESPACE)));
    assert!(!sandbox.can_observe(None));
    // An isolated caller cannot vouch for unstamped rows even when its
    // namespace inode reads as the initial one.
    let masked = PidObserver {
        namespace: Some(INITIAL_PID_NAMESPACE),
        isolated: true,
    };
    assert!(!masked.can_observe(None));
}

// BUG-2212: from a sandboxed shell a live host session's pid is invisible.
// agent ls must list it on its fresh heartbeat, labelled as such, and only
// age it out of the DISPLAY listing once the heartbeat is past the window.
// trace:BUG-2212 | ai:claude
#[test]
fn list_agent_views_falls_back_to_heartbeat_for_unobservable_rows() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut fresh = entry_with((i32::MAX as u32) - 1, now - chrono::Duration::minutes(5));
    fresh.pid_namespace = current_pid_namespace();
    write_entry(tmp.path(), &fresh).unwrap();
    let mut old = entry_with((i32::MAX as u32) - 2, now - chrono::Duration::hours(25));
    old.pid_namespace = current_pid_namespace();
    write_entry(tmp.path(), &old).unwrap();
    let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));

    let views = list_agent_views(
        tmp.path(),
        &ctx(now, 30, vec![]).with_observer(sandbox).for_display(),
    );
    let by_pid: std::collections::HashMap<u32, &AgentRegistryView> =
        views.iter().map(|v| (v.pid, v)).collect();
    assert_eq!(by_pid[&((i32::MAX as u32) - 1)].status, AgentStatus::Idle);
    assert_eq!(
        by_pid[&((i32::MAX as u32) - 1)].liveness,
        LivenessBasis::Heartbeat
    );
    assert_eq!(by_pid[&((i32::MAX as u32) - 2)].status, AgentStatus::Stale);
    assert_eq!(
        by_pid[&((i32::MAX as u32) - 2)].liveness,
        LivenessBasis::Heartbeat
    );

    // From the host the same dead pids read Stale on process evidence.
    let host = PidObserver {
        namespace: current_pid_namespace(),
        isolated: false,
    };
    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]).with_observer(host));
    assert!(views
        .iter()
        .all(|v| v.status == AgentStatus::Stale && v.liveness == LivenessBasis::Process));
}

// TASK-2272 (TASK-2280): guards FAIL CLOSED. A session-start-hook row
// heartbeats exactly once, so from a sandboxed launcher every live host
// session's heartbeat goes arbitrarily old. A row whose pid this observer
// cannot check and that has no `ended_at` therefore counts as live
// (unverified) however old its heartbeat — the duplicate launch is
// refused. Only a PROVABLY dead row (ended_at, or a checkable dead pid)
// frees the seat.
// trace:TASK-2272 | ai:claude
#[test]
fn guards_fail_closed_on_unverifiable_rows_and_open_only_on_provable_death() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    // A host row whose only heartbeat is its session start, 17h ago.
    let mut unverifiable = entry_with((i32::MAX as u32) - 1, now - chrono::Duration::hours(17));
    unverifiable.pid_namespace = Some(current_pid_namespace().unwrap() ^ 1);
    unverifiable.role = Some("advisor".to_string());
    write_entry(tmp.path(), &unverifiable).unwrap();
    let sandbox = PidObserver {
        namespace: current_pid_namespace(),
        isolated: false,
    };

    let guard = list_agent_views(tmp.path(), &ctx(now, 30, vec![]).with_observer(sandbox));
    let row = guard
        .iter()
        .find(|v| v.pid == (i32::MAX as u32) - 1)
        .unwrap();
    assert_ne!(row.status, AgentStatus::Stale);
    assert_eq!(row.liveness, LivenessBasis::Heartbeat);
    let dup = crate::agent_launch::matching_live_duplicate(
        &guard,
        &unverifiable.agent_type,
        Some("advisor"),
        None,
    )
    .expect("an unverifiable host row refuses the duplicate launch");
    assert_eq!(dup.pid, (i32::MAX as u32) - 1);

    // An ended row is provably dead from any namespace: launch allowed.
    let mut ended = entry_with((i32::MAX as u32) - 2, now - chrono::Duration::minutes(1));
    ended.pid_namespace = Some(current_pid_namespace().unwrap() ^ 1);
    ended.role = Some("advisor".to_string());
    ended.ended_at = Some(now);
    write_entry(tmp.path(), &ended).unwrap();
    // A checkable dead pid (our own namespace) is provably dead too.
    let mut dead = entry_with((i32::MAX as u32) - 3, now - chrono::Duration::minutes(1));
    dead.pid_namespace = current_pid_namespace();
    dead.role = Some("advisor".to_string());
    write_entry(tmp.path(), &dead).unwrap();

    let sandbox_ctx = ctx(now, 30, vec![]).with_observer(sandbox);
    let provably_dead: Vec<_> = list_agent_views(tmp.path(), &sandbox_ctx)
        .into_iter()
        .filter(|v| v.pid != (i32::MAX as u32) - 1)
        .collect();
    assert!(provably_dead.iter().all(|v| v.status == AgentStatus::Stale));
    assert!(crate::agent_launch::matching_live_duplicate(
        &provably_dead,
        &unverifiable.agent_type,
        Some("advisor"),
        None,
    )
    .is_none());
}

// trace:BUG-2212 | ai:claude
#[test]
fn observer_liveness_treats_ended_rows_as_dead_from_any_namespace() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut ended = entry_with((i32::MAX as u32) - 1, now);
    ended.pid_namespace = Some(INITIAL_PID_NAMESPACE);
    ended.ended_at = Some(now);
    let sandbox = PidObserver::from_probe(&probe(7, None, 1, "bwrap"));
    assert_eq!(
        observer_liveness(tmp.path(), &ended, sandbox, now, None),
        (false, LivenessBasis::Process)
    );
}

// TASK-543: raw-registered agents use the same stale-PID transition as
// launcher/MCP entries.
#[test]
fn manual_registered_entry_with_dead_pid_is_stale() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let mut record = entry_with((i32::MAX as u32) - 1, now);
    record.source = "manual-register".to_string();
    record.name = Some("codex-raw".to_string());
    write_entry(tmp.path(), &record).unwrap();

    let views = list_agent_views(tmp.path(), &ctx(now, 30, vec![]));
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].source, "manual-register");
    assert_eq!(views[0].name.as_deref(), Some("codex-raw"));
    assert_eq!(views[0].status, AgentStatus::Stale);
}

/// BUG-416: live_agents_covering_cwd counts only LIVE agents whose
/// worktree covers the cwd (equal or ancestor), skipping dead PIDs. A
/// worktree with ≥2 live agents reads as "shared" (the add-hint suppressor
/// fires); a lone agent or an unrelated dir does not. trace:BUG-416
#[test]
fn live_agents_covering_cwd_counts_shared_live_worktrees_only() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let wt_a = root.join("aida-a");
    let wt_b = root.join("aida-b");
    let nested = wt_a.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir_all(&wt_b).unwrap();

    let live = |id: &str, wt: &Path| {
        let mut e = entry_with(std::process::id(), Utc::now());
        e.id = id.to_string();
        e.worktree_path = wt.to_path_buf();
        write_entry(root, &e).unwrap();
    };
    // Two live agents share wt_a.
    live("a-codex-1", &wt_a);
    live("a-claude-2", &wt_a);
    // One live agent alone in wt_b.
    live("b-codex-solo", &wt_b);
    // A dead agent also recorded in wt_b — must be skipped.
    let mut dead = entry_with((i32::MAX as u32) - 1, Utc::now());
    dead.id = "b-codex-dead".to_string();
    dead.worktree_path = wt_b.clone();
    write_entry(root, &dead).unwrap();

    // wt_a shared by 2 live agents.
    assert_eq!(live_agents_covering_cwd(root, &wt_a), 2);
    // A descendant of wt_a is still "covered" by both.
    assert_eq!(live_agents_covering_cwd(root, &nested), 2);
    // wt_b: 1 live + 1 dead → 1 (lone occupant; dead skipped).
    assert_eq!(live_agents_covering_cwd(root, &wt_b), 1);
    // The parent dir is no agent's worktree → 0 (human-alone case hints).
    assert_eq!(live_agents_covering_cwd(root, root), 0);
}

#[test]
fn format_agent_status_lines_appends_freshness_hint() {
    let now = Utc::now();
    let view = AgentRegistryView {
        id: "codex-42".to_string(),
        agent_type: "codex".to_string(),
        pid: 42,
        pid_start: None,
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        // Freshness column is computed relative to `Utc::now()` inside
        // `format_agent_status_lines`. Anchor it at "now - 0s" so the
        // rendered hint is "(0s)" deterministically.
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: Some("STORY-431".to_string()),
        worktree_path: PathBuf::from("/tmp/aida-story-431"),
        source: "mcp".to_string(),
        binary_version: None,
        build_sha: None,
        status: AgentStatus::Busy,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
        liveness: LivenessBasis::Process,
        mail_wake: None,
    };

    let lines = format_agent_status_lines(&[view]);
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    // Strict column-shape assertion for the static part; the (Xs)
    // trailing hint is loose because Utc::now() inside the formatter
    // can tick a second on a slow runner.
    assert!(
        line.starts_with(
            "  codex#42        process  implementer STORY-431    spec:STORY-431     busy  ("
        ),
        "unexpected line: {line:?}"
    );
    assert!(
        line.ends_with(" /tmp/aida-story-431"),
        "unexpected line: {line:?}"
    );
    assert!(line.contains("s)"), "expected (Xs) hint, got: {line:?}");
}

#[test]
fn mcp_authority_status_lines_name_role_and_relaunch_command() {
    let now = Utc::now();
    let view = AgentRegistryView {
        id: "codex-42".to_string(),
        agent_type: "codex".to_string(),
        pid: 42,
        pid_start: None,
        name: None,
        description: None,
        tty: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: Some("BUG-1043".to_string()),
        terminal: None,
        worktree_path: PathBuf::from("/tmp/aida-bug-1043"),
        source: "mcp".to_string(),
        binary_version: None,
        build_sha: None,
        status: AgentStatus::Busy,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
        liveness: LivenessBasis::Process,
        mail_wake: None,
    };

    let lines = mcp_authority_status_lines(&[view]);
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert!(line.contains("MCP authority:"), "{line}");
    // ADR-66: the line names the server's granted seat and points at TTY
    // issuance, never an env-var relaunch. trace:STORY-1473 | ai:claude
    assert!(line.contains("seat=implementer"), "{line}");
    assert!(line.contains("advisor authority: no"), "{line}");
    assert!(
        line.contains("enter advisor at a TTY, then restart MCP"),
        "{line}"
    );
    assert!(!line.contains("AIDA_SESSION_ROLE"), "{line}");
}

#[test]
fn humanize_elapsed_thresholds() {
    assert_eq!(humanize_elapsed(0), "0s");
    assert_eq!(humanize_elapsed(59), "59s");
    assert_eq!(humanize_elapsed(60), "1m");
    assert_eq!(humanize_elapsed(90), "1m");
    assert_eq!(humanize_elapsed(3599), "59m");
    assert_eq!(humanize_elapsed(3600), "1h");
    assert_eq!(humanize_elapsed(7200), "2h");
    assert_eq!(humanize_elapsed(86_399), "23h");
    assert_eq!(humanize_elapsed(86_400), "1d");
    assert_eq!(humanize_elapsed(90_000), "1d");
    // Negative inputs clamp to 0s.
    assert_eq!(humanize_elapsed(-5), "0s");
}

#[test]
fn config_load_defaults_when_section_missing() {
    let cfg = Config::from_toml_str("");
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.busy_threshold_secs, 30);
    assert_eq!(cfg.work_grace_secs, 10);

    let cfg = Config::from_toml_str("[other]\nfoo = 1\n");
    assert_eq!(cfg.busy_threshold_secs, 30);
}

#[test]
fn config_parses_busy_threshold_secs() {
    let cfg = Config::from_toml_str("[agent_registry]\nbusy_threshold_secs = 90\n");
    assert_eq!(cfg.busy_threshold_secs, 90);
}

// trace:BUG-1704 | ai:codex
#[test]
fn config_parses_work_grace_secs() {
    let cfg = Config::from_toml_str("[agent_registry]\nwork_grace_secs = 25\n");
    assert_eq!(cfg.work_grace_secs, 25);
    assert_eq!(Config::default().work_grace_secs, 10);
}

#[test]
fn config_ignores_unknown_keys() {
    let body = "[agent_registry]\nbusy_threshold_secs = 45\nunknown = \"x\"\n";
    let cfg = Config::from_toml_str(body);
    assert_eq!(cfg.busy_threshold_secs, 45);
}

#[test]
fn config_load_reads_from_disk() {
    let tmp = TempDir::new().unwrap();
    let aida = tmp.path().join(".aida");
    std::fs::create_dir_all(&aida).unwrap();
    std::fs::write(
        aida.join("config.toml"),
        "[agent_registry]\nbusy_threshold_secs = 7\n",
    )
    .unwrap();
    assert_eq!(Config::load(tmp.path()).busy_threshold_secs, 7);
}

// STORY-528: a registry record written before the availability field
// existed must still deserialize — defaulting to Available with no pause
// metadata. This is the backward-compatibility guarantee.
#[test]
fn legacy_record_without_availability_reads_as_available() {
    let legacy = r#"
id = "codex-42"
agent_type = "codex"
pid = 42
started_at = "2026-06-07T10:00:00Z"
last_active_at = "2026-06-07T10:00:00Z"
worktree_path = "/tmp/aida-story"
source = "mcp"
"#;
    let entry: AgentRegistryEntry = toml::from_str(legacy).unwrap();
    assert_eq!(entry.availability, Availability::Available);
    assert!(entry.paused_since.is_none());
    assert!(entry.paused_reason.is_none());
    assert!(entry.expected_back.is_none());
    assert!(entry.description.is_none());
}

#[cfg(unix)]
// STORY-791: MCP child registration can attach to the launcher row by
// walking process ancestry rather than creating a second row.
#[test]
fn pid_has_ancestor_detects_live_child_parent() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let child_pid = child.id();
    assert!(pid_has_ancestor(child_pid, std::process::id()));
    let _ = child.kill();
    let _ = child.wait();
}

// STORY-528: pause then resume roundtrips through the on-disk record.
#[test]
fn pause_then_resume_roundtrips() {
    let tmp = TempDir::new().unwrap();
    let mut e = entry_with(std::process::id(), Utc::now());
    e.name = Some("codex-impl-1".to_string());
    write_entry(tmp.path(), &e).unwrap();

    let paused = pause_agent(
        tmp.path(),
        "codex-impl-1",
        PauseReason::Budget,
        Some(Utc::now() + Duration::hours(2)),
    )
    .unwrap();
    assert_eq!(paused.availability, Availability::Paused);
    assert_eq!(paused.paused_reason, Some(PauseReason::Budget));
    assert!(paused.paused_since.is_some());
    assert!(paused.expected_back.is_some());

    // Reloaded from disk it's still paused.
    let ctx = ctx(Utc::now(), 30, vec![]);
    let views = list_agent_views(tmp.path(), &ctx);
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].availability, Availability::Paused);

    let resumed = resume_agent(tmp.path(), "codex-impl-1").unwrap();
    assert_eq!(resumed.availability, Availability::Available);
    assert!(resumed.paused_since.is_none());
    assert!(resumed.paused_reason.is_none());
    assert!(resumed.expected_back.is_none());
}

// STORY-528: pausing resolves via the synthetic <type>#<pid> id too.
#[test]
fn pause_resolves_via_hash_id() {
    let tmp = TempDir::new().unwrap();
    let e = entry_with(std::process::id(), Utc::now());
    write_entry(tmp.path(), &e).unwrap();
    let target = format!("codex#{}", std::process::id());
    let paused = pause_agent(tmp.path(), &target, PauseReason::RateLimit, None).unwrap();
    assert_eq!(paused.availability, Availability::Paused);
    assert_eq!(paused.paused_reason, Some(PauseReason::RateLimit));
}

// STORY-528: an unknown target errors rather than silently no-op'ing.
#[test]
fn pause_unknown_target_errors() {
    let tmp = TempDir::new().unwrap();
    let err = pause_agent(tmp.path(), "nope-9999", PauseReason::Manual, None).unwrap_err();
    assert!(err.to_string().contains("no active agent"));
}

// STORY-528: brief-time guard WARNS (returns Some) for a paused agent and
// is silent (None) for an available agent / unknown target.
#[test]
fn paused_warning_fires_only_for_paused_target() {
    let tmp = TempDir::new().unwrap();
    let mut e = entry_with(std::process::id(), Utc::now());
    e.name = Some("codex-impl-1".to_string());
    write_entry(tmp.path(), &e).unwrap();

    // Available → no warning.
    assert!(paused_warning_for_target(tmp.path(), "codex-impl-1").is_none());
    // Unknown target → no warning.
    assert!(paused_warning_for_target(tmp.path(), "ghost").is_none());

    pause_agent(tmp.path(), "codex-impl-1", PauseReason::Budget, None).unwrap();
    let warning = paused_warning_for_target(tmp.path(), "codex-impl-1").unwrap();
    assert!(warning.contains("paused"));
    assert!(warning.contains("budget"));
}

// STORY-528: the status-line formatter appends the ⏸ glyph for a paused
// agent and leaves an available agent's line glyph-free.
#[test]
fn format_status_line_appends_pause_glyph() {
    let now = Utc::now();
    let mk = |availability: Availability, reason: Option<PauseReason>| AgentRegistryView {
        id: "codex-42".to_string(),
        agent_type: "codex".to_string(),
        pid: 42,
        pid_start: None,
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: Some("STORY-528".to_string()),
        worktree_path: PathBuf::from("/tmp/aida-story-528"),
        source: "mcp".to_string(),
        binary_version: None,
        build_sha: None,
        status: AgentStatus::Idle,
        availability,
        paused_since: reason.map(|_| now),
        paused_reason: reason,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
        liveness: LivenessBasis::Process,
        mail_wake: None,
    };

    let avail_line = &format_agent_status_lines(&[mk(Availability::Available, None)])[0];
    assert!(!avail_line.contains("⏸"), "available agent: {avail_line:?}");

    let paused_line =
        &format_agent_status_lines(&[mk(Availability::Paused, Some(PauseReason::Budget))])[0];
    assert!(paused_line.contains("⏸ paused (budget)"), "{paused_line:?}");
}

// STORY-528: pause_detail formats the ~back HH:MM fragment when an
// expected-back time is present, and omits it otherwise.
#[test]
fn pause_detail_formats_expected_back() {
    assert_eq!(pause_detail(PauseReason::Budget, None), "paused (budget)");
    let detail = pause_detail(PauseReason::RateLimit, Some(Utc::now()));
    assert!(detail.starts_with("paused (rate-limit, ~back "));
}

// STORY-528: reason parsing accepts the documented spellings.
#[test]
fn pause_reason_parse_accepts_known_reasons() {
    assert_eq!(PauseReason::parse("budget"), Some(PauseReason::Budget));
    assert_eq!(
        PauseReason::parse("RATE-LIMIT"),
        Some(PauseReason::RateLimit)
    );
    assert_eq!(
        PauseReason::parse("rate_limit"),
        Some(PauseReason::RateLimit)
    );
    assert_eq!(PauseReason::parse("manual"), Some(PauseReason::Manual));
    assert_eq!(PauseReason::parse("unknown"), Some(PauseReason::Unknown));
    assert_eq!(PauseReason::parse("bogus"), None);
}

#[test]
fn looks_like_spec_id_accepts_canonical_ids_only() {
    assert!(looks_like_spec_id("STORY-431"));
    assert!(looks_like_spec_id("TASK-498"));
    assert!(!looks_like_spec_id("story-431"));
    assert!(!looks_like_spec_id("branch-name"));
    assert!(looks_like_spec_id(
        "TASK-b6b1278890774ef993d2ebc327862470-007"
    ));
    assert!(!looks_like_spec_id(
        "task-b6b1278890774ef993d2ebc327862470-007"
    ));
}

// trace:BUG-1920 | ai:claude
#[test]
fn detect_agent_type_prefers_live_session_over_api_key() {
    assert_eq!(
        detect_agent_type_from_env([("GEMINI_API_KEY", "x"), ("CLAUDECODE", "1")]),
        "claude"
    );
    assert_eq!(
        detect_agent_type_from_env([("GEMINI_API_KEY", "x"), ("CLAUDE_CODE_ENTRYPOINT", "cli"),]),
        "claude"
    );
}

// trace:BUG-1920 | ai:claude
#[test]
fn detect_agent_type_api_key_never_decides() {
    assert_eq!(
        detect_agent_type_from_env([("ANTHROPIC_API_KEY", "x"), ("ANTIGRAVITY_SESSION_ID", "s1"),]),
        "antigravity"
    );
    assert_eq!(
        detect_agent_type_from_env([("GEMINI_API_KEY", "x")]),
        "other"
    );
    assert_eq!(
        detect_agent_type_from_env([("CLAUDE_CODE_OAUTH_TOKEN", "x")]),
        "other"
    );
}

// trace:BUG-1920 | ai:claude
#[test]
fn detect_agent_type_live_marker_beats_stale_weak_prefix() {
    assert_eq!(
        detect_agent_type_from_env([("GEMINI_MODEL", "pro"), ("CLAUDECODE", "1")]),
        "claude"
    );
    assert_eq!(
        detect_agent_type_from_env([("GEMINI_CLI", "1")]),
        "antigravity"
    );
    assert_eq!(
        detect_agent_type_from_env([("CLAUDE_CONFIG_DIR", "/x")]),
        "claude"
    );
    assert_eq!(
        detect_agent_type_from_env(Vec::<(String, String)>::new()),
        "other"
    );
}

// trace:BUG-1920 | ai:claude
#[test]
fn detect_agent_type_explicit_override_wins() {
    assert_eq!(
        detect_agent_type_from_env([
            ("CLAUDECODE", "1"),
            ("GEMINI_API_KEY", "x"),
            ("AIDA_AGENT_TYPE", "codex"),
        ]),
        "codex"
    );
    // A blank override is ignored rather than forcing "other".
    assert_eq!(
        detect_agent_type_from_env([("AIDA_AGENT_TYPE", "  "), ("CLAUDECODE", "1")]),
        "claude"
    );
}
