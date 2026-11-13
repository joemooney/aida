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

// Exercise successful tools/call replies through the real broker and durable
// host writer, then compare their envelope to tools/list's schema. A rejected
// call cannot cover a malformed success reply after a committed mutation.
// trace:TASK-3223 | ai:codex
#[cfg(test)] // Explicit test boundary for the source writer inventory.
#[test]
fn mediated_mcp_committed_success_matches_advertised_output_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let route = crate::launch::contained::MediatedTestRoute::at(tmp.path().join("store"));
    let client = crate::store_mutation_client::Client::fixture(
        route.endpoint().to_str().unwrap().into(),
        route.epoch(),
    );
    let listed = mediated_mcp_response(
        &client,
        &route.root(),
        McpProfile::Full,
        None,
        &request("tools/list", json!({})),
    )
    .unwrap()
    .result
    .unwrap();
    for (name, arguments) in [
        (
            "add_comment",
            json!({"id":route.origin,"text":"schema accepted comment"}),
        ),
        (
            "update_requirement",
            json!({"id":route.origin,"title":"schema accepted title","weight":0.75,"tags":["alpha", " beta ", "alpha", ""]}),
        ),
    ] {
        let result = mediated_mcp_response(
            &client,
            &route.root(),
            McpProfile::Full,
            None,
            &request("tools/call", json!({"name":name,"arguments":arguments})),
        )
        .unwrap()
        .result
        .unwrap();
        assert_ne!(result["isError"], true, "{result}");
        let descriptor = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        let schema = &descriptor["outputSchema"];
        // Both the top-level result and the structured mirror use the
        // descriptor's content-array schema and must carry its required keys.
        for (envelope, schema) in [
            (&result, schema),
            (
                &result["structuredContent"],
                &schema["properties"]["structuredContent"],
            ),
        ] {
            assert_eq!(schema["type"], "object");
            for key in schema["required"].as_array().unwrap() {
                assert!(envelope.get(key.as_str().unwrap()).is_some(), "{envelope}");
            }
            let items = envelope["content"].as_array().unwrap();
            assert_eq!(items.len(), 1);
            let item_schema = &schema["properties"]["content"]["items"];
            for key in item_schema["required"].as_array().unwrap() {
                assert!(items[0][key.as_str().unwrap()].is_string());
            }
            assert_eq!(items[0]["type"], item_schema["properties"]["type"]["const"]);
            serde_json::from_str::<Value>(items[0]["text"].as_str().unwrap()).unwrap();
        }
        assert_eq!(result["structuredContent"]["content"], result["content"]);
    }
    let row = route.row();
    assert_eq!(row.title, "schema accepted title");
    assert_eq!(row.weight, Some(0.75));
    assert_eq!(
        row.tags,
        ["alpha".to_owned(), "beta".to_owned()]
            .into_iter()
            .collect()
    );
    assert_eq!(row.comments.len(), 1);
    assert_eq!(row.comments[0].content, "schema accepted comment");
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
        let result = response.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), count);
        // trace:TASK-3223 | ai:codex
        for tool in tools {
            if tool["name"] == "update_requirement" {
                assert_eq!(
                    tool["description"],
                    "Update an owned requirement's title, description, priority, weight or nonstructural tags through the mediated store service. At least one edit field is required."
                );
                let properties = tool["inputSchema"]["properties"].as_object().unwrap();
                assert!(properties.contains_key("priority"));
                assert_eq!(properties["tags"]["type"], "array");
                assert!(!properties.contains_key("force"));
                assert_eq!(properties["weight"]["type"], json!(["number", "null"]));
                assert!(!properties.contains_key("status"));
                assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            }
        }
    }
}

