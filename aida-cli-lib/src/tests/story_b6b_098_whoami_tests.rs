// `aida whoami`: the handle it prints is the address mail is delivered to,
// its mail line counts that inbox without marking it seen, and each line of
// the compact view renders from the collected facts.
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

/// One unread message to `to`, written the way `aida mailbox send <to>`
/// delivers it.
fn deliver(project_root: &Path, id: &str, to: &str, urgent: bool) {
    let msg = aida_core::mailbox::Message {
        subject: None,
        id: id.to_string(),
        thread_id: format!("t-{id}"),
        from: "someone-else".to_string(),
        to: aida_core::mailbox::Recipient::Agent(to.to_string()),
        timestamp: 1_000,
        in_reply_to: None,
        body: "hello".to_string(),
        urgent,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
        archived: false,
        from_source: aida_core::mailbox::SenderSource::Explicit,
        from_role: None,
        relayed_from: None,
    };
    crate::mailbox_store::write_message(project_root, &msg).unwrap();
}

// AC1: for a registered Claude, Codex and agy session, the handle whoami
// prints is the one mail is delivered to and the one the notice path reads:
// mail sent to it is counted as whoami's direct unread, and the per-turn
// notice's collector agrees.
#[test]
fn whoami_handle_is_the_send_and_notice_address_for_every_vendor() {
    for (vendor, handle) in [
        ("claude", "claude-product-01"),
        ("codex", "codex-implementer-02"),
        ("antigravity", "agy-reviewer-03"),
    ] {
        let dir = tempdir().unwrap();
        let _env = clean_env();
        let facts = crate::mailbox_view::bound_facts(handle, vendor);
        deliver(dir.path(), &format!("m-{vendor}"), handle, true);
        deliver(
            dir.path(),
            &format!("u-{vendor}"),
            "operator-under-test",
            false,
        );

        let view = collect(dir.path(), dir.path(), &facts, Utc::now());
        assert_eq!(
            view.handle, handle,
            "{vendor}: whoami prints the bound handle"
        );
        assert_eq!(
            view.mail.unread, 1,
            "{vendor}: mail sent to the printed handle is this session's own unread"
        );
        assert_eq!(view.mail.urgent, 1, "{vendor}: urgent is counted");

        let notice = crate::collect_mail_channel(
            dir.path(),
            None,
            &facts,
            &crate::mailbox_view::passive_limits(),
        );
        assert_eq!(
            (notice.unread, notice.urgent),
            (view.mail.unread, view.mail.urgent),
            "{vendor}: whoami and the mail notice count the same inbox"
        );
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
    let dir = tempdir().unwrap();
    let _env = clean_env();
    let facts = crate::mailbox_view::bound_facts("claude-product-01", "claude");
    deliver(dir.path(), "m-1", "claude-product-01", false);
    let first = collect(dir.path(), dir.path(), &facts, Utc::now());
    let second = collect(dir.path(), dir.path(), &facts, Utc::now());
    assert_eq!(first.mail.unread, 1);
    assert_eq!(second.mail.unread, 1, "whoami must not acknowledge mail");
}

fn sample() -> WhoamiView {
    WhoamiView {
        handle: "claude-product-01".into(),
        role: "product".into(),
        agent_type: "claude".into(),
        session_id: Some("1b444f41".into()),
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
        mandate: None,
        purpose: Some("triage".into()),
    });
    assert!(v.render_toon().contains("goal: purpose: triage"));
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
// pointer, and a detached HEAD.
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
    assert_eq!(head_branch(tempdir().unwrap().path()), None);
}
