// `aida whoami`: the handle it prints is the address mail is delivered to,
// its mail line counts that inbox without marking it seen, and each line of
// the compact view is collected from local state and rendered.
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude

use super::*;
use tempfile::tempdir;

fn clean_env() -> crate::test_env::EnvVarsGuard {
    crate::test_env::EnvVarsGuard::apply(&[
        ("AIDA_USER", Some("operator-under-test")),
        ("AIDA_HANDLE", None),
        ("AIDA_AGENT_NAME", None),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
        ("AIDA_SESSION_ID", None),
        ("AIDA_SESSION_PURPOSE", None),
        ("AIDA_HOOK_PAYLOAD", None),
    ])
}

/// Session inputs with nothing claimed, no mandate and no purpose.
fn quiet_inputs() -> SessionInputs {
    SessionInputs {
        entry: None,
        lease_id: None,
        purpose: None,
        mandate: crate::gate::mandate::MandateState::None,
        user: "operator-under-test".into(),
    }
}

/// A live registry row for this process named `handle`, registered the way a
/// session launch registers it, with its handle reservation.
fn register(
    project: &Path,
    vendor: &str,
    handle: &str,
) -> crate::agent_registry::AgentRegistryEntry {
    let entry = crate::agent_registry::register_existing_agent(
        project,
        vendor,
        std::process::id(),
        "implementer".into(),
        None,
        project.to_path_buf(),
        Some(handle.into()),
        None,
    )
    .unwrap();
    crate::mailbox_resolver::reserve_test_reader(project, &entry);
    entry
}

/// The facts the production collector derives from `entry`'s row.
fn facts_for(
    project: &Path,
    entry: &crate::agent_registry::AgentRegistryEntry,
) -> MailIdentityFacts {
    use aida_core::mailbox_identity::{IdentityEnv, PeopleCatalogView};
    let env = IdentityEnv::observed();
    let row = crate::agent_registry::registry_path(project, &entry.id);
    let binding = crate::session_binding_from_row(Some(&row), &env);
    MailIdentityFacts::observed(binding, &PeopleCatalogView::default(), env)
}

/// `aida mailbox send --to <to>`, through the real command.
fn send(store: &Path, to: &str, urgent: bool) {
    let cmd = crate::cli::MailboxCommand::Send {
        to: Some(to.into()),
        broadcast: false,
        body: Some("status update".into()),
        subject: None,
        body_file: None,
        stdin: false,
        thread: None,
        in_reply_to: None,
        from: Some("someone-else".into()),
        urgent,
        intent: "fyi".into(),
        relayed_from: None,
        allow_second_person: false,
        queue_for_role: false,
    };
    crate::mailbox_cmd::handle_mailbox_command(&cmd, store).unwrap();
}

/// A project directory with an (empty) store.
fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let store = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store).unwrap();
    (dir, store)
}

// AC1: for a registered Claude, Codex and agy session, the handle whoami
// prints is (a) the address `aida mailbox send` resolves and delivers to,
// (b) one of the inboxes `aida mailbox notice` reads (the own-audience
// resolver, read_own_mail_binding and notice_text), and (c) the sender
// identity a send from this session stamps.
#[test]
fn whoami_handle_is_the_send_and_notice_address_for_every_vendor() {
    for (vendor, handle) in [
        ("claude", "claude-product-01"),
        ("codex", "codex-implementer-02"),
        ("antigravity", "agy-reviewer-03"),
    ] {
        let (dir, store) = project();
        let project = dir.path();
        let _env = clean_env();
        let entry = register(project, vendor, handle);
        let facts = facts_for(project, &entry);

        let view = collect_with(project, project, &facts, quiet_inputs(), Utc::now());
        assert_eq!(
            view.handle, handle,
            "{vendor}: whoami prints the bound handle"
        );

        // (a) send resolves the printed handle to itself and delivers there.
        let resolved = crate::mailbox_resolver::RecipientDirectory::load_for_send(project, &store)
            .unwrap()
            .resolve_send(&view.handle, false)
            .unwrap();
        assert_eq!(resolved.address, view.handle, "{vendor}: send's address");
        send(&store, &view.handle, true);

        // (b) the notice's audience holds the handle and counts that mail.
        let audience = aida_core::mailbox_logical::ResolvedAudience::from_facts(&facts);
        assert!(
            audience.keys().any(|k| k == view.handle),
            "{vendor}: notice reads {:?}",
            audience.keys().collect::<Vec<_>>()
        );
        let notice_view = crate::mailbox_view::read_own_mail_binding(
            project,
            &store,
            &audience,
            &crate::mailbox_view::passive_limits(),
            crate::mailbox_view::BindWait::Notice,
        );
        assert_eq!(
            notice_view.unread().len(),
            1,
            "{vendor}: notice sees the mail"
        );
        let identities: Vec<String> = audience.keys().map(str::to_string).collect();
        assert!(
            !crate::mailbox_cmd::notice_text(&notice_view, &identities, 10).is_empty(),
            "{vendor}: the notice announces it"
        );
        let view = collect_with(project, project, &facts, quiet_inputs(), Utc::now());
        assert_eq!(
            (view.mail.unread, view.mail.urgent),
            (1, 1),
            "{vendor}: whoami counts the same delivery"
        );

        // (c) a send from this session is signed with the same handle.
        let (sender, _) = crate::queue_misc::resolve_mail_sender_identity_with(
            None,
            crate::agent_handle::valid_handle(&entry),
        );
        assert_eq!(sender, view.handle, "{vendor}: send's sender identity");
    }
}