#[test]
fn mediated_mcp_refuses_unrouted_calls_and_extra_mutation_semantics_before_store_access() {
    let client = client();
    let store = Path::new("/missing-mcp-store");
    for (name, args) in [
        (
            "update_requirement",
            json!({"id":"TASK-1", "tags":[], "force":true}),
        ),
        (
            "update_requirement",
            json!({"id":"TASK-1", "tags":[], "replace_tags":true}),
        ),
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

#[test]
fn mediated_mcp_invalid_role_refuses_before_reading_stdio() {
    use std::os::unix::ffi::OsStringExt;
    let environment = aida_core::exec_context::Environment::from_pairs([
        ("AIDA_MCP_PROFILE".into(), "full".into()),
        (
            "AIDA_SESSION_ROLE".into(),
            std::ffi::OsString::from_vec(vec![0xff]),
        ),
    ]);
    let error = run_mediated_mcp_server(&client(), &environment, Path::new("/missing-mcp-store"))
        .unwrap_err();
    assert_eq!(error.to_string(), "invalid session role");
}

#[cfg(test)] // Explicit test boundary for the config trust source guard.
#[test]
fn mediated_mcp_non_string_config_profile_refuses_before_stdio() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".aida")).unwrap();
    let store = tmp.path().join(".aida-store");
    for value in ["3", "true", "[]", "{ nested = 'full' }", "'unknown'"] {
        std::fs::write(
            tmp.path().join(".aida/config.toml"),
            format!("[mcp]\nprofile = {value}\n"),
        )
        .unwrap();
        let error = run_mediated_mcp_server(
            &client(),
            &aida_core::exec_context::Environment::default(),
            &store,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "invalid MCP profile", "{value}");
    }
}

// trace:TASK-3223 | ai:codex
#[test]
fn mediated_mcp_oversized_lines_error_and_preserve_the_next_request() {
    // Exercise newline at the limit read, beyond it, and missing at EOF.
    // Small BufReader capacity forces drainage across multiple input chunks.
    for (size, terminated) in [
        (65536, true),
        (65537, true),
        (200000, true),
        (200000, false),
    ] {
        let mut bytes = vec![b'x'; size];
        if terminated {
            bytes.push(b'\n');
            bytes.extend_from_slice(
                b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"initialize\",\"params\":{}}\n",
            );
        }
        let mut input = std::io::BufReader::with_capacity(127, bytes.as_slice());
        let mut output = Vec::new();
        mediated_mcp_stream(
            &client(),
            Path::new("/missing-mcp-store"),
            McpProfile::Full,
            None,
            &mut input,
            &mut output,
        )
        .unwrap();
        let responses: Vec<Value> = output
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(responses.len(), if terminated { 2 } else { 1 });
        assert_eq!(responses[0]["id"], Value::Null);
        assert_eq!(responses[0]["error"]["code"], -32600);
        if terminated {
            assert_eq!(responses[1]["id"], 7);
            assert_eq!(responses[1]["result"]["serverInfo"]["name"], "aida");
        }
    }
}

// trace:TASK-3223 | ai:codex
#[test]
fn mediated_mcp_closed_refusals_preserve_machine_codes_through_request_context() {
    for (token, code, recoverable) in [
        ("Unauthorized", "permission_denied", false),
        ("Expired", "permission_denied", false),
        ("Revoked", "permission_denied", false),
        ("UnsupportedOperation", "invalid_arg", true),
        ("UnsupportedVersion", "invalid_arg", true),
        ("InvalidRequest", "invalid_arg", true),
        ("Conflict", "conflict", false),
        ("Busy", "conflict", false),
        ("Unavailable", "io_error", false),
        ("OutcomeUnknown", "outcome_unknown", false),
    ] {
        let error = anyhow::anyhow!("{token}")
            .context("store service request 00000000-0000-0000-0000-000000000001");
        let result = mediated_mcp_error("add_comment", &error).to_result_value();
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredError"]["code"], code, "{token}");
        assert_eq!(result["structuredError"]["recoverable"], recoverable);
        assert!(result["structuredError"]["message"]
            .as_str()
            .unwrap()
            .contains(token));
        assert!(result["structuredError"]["message"]
            .as_str()
            .unwrap()
            .contains("00000000-0000-0000-0000-000000000001"));
    }
    let error = anyhow::anyhow!("unrecognized OutcomeUnknown diagnostic");
    assert_eq!(
        mediated_mcp_error("add_comment", &error).code,
        McpErrorCode::Internal
    );
}
