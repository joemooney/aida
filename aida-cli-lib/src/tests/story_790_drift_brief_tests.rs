//! STORY-790 review findings (PR #1634): the resume drift brief must render
//! spec status THEN vs NOW and the coordination drift (mail + briefs) scoped
//! to the previous session's ended_at.

use chrono::{Duration, Utc};

fn ended_entry(
    tmp: &std::path::Path,
    name: &str,
    spec_status_at_end: Option<&str>,
    ended_ago: Duration,
) -> crate::agent_registry::AgentRegistryEntry {
    crate::agent_registry::AgentRegistryEntry {
        id: format!("{name}#1"),
        agent_type: "claude".into(),
        pid: 4_294_967_294,
        name: Some(name.to_string()),
        tty: None,
        started_at: Utc::now() - Duration::hours(2),
        last_active_at: Utc::now() - ended_ago,
        role: Some("implementer".into()),
        current_spec: Some("STORY-790".into()),
        worktree_path: tmp.to_path_buf(),
        source: "agent-launcher".into(),
        binary_version: None,
        build_sha: None,
        availability: Default::default(),
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        claude_session_id: None,
        ended_at: Some(Utc::now() - ended_ago),
        resumed_from: None,
        spec_status_at_end: spec_status_at_end.map(str::to_string),
    }
}

#[test]
fn mail_since_exit_counts_addressed_and_broadcast_not_others_or_older() {
    let tmp = tempfile::tempdir().unwrap();
    let ended_at = Utc::now() - Duration::minutes(30);
    let mk = |id: &str, to: aida_core::mailbox::Recipient, ts: i64| aida_core::mailbox::Message {
        id: id.into(),
        thread_id: id.into(),
        from: "advisor".into(),
        to,
        timestamp: ts,
        in_reply_to: None,
        body: "x".into(),
        urgent: false,
        intent: Default::default(),
        retracted: false,
        deleted: false,
    };
    let fresh = Utc::now().timestamp_millis();
    let stale = (Utc::now() - Duration::hours(2)).timestamp_millis();
    for m in [
        mk(
            "m1",
            aida_core::mailbox::Recipient::Agent("claude-1".into()),
            fresh,
        ),
        mk("m2", aida_core::mailbox::Recipient::Broadcast, fresh),
        mk(
            "m3",
            aida_core::mailbox::Recipient::Agent("someone-else".into()),
            fresh,
        ),
        mk(
            "m4",
            aida_core::mailbox::Recipient::Agent("claude-1".into()),
            stale,
        ),
    ] {
        crate::mailbox_store::write_message(tmp.path(), &m).unwrap();
    }
    let entry = ended_entry(tmp.path(), "claude-1", None, Duration::minutes(30));
    assert_eq!(
        crate::mail_count_for_agent_since(tmp.path(), &entry, ended_at),
        2,
        "addressed + broadcast since exit count; other-recipient and pre-exit mail do not"
    );
}

#[test]
fn briefs_since_exit_gate_on_the_cutoff() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp
        .path()
        .join(".aida")
        .join("agent-briefs")
        .join("claude-1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("b1.md"), "brief").unwrap();
    std::fs::write(dir.join("b2.md"), "brief").unwrap();
    let entry = ended_entry(tmp.path(), "claude-1", None, Duration::minutes(30));
    let before_files = Utc::now() - Duration::hours(1);
    let after_files = Utc::now() + Duration::hours(1);
    assert_eq!(
        crate::brief_count_for_agent_since(tmp.path(), &entry, before_files),
        2
    );
    assert_eq!(
        crate::brief_count_for_agent_since(tmp.path(), &entry, after_files),
        0,
        "briefs filed before the previous exit are not drift"
    );
}

#[test]
fn spec_status_at_end_survives_a_registry_round_trip() {
    // The then-half of then-vs-now: the field serializes, and legacy records
    // without it deserialize as None.
    let tmp = tempfile::tempdir().unwrap();
    let entry = ended_entry(
        tmp.path(),
        "claude-1",
        Some("In Progress"),
        Duration::minutes(5),
    );
    let toml_body = toml::to_string(&entry).unwrap();
    assert!(toml_body.contains("spec_status_at_end"));
    let back: crate::agent_registry::AgentRegistryEntry = toml::from_str(&toml_body).unwrap();
    assert_eq!(back.spec_status_at_end.as_deref(), Some("In Progress"));
    let legacy: crate::agent_registry::AgentRegistryEntry =
        toml::from_str(&toml_body.replace(&format!("spec_status_at_end = \"In Progress\"\n"), ""))
            .unwrap();
    assert_eq!(legacy.spec_status_at_end, None);
}
