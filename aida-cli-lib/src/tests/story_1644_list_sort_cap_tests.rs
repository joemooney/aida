//! STORY-1644: `aida list` sorts by natural numeric ID by default, the default
//! sort and the agent-mode row cap are configurable, and the cap announces
//! itself when it cuts rows. `aida history`'s limit is configurable the same
//! way. These pin the pure resolution rules; the end-to-end behaviour is in
//! `aida-cli/tests/story_1644_list_sort_cap.rs`.
//!
//! trace:STORY-1644 | ai:claude

use super::*;

fn write_config(root: &std::path::Path, body: &str) {
    let config_dir = root.join(".aida");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), body).unwrap();
}

// --- default sort ---

#[test]
fn default_sort_is_id_when_nothing_configured() {
    assert_eq!(resolve_list_sort(None, None), ("id".to_string(), None));
}

#[test]
fn config_default_sort_applies_without_flag() {
    assert_eq!(
        resolve_list_sort(None, Some("Modified")),
        ("modified".to_string(), None)
    );
}

#[test]
fn explicit_sort_flag_beats_config() {
    assert_eq!(
        resolve_list_sort(Some("heft"), Some("modified")),
        ("heft".to_string(), None)
    );
}

#[test]
fn unknown_config_sort_warns_and_uses_id() {
    let (sort, warning) = resolve_list_sort(None, Some("bogus"));
    assert_eq!(sort, "id");
    let w = warning.expect("an unknown config value must warn");
    assert!(w.contains("default_sort") && w.contains("bogus"), "{w}");
}

#[test]
fn reads_default_sort_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_list_default_sort(tmp.path()), None);
    write_config(tmp.path(), "[list]\ndefault_sort = \"created\"\n");
    assert_eq!(
        read_list_default_sort(tmp.path()).as_deref(),
        Some("created")
    );
}

// --- agent cap ---

#[test]
fn agent_cap_default_is_100() {
    assert_eq!(AGENT_LIST_DEFAULT_LIMIT, 100);
    assert_eq!(resolve_agent_list_limit(None, None), 100);
}

#[test]
fn agent_cap_env_beats_config_beats_default() {
    assert_eq!(resolve_agent_list_limit(None, Some(40)), 40);
    assert_eq!(resolve_agent_list_limit(Some("7"), Some(40)), 7);
    // Garbage env falls through to config, then the default.
    assert_eq!(resolve_agent_list_limit(Some("lots"), Some(40)), 40);
    assert_eq!(resolve_agent_list_limit(Some("lots"), None), 100);
    // A negative config value is ignored.
    assert_eq!(resolve_agent_list_limit(None, Some(-1)), 100);
}

#[test]
fn agent_cap_zero_is_unbounded() {
    assert_eq!(resolve_agent_list_limit(Some("0"), Some(40)), 0);
    assert_eq!(
        agent_list_default_cap(None, false, false, false, false, true, 0),
        None
    );
    assert_eq!(
        agent_list_default_cap(None, false, false, false, false, true, 12),
        Some(12)
    );
}

#[test]
fn reads_agent_limit_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_list_agent_limit(tmp.path()), None);
    write_config(tmp.path(), "[list]\nagent_limit = 250\n");
    assert_eq!(read_list_agent_limit(tmp.path()), Some(250));
}

#[test]
fn truncation_header_names_what_was_cut_and_how_to_see_it() {
    assert_eq!(
        agent_list_truncation_header(100, 142, "open", "id"),
        "count: 100 of 142 open — showing first 100 by id; newest hidden: --sort modified, or --limit 0"
    );
    let modified = agent_list_truncation_header(5, 9, "matched", "modified");
    assert!(modified.starts_with("count: 5 of 9 matched — showing first 5 by modified;"));
    assert!(modified.contains("--limit 0"), "{modified}");
}

// --- history limit ---

#[test]
fn history_limit_precedence() {
    assert_eq!(resolve_history_limit(None, None, None, 20), 20);
    assert_eq!(resolve_history_limit(None, None, Some(50), 20), 50);
    assert_eq!(resolve_history_limit(None, Some("30"), Some(50), 20), 30);
    assert_eq!(resolve_history_limit(Some(5), Some("30"), Some(50), 20), 5);
    // Zero / garbage env and non-positive config are ignored.
    assert_eq!(resolve_history_limit(None, Some("0"), Some(50), 20), 50);
    assert_eq!(resolve_history_limit(None, Some("x"), Some(0), 20), 20);
}

#[test]
fn reads_history_limit_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_history_limit(tmp.path()), None);
    write_config(tmp.path(), "[history]\nlimit = 75\n");
    assert_eq!(read_history_limit(tmp.path()), Some(75));
    write_config(tmp.path(), "[history]\nlimit = 0\n");
    assert_eq!(read_history_limit(tmp.path()), None);
}
