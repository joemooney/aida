// trace:TASK-3424 | ai:codex
use super::*;
#[test]
fn published_spec_id_inputs_accept_opaque_nodes_and_reject_cross_store_text() {
    let descriptors = tool_descriptors();
    let tools = descriptors.as_array().expect("tool descriptor array");
    let mut checked = 0;
    for tool in tools {
        if let Some(properties) = tool["inputSchema"]["properties"].as_object() {
            for field in properties.values() {
                if field["pattern"].as_str()
                    == Some("^[A-Z]+-(?:[A-Za-z0-9][A-Za-z0-9_-]*-)?[0-9]+$")
                {
                    let regex = regex::Regex::new(field["pattern"].as_str().unwrap()).unwrap();
                    for valid in [
                        "BUG-001",
                        "BUG-7-001",
                        "BUG-0123456789abcdef0123456789abcdef-001",
                        "TASK-JM-201",
                        "BUG-1-2-003",
                        "BUG-team_a--west-003",
                    ] {
                        assert!(regex.is_match(valid), "{valid}");
                    }
                    assert!(!regex.is_match("other#BUG-001"));
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked >= 10,
        "published ID inputs were not exercised: {checked}"
    );
}
