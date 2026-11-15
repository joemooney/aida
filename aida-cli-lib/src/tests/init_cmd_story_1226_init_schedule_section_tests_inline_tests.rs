use super::*;

#[test]
fn init_schedule_section_lists_default_jobs() {
    let section = init_schedule_config_section();
    for job in [
        "session-reap",
        "doctor",
        "queue-gc",
        "mailbox-latency",
        "mailbox-triage",
        "product-wave-relaunch",
        "shelf-triage",
        // trace:TASK-1527 | ai:claude
        "cache-refresh-tick",
        "history-index-tick",
        "dispatch-tick",
        // trace:STORY-HOME-128 | ai:claude
        "mail-backlog-guard",
        "groom-drafts",
        "hub-drift-guard",
        "hub-drift-guard-route",
        "stranded-branches-guard",
        "stranded-branches-guard-route",
        "disk-headroom-guard",
        "disk-headroom-guard-route",
        "session-hygiene",
        "session-hygiene-route",
        "watchdog",
        "watchdog-route",
    ] {
        assert!(
            section.contains(&format!("name = \"{job}\"")),
            "{job} missing"
        );
    }
    assert!(section.contains("seats = [\"advisor\"]"));
    assert!(section.contains("seats = [\"product\"]"));
    assert!(section.contains("on = [\"MailReceived\"]"));
    assert!(section.contains("when = \"mail.oldest_unread_age > 15m\""));
    let parsed: toml::Value = toml::from_str(section).expect("defaults parse");
    let jobs = parsed["schedule"]["jobs"].as_array().unwrap();
    // The history tick and dispatch-tick join the four existing enabled defaults.
    // trace:STORY-1661 | ai:codex
    // trace:STORY-1917 | ai:agy
    // trace:STORY-HOME-128 | ai:claude
    // trace:STORY-2220 | ai:claude — plus the idle-only docs-sweep.
    assert_eq!(jobs.len(), 8);
    assert!(jobs
        .iter()
        .all(|job| job["enabled"].as_bool() == Some(true)));
    let dispatch = jobs
        .iter()
        .find(|job| job["name"].as_str() == Some("dispatch-tick"))
        .expect("dispatch-tick scaffolded");
    assert_eq!(dispatch["command"].as_str(), Some("dispatch tick"));
    assert_eq!(dispatch["every"].as_str(), Some("2m"));
    assert!(dispatch.get("when").is_none());
    assert_eq!(
        dispatch["while"].as_str(),
        Some("mandate.dispatch_active == true && dispatch.pending > 0")
    );
    // trace:TASK-1527 | ai:claude
    let tick = jobs
        .iter()
        .find(|job| job["name"].as_str() == Some("cache-refresh-tick"))
        .expect("cache-refresh-tick scaffolded");
    assert_eq!(
        tick["command"].as_str(),
        Some("cache refresh --if-requested")
    );
    assert_eq!(tick["every"].as_str(), Some("15m"));
    let history = jobs
        .iter()
        .find(|job| job["name"].as_str() == Some("history-index-tick"))
        .expect("history-index-tick scaffolded");
    assert_eq!(
        history["command"].as_str(),
        Some("cache refresh --history --if-behind")
    );
    assert_eq!(history["every"].as_str(), Some("15m"));
}
