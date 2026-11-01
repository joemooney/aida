//! A live seat with no registry row (BUG-HOME-151): SessionStart registration
//! fails open on a contended lock and was never retried; the per-prompt
//! heartbeat now refreshes and restores the row, a recycled pid's row is
//! retired instead of erased, and the advisor handshake says "no registered
//! advisor" (or "live but unregistered") instead of "no live advisor".
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude

use super::*;
use crate::advisor_handshake::{
    presence_from, report_findings, save_record, state, unreachable_reason, AdvisorPresence,
    Census, HelloRecord, HomeKind, Policy, State,
};
use crate::gate::seat_authority::{held_grants_in, HeldGrant};
use chrono::Duration;
use tempfile::TempDir;

fn binary() -> AgentBinaryIdentity {
    AgentBinaryIdentity::new("0.16.0".into(), "abc1234".into())
}

fn own_row(root: &Path) -> Option<AgentRegistryEntry> {
    let path = registry_path(root, &agent_id(&detect_agent_type(), std::process::id()));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|b| toml::from_str(&b).ok())
}

fn rewrite(root: &Path, entry: &AgentRegistryEntry) {
    write_entry(root, entry).unwrap();
}

// AC1: the root cause. A SessionStart write that loses the namespace lock
// leaves no row, and before this fix nothing ever wrote one later.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac2ed7f1 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac7c7798 | ai:claude
#[test]
fn a_session_start_write_that_loses_the_lock_is_restored_by_the_next_prompt() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    {
        let _held =
            crate::agent_handle::lock_namespace_within(root, std::time::Duration::from_secs(1))
                .unwrap();
        let missed = touch_session_start_agent_for(root, root, Some("sid-a"), &binary(), pid);
        assert!(missed.is_err(), "the SessionStart budget lost the lock");
        assert!(own_row(root).is_none(), "the session is unregistered");
    }
    let beat = heartbeat_session_agent_for(root, root, Some("sid-a"), &binary(), pid).unwrap();
    assert!(matches!(beat, Heartbeat::Reregistered(_)), "{beat:?}");
    let row = own_row(root).expect("the prompt heartbeat registered the session");
    assert_eq!(row.claude_session_id.as_deref(), Some("sid-a"));
    assert!(row.ended_at.is_none());
    assert!(registry_entry_is_live(root, &row));
}

// AC2: delete a live session's row; the prompt hook brings it back with a
// newer heartbeat.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac7c7798 | ai:claude
#[test]
fn a_deleted_live_row_is_back_after_the_prompt_heartbeat_with_a_newer_heartbeat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let first = touch_session_start_agent_for(root, root, Some("sid-b"), &binary(), pid).unwrap();
    let path = registry_path(root, &first.id);
    std::fs::remove_file(&path).unwrap();

    let beat = heartbeat_session_agent_for(root, root, Some("sid-b"), &binary(), pid).unwrap();
    assert!(matches!(beat, Heartbeat::Reregistered(_)), "{beat:?}");
    let back = own_row(root).expect("row restored");
    assert!(back.last_active_at > first.last_active_at, "{back:?}");
    assert_eq!(back.claude_session_id.as_deref(), Some("sid-b"));
}

// AC2 / AC5: a present row is refreshed in place, so `last_active_at` moves
// past `started_at`.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac7c7798 | ai:claude
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac551a24 | ai:claude
#[test]
fn the_prompt_heartbeat_refreshes_last_active_at_and_keeps_started_at() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let mut row = touch_session_start_agent_for(root, root, Some("sid-c"), &binary(), pid).unwrap();
    let started = Utc::now() - Duration::hours(1);
    row.started_at = started;
    row.last_active_at = started;
    row.role = Some("advisor".into());
    rewrite(root, &row);

    let beat = heartbeat_session_agent_for(root, root, Some("sid-c"), &binary(), pid).unwrap();
    assert_eq!(beat, Heartbeat::Refreshed);
    let after = own_row(root).unwrap();
    assert_eq!(after.started_at, started);
    assert!(after.last_active_at > started);
    assert_eq!(
        after.role.as_deref(),
        Some("advisor"),
        "refresh keeps the row"
    );
}