// AC1: an unregistered shell reads as its user, exactly as the mail channel's
// headline does.
#[test]
fn unbound_reader_handle_is_the_user() {
    let _env = clean_env();
    let facts = crate::mailbox_view::observed_unbound_facts();
    assert_eq!(resolved_handle(&facts), "operator-under-test");
}

// AC2: collecting the view never marks mail seen.
#[test]
fn collecting_twice_still_sees_the_mail_unread() {
    let (dir, store) = project();
    let _env = clean_env();
    let entry = register(dir.path(), "claude", "claude-product-01");
    let facts = facts_for(dir.path(), &entry);
    send(&store, "claude-product-01", false);
    let first = collect_with(dir.path(), dir.path(), &facts, quiet_inputs(), Utc::now());
    let second = collect_with(dir.path(), dir.path(), &facts, quiet_inputs(), Utc::now());
    assert_eq!(first.mail.unread, 1);
    assert_eq!(second.mail.unread, 1, "whoami must not acknowledge mail");
}

/// A lease file for `scope` on `worktree`, started `age_secs` before `now`.
fn write_lease(
    main_root: &Path,
    id: &str,
    scope: &str,
    worktree: &Path,
    now: DateTime<Utc>,
    age_secs: i64,
) {
    let dir = crate::leases_dir(main_root);
    std::fs::create_dir_all(&dir).unwrap();
    let mut lease = toml::Table::new();
    for (key, value) in [
        ("id", id.to_string()),
        ("scope", scope.to_string()),
        ("slug", "s".into()),
        ("owner", "joe".into()),
        (
            "worktree_path",
            worktree.canonicalize().unwrap().display().to_string(),
        ),
        ("branch", "b".into()),
        (
            "started_at",
            (now - chrono::Duration::seconds(age_secs)).to_rfc3339(),
        ),
        ("hostname", "h".into()),
    ] {
        lease.insert(key.into(), toml::Value::String(value));
    }
    std::fs::write(dir.join(format!("{id}.toml")), lease.to_string()).unwrap();
}

// AC3: the lease named by AIDA_SESSION_ID wins and is aged from its start.
#[test]
fn working_is_the_lease_named_by_the_session_id_with_its_age() {
    let (dir, _store) = project();
    let _env = clean_env();
    let elsewhere = tempdir().unwrap();
    let seat = tempdir().unwrap();
    let now = Utc::now();
    write_lease(
        dir.path(),
        "aaaaaaaaaaaa",
        "STORY-1-001",
        seat.path(),
        now,
        150,
    );
    let facts = crate::mailbox_view::observed_unbound_facts();
    let inputs = SessionInputs {
        lease_id: Some("aaaaaaaaaaaa".into()),
        ..quiet_inputs()
    };
    let view = collect_with(dir.path(), elsewhere.path(), &facts, inputs, now);
    assert_eq!(
        view.working,
        Working::Spec {
            spec: "STORY-1-001".into(),
            lease: Some("aaaaaaaaaaaa".into()),
            age_secs: Some(150),
        }
    );
    assert_eq!(view.lease_id.as_deref(), Some("aaaaaaaaaaaa"));
    assert!(view
        .render_toon()
        .contains("working: STORY-1-001 (lease 2m)"));
}

