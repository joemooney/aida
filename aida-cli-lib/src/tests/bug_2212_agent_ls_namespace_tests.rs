// BUG-2212: `agent ls` from a sandboxed shell must not pass off "no visible
// process" as "no agents". trace:BUG-2212 | ai:claude
use super::*;

fn view(liveness: agent_registry::LivenessBasis) -> agent_registry::AgentRegistryView {
    let now = chrono::Utc::now();
    agent_registry::AgentRegistryView {
        id: "claude-4242".to_string(),
        agent_type: "claude".to_string(),
        pid: 4242,
        pid_start: None,
        name: None,
        description: None,
        tty: None,
        terminal: None,
        started_at: now,
        last_active_at: now,
        role: Some("implementer".to_string()),
        current_spec: None,
        worktree_path: std::path::PathBuf::from("/tmp/aida-fixture"),
        source: "session-start-hook".to_string(),
        binary_version: None,
        build_sha: None,
        status: agent_registry::AgentStatus::Idle,
        availability: agent_registry::Availability::Available,
        paused_since: None,
        paused_reason: None,
        expected_back: None,
        native_session_id: None,
        ended_at: None,
        resumed_from: None,
        liveness,
    }
}

const HOST: agent_registry::PidObserver = agent_registry::PidObserver {
    namespace: Some(agent_registry::INITIAL_PID_NAMESPACE),
    isolated: false,
};
const SANDBOX: agent_registry::PidObserver = agent_registry::PidObserver {
    namespace: Some(7),
    isolated: true,
};

#[test]
fn host_listing_has_no_notes() {
    assert!(unobserved_liveness_notes(&[], HOST).is_empty());
    assert!(
        unobserved_liveness_notes(&[view(agent_registry::LivenessBasis::Process)], HOST).is_empty()
    );
}

#[test]
fn empty_sandboxed_listing_says_liveness_is_unobservable() {
    let notes = unobserved_liveness_notes(&[], SANDBOX);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("cannot observe agent processes"));
    assert!(notes[0].contains("does not mean no agents are running"));
    assert!(notes[1].contains("outside the sandbox"));
}

#[test]
fn heartbeat_rows_are_footnoted_as_heartbeat_based() {
    let notes =
        unobserved_liveness_notes(&[view(agent_registry::LivenessBasis::Heartbeat)], SANDBOX);
    assert!(notes[0].starts_with("* heartbeat-based"), "{notes:?}");
    assert!(notes[0].contains("24h"));
    assert!(notes[1].contains("outside the sandbox"));
}

#[test]
fn notes_never_name_internal_spec_ids() {
    let id = regex::Regex::new(r"\b(BUG|TASK|STORY|FR|SPIKE)-\d+").unwrap();
    for notes in [
        unobserved_liveness_notes(&[], SANDBOX),
        unobserved_liveness_notes(&[view(agent_registry::LivenessBasis::Heartbeat)], HOST),
    ] {
        for note in notes {
            assert!(!id.is_match(&note), "{note}");
        }
    }
}