// A gc that wrongly ended a live session's row is undone by its next prompt.
#[test]
fn a_live_sessions_ended_row_is_unended_by_its_heartbeat() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let mut row = touch_session_start_agent_for(root, root, Some("sid-d"), &binary(), pid).unwrap();
    row.ended_at = Some(row.started_at);
    rewrite(root, &row);
    assert!(!registry_entry_is_live(root, &own_row(root).unwrap()));

    heartbeat_session_agent_for(root, root, Some("sid-d"), &binary(), pid).unwrap();
    let after = own_row(root).unwrap();
    assert!(after.ended_at.is_none());
    assert!(registry_entry_is_live(root, &after));
}

fn recycled_row(pid: u32) -> AgentRegistryEntry {
    let at = Utc::now() - Duration::days(1);
    AgentRegistryEntry {
        id: agent_id(&detect_agent_type(), pid),
        agent_type: detect_agent_type(),
        pid,
        pid_start: Some("linux-starttime:1".into()),
        pid_namespace: current_pid_namespace(),
        name: Some("claude-advisor-01".into()),
        description: None,
        tty: None,
        terminal: None,
        started_at: at,
        last_active_at: at + Duration::hours(2),
        role: Some("advisor".into()),
        current_spec: None,
        worktree_path: PathBuf::from("/tmp/earlier"),
        source: "session-start-hook".into(),
        binary_version: None,
        build_sha: None,
        availability: Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: Some("earlier-session".into()),
        ended_at: None,
        spec_status_at_end: None,
        mail_wake: None,
        resumed_from: None,
    }
}

fn retired_rows(root: &Path) -> Vec<AgentRegistryEntry> {
    load_entries_in(&agents_dir(root).join("retired"))
        .into_iter()
        .map(|(_, e)| e)
        .collect()
}

// AC1: row names are per pid, so a reused pid used to erase the earlier
// process's row. It is now retired under a name unique to that process.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac2ed7f1 | ai:claude
#[test]
fn a_recycled_pids_row_is_retired_not_overwritten_at_session_start() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let old = recycled_row(pid);
    rewrite(root, &old);

    let new = touch_session_start_agent_for(root, root, Some("sid-e"), &binary(), pid).unwrap();
    assert_eq!(new.claude_session_id.as_deref(), Some("sid-e"));
    let retired = retired_rows(root);
    assert_eq!(retired.len(), 1, "{retired:?}");
    let kept = &retired[0];
    assert_eq!(kept.claude_session_id.as_deref(), Some("earlier-session"));
    assert_ne!(kept.id, new.id, "the retired row has its own id");
    assert_eq!(kept.ended_at, Some(old.last_active_at));
}

// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac2ed7f1 | ai:claude
#[test]
fn a_recycled_pids_row_is_retired_by_the_launcher_and_the_mcp_touch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let agent_type = detect_agent_type();
    rewrite(root, &recycled_row(pid));
    register_spawned_agent(
        root,
        &agent_type,
        pid,
        Some("reviewer".into()),
        None,
        root.to_path_buf(),
        Some(&binary()),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(retired_rows(root).len(), 1);

    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    rewrite(root, &recycled_row(pid));
    let entry = touch_mcp_agent(root, &binary()).unwrap();
    assert_ne!(entry.claude_session_id.as_deref(), Some("earlier-session"));
    assert_ne!(entry.name.as_deref(), Some("claude-advisor-01"));
    assert_eq!(retired_rows(root).len(), 1);
}

// AC5: with a refreshed heartbeat, gc ends a dead row at its last activity,
// not at its start.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac551a24 | ai:claude
#[test]
fn gc_ends_a_dead_row_at_its_last_heartbeat_not_its_start() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let mut row = recycled_row(u32::MAX - 1);
    row.pid_start = None;
    rewrite(root, &row);
    gc_dead_agents(root, false, None).unwrap();
    let ended = load_entries(root).remove(0).1;
    assert_eq!(ended.ended_at, Some(row.last_active_at));
    assert_ne!(ended.ended_at, Some(row.started_at));
}

