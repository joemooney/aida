//! The mediated MCP envelope never enters the ordinary writing dispatcher.
// trace:TASK-3223 | ai:codex
use super::*;

fn client() -> crate::store_mutation_client::Client {
    let env = aida_core::exec_context::Environment::from_pairs([
        ("AIDA_STORE_MUTATION_PROFILE".into(), "mediated".into()),
        (
            "AIDA_STORE_MUTATION_ENDPOINT".into(),
            format!("/run/aida-mutation/{}/service", uuid::Uuid::new_v4()).into(),
        ),
    ]);
    crate::store_mutation_client::Client::from_environment(&env)
        .unwrap()
        .unwrap()
}

fn request(method: &str, params: Value) -> JsonRpcRequest {
    serde_json::from_value(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})).unwrap()
}

#[test]
fn mediated_mcp_advertises_only_translated_tools_with_existing_profile_and_persona_limits() {
    let client = client();
    let store = Path::new("/missing-mcp-store");
    for (profile, role, count) in [
        (McpProfile::Full, None, 2),
        (McpProfile::ReadOnly, None, 0),
        (McpProfile::Full, Some("guest"), 0),
    ] {
        let response = mediated_mcp_response(
            &client,
            store,
            profile,
            role,
            &request("tools/list", json!({})),
        )
        .unwrap();
        assert_eq!(
            response.result.unwrap()["tools"].as_array().unwrap().len(),
            count
        );
    }
}

#[test]
fn mediated_mcp_refuses_unrouted_calls_and_extra_mutation_semantics_before_store_access() {
    let client = client();
    let store = Path::new("/missing-mcp-store");
    for (name, args) in [
        ("claim_task", json!({"id":"TASK-1"})),
        ("queue_done", json!({"id":"TASK-1"})),
        ("add_requirement", json!({"title":"no allocation"})),
        (
            "add_comment",
            json!({"id":"TASK-1","text":"hello","relayed_from":"advisor"}),
        ),
        (
            "update_requirement",
            json!({"id":"TASK-1","title":"ok","status":"Completed"}),
        ),
    ] {
        let response = mediated_mcp_response(
            &client,
            store,
            McpProfile::Full,
            None,
            &request("tools/call", json!({"name":name,"arguments":args})),
        )
        .unwrap();
        let result = response.result.unwrap();
        assert_eq!(result["isError"], true, "{name}: {result}");
        assert!(
            result.to_string().contains("UnsupportedOperation"),
            "{result}"
        );
    }
}

#[test]
fn mediated_mcp_initialization_and_notifications_have_no_registry_or_backend_setup() {
    let client = client();
    let store = Path::new("/missing-mcp-store");
    let initialize = mediated_mcp_response(
        &client,
        store,
        McpProfile::Full,
        None,
        &request("initialize", json!({})),
    )
    .unwrap();
    assert_eq!(initialize.result.unwrap()["serverInfo"]["name"], "aida");
    let mut notification = request("notifications/initialized", json!({}));
    notification.id = None;
    assert!(mediated_mcp_response(&client, store, McpProfile::Full, None, &notification).is_none());
}