// AC3: with no session id, the lease whose worktree holds the cwd is used.
#[test]
fn working_falls_back_to_the_lease_covering_the_worktree() {
    let (dir, _store) = project();
    let _env = clean_env();
    let seat = tempdir().unwrap();
    let now = Utc::now();
    write_lease(
        dir.path(),
        "bbbbbbbbbbbb",
        "BUG-1-002",
        seat.path(),
        now,
        3700,
    );
    let facts = crate::mailbox_view::observed_unbound_facts();
    let view = collect_with(dir.path(), seat.path(), &facts, quiet_inputs(), now);
    assert_eq!(
        view.working,
        Working::Spec {
            spec: "BUG-1-002".into(),
            lease: Some("bbbbbbbbbbbb".into()),
            age_secs: Some(3700),
        }
    );
}

// AC3: with no lease, the registry row's claim is shown with its row's age,
// labelled a claim rather than a lease.
#[test]
fn working_falls_back_to_the_registry_claim_with_its_age() {
    let (dir, _store) = project();
    let _env = clean_env();
    let now = Utc::now();
    let mut entry = register(dir.path(), "claude", "claude-implementer-04");
    entry.current_spec = Some("TASK-1-003".into());
    entry.started_at = now - chrono::Duration::seconds(600);
    let facts = crate::mailbox_view::observed_unbound_facts();
    let inputs = SessionInputs {
        entry: Some(entry),
        ..quiet_inputs()
    };
    let view = collect_with(dir.path(), dir.path(), &facts, inputs, now);
    assert_eq!(
        view.working,
        Working::Spec {
            spec: "TASK-1-003".into(),
            lease: None,
            age_secs: Some(600),
        }
    );
    assert!(view
        .render_toon()
        .contains("working: TASK-1-003 (claim 10m)"));
}