// AC5, end to end: a one-prompt `claude -p` lane registers at its start and
// never heartbeats again, then runs for hours. Its launcher ends the row at
// the child's exit, so the recorded end is the real end, and a later gc keeps
// it rather than rewinding it to the start.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac551a24 | ai:claude
#[test]
fn a_headless_lane_that_ends_hours_after_its_one_prompt_is_ended_at_its_exit() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let sid = "6f1c2a43-1d2e-4b7a-9c3f-2a1b0c9d8e7f";
    let started = Utc::now() - Duration::hours(3);
    let mut row = recycled_row(u32::MAX - 1);
    row.pid_start = None;
    row.started_at = started;
    row.last_active_at = started;
    row.claude_session_id = Some(sid.into());
    rewrite(root, &row);

    let before_exit = Utc::now();
    crate::session::record_headless_session_end(root, sid);
    let ended = load_entries(root).remove(0).1;
    let end = ended.ended_at.expect("the lane's exit ended its row");
    assert!(
        end >= before_exit,
        "ended at its exit, not its start: {end}"
    );
    assert!(end - started >= Duration::hours(3), "{end} vs {started}");

    gc_dead_agents(root, false, None).unwrap();
    let after_gc = load_entries(root).remove(0).1;
    assert_eq!(after_gc.ended_at, Some(end), "gc keeps the recorded end");
}

// Only the exiting lane's own live row is ended; another session's row and an
// already-ended row are left alone.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac551a24 | ai:claude
#[test]
fn mark_session_ended_touches_only_the_live_row_of_that_session() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let mut other = recycled_row(u32::MAX - 2);
    other.claude_session_id = Some("other-session".into());
    rewrite(root, &other);
    assert_eq!(mark_session_ended(root, "lane-session").unwrap(), None);
    assert_eq!(mark_session_ended(root, "  ").unwrap(), None);
    let mut done = recycled_row(u32::MAX - 3);
    done.claude_session_id = Some("lane-session".into());
    done.ended_at = Some(done.last_active_at);
    rewrite(root, &done);
    assert_eq!(mark_session_ended(root, "lane-session").unwrap(), None);
    for (_, row) in load_entries(root) {
        let expected = (row.pid == u32::MAX - 3).then_some(done.last_active_at);
        assert_eq!(row.ended_at, expected, "{row:?}");
    }
}

// A row the prompt heartbeat registers stands in for the lost SessionStart
// write: it keeps the launcher's pending mail wake and now records the
// session's thread, which is what arming the wake needs.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
#[test]
fn a_reregistered_row_keeps_the_launch_mail_wake_and_records_the_thread() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let pid = std::process::id();
    let sid = "0b6c3d1e-8f7a-4e2d-9a1b-3c4d5e6f7a8b";
    let mut launched = touch_session_start_agent_for(root, root, None, &binary(), pid).unwrap();
    launched.mail_wake = Some(MailWake::Pending);
    launched.claude_session_id = None;
    rewrite(root, &launched);

    let beat = heartbeat_session_agent_for(root, root, Some(sid), &binary(), pid).unwrap();
    let Heartbeat::Reregistered(entry) = beat else {
        panic!("expected a re-registration, got {beat:?}");
    };
    assert_eq!(entry.mail_wake, Some(MailWake::Pending));
    assert_eq!(
        crate::mailbox_wake_run::row_thread(&entry).as_deref(),
        Some(sid)
    );
    let stored = own_row(root).unwrap();
    assert_eq!(stored.mail_wake, Some(MailWake::Pending));
    assert_eq!(stored.claude_session_id.as_deref(), Some(sid));
}

fn grant(id: &str, holders: Vec<(u32, bool)>) -> HeldGrant {
    HeldGrant {
        id: id.into(),
        seat: "advisor".into(),
        expires_at: Utc::now() + Duration::hours(3),
        holders,
    }
}

fn overdue_record(main: &Path) -> HelloRecord {
    HelloRecord {
        handle: "claude-implementer-9".into(),
        role: "implementer".into(),
        spec: Some("BUG-1".into()),
        path: main.display().to_string(),
        hello_id: uuid::Uuid::new_v4().to_string(),
        sent_at: Utc::now() - Duration::minutes(30),
        mailbox: main.join(".aida/mailbox").display().to_string(),
        home: HomeKind::Checkout,
        advisor_live: false,
        acked_at: None,
        acked_by: None,
        ack_reported: false,
        rotation: None,
    }
}

