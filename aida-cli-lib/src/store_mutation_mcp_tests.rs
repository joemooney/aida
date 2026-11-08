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