/// A store holding queue files and a cache that knows `specs`
/// (uuid, display id, title, status).
fn queue_fixture(
    main_root: &Path,
    queues: &[(&str, Vec<(uuid::Uuid, i64, Option<&str>)>)],
    specs: &[(uuid::Uuid, &str, &str, &str)],
) {
    let store = main_root.join(".aida-store");
    std::fs::create_dir_all(store.join("objects")).unwrap();
    std::fs::write(store.join("metadata.yaml"), "{}\n").unwrap();
    let qdir = store.join("registry/queues");
    std::fs::create_dir_all(&qdir).unwrap();
    for (user, rows) in queues {
        let entries: Vec<aida_core::QueueEntry> = rows
            .iter()
            .map(|(id, position, role)| aida_core::QueueEntry {
                user_id: user.to_string(),
                requirement_id: *id,
                position: *position,
                added_by: user.to_string(),
                note: None,
                added_at: Utc::now(),
                for_role: role.map(str::to_string),
                for_scope: None,
                for_session: None,
                added_by_machine: None,
            })
            .collect();
        let file = user.replace(':', "%3A");
        std::fs::write(
            qdir.join(format!("{file}.yaml")),
            serde_yaml::to_string(&entries).unwrap(),
        )
        .unwrap();
    }
    std::fs::create_dir_all(main_root.join(".aida")).unwrap();
    let conn = rusqlite::Connection::open(main_root.join(".aida/cache.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE requirements_cache (id TEXT PRIMARY KEY, agreed_id TEXT, \
         spec_id TEXT, title TEXT NOT NULL, status TEXT NOT NULL);",
    )
    .unwrap();
    for (id, display, title, status) in specs {
        conn.execute(
            "INSERT INTO requirements_cache VALUES (?1, NULL, ?2, ?3, ?4)",
            [
                id.to_string(),
                display.to_string(),
                title.to_string(),
                status.to_string(),
            ],
        )
        .unwrap();
    }
}

// AC3: an idle seat's next item comes from its own queue first, skips a
// finished spec there, then falls through to the role pool; another role's
// pool is not this seat's.
#[test]
fn idle_next_item_skips_finished_work_and_reaches_the_role_pool() {
    let dir = tempdir().unwrap();
    let _env = clean_env();
    let done = uuid::Uuid::new_v4();
    let superseded = uuid::Uuid::new_v4();
    let open = uuid::Uuid::new_v4();
    let advisory = uuid::Uuid::new_v4();
    queue_fixture(
        dir.path(),
        &[
            (
                "operator-under-test",
                vec![(done, 1, Some("implementer")), (superseded, 2, None)],
            ),
            ("role:implementer", vec![(open, 5, Some("implementer"))]),
            ("role:advisor", vec![(advisory, 1, Some("advisor"))]),
        ],
        &[
            (done, "TASK-1-010", "Already shipped", "Completed"),
            (superseded, "TASK-1-011", "Replaced", "Superseded"),
            (open, "TASK-1-012", "Open work", "Approved"),
            (advisory, "STORY-1-013", "Advise", "Approved"),
        ],
    );
    let aliases = aida_core::node_alias::NodeAliasMap::default();
    assert_eq!(
        queue_head(dir.path(), "operator-under-test", "implementer", &aliases).as_deref(),
        Some("TASK-1-012 Open work")
    );
    assert_eq!(
        queue_head(dir.path(), "operator-under-test", "advisor", &aliases).as_deref(),
        Some("STORY-1-013 Advise")
    );
    assert_eq!(
        queue_head(dir.path(), "operator-under-test", "reviewer", &aliases),
        None
    );

    // Collected end to end: an idle implementer shows the role-pool head.
    let facts = crate::mailbox_view::observed_unbound_facts();
    let view = collect_with(dir.path(), dir.path(), &facts, quiet_inputs(), Utc::now());
    assert_eq!(
        view.working,
        Working::Idle {
            next: Some("TASK-1-012 Open work".into())
        }
    );
}

fn active_mandate() -> crate::gate::mandate::Mandate {
    crate::gate::mandate::Mandate {
        id: "mandate-1".into(),
        principal: "joe".into(),
        seat: crate::gate::mandate::MANDATE_SEAT.into(),
        scope: crate::gate::mandate::SCOPE_SHIP_HELD.into(),
        include_supervised: false,
        reason: None,
        issued_at: Utc::now() - chrono::Duration::hours(1),
        expires_at: Utc::now() + chrono::Duration::hours(2),
        revoked_at: None,
        revoked_by: None,
        scopes: Default::default(),
        kind: None,
        renewals: Vec::new(),
    }
}

// AC4: the goal line joins the active mandate (time left, principal, gates)
// and the seat purpose; a mandate that is not active contributes nothing.
#[test]
fn goal_line_gathers_the_mandate_and_the_purpose() {
    use crate::gate::mandate::MandateState;
    let now = Utc::now();
    let goal = goal_line(
        &MandateState::Active(active_mandate()),
        Some("product intake".into()),
        now,
        || "none".into(),
    )
    .unwrap();
    let mandate = goal.mandate.as_deref().unwrap();
    assert!(mandate.starts_with("MANDATE ship-held"), "{mandate}");
    assert!(mandate.contains(" left · by joe"), "{mandate}");
    assert!(mandate.ends_with("gates: none"), "{mandate}");
    assert_eq!(goal.purpose.as_deref(), Some("product intake"));

    let expired = goal_line(
        &MandateState::Expired(active_mandate()),
        Some("triage".into()),
        now,
        || unreachable!("no gates are described without an active mandate"),
    )
    .unwrap();
    assert_eq!(expired.mandate, None);
    assert_eq!(expired.purpose.as_deref(), Some("triage"));

    assert_eq!(
        goal_line(&MandateState::None, None, now, || "none".into()),
        None
    );
}

fn sample() -> WhoamiView {
    WhoamiView {
        handle: "claude-product-01".into(),
        role: "product".into(),
        agent_type: "claude".into(),
        session_id: Some("1b444f41".into()),
        lease_id: None,
        node: Some(NodeIdentity {
            id: "b6b1278890774ef993d2ebc327862470".into(),
            label: Some("HOME".into()),
            authoritative: false,
        }),
        mail: MailLine {
            unread: 0,
            shared_unread: 1,
            urgent: 0,
            shared_scope: Some("product".into()),
            partial: false,
            read_command: MAIL_READ_COMMAND,
        },
        working: Working::Spec {
            spec: "STORY-HOME-095".into(),
            lease: Some("01a18d217056".into()),
            age_secs: Some(150),
        },
        goal: Some(GoalLine {
            advisor_goal: None,
            mandate: Some("MANDATE ship-held · 5h50m left · by joe · gates: none".into()),
            purpose: Some("product intake".into()),
        }),
        worktree: WorktreeLine {
            path: "/home/joe/ai/aida".into(),
            branch: Some("main".into()),
        },
    }
}

// The compact block carries every line, in order, with the node's label and
// canonical id and its role as prose.
#[test]
fn human_block_renders_every_line() {
    // Color is off when stdout is not a terminal, as under the test harness.
    let out = sample().render_human();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        "claude-product-01 · product · claude · session 1b444f41"
    );
    assert_eq!(
        lines[1],
        "node:     HOME (b6b1278890774ef993d2ebc327862470) · work node, node-local allocation"
    );
    assert!(lines[2].starts_with("mail:     0 unread · 1 shared · 0 urgent"));
    assert!(lines[2].ends_with("→ aida mailbox inbox"));
    assert_eq!(lines[3], "working:  STORY-HOME-095 (lease 2m)");
    assert_eq!(
        lines[4],
        "goal:     MANDATE ship-held · 5h50m left · by joe · gates: none · purpose: product intake"
    );
    assert_eq!(lines[5], "worktree: /home/joe/ai/aida (main)");
    assert_eq!(lines.len(), 6);
}