// AC3: the reason never asserts "no live advisor"; a live grant holder makes
// the advisor "live but unregistered".
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.acecfc34 | ai:claude
#[test]
fn the_unreachable_reason_says_registered_or_live_but_unregistered() {
    let tmp = TempDir::new().unwrap();
    let rec = overdue_record(tmp.path());
    let policy = Policy::default();

    let none = unreachable_reason(&rec, &AdvisorPresence::Unknown, &policy);
    assert!(none.starts_with("no registered advisor"), "{none}");
    assert!(!none.contains("no live advisor"), "{none}");

    let presence = presence_from(&[], &[grant("0123456789ab", vec![(4242, true)])], false);
    let AdvisorPresence::Unregistered(evidence) = &presence else {
        panic!("{presence:?}");
    };
    assert!(evidence.contains("pid 4242") && evidence.contains("01234567"));
    let State::Unreachable(live) = state(&rec, || presence.clone(), &policy, Utc::now()) else {
        panic!("expected unreachable");
    };
    assert!(live.contains("live but unregistered"), "{live}");

    // A grant whose holder is gone is no evidence; a recent advisor turn is.
    let dead = presence_from(&[], &[grant("dead", vec![(4243, false)])], false);
    assert_eq!(dead, AdvisorPresence::Unknown);
    assert!(matches!(
        presence_from(&[], &[], true),
        AdvisorPresence::Unregistered(_)
    ));
}

#[test]
fn held_grants_reads_only_unexpired_unrevoked_grants_for_the_seat() {
    let tmp = TempDir::new().unwrap();
    let now = Utc::now();
    let write = |id: &str, seat: &str, expires: DateTime<Utc>, revoked: bool| {
        let body = serde_json::json!({
            "id": id, "principal": "joe", "subject": "s", "session_id": "x",
            "seat": seat, "tty_issued_at": now, "delegable_seats": [],
            "parent_grant_id": null, "issued_at": now, "expires_at": expires,
            "revoked_at": if revoked { Some(now) } else { None },
            "binding": ["proc:4242@linux-starttime:7", "key:abc"],
        });
        std::fs::write(tmp.path().join(format!("{id}.json")), body.to_string()).unwrap();
    };
    write("a-live", "advisor", now + Duration::hours(1), false);
    write("b-expired", "advisor", now - Duration::hours(1), false);
    write("c-revoked", "advisor", now + Duration::hours(1), true);
    write("d-reviewer", "reviewer", now + Duration::hours(1), false);
    let grants = held_grants_in(tmp.path(), "advisor", now, |pid, start| {
        pid == 4242 && start == "linux-starttime:7"
    });
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0].id, "a-live");
    assert_eq!(grants[0].live_holder(), Some(4242));
}

// AC4: the doctor check reports the registered advisors, the unregistered
// evidence and the hello's state instead of nothing.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151.ac561615 | ai:claude
#[test]
fn the_doctor_report_names_advisors_grant_evidence_and_hello_state() {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path();
    save_record(main, &overdue_record(main)).unwrap();
    let census = Census::new(
        main,
        main.join(".aida-store"),
        Vec::new(),
        Policy::default(),
        Utc::now(),
    );
    let rows = report_findings(
        &census,
        &[grant("0123456789ab", vec![(4242, true)])],
        false,
        false,
    );
    let summaries: Vec<&str> = rows.iter().map(|r| r.summary.as_str()).collect();
    let has = |needle: &str| summaries.iter().any(|s| s.contains(needle));
    assert!(has("registered advisors: none"), "{summaries:#?}");
    assert!(has("advisor seat grant 01234567"), "{summaries:#?}");
    assert!(has("pid 4242 live"), "{summaries:#?}");
    assert!(has("an advisor is live but unregistered"), "{summaries:#?}");
    assert!(
        has("hello from claude-implementer-9") && has("live but unregistered"),
        "{summaries:#?}"
    );
    assert!(!has("no live advisor"), "{summaries:#?}");

    // From a sandbox, a holder that reads as gone is reported unobservable.
    let sandboxed = report_findings(&census, &[grant("feed", vec![(7, false)])], false, true);
    assert!(sandboxed
        .iter()
        .any(|r| r.summary.contains("not observable from this sandbox")));
}