// An AIDA lease id is labelled `lease`, never `session`: `session` is kept
// for the harness's own id.
#[test]
fn lease_id_is_labelled_lease_not_session() {
    let mut v = sample();
    v.session_id = None;
    v.lease_id = Some("01a18d217056".into());
    assert_eq!(
        v.render_human().lines().next().unwrap(),
        "claude-product-01 · product · claude · lease 01a18d217056"
    );
    let toon = v.render_toon();
    assert!(toon.contains("session: -\n"), "{toon}");
    assert!(toon.contains("lease: 01a18d217056\n"), "{toon}");
}

// A harness session id hides the lease id from the header.
#[test]
fn harness_session_id_is_preferred_over_the_lease_id() {
    let (dir, _store) = project();
    let _env = clean_env();
    let mut entry = register(dir.path(), "codex", "codex-implementer-05");
    entry.native_session_id = Some("019a2b3c-4d5e".into());
    let facts = crate::mailbox_view::observed_unbound_facts();
    let inputs = SessionInputs {
        entry: Some(entry),
        lease_id: Some("cccccccccccc".into()),
        ..quiet_inputs()
    };
    let view = collect_with(dir.path(), dir.path(), &facts, inputs, Utc::now());
    assert_eq!(view.session_id.as_deref(), Some("019a2b3c"));
    assert_eq!(view.lease_id, None);
}

// AC2: a partial mail view says so instead of reading as caught up.
#[test]
fn partial_mail_view_is_marked() {
    let mut v = sample();
    v.mail.partial = true;
    assert!(v.render_toon().contains("partial view"));
}

// AC3: idle shows the next queued item, or just `idle`.
#[test]
fn idle_shows_the_next_queued_item() {
    let mut v = sample();
    v.working = Working::Idle {
        next: Some("TASK-HOME-012 Fix the thing".into()),
    };
    assert!(v
        .render_toon()
        .contains("working: idle · next: TASK-HOME-012 Fix the thing"));
    v.working = Working::Idle { next: None };
    assert!(v.render_toon().contains("working: idle\n"));
}

// AC4: the goal line is omitted when there is no mandate and no purpose.
#[test]
fn goal_line_omitted_when_absent() {
    let mut v = sample();
    v.goal = None;
    assert!(!v.render_toon().contains("goal:"));
    v.goal = Some(GoalLine {
        advisor_goal: None,
        mandate: None,
        purpose: Some("triage".into()),
    });
    assert!(v.render_toon().contains("goal: purpose: triage"));
}

// AC6: a terminal gets the human block; a pipe (no terminal, no pin) gets
// flat toon lines, one `key: value` per line.
#[test]
fn output_mode_follows_the_terminal() {
    let v = sample();
    let at_tty = crate::resolve_agent_mode(None, None, true);
    let piped = crate::resolve_agent_mode(None, None, false);
    assert_eq!(v.render(at_tty), v.render_human());
    let toon = v.render(piped);
    assert_eq!(toon, v.render_toon());
    let keys: Vec<&str> = toon
        .lines()
        .map(|l| l.split_once(": ").expect("key: value").0)
        .collect();
    assert_eq!(
        keys,
        [
            "handle",
            "role",
            "agent_type",
            "session",
            "node",
            "mail",
            "working",
            "goal",
            "worktree"
        ]
    );
}

// AC6: the JSON projection carries each line as structured data.
#[test]
fn json_projection_is_structured() {
    let value = serde_json::to_value(sample()).unwrap();
    assert_eq!(value["handle"], "claude-product-01");
    assert_eq!(value["mail"]["shared_unread"], 1);
    assert_eq!(value["mail"]["read_command"], "aida mailbox inbox");
    assert_eq!(value["working"]["state"], "spec");
    assert_eq!(value["working"]["age_secs"], 150);
    assert_eq!(value["node"]["label"], "HOME");
    assert_eq!(value["worktree"]["branch"], "main");
}

// AC7: the caller-identity fields scripts read stay available.
#[test]
fn verbose_identity_keeps_the_legacy_fields() {
    let _env = clean_env();
    let fields: Vec<&str> = crate::session_misc_cmd::caller_identity_rows()
        .into_iter()
        .map(|(f, _, _)| f)
        .collect();
    assert_eq!(
        fields,
        [
            "role",
            "agent-type",
            "agent-name",
            "user-id",
            "headless",
            "ai-tool",
            "scope",
            "project",
            "purpose",
            "session-id"
        ]
    );
}

// Operator addition: `aida node show` names the role in prose and prints the
// label on its own line, so a label called WORK cannot read as the role.
#[test]
fn node_show_heading_separates_label_from_role() {
    let heading = crate::node_cmd::work_node_heading("b6b1", Some("WORK"));
    assert!(!heading.contains("WORK node"), "{heading}");
    assert!(heading.contains("Label: WORK"), "{heading}");
    assert!(heading.contains("Role:  work node"), "{heading}");
    let unlabelled = crate::node_cmd::work_node_heading("b6b1", None);
    assert!(!unlabelled.contains("Label"), "{unlabelled}");
}

#[test]
fn ages_are_compact() {
    assert_eq!(format_age(42), "42s");
    assert_eq!(format_age(150), "2m");
    assert_eq!(format_age(3 * 3600 + 5 * 60), "3h05m");
    assert_eq!(format_age(2 * 86400 + 4 * 3600), "2d4h");
}

// AC5: the branch comes from HEAD for a clone, a linked worktree's gitdir
// pointer, and a detached HEAD; a non-ASCII HEAD is cut by characters.
#[test]
fn branch_is_read_from_head() {
    let clone = tempdir().unwrap();
    std::fs::create_dir_all(clone.path().join(".git")).unwrap();
    std::fs::write(clone.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    assert_eq!(head_branch(clone.path()).as_deref(), Some("main"));

    let linked = tempdir().unwrap();
    let gitdir = clone.path().join(".git/worktrees/feature");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/story-098\n").unwrap();
    std::fs::write(
        linked.path().join(".git"),
        format!("gitdir: {}\n", gitdir.display()),
    )
    .unwrap();
    assert_eq!(head_branch(linked.path()).as_deref(), Some("story-098"));

    std::fs::write(gitdir.join("HEAD"), "0123456789abcdef\n").unwrap();
    assert_eq!(
        head_branch(linked.path()).as_deref(),
        Some("detached 01234567")
    );
    std::fs::write(gitdir.join("HEAD"), "ééééééééé\n").unwrap();
    assert_eq!(
        head_branch(linked.path()).as_deref(),
        Some("detached éééééééé")
    );
    assert_eq!(head_branch(tempdir().unwrap().path()), None);
}
