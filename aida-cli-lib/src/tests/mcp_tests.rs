// trace:TASK-3424 | ai:codex
use super::*;
use clap::Parser;
use std::sync::Arc;
use std::thread;
use tempfile::tempdir;

// trace:TASK-1526 | ai:codex
#[test]
fn mcp_read_tool_result_carries_cache_object_each_call() {
    use aida_core::db::cache_refresh::*;
    for state in [
        RefreshState::Deferred,
        RefreshState::WriterBusy,
        RefreshState::WorkerRunning,
    ] {
        let scope = CacheReadScope::new();
        record_read(Some(StaleServe {
            stale: true,
            cache_head: Some("old".into()),
            store_head: "new".into(),
            built_at: None,
            refreshing: state,
        }));
        let result = cache_labelled_tool_result("rows".into(), &scope);
        assert_eq!(result["structuredContent"]["cache"]["stale"], true);
        assert_eq!(
            result["structuredContent"]["cache"]["refreshing"],
            serde_json::to_value(state).unwrap()
        );
        assert_eq!(result["content"].as_array().unwrap().len(), 2);
        assert_eq!(result["content"][1]["text"], scope.stale().unwrap().note());
    }
    {
        let scope = CacheReadScope::new();
        record_read(Some(StaleServe {
            stale: true,
            cache_head: None,
            store_head: "new".into(),
            built_at: None,
            refreshing: RefreshState::Deferred,
        }));
        let result = cache_labelled_result(
            json!({"isError": true, "content": [{"type": "text", "text": "failed"}], "structuredError": {"code": "failed"}}),
            &scope,
        );
        assert_eq!(result["structuredContent"]["cache"]["stale"], true);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredError"]["code"], "failed");
    }
    let scope = CacheReadScope::new();
    record_read(None);
    let result = cache_labelled_tool_result("rows".into(), &scope);
    assert_eq!(result["structuredContent"]["cache"]["stale"], false);
    drop(scope);
    let scope = CacheReadScope::new();
    assert!(
        cache_labelled_tool_result("untouched".into(), &scope)["structuredContent"]
            .get("cache")
            .is_none()
    );
}

// trace:STORY-1468 | ai:codex
#[test]
fn origin_prompt_mcp_capture_redaction_and_explicit_retrieval() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    for redacted in [false, true] {
        let response = server.tool_add_requirement(&json!({
                "title": "Normal feature", "description": "Derived acceptance", "type": "story",
                "origin_prompt": {"text": "HISTORICAL_PROMPT_SENTINEL [REDACTED]", "redacted": redacted}
            })).unwrap();
        assert!(!response.contains("HISTORICAL_PROMPT_SENTINEL"));
        let id = added_spec_id(&response);
        for args in [
            json!({"id": id, "include_git": false}),
            json!({"id": id, "include_git": false, "verbose": true}),
        ] {
            assert!(!server
                .tool_show_requirement(&args)
                .unwrap()
                .contains("HISTORICAL_PROMPT_SENTINEL"));
        }
        assert!(!server
            .tool_list_requirements(&json!({}))
            .unwrap()
            .contains("HISTORICAL_PROMPT_SENTINEL"));
        assert!(!server
            .tool_search_requirements(&json!({"query": "HISTORICAL_PROMPT_SENTINEL"}))
            .unwrap()
            .contains("Normal feature"));
        server
            .tool_update_requirement(&json!({"id": id, "description": "New description"}))
            .unwrap();
        let output = server
            .tool_show_requirement(&json!({"id": id, "include_origin_prompt": true}))
            .unwrap();
        assert!(output.contains("HISTORICAL_PROMPT_SENTINEL"));
        assert!(output.contains("untrusted historical user input"));
        assert!(output.contains(if redacted { "Redacted" } else { "Captured" }));
        let serialized = serde_json::to_string(&server.storage.load().unwrap()).unwrap();
        assert!(!serialized.contains("HISTORICAL_PROMPT_SENTINEL"));
    }
    let ordinary = server
        .tool_add_requirement(
            &json!({"title": "No capture", "description": "Derived", "type": "task"}),
        )
        .unwrap();
    assert_eq!(
        server
            .tool_show_requirement(
                &json!({"id": added_spec_id(&ordinary), "include_origin_prompt": true})
            )
            .unwrap(),
        "No originating prompt captured"
    );
    let corpus = dir.path().join("corpus");
    crate::regen::export_corpus(server.storage.path(), &corpus, true).unwrap();
    for file in ["corpus.json", "manifest.json"] {
        assert!(!std::fs::read_to_string(corpus.join(file))
            .unwrap()
            .contains("HISTORICAL_PROMPT_SENTINEL"));
    }
    assert!(!server
        .tool_history(&json!({"events": true}))
        .unwrap()
        .contains("HISTORICAL_PROMPT_SENTINEL"));
    let before = server.storage.load().unwrap().requirements.len();
    let error = server.tool_add_requirement(&json!({"title": "Refused", "description": "Derived", "type": "task", "origin_prompt": {"text": "password=do-not-store-this"}})).unwrap_err();
    assert!(!error.contains("do-not-store-this"));
    assert_eq!(server.storage.load().unwrap().requirements.len(), before);
}

// trace:STORY-1468 | ai:codex
#[test]
fn origin_prompt_mcp_optional_null_and_unknown_fields() {
    let dir = tempdir().unwrap();
    let legacy = mk_server(dir.path());
    let response = legacy
        .tool_add_requirement(&json!({
            "title": "No capture", "description": "Derived", "type": "task",
            "origin_prompt": null
        }))
        .unwrap();
    assert!(response.contains("Requirement added:"));
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let response = server
        .tool_add_requirement(&json!({
            "title": "Default disposition", "description": "Derived", "type": "task",
            "origin_prompt": {"text": "Reviewed historical request", "redacted": null}
        }))
        .unwrap();
    let view = server
        .tool_show_requirement(&json!({
            "id": added_spec_id(&response), "include_origin_prompt": true
        }))
        .unwrap();
    assert!(view.contains("Captured"));
    let before = server.storage.load().unwrap().requirements.len();
    for input in [
        json!({"text": "Reviewed request", "redact": true}),
        json!(false),
    ] {
        assert!(server
            .tool_add_requirement(&json!({
                "title": "Refused", "description": "Derived", "type": "task",
                "origin_prompt": input
            }))
            .is_err());
    }
    assert_eq!(server.storage.load().unwrap().requirements.len(), before);
}

#[test]
fn origin_prompt_mcp_parent_link_is_persisted() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let parent = server
        .tool_add_requirement(&json!({
            "title": "Parent", "description": "Derived", "type": "story"
        }))
        .unwrap();
    let parent_spec = added_spec_id(&parent);
    let child = server
        .tool_add_requirement(&json!({
            "title": "Child", "description": "Derived", "type": "task", "parent": parent_spec,
            "origin_prompt": {"text": "Reviewed child request"}
        }))
        .unwrap();
    assert!(child.contains(&format!("linked under {parent_spec}")));
    let store = server.storage.load().unwrap();
    let parent = store
        .get_requirement_unambiguous(parent_spec)
        .unwrap()
        .unwrap();
    let child = store
        .get_requirement_unambiguous(added_spec_id(&child))
        .unwrap()
        .unwrap();
    assert!(parent
        .relationships
        .iter()
        .any(|r| r.target_id == child.id && r.rel_type == RelationshipType::Parent));
    assert!(server
        .storage
        .read_origin_prompt(child.id)
        .unwrap()
        .is_some());
}

fn mk_server(dir: &Path) -> McpServer<'static> {
    // Cache-backed Storage isn't required for the coordination-only tools;
    // we still need *a* Storage to instantiate the server. Point it at a
    // throwaway YAML file under the temp dir.
    let cache_path = dir.join(".aida").join("mcp-test-cache.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    McpServer::new(storage, dir.to_path_buf())
}

/// BUG-1617: the `history` MCP tool's JSON now carries a top-level
/// `window_exhausted` boolean alongside `count`/`events`. This is a
/// wiring/shape test — a small fixture (one status flip) is nowhere
/// near the tool's 500-commit window floor, so `window_exhausted` is
/// always false here; the exhaustion-detection *logic* itself (true vs.
/// false depending on where the commit cap lands relative to the
/// requested limit) is covered directly against
/// `collect_filtered_events` in `history.rs`'s own tests, cheaply,
/// without needing hundreds of commits to trip the MCP tool's fixed
/// floor.
// trace:BUG-1617 | ai:claude
#[test]
fn tool_history_json_carries_window_exhausted_field() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    std::fs::create_dir_all(&store_root).unwrap();

    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&store_root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);

    let rel = std::path::Path::new("objects/BUG/000/BUG-1.yaml");
    let full = store_root.join(rel);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "seed"]);
    std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Approved\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "flip"]);

    let storage = Storage::new(&store_root);
    let server = McpServer::new(&storage, dir.path().to_path_buf());

    let response = server
        .tool_history(&json!({"limit": 5}))
        .expect("tool_history should succeed against a fresh git fixture");
    let parsed: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
            parsed.get("window_exhausted"),
            Some(&Value::Bool(false)),
            "plenty of window left relative to the tiny fixture — expected window_exhausted: false, got: {parsed}"
        );
    assert!(
        parsed.get("count").and_then(Value::as_u64).unwrap_or(0) >= 1,
        "expected at least the one status-change event, got: {parsed}"
    );
}

/// TASK-1512: the `history` tool takes `to`/`from`/`opened` with the
/// CLI's semantics: the same status spellings, the same refusal for an
/// unknown status or `shipped` combined with `to`/`opened`, and the
/// union of creations and transitions.
// trace:TASK-1512 | ai:claude
#[test]
fn task_1512_mcp_history_transition_and_opened_filters() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("store");
    std::fs::create_dir_all(&store_root).unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&store_root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    let bug1 = store_root.join("objects/BUG/000/BUG-1.yaml");
    let bug2 = store_root.join("objects/BUG/000/BUG-2.yaml");
    std::fs::create_dir_all(bug1.parent().unwrap()).unwrap();
    std::fs::write(&bug1, "spec_id: BUG-1\ntitle: t\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "file 1"]);
    std::fs::write(&bug1, "spec_id: BUG-1\ntitle: t\nstatus: InProgress\n").unwrap();
    std::fs::write(&bug2, "spec_id: BUG-2\ntitle: u\nstatus: Approved\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "start 1, file 2 approved"]);
    std::fs::write(&bug1, "spec_id: BUG-1\ntitle: t\nstatus: Approved\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "bounce 1"]);

    let storage = Storage::new(&store_root);
    let server = McpServer::new(&storage, dir.path().to_path_buf());
    let kinds = |args: Value| -> Vec<(String, String)> {
        let v: Value = serde_json::from_str(&server.tool_history(&args).unwrap()).unwrap();
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_string(),
                    e["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());

    assert_eq!(
        kinds(json!({"from": "in_progress", "to": "accepted"})),
        vec![pair("BUG-1", "status_change")]
    );
    assert_eq!(
        kinds(json!({"opened": true})),
        vec![pair("BUG-2", "added"), pair("BUG-1", "added")]
    );
    assert_eq!(
        kinds(json!({"opened": true, "to": "approved"})),
        vec![
            pair("BUG-1", "status_change"),
            pair("BUG-2", "added"),
            pair("BUG-1", "added")
        ]
    );
    let err = server.tool_history(&json!({"to": "nope"})).unwrap_err();
    assert!(err.contains("expected one of"), "{err}");
    let err = server
        .tool_history(&json!({"shipped": true, "opened": true}))
        .unwrap_err();
    assert!(err.contains("shipped"), "{err}");
    let err = server
        .tool_history(&json!({"from": "approved", "to": "accepted"}))
        .unwrap_err();
    assert!(err.contains("both `Approved`"), "{err}");

    // Every refusal reaches the client as `invalid_arg`, not `internal`.
    for args in [
        json!({"shipped": true, "opened": true}),
        json!({"shipped": true, "to": "approved"}),
        json!({"from": "in-progress", "to": "in_progress"}),
        json!({"to": "nope"}),
    ] {
        let resp = server.handle_tools_call(
            &json!(1),
            &json!({"name": "history", "arguments": args.clone()}),
        );
        let result = resp.result.expect("tools/call returns a result");
        assert_eq!(result["isError"], json!(true), "{args}");
        assert_eq!(
            result["structuredError"]["code"],
            json!("invalid_arg"),
            "{args}: {result}"
        );
    }
}

/// TASK-1505 slice 2: the `history` tool reports where its answer came
/// from. With the index switched on it answers from the index as of the
/// store HEAD (`source: history-cache`, `index_tip: <HEAD>`); switched
/// off it walks git (`source: git-walk`, `index_tip: null`), and the
/// events are the same either way. The store lives one level down in the
/// temp dir so the index file lands inside it.
// trace:TASK-1508 | ai:claude
#[test]
fn task_1508_mcp_history_reports_source_field() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join("proj").join("store");
    std::fs::create_dir_all(&store_root).unwrap();
    let git = |args: &[&str]| -> String {
        let out = crate::test_git::git_command()
            .arg("-C")
            .arg(&store_root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["config", "commit.gpgsign", "false"]);
    let full = store_root.join("objects/BUG/000/BUG-1.yaml");
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Draft\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "seed"]);
    std::fs::write(&full, "spec_id: BUG-1\ntitle: t\nstatus: Approved\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "flip"]);
    let head = git(&["rev-parse", "HEAD"]);

    let storage = Storage::new(&store_root);
    let server = McpServer::new(&storage, dir.path().to_path_buf());
    let call = || -> Value {
        serde_json::from_str(&server.tool_history(&json!({"limit": 5})).unwrap()).unwrap()
    };

    crate::history_cache::set_test_serve_enabled(true);
    let served = call();
    crate::history_cache::set_test_serve_enabled(false);
    assert_eq!(served["source"], json!("history-cache"), "{served}");
    assert_eq!(served["index_tip"], json!(head), "{served}");

    let walked = call();
    assert_eq!(walked["source"], json!("git-walk"), "{walked}");
    assert_eq!(walked["index_tip"], Value::Null, "{walked}");
    assert_eq!(served["events"], walked["events"]);
    assert_eq!(served["count"], walked["count"]);
    assert_eq!(served["window_exhausted"], walked["window_exhausted"]);
}

#[test]
// trace:TASK-1278 | ai:codex
fn protocol_resource_uses_the_canonical_cli_renderer() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let mut store = aida_core::RequirementsStore::default();
    aida_core::seed_meta_requirements(&mut store).unwrap();
    server.storage.save(&store).unwrap();

    let expected = aida_core::resolve_protocol(&store, "spike", Some("research"))
        .unwrap()
        .render();
    assert_eq!(
        server.resource_protocol("spike/research").unwrap(),
        expected
    );
    assert!(server.resource_protocol("spike/missing").is_err());
}

/// BUG-449: set a gated status directly in the store, bypassing the MCP
/// gate — mirrors how the advisor (CLI) or a merge auto-bump sets
/// Approved/Planned/Completed out-of-band. Tests use this to reach a
/// precondition the MCP caller itself is (correctly) forbidden to set.
fn force_status(server: &McpServer<'static>, spec_id: &str, status: RequirementStatus) {
    let mut store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id_mut(spec_id)
        .expect("spec should exist for force_status");
    req.status = status;
    server.storage.save(&store).unwrap();
}

fn added_spec_id(response: &str) -> &str {
    response
        .strip_prefix("Requirement added: ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("unexpected add_requirement response: {response}"))
}

// ===================================================================
// STORY-401: stable structured error shapes
// ===================================================================

#[test]
fn mcp_error_classify_invalid_arg() {
    let e = McpError::classify("post_punt", "Missing required parameter: spec_id");
    assert_eq!(e.code, McpErrorCode::InvalidArg);
    assert!(e.code.recoverable());
    assert_eq!(e.tool, "post_punt");
    // short machine-friendly text shape `<tool>: <code>: <message>`
    assert_eq!(
        e.envelope_text(),
        "post_punt: invalid_arg: Missing required parameter: spec_id"
    );
}

#[test]
fn mcp_error_classify_not_found() {
    let e = McpError::classify("show_requirement", "Requirement 'TASK-999' not found");
    assert_eq!(e.code, McpErrorCode::NotFound);
    assert!(e.code.recoverable());

    let unknown = McpError::classify(
        "definitely_not_a_tool",
        "Unknown tool: definitely_not_a_tool",
    );
    assert_eq!(unknown.code, McpErrorCode::NotFound);
}

#[test]
fn mcp_error_classify_conflict_and_permission() {
    let conflict = McpError::classify("claim_task", "lease already claimed by another agent");
    assert_eq!(conflict.code, McpErrorCode::Conflict);
    assert!(!conflict.code.recoverable());

    let denied = McpError::classify(
        "update_requirement",
        "MCP is not advisor authority; cannot self-advance to Planned",
    );
    assert_eq!(denied.code, McpErrorCode::PermissionDenied);
    assert!(!denied.code.recoverable());
}

#[test]
fn mcp_error_classify_falls_back_to_internal() {
    let e = McpError::classify("history", "something totally unexpected happened");
    assert_eq!(e.code, McpErrorCode::Internal);
    assert!(!e.code.recoverable());
}

#[test]
fn mcp_error_result_value_is_back_compatible_envelope() {
    let e = McpError::classify("post_punt", "Missing required parameter: spec_id");
    let v = e.to_result_value();
    // Back-compatible: isError:true + a text content array.
    assert_eq!(v["isError"], json!(true));
    let text = v["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Missing required parameter: spec_id"));
    assert_eq!(v["content"][0]["type"], json!("text"));
    // Additive structured payload.
    let se = &v["structuredError"];
    assert_eq!(se["code"], json!("invalid_arg"));
    assert_eq!(se["message"], json!("Missing required parameter: spec_id"));
    assert_eq!(se["tool"], json!("post_punt"));
    assert_eq!(se["recoverable"], json!(true));
}

#[test]
fn mcp_dispatch_unknown_tool_yields_structured_error() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let resp = server.handle_tools_call(
        &json!(1),
        &json!({"name": "definitely_not_a_tool", "arguments": {}}),
    );
    let result = resp
        .result
        .expect("tools/call should return a result object");
    assert_eq!(result["isError"], json!(true));
    // Stable code + the legacy "Unknown tool" phrase the stdio suite asserts.
    assert_eq!(result["structuredError"]["code"], json!("not_found"));
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Unknown tool"), "text was: {text}");
}

// STORY-399 (Path B): a successful tools/call must carry `structuredContent`
// whose `content` array mirrors the legacy text envelope verbatim, so both
// legacy text consumers and schema-driven clients see the same logical
// result. Error responses must NOT carry structuredContent (they carry the
// STORY-401 `structuredError` payload instead). trace:STORY-399 | ai:claude
#[test]
fn mcp_dispatch_success_emits_structured_content_mirroring_text() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // `list_active_leases` takes no args and never errors on an empty store —
    // a clean success-path probe of the dispatch envelope.
    let resp = server.handle_tools_call(
        &json!(1),
        &json!({"name": "list_active_leases", "arguments": {}}),
    );
    let result = resp
        .result
        .expect("tools/call should return a result object");

    // Legacy text envelope preserved.
    assert!(
        result.get("isError").is_none(),
        "success must not set isError"
    );
    let text = result["content"][0]["text"]
        .as_str()
        .expect("success must carry a text content item");
    assert_eq!(result["content"][0]["type"], json!("text"));

    // Path B: structuredContent mirrors the same payload.
    let structured = result
        .get("structuredContent")
        .expect("success must carry structuredContent (STORY-399)");
    assert_eq!(
        structured["content"][0]["type"],
        json!("text"),
        "structuredContent.content must hold a text item"
    );
    assert_eq!(
        structured["content"][0]["text"].as_str(),
        Some(text),
        "structuredContent must mirror the text envelope verbatim"
    );
}

#[test]
fn mcp_dispatch_error_has_no_structured_content() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let resp = server.handle_tools_call(
        &json!(1),
        &json!({"name": "definitely_not_a_tool", "arguments": {}}),
    );
    let result = resp
        .result
        .expect("tools/call should return a result object");
    assert_eq!(result["isError"], json!(true));
    assert!(
        result.get("structuredContent").is_none(),
        "error responses carry structuredError, not structuredContent: {result}"
    );
    assert!(result.get("structuredError").is_some());
}

#[test]
fn handle_request_touches_agent_registry() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "ping".to_string(),
        params: json!({}),
        id: Some(json!(1)),
    };
    let response = server.handle_request(&request);

    assert!(response.is_some());
    let ctx = agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![]);
    let agents = agent_registry::list_agent_views(dir.path(), &ctx);
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].source, "mcp");
    assert_eq!(
        agents[0].binary_version.as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

// STORY-435: every handled JSON-RPC request must push `last_active_at`
// forward. This locks the heartbeat that powers busy/idle freshness.
// trace:STORY-435 | ai:claude
#[test]
fn handle_request_advances_last_active_at() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "ping".to_string(),
        params: json!({}),
        id: Some(json!(1)),
    };

    let _ = server.handle_request(&request);
    let ctx = agent_registry::AgentClassifyContext::new(chrono::Utc::now(), 30, vec![]);
    let t0 = agent_registry::list_agent_views(dir.path(), &ctx)[0].last_active_at;

    // chrono is microsecond-resolution; 5ms is comfortably above the
    // tick so the second touch will strictly advance the timestamp.
    thread::sleep(std::time::Duration::from_millis(5));
    let _ = server.handle_request(&request);
    let t1 = agent_registry::list_agent_views(dir.path(), &ctx)[0].last_active_at;

    assert!(
        t1 > t0,
        "expected last_active_at to advance: t0={t0} t1={t1}"
    );
}

#[test]
fn parses_aida_version_banner_identity() {
    let ident =
        parse_aida_binary_identity("aida 0.9.1 (built 2026-05-23 01:00:00 MST, sha abc1234+dirty)")
            .expect("banner should parse");
    assert_eq!(ident.version, "0.9.1");
    assert_eq!(ident.sha, "abc1234");
    assert!(ident.dirty);

    let clean = parse_aida_binary_identity("aida-cli 0.9.2 (built x, sha deadbee)")
        .expect("aida-cli prefix should parse");
    assert_eq!(clean.version, "0.9.2");
    assert_eq!(clean.sha, "deadbee");
    assert!(!clean.dirty);
}

#[test]
fn mcp_respawn_decision_uses_version_then_build_identity() {
    let running = McpBinaryIdentity {
        version: "0.9.1".to_string(),
        sha: "aaaaaaa".to_string(),
        dirty: false,
    };
    let newer_version = McpBinaryIdentity {
        version: "0.9.2".to_string(),
        sha: "bbbbbbb".to_string(),
        dirty: false,
    };
    assert!(mcp_binary_is_newer_or_different(&running, &newer_version));

    let same_version_new_sha = McpBinaryIdentity {
        version: "0.9.1".to_string(),
        sha: "bbbbbbb".to_string(),
        dirty: false,
    };
    assert!(mcp_binary_is_newer_or_different(
        &running,
        &same_version_new_sha
    ));

    let older_version = McpBinaryIdentity {
        version: "0.9.0".to_string(),
        sha: "zzzzzzz".to_string(),
        dirty: false,
    };
    assert!(!mcp_binary_is_newer_or_different(&running, &older_version));

    let unknown_sha = McpBinaryIdentity {
        version: "0.9.1".to_string(),
        sha: "unknown".to_string(),
        dirty: false,
    };
    assert!(
        !mcp_binary_is_newer_or_different(&running, &unknown_sha),
        "unknown same-version SHA must not cause an endless respawn loop"
    );
}

#[test]
fn mcp_respawn_plan_preserves_original_argv() {
    let plan = McpRespawnPlan {
        exe: PathBuf::from("/nonexistent/aida"),
        argv: vec![OsString::from("aida"), OsString::from("mcp-serve")],
        running: McpBinaryIdentity {
            version: "0.9.1".to_string(),
            sha: "aaaaaaa".to_string(),
            dirty: false,
        },
        on_disk: McpBinaryIdentity {
            version: "0.9.1".to_string(),
            sha: "bbbbbbb".to_string(),
            dirty: false,
        },
    };
    assert_eq!(plan.exe, PathBuf::from("/nonexistent/aida"));
    assert_eq!(plan.argv[1], OsString::from("mcp-serve"));
    assert_eq!(plan.running.label(), "0.9.1 sha aaaaaaa");
    assert_eq!(plan.on_disk.label(), "0.9.1 sha bbbbbbb");
}

#[test]
fn tool_descriptors_lists_coordination_tools() {
    let desc = tool_descriptors();
    let arr = desc.as_array().unwrap();
    let names: Vec<&str> = arr
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    // 10 spec-graph (incl. query_graph) + 19 coordination (incl. the two
    // mailbox tools) = 29 total.
    assert!(names.len() >= 29, "expected ≥29 tools, got {}", names.len());
    for required in [
        "add_relationship",
        "query_graph",
        "send_message",
        "read_inbox",
        "history",
        "list_punts",
        "read_punt",
        "post_punt",
        "resolve_punt",
        "escalate_punt",
        "list_findings",
        "file_finding",
        "triage_finding",
        "claim_task",
        "release_task",
        "list_active_leases",
        "post_directive",
        "list_directives",
        "ack_directive",
        "list_briefs",
        "read_brief",
        "ack_brief",
    ] {
        assert!(names.contains(&required), "missing tool: {}", required);
    }
}

// trace:TASK-440 | ai:claude
/// Every tool descriptor must carry a non-null `outputSchema` that passes
/// basic JSON-Schema structural validation: an object with `type` =
/// "object", a `properties` map, and a non-empty `description` so
/// schema-driven MCP clients (Codex, Cursor, …) can render the response
/// shape instead of treating it as opaque text.
#[test]
fn every_tool_descriptor_has_a_valid_output_schema() {
    let desc = tool_descriptors();
    let arr = desc.as_array().expect("tool_descriptors must be an array");
    assert!(
            arr.len() >= 29,
            "expected ≥29 tool descriptors (10 spec-graph incl. query_graph + 19 coordination incl. mailbox), got {}",
            arr.len()
        );

    for tool in arr {
        let name = tool
            .get("name")
            .and_then(|n| n.as_str())
            .expect("each tool descriptor must have a name");

        let output_schema = tool
            .get("outputSchema")
            .unwrap_or_else(|| panic!("tool '{}' is missing outputSchema", name));
        assert!(
            !output_schema.is_null(),
            "tool '{}' has a null outputSchema",
            name
        );

        // Basic JSON-Schema structural validation.
        let obj = output_schema
            .as_object()
            .unwrap_or_else(|| panic!("tool '{}' outputSchema is not a JSON object", name));

        let schema_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("tool '{}' outputSchema is missing `type`", name));
        assert_eq!(
            schema_type, "object",
            "tool '{}' outputSchema `type` must be \"object\" (Path A — text envelope), got {:?}",
            name, schema_type
        );

        let description = obj
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("tool '{}' outputSchema is missing `description`", name));
        assert!(
            !description.trim().is_empty(),
            "tool '{}' outputSchema description must be non-empty",
            name
        );

        let properties = obj
            .get("properties")
            .and_then(|v| v.as_object())
            .unwrap_or_else(|| {
                panic!(
                    "tool '{}' outputSchema is missing a `properties` object",
                    name
                )
            });
        assert!(
                properties.contains_key("content"),
                "tool '{}' outputSchema must declare a `content` property (Path A — text envelope wraps every response)",
                name
            );

        // STORY-399 (Path B): the outputSchema must also declare the
        // additive `structuredContent` object, and that object must itself
        // require a `content` array — the machine-readable mirror.
        // trace:STORY-399 | ai:claude
        let structured = properties
                .get("structuredContent")
                .and_then(|v| v.as_object())
                .unwrap_or_else(|| {
                    panic!(
                        "tool '{}' outputSchema must declare a `structuredContent` object (Path B — STORY-399)",
                        name
                    )
                });
        assert_eq!(
            structured.get("type").and_then(|v| v.as_str()),
            Some("object"),
            "tool '{}' structuredContent schema must be an object",
            name
        );
        assert!(
            structured
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|p| p.contains_key("content"))
                .unwrap_or(false),
            "tool '{}' structuredContent must declare a `content` property",
            name
        );
    }
}

#[test]
fn every_tool_parameter_has_examples_patterns_or_enums() {
    let desc = tool_descriptors();
    let arr = desc.as_array().expect("tool_descriptors must be an array");

    for tool in arr {
        let tool_name = tool
            .get("name")
            .and_then(|n| n.as_str())
            .expect("each tool descriptor must have a name");

        let properties = tool
            .pointer("/inputSchema/properties")
            .and_then(|v| v.as_object());

        if let Some(props) = properties {
            for (param_name, param_val) in props {
                let param_obj = param_val.as_object().unwrap_or_else(|| {
                    panic!(
                        "tool '{}' parameter '{}' is not a JSON object",
                        tool_name, param_name
                    )
                });

                let has_example = param_obj.contains_key("example");
                let has_enum = param_obj.contains_key("enum");
                let has_pattern = param_obj.contains_key("pattern");
                let has_minimum = param_obj.contains_key("minimum");

                assert!(
                        has_example || has_enum || has_pattern || has_minimum,
                        "tool '{}' parameter '{}' must have an 'example', 'enum', 'pattern', or 'minimum' field",
                        tool_name,
                        param_name
                    );

                assert!(
                    has_example,
                    "tool '{}' parameter '{}' is missing the 'example' field",
                    tool_name, param_name
                );
            }
        }
    }
}

// trace:BUG-332 | ai:codex
#[test]
fn add_requirement_descriptor_requires_type_and_documents_normalized_prefix() {
    let desc = tool_descriptors();
    let arr = desc.as_array().expect("tool_descriptors must be an array");
    let tool = arr
        .iter()
        .find(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("add_requirement"))
        .expect("add_requirement descriptor must exist");

    let required = tool
        .pointer("/inputSchema/required")
        .and_then(|v| v.as_array())
        .expect("add_requirement inputSchema must declare required args");
    assert!(
        required.iter().any(|v| v.as_str() == Some("type")),
        "add_requirement must require `type`"
    );

    let type_description = tool
        .pointer("/inputSchema/properties/type/description")
        .and_then(|v| v.as_str())
        .expect("type property must have a description");
    for expected in ["task", "bug", "folder", "meta", "doc", "faq"] {
        assert!(
            type_description.contains(expected),
            "type description should name valid taxonomy member {expected}"
        );
    }

    let output_description = tool
        .pointer("/outputSchema/description")
        .and_then(|v| v.as_str())
        .expect("outputSchema must have a description");
    assert!(
        output_description.contains("auto-normalized"),
        "outputSchema should document auto-normalized ID prefixes"
    );
}

// trace:BUG-332 | ai:codex
#[test]
fn mcp_add_requirement_rejects_missing_or_invalid_type() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let missing = serde_json::json!({
        "title": "Missing type",
        "description": "Type is required",
    });
    let err = server
        .tool_add_requirement(&missing)
        .expect_err("missing type should fail");
    assert!(err.contains("Missing required parameter: type"));
    assert!(err.contains("task"));
    assert!(err.contains("doc"));
    assert!(err.contains("faq"));

    let invalid = serde_json::json!({
        "title": "Invalid type",
        "description": "Type must be from the taxonomy",
        "type": "spec",
    });
    let err = server
        .tool_add_requirement(&invalid)
        .expect_err("invalid type should fail");
    assert!(err.contains("Invalid requirement type 'spec'"));
    assert!(err.contains("functional"));
    assert!(err.contains("meta"));
}

// trace:BUG-332 | ai:codex
#[test]
fn mcp_add_requirement_normalizes_every_valid_type_prefix() {
    let valid_types = [
        ("functional", "FR-"),
        ("non-functional", "NFR-"),
        ("system", "SR-"),
        ("user", "UR-"),
        ("bug", "BUG-"),
        ("epic", "EPIC-"),
        ("story", "STORY-"),
        ("task", "TASK-"),
        ("spike", "SPIKE-"),
        ("sprint", "SPRINT-"),
        ("folder", "FOLDER-"),
        ("meta", "META-"),
        ("doc", "DOC-"),
        ("faq", "FAQ-"),
    ];

    for (type_name, expected_prefix) in valid_types {
        let dir = tempdir().unwrap();
        let server = mk_server(dir.path());
        let args = serde_json::json!({
            "title": format!("Canonical {type_name}"),
            "description": "MCP add_requirement must use type-derived prefixes",
            "type": type_name,
        });

        let response = server
            .tool_add_requirement(&args)
            .unwrap_or_else(|err| panic!("add_requirement failed for {type_name}: {err}"));
        let spec_id = added_spec_id(&response);
        assert!(
            spec_id.starts_with(expected_prefix),
            "{type_name} should produce {expected_prefix}*, got {spec_id}"
        );
        assert!(
            !spec_id.starts_with("SPEC-"),
            "{type_name} must not produce generic SPEC-* IDs"
        );
    }
}

// trace:BUG-377 TASK-550 TASK-647 | ai:codex
#[test]
fn mcp_add_requirement_persists_all_advertised_fields() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let response = server
        .tool_add_requirement(&json!({
            "title": "MCP write map",
            "description": "All advertised fields should persist",
            "type": "bug",
            "status": "approved",
            "priority": "high",
            "tags": ["mcp", "roundtrip"],
        }))
        .unwrap();
    let spec_id = added_spec_id(&response);
    // TASK-647 (ADR-3): MCP intake always lands `draft` (advisor-gated),
    // and the response says so — even though `approved` was requested.
    assert!(
        response.contains("advisor triage"),
        "expected triage notice in: {response}"
    );

    let store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id(spec_id)
        .expect("requirement should be visible after MCP add");
    assert_eq!(req.title, "MCP write map");
    assert_eq!(req.description, "All advertised fields should persist");
    assert_eq!(req.req_type, RequirementType::Bug);
    // TASK-647: requested `approved` was downgraded to `draft` at intake.
    assert_eq!(req.status, RequirementStatus::Draft);
    assert_eq!(req.priority, RequirementPriority::High);
    assert!(req.tags.contains("mcp"));
    assert!(req.tags.contains("roundtrip"));
}

// trace:BUG-381 | ai:codex
#[test]
fn mcp_list_requirements_normalizes_status_type_and_priority_filters() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // TASK-647 (ADR-3): MCP add lands draft, so set the non-draft statuses
    // this filter test needs via the (ungated) update tool.
    let working = server
        .tool_add_requirement(&json!({
            "title": "Working MCP item",
            "description": "status filter target",
            "type": "task",
            "priority": "high",
        }))
        .unwrap();
    let working_id = added_spec_id(&working).to_string();
    // BUG-481: Draft → InProgress via MCP is now advisor-gated; reach the
    // InProgress filter target out-of-band like the other gated statuses.
    force_status(&server, &working_id, RequirementStatus::InProgress);
    let planned = server
        .tool_add_requirement(&json!({
            "title": "Planned MCP item",
            "description": "negative control",
            "type": "story",
            "priority": "low",
        }))
        .unwrap();
    let planned_id = added_spec_id(&planned).to_string();
    // BUG-449: Planned is advisor-gated via MCP — set it out-of-band.
    force_status(&server, &planned_id, RequirementStatus::Planned);
    let nfr = server
        .tool_add_requirement(&json!({
            "title": "Nonfunctional MCP item",
            "description": "type filter target",
            "type": "non-functional",
            "priority": "medium",
        }))
        .unwrap();
    let nfr_id = added_spec_id(&nfr).to_string();

    for status in ["in-progress", "InProgress", "In Progress", "in_progress"] {
        let output = server
            .tool_list_requirements(&json!({ "status": status }))
            .unwrap();
        assert!(output.contains(&working_id), "{status}: {output}");
        assert!(!output.contains(&planned_id), "{status}: {output}");
        assert!(
            !output.contains("No requirements found"),
            "{status}: {output}"
        );
    }

    let type_output = server
        .tool_list_requirements(&json!({ "type": "non functional" }))
        .unwrap();
    assert!(type_output.contains(&nfr_id), "{type_output}");
    assert!(!type_output.contains(&working_id), "{type_output}");

    let priority_output = server
        .tool_list_requirements(&json!({ "priority": " HIGH " }))
        .unwrap();
    assert!(priority_output.contains(&working_id), "{priority_output}");
    assert!(!priority_output.contains(&planned_id), "{priority_output}");
}

// trace:BUG-1174 | ai:codex
#[test]
fn mcp_open_lens_hides_accepted_decisions_but_keeps_explicit_queries() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let accepted_adr = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Ratified ADR",
                "description": "accepted decision should be terminal",
                "type": "decision",
            }))
            .unwrap(),
    )
    .to_string();
    let proposed_adr = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Proposed ADR",
                "description": "draft decision is still open",
                "type": "decision",
            }))
            .unwrap(),
    )
    .to_string();
    let approved_task = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Approved task",
                "description": "approved task is still open work",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();

    force_status(&server, &accepted_adr, RequirementStatus::Approved);
    force_status(&server, &approved_task, RequirementStatus::Approved);

    let default = server.tool_list_requirements(&json!({})).unwrap();
    assert!(
        !default.contains(&accepted_adr),
        "accepted ADR must not appear in default MCP open lens: {default}"
    );
    assert!(
        default.contains(&proposed_adr),
        "draft ADR remains open/proposed: {default}"
    );
    assert!(
        default.contains(&approved_task),
        "approved task remains open work: {default}"
    );

    let open = server
        .tool_list_requirements(&json!({ "status": "open" }))
        .unwrap();
    assert!(
        !open.contains(&accepted_adr),
        "accepted ADR must not appear in MCP status=open lens: {open}"
    );
    assert!(open.contains(&proposed_adr), "{open}");
    assert!(open.contains(&approved_task), "{open}");

    let decision = server
        .tool_list_requirements(&json!({ "type": "decision" }))
        .unwrap();
    assert!(
        decision.contains(&accepted_adr),
        "explicit decision type must surface accepted ADRs: {decision}"
    );

    let approved = server
        .tool_list_requirements(&json!({ "status": "approved" }))
        .unwrap();
    assert!(
        approved.contains(&accepted_adr),
        "explicit approved status must surface accepted ADRs: {approved}"
    );
}

// ===================================================================
// STORY-82: modernized filter/field surface on the 7 core tools
// ===================================================================

// trace:STORY-82 | ai:claude
#[test]
fn mcp_list_requirements_filters_by_tags_batch_and_role() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let tagged = server
        .tool_add_requirement(&json!({
            "title": "Tagged + batched + roled",
            "description": "matches all three filters",
            "type": "task",
            "tags": ["mcp", "papercut", "batch:fall-cleanup", "role:implementer"],
        }))
        .unwrap();
    let tagged_id = added_spec_id(&tagged).to_string();
    let other = server
        .tool_add_requirement(&json!({
            "title": "Unrelated",
            "description": "negative control",
            "type": "task",
            "tags": ["docs"],
        }))
        .unwrap();
    let other_id = added_spec_id(&other).to_string();

    // tags CSV: AND-match across the listed tags.
    let out = server
        .tool_list_requirements(&json!({ "tags": "mcp,papercut" }))
        .unwrap();
    assert!(out.contains(&tagged_id), "{out}");
    assert!(!out.contains(&other_id), "{out}");

    // a tag the row lacks excludes it.
    let none = server
        .tool_list_requirements(&json!({ "tags": "mcp,does-not-exist" }))
        .unwrap();
    assert!(none.contains("No requirements found"), "{none}");

    // batch shorthand → batch:<name> tag.
    let batch_out = server
        .tool_list_requirements(&json!({ "batch": "fall-cleanup" }))
        .unwrap();
    assert!(batch_out.contains(&tagged_id), "{batch_out}");
    assert!(!batch_out.contains(&other_id), "{batch_out}");

    // role / for → role:<name> tag (case-insensitive); `for` is an alias.
    for key in ["role", "for"] {
        let role_out = server
            .tool_list_requirements(&json!({ key: "IMPLEMENTER" }))
            .unwrap();
        assert!(role_out.contains(&tagged_id), "{key}: {role_out}");
        assert!(!role_out.contains(&other_id), "{key}: {role_out}");
    }
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_list_requirements_filters_by_parent() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let epic = server
        .tool_add_requirement(&json!({
            "title": "Parent epic",
            "description": "has children",
            "type": "epic",
        }))
        .unwrap();
    let epic_id = added_spec_id(&epic).to_string();
    let child = server
        .tool_add_requirement(&json!({
            "title": "Child story",
            "description": "linked under the epic",
            "type": "story",
            "parent": epic_id,
        }))
        .unwrap();
    let child_id = added_spec_id(&child).to_string();
    let stray = server
        .tool_add_requirement(&json!({
            "title": "Unparented",
            "description": "not a child",
            "type": "story",
        }))
        .unwrap();
    let stray_id = added_spec_id(&stray).to_string();

    let out = server
        .tool_list_requirements(&json!({ "parent": epic_id }))
        .unwrap();
    assert!(out.contains(&child_id), "child must show: {out}");
    assert!(!out.contains(&stray_id), "stray must not show: {out}");
}

/// Set the `archived` view-flag directly in the store, bypassing the gate —
/// mirrors `aida archive <ID>`. trace:BUG-591 | ai:claude
fn force_archive(server: &McpServer<'static>, spec_id: &str) {
    let mut store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id_mut(spec_id)
        .expect("spec should exist for force_archive");
    req.archived = true;
    server.storage.save(&store).unwrap();
}

/// Set the `deferred` view-flag directly in the store — mirrors
/// `aida defer <ID>`. trace:BUG-591 | ai:claude
fn force_defer(server: &McpServer<'static>, spec_id: &str) {
    let mut store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id_mut(spec_id)
        .expect("spec should exist for force_defer");
    req.deferred = true;
    server.storage.save(&store).unwrap();
}

/// BUG-591: `list_requirements` must apply the same archive/deferred view
/// tiers as `aida list` — archived + deferred rows hidden by DEFAULT, and
/// surfaced via the `archived` / `deferred` / `all` filters.
// trace:BUG-591 | ai:claude
#[test]
fn mcp_list_requirements_honors_archive_and_defer_view_tiers() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let active = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Active spec",
                "description": "stays visible",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();
    let archived = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Archived spec",
                "description": "filed away",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();
    let deferred = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "Deferred spec",
                "description": "primed shelf",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();
    force_archive(&server, &archived);
    force_defer(&server, &deferred);

    // DEFAULT view: active only; archived + deferred are hidden.
    let default = server.tool_list_requirements(&json!({})).unwrap();
    assert!(
        default.contains(&active),
        "active must show by default: {default}"
    );
    assert!(
        !default.contains(&archived),
        "archived must be hidden by default: {default}"
    );
    assert!(
        !default.contains(&deferred),
        "deferred must be hidden by default: {default}"
    );

    // archived=true → only the archived row.
    let arc = server
        .tool_list_requirements(&json!({ "archived": true }))
        .unwrap();
    assert!(
        arc.contains(&archived),
        "archived view must show archived: {arc}"
    );
    assert!(
        !arc.contains(&active),
        "archived view excludes active: {arc}"
    );
    assert!(
        !arc.contains(&deferred),
        "archived view excludes deferred: {arc}"
    );

    // deferred=true → only the deferred row.
    let def = server
        .tool_list_requirements(&json!({ "deferred": true }))
        .unwrap();
    assert!(
        def.contains(&deferred),
        "deferred view must show deferred: {def}"
    );
    assert!(
        !def.contains(&active),
        "deferred view excludes active: {def}"
    );
    assert!(
        !def.contains(&archived),
        "deferred view excludes archived: {def}"
    );

    // all=true → the union of all three tiers.
    let all = server
        .tool_list_requirements(&json!({ "all": true }))
        .unwrap();
    assert!(all.contains(&active), "all view shows active: {all}");
    assert!(all.contains(&archived), "all view shows archived: {all}");
    assert!(all.contains(&deferred), "all view shows deferred: {all}");
}

/// BUG-591: `search_requirements` mirrors the same view-tier default-hide as
/// `aida search` — an archived spec matching the query is hidden by default
/// but surfaced with `archived` / `all`.
// trace:BUG-591 | ai:claude
#[test]
fn mcp_search_requirements_honors_archive_view_tiers() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let active = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "seedling active",
                "description": "matches seedquery",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();
    let archived = added_spec_id(
        &server
            .tool_add_requirement(&json!({
                "title": "seedling archived",
                "description": "matches seedquery",
                "type": "task",
            }))
            .unwrap(),
    )
    .to_string();
    force_archive(&server, &archived);

    // DEFAULT search hides the archived match.
    let default = server
        .tool_search_requirements(&json!({ "query": "seedquery" }))
        .unwrap();
    assert!(
        default.contains(&active),
        "active match must show: {default}"
    );
    assert!(
        !default.contains(&archived),
        "archived match must be hidden by default: {default}"
    );

    // archived=true surfaces it.
    let arc = server
        .tool_search_requirements(&json!({ "query": "seedquery", "archived": true }))
        .unwrap();
    assert!(
        arc.contains(&archived),
        "archived search must show archived: {arc}"
    );
    assert!(
        !arc.contains(&active),
        "archived search excludes active: {arc}"
    );

    // all=true → both.
    let all = server
        .tool_search_requirements(&json!({ "query": "seedquery", "all": true }))
        .unwrap();
    assert!(all.contains(&active), "all search shows active: {all}");
    assert!(all.contains(&archived), "all search shows archived: {all}");
}

/// BUG-591: the list_requirements descriptor advertises the new view-tier
/// filters so schema-driven MCP clients can request them.
// trace:BUG-591 | ai:claude
#[test]
fn mcp_list_and_search_descriptors_advertise_view_tier_filters() {
    let desc = tool_descriptors();
    let arr = desc.as_array().unwrap();
    for tool_name in ["list_requirements", "search_requirements"] {
        let tool = arr
            .iter()
            .find(|t| t.get("name").and_then(|v| v.as_str()) == Some(tool_name))
            .unwrap_or_else(|| panic!("{tool_name} descriptor must exist"));
        let props = tool
            .pointer("/inputSchema/properties")
            .and_then(|v| v.as_object())
            .unwrap();
        for p in ["archived", "deferred", "all"] {
            assert!(props.contains_key(p), "{tool_name} must advertise `{p}`");
        }
    }
    // TASK-883: post_punt advertises the `reason` alias.
    let post_punt = arr
        .iter()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("post_punt"))
        .expect("post_punt descriptor must exist");
    let pp_props = post_punt
        .pointer("/inputSchema/properties")
        .and_then(|v| v.as_object())
        .unwrap();
    assert!(
        pp_props.contains_key("reason"),
        "post_punt must advertise the `reason` alias"
    );
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_add_requirement_persists_parent_feature_owner() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let epic = server
        .tool_add_requirement(&json!({
            "title": "Owner of children",
            "description": "epic",
            "type": "epic",
        }))
        .unwrap();
    let epic_id = added_spec_id(&epic).to_string();

    let response = server
        .tool_add_requirement(&json!({
            "title": "Full-field child",
            "description": "feature + owner + parent",
            "type": "task",
            "feature": "auth",
            "owner": "alice",
            "parent": epic_id,
        }))
        .unwrap();
    assert!(
        response.contains(&epic_id),
        "result notes parent: {response}"
    );
    let child_id = added_spec_id(&response).to_string();

    let store = server.storage.load().unwrap();
    let child = store.get_requirement_by_spec_id(&child_id).unwrap();
    // The legacy YAML test store normalizes feature names on load
    // (migrate_features prefixes a stable number), so assert containment
    // rather than exact equality.
    assert!(child.feature.contains("auth"), "feature: {}", child.feature);
    assert_eq!(child.owner, "alice");
    // Child carries a Child→parent edge; parent carries Parent→child.
    let parent = store.get_requirement_by_spec_id(&epic_id).unwrap();
    assert!(
        parent
            .relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Parent && r.target_id == child.id),
        "parent should have a Parent edge to the child"
    );
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_add_requirement_rejects_missing_parent() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let err = server
        .tool_add_requirement(&json!({
            "title": "Orphan",
            "description": "bad parent",
            "type": "task",
            "parent": "EPIC-999",
        }))
        .expect_err("missing parent should fail");
    assert!(err.contains("EPIC-999"), "{err}");
    assert!(err.contains("not found"), "{err}");
}

// trace:STORY-1675 | ai:codex
#[test]
fn hypothesis_mcp_update_refuses_description_and_allows_amendment() {
    use aida_core::db::DatabaseBackend;
    let dir = tempdir().unwrap();
    let root = dir.path().join("store");
    let backend = crate::test_env::home_fixture_backend(&root)
        .unwrap()
        .with_auto_commit(false);
    let mut h = Requirement::new("hypothesis".into(), "pre-result".into());
    h.spec_id = Some("SPIKE-991".into());
    h.req_type = RequirementType::Spike;
    h.tags.insert("hypothesis".into());
    let mut result = Requirement::new("result".into(), "data".into());
    result.spec_id = Some("TASK-992".into());
    result.status = RequirementStatus::Completed;
    h.relationships.push(aida_core::models::Relationship {
        rel_type: RelationshipType::VerifiedBy,
        target_id: result.id,
        created_at: None,
        created_by: None,
    });
    let mut store = aida_core::RequirementsStore::new();
    store.requirements = vec![h, result];
    backend.save(&store).unwrap();
    let storage = Storage::new(root);
    let server = McpServer::new(&storage, dir.path().to_path_buf());
    let error = server
        .tool_update_requirement(&json!({"id": "SPIKE-991", "description": "post hoc"}))
        .unwrap_err();
    assert!(error.contains("aida comment add SPIKE-991"), "{error}");
    server
        .tool_update_requirement(&json!({"id": "SPIKE-991", "title": "retitled"}))
        .unwrap();
    server
        .tool_add_comment(&json!({"id": "SPIKE-991", "text": "amendment"}))
        .unwrap();
    let loaded = storage.load().unwrap();
    let h = loaded.get_requirement_by_spec_id("SPIKE-991").unwrap();
    assert_eq!(h.description, "pre-result");
    assert_eq!(h.title, "retitled");
    assert!(h.hypothesis_lock.is_some());
    assert!(h.comments.iter().any(|c| c.content == "amendment"));
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_update_requirement_sets_title_type_priority_tags_parent() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let epic = server
        .tool_add_requirement(&json!({
            "title": "Reparent target",
            "description": "epic",
            "type": "epic",
        }))
        .unwrap();
    let epic_id = added_spec_id(&epic).to_string();
    let added = server
        .tool_add_requirement(&json!({
            "title": "Original title",
            "description": "to be edited",
            "type": "task",
            "priority": "low",
            "tags": ["old"],
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();

    let result = server
        .tool_update_requirement(&json!({
            "id": id,
            "title": "New title",
            "type": "story",
            "priority": "high",
            "tags": ["fresh", "batch:x"],
            "parent": epic_id,
        }))
        .unwrap();
    assert!(result.starts_with(&format!("Updated {}", id)), "{result}");

    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(req.title, "New title");
    assert_eq!(req.req_type, RequirementType::Story);
    assert_eq!(req.priority, RequirementPriority::High);
    assert!(req.tags.contains("fresh") && req.tags.contains("batch:x"));
    assert!(!req.tags.contains("old"), "tags should be replaced");
    assert!(
        req.relationships
            .iter()
            .any(|r| r.rel_type == RelationshipType::Child),
        "target should carry a Child edge to the new parent"
    );
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_update_requirement_rejects_invalid_type() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "T", "description": "d", "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();
    let err = server
        .tool_update_requirement(&json!({ "id": id, "type": "nonsense" }))
        .expect_err("invalid type should fail");
    assert!(err.contains("Invalid requirement type 'nonsense'"), "{err}");
}

#[test]
// Forced MCP replacement must use the same destructive-edit ledger subject
// as `aida edit --tags ... --force`. trace:BUG-1252 | ai:codex
fn mcp_force_tag_replacement_records_dropped_tags_in_git_subject() {
    let dir = tempdir().unwrap();
    let store_root = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store_root).unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "AIDA Test"],
    ] {
        assert!(crate::test_git::git_command()
            .args(args)
            .current_dir(&store_root)
            .status()
            .unwrap()
            .success());
    }
    // trace:BUG-2017 | ai:codex — explicit positive global-ID fixture.
    crate::test_env::enroll_synthetic_home(&store_root).unwrap();
    let storage = Box::leak(Box::new(Storage::new(&store_root)));
    let server = McpServer::new(storage, dir.path().to_path_buf());
    let mut store = aida_core::RequirementsStore::default();
    let mut req = Requirement::new("destructive edit".into(), String::new());
    req.spec_id = Some("BUG-1252".into());
    req.tags = ["parent:EPIC-28", "lane:research", "ordinary"]
        .into_iter()
        .map(str::to_string)
        .collect();
    store.requirements.push(req);
    storage.save(&store).unwrap();

    server
        .tool_update_requirement(&json!({
            "id": "BUG-1252", "tags": ["ordinary"], "force": true
        }))
        .unwrap();
    let subject = String::from_utf8(
        crate::test_git::git_command()
            .args(["log", "-1", "--format=%s"])
            .current_dir(&store_root)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        subject.contains("replaced tags, dropped: lane:research, parent:EPIC-28"),
        "{subject}"
    );
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_search_requirements_narrows_by_type_and_status() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());

    let bug = server
        .tool_add_requirement(&json!({
            "title": "Login oauth bug",
            "description": "oauth token refresh fails",
            "type": "bug",
        }))
        .unwrap();
    let bug_id = added_spec_id(&bug).to_string();
    let story = server
        .tool_add_requirement(&json!({
            "title": "Login oauth story",
            "description": "oauth happy path",
            "type": "story",
        }))
        .unwrap();
    let story_id = added_spec_id(&story).to_string();

    // type narrows.
    let out = server
        .tool_search_requirements(&json!({ "query": "oauth", "type": "bug" }))
        .unwrap();
    assert!(out.contains(&bug_id), "{out}");
    assert!(!out.contains(&story_id), "{out}");

    // status narrows (both are draft on MCP intake).
    let drafts = server
        .tool_search_requirements(&json!({ "query": "oauth", "status": "draft" }))
        .unwrap();
    assert!(
        drafts.contains(&bug_id) && drafts.contains(&story_id),
        "{drafts}"
    );
    let none = server
        .tool_search_requirements(&json!({ "query": "oauth", "status": "completed" }))
        .unwrap();
    assert!(none.contains("No requirements found"), "{none}");
}

// trace:STORY-82 | ai:claude
#[test]
fn mcp_show_requirement_appends_git_linkage_by_default() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Show with git", "description": "d", "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();

    // Default: git linkage present (no repo/commits → the empty-state line).
    let with_git = server.tool_show_requirement(&json!({ "id": id })).unwrap();
    assert!(with_git.contains("## Git linkage"), "{with_git}");

    // include_git=false suppresses it (the `aida show --no-git` view).
    let no_git = server
        .tool_show_requirement(&json!({ "id": id, "include_git": false }))
        .unwrap();
    assert!(!no_git.contains("## Git linkage"), "{no_git}");
}

// trace:STORY-82 | ai:claude
#[test]
fn list_requirements_descriptor_advertises_modern_filters() {
    let desc = tool_descriptors();
    let arr = desc.as_array().unwrap();
    let tool = arr
        .iter()
        .find(|t| t.get("name").and_then(|v| v.as_str()) == Some("list_requirements"))
        .unwrap();
    let props = tool
        .pointer("/inputSchema/properties")
        .and_then(|v| v.as_object())
        .unwrap();
    for p in ["tags", "batch", "parent", "role", "for", "in_flight"] {
        assert!(
            props.contains_key(p),
            "list_requirements must advertise `{p}`"
        );
    }
    // type enum now covers the full taxonomy.
    let type_enum = tool
        .pointer("/inputSchema/properties/type/enum")
        .and_then(|v| v.as_array())
        .unwrap();
    let names: Vec<&str> = type_enum.iter().filter_map(|v| v.as_str()).collect();
    for t in ["folder", "meta", "doc", "faq", "sprint"] {
        assert!(names.contains(&t), "type enum must include {t}: {names:?}");
    }
}

// trace:STORY-1843 | ai:codex
#[test]
fn mcp_mailbox_live_resolution_refuses_without_writes_and_canonicalizes() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    for target in ["absent-worker", "implementer", "claude"] {
        let error = server
            .tool_send_message(&json!({"to":target,"body":"routing probe"}))
            .unwrap_err();
        // trace:TASK-B6B1278890774EF993D2EBC327862470-117 | ai:codex
        for hint in ["aida agent list", "aida agent ls"] {
            assert!(!error.contains(hint), "{error}");
        }
    }
    assert!(crate::mailbox_store::read_local_messages(dir.path())
        .unwrap()
        .is_empty());
    let mut entry = crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        dir.path().to_path_buf(),
        Some("Case-Worker".into()),
        None,
    )
    .unwrap();
    // One damaged record must be reported without preventing valid delivery.
    // trace:STORY-1843 | ai:codex
    std::fs::write(
        crate::agent_registry::agents_dir(dir.path()).join("broken.toml"),
        "invalid toml [",
    )
    .unwrap();
    let sent = server
        .tool_send_message(&json!({"to":" case-worker ","body":"routing probe"}))
        .unwrap();
    assert!(
        sent.contains("skipping invalid agent registry record"),
        "{sent}"
    );
    assert!(sent.contains("broken.toml"), "{sent}");
    let messages = crate::mailbox_store::read_local_messages(dir.path()).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].to,
        aida_core::mailbox::Recipient::Agent("Case-Worker".into())
    );
    entry.ended_at = Some(chrono::Utc::now());
    crate::agent_registry::write_entry(dir.path(), &entry).unwrap();
    let error = server
        .tool_send_message(&json!({"to":"Case-Worker","body":"late probe"}))
        .unwrap_err();
    assert!(error.contains("no live agent"), "{error}");
    assert!(error.contains("broken.toml"), "{error}");
    for hint in ["aida agent list", "aida agent ls"] {
        assert!(!error.contains(hint), "{error}");
    }
    assert_eq!(
        crate::mailbox_store::read_local_messages(dir.path())
            .unwrap()
            .len(),
        1
    );
}

/// send_message → read_inbox round-trip: a direct message and a broadcast
/// both land in the recipient's inbox; the sender's own message does not.
// trace:STORY-493 | ai:claude
#[test]
fn mcp_mailbox_send_then_read_inbox_roundtrip() {
    // STORY-1843: `agent` narrows the server's own verified audience;
    // this server is an unbound `claude` reader whatever session runs
    // the suite.
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // Direct vendor mail now requires a real live registry reader.
    // trace:STORY-1843 | ai:codex
    crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        dir.path().to_path_buf(),
        Some("mail-reader".into()),
        None,
    )
    .unwrap();

    server
        .tool_send_message(&json!({ "to": "claude", "body": "direct hi", "from": "codex" }))
        .unwrap();
    server
        .tool_send_message(&json!({ "broadcast": true, "body": "all hands", "from": "agy" }))
        .unwrap();

    let out = server
        .tool_read_inbox(&json!({ "agent": "claude" }))
        .unwrap();
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["agent"], "claude");
    assert_eq!(parsed["count"], 2, "direct + broadcast: {out}");

    // The sender (codex) sees the broadcast but not its own direct message.
    let codex = server
        .tool_read_inbox(&json!({ "agent": "codex" }))
        .unwrap();
    let codex_parsed: Value = serde_json::from_str(&codex).unwrap();
    assert_eq!(
        codex_parsed["count"], 1,
        "codex sees only agy's broadcast: {codex}"
    );

    // Neither `to` nor `broadcast` is a clean error, not a panic.
    let err = server.tool_send_message(&json!({ "body": "orphan" }));
    assert!(err.is_err(), "must require to/broadcast: {err:?}");
}

/// TASK-1928 / TASK-1918: MCP `send_message` trims `to` and stores a
/// known identity under its stored spelling, exactly as the CLI does.
// trace:TASK-1955 | ai:claude (TASK-1928, TASK-1918)
#[test]
fn mcp_send_message_trims_and_canonicalizes_a_known_recipient() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // The role and private name need a real live reader under the shared resolver.
    // trace:STORY-1843 | ai:codex
    let entry = crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "advisor".into(),
        None,
        dir.path().to_path_buf(),
        Some("newagent".into()),
        None,
    )
    .unwrap();
    crate::mailbox_resolver::reserve_test_reader(dir.path(), &entry);
    server
        .tool_send_message(&json!({ "to": "ADVISOR", "body": "upper", "from": "codex" }))
        .unwrap();
    server
        .tool_send_message(&json!({ "to": "  newagent  ", "body": "padded", "from": "codex" }))
        .unwrap();
    let mut to: Vec<_> = crate::mailbox_store::read_local_messages(dir.path())
        .unwrap()
        .into_iter()
        .map(|m| m.to)
        .collect();
    to.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(
        to,
        vec![
            aida_core::mailbox::Recipient::Agent("advisor".into()),
            aida_core::mailbox::Recipient::Agent("newagent".into()),
        ]
    );
}

/// send_message → read_inbox round-trip for `subject` (BUG-1461): the CLI
/// gained `aida mailbox send --subject` in BUG-1231, but the MCP schema and
/// handler were never mirrored — `subject` was accepted by no schema
/// property and dropped on the floor (`Message { subject: None, .. }`
/// unconditionally). This asserts the subject set via MCP survives into
/// the recipient's read_inbox view.
// trace:BUG-1461 | ai:claude
#[test]
fn mcp_mailbox_send_with_subject_then_read_inbox_roundtrip() {
    // STORY-1843: `agent` narrows the server's own verified audience;
    // this server is an unbound `claude` reader whatever session runs
    // the suite.
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // Direct vendor mail now requires a real live registry reader.
    // trace:STORY-1843 | ai:codex
    crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        dir.path().to_path_buf(),
        Some("mail-reader".into()),
        None,
    )
    .unwrap();

    server
        .tool_send_message(&json!({
            "to": "claude",
            "body": "see the attached diff",
            "from": "codex",
            "subject": "Review requested",
        }))
        .unwrap();

    let out = server
        .tool_read_inbox(&json!({ "agent": "claude" }))
        .unwrap();
    let parsed: Value = serde_json::from_str(&out).unwrap();
    let messages = parsed["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{out}");
    assert_eq!(
        messages[0]["subject"], "Review requested",
        "subject must round-trip through send_message -> read_inbox: {out}"
    );
}

/// STORY-1843 F5: MCP `read_inbox` on the shared actual-Git matrix. A
/// sibling-only message is in the peek, the unread slice and the
/// `mark_seen` ack (a receipt in this checkout); afterwards it reads as
/// read. Another inbox is an inspection that refuses `mark_seen`; a
/// missing worktree makes the view partial; a reply to the listed ID
/// attaches to its thread.
// trace:STORY-1843 | ai:claude
#[test]
fn mcp_read_inbox_shares_the_logical_view() {
    use crate::mailbox_view::tests::{mail, mail_repo};
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.sib, &mail("sib-only", "operator", 10, false)).unwrap();
    let server = mk_server(&r.main);
    let read = |args: Value| -> Value {
        serde_json::from_str(&server.tool_read_inbox(&args).unwrap()).unwrap()
    };

    let peek = read(json!({}));
    assert_eq!(peek["count"], 1, "{peek}");
    assert_eq!(peek["partial"], false);
    assert_eq!(peek["messages"][0]["id"], "sib-only");
    assert_eq!(peek["messages"][0]["read"], "unread");
    assert_eq!(
        read(json!({ "unread": true }))["count"],
        1,
        "peek did not consume"
    );

    crate::agent_registry::register_existing_agent(
        &r.main,
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        r.main.clone(),
        Some("mail-reader".into()),
        None,
    )
    .unwrap();
    let reply = server
        .tool_send_message(&json!({
            "to": "claude", "body": "ack", "from": "operator", "in_reply_to": "sib-only"
        }))
        .unwrap();
    assert!(reply.contains("(thread sib-only)"), "{reply}");

    let acked: Value =
        serde_json::from_str(&server.read_inbox_delivered(
            json!({ "agent": "operator", "mark_seen": true, "unread": true }),
        ))
        .unwrap();
    assert_eq!(acked["count"], 1);
    assert_eq!(acked["ack"], "requested");
    assert!(crate::mailbox_store::read_receipts(&r.main, "operator").contains_key("sib-only"));
    let after = read(json!({ "unread": true }));
    assert_eq!(after["count"], 0, "{after}");
    assert_eq!(read(json!({}))["messages"][0]["read"], "read");

    let err = server
        .tool_read_inbox(&json!({ "agent": "stranger", "mark_seen": true }))
        .unwrap_err();
    assert!(err.contains("not one of them"), "{err}");
    assert_eq!(read(json!({ "agent": "stranger" }))["inspection"], true);

    std::fs::remove_dir_all(&r.sib).unwrap();
    let partial = read(json!({}));
    assert_eq!(partial["partial"], true, "{partial}");
    assert!(partial["partial_note"].as_str().is_some());
}

/// STORY-1843 B2c: `read_inbox` with `mark_seen` acknowledges only after
/// its OWN response line was written and flushed. A response whose
/// write fails commits nothing; two prepared responses carry separate
/// plans (delivering one commits only its own messages); a commit that
/// fails after the flush adds no second JSON-RPC line and leaves the
/// mail unread.
// trace:STORY-1843 | ai:claude
#[test]
fn mcp_read_inbox_ack_commits_only_after_its_own_response_is_flushed() {
    use crate::mailbox_view::tests::{mail, mail_repo};
    struct BrokenPipe;
    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let r = mail_repo();
    crate::mailbox_store::write_message(&r.sib, &mail("m1", "operator", 10, false)).unwrap();
    let server = mk_server(&r.main);
    let call = |args: Value| {
        server.handle_tools_call(
            &json!(1),
            &json!({ "name": "read_inbox", "arguments": args }),
        )
    };
    let unread = || -> Vec<String> {
        let v: Value =
            serde_json::from_str(&server.tool_read_inbox(&json!({ "unread": true })).unwrap())
                .unwrap();
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect()
    };

    // A direct tool call and a failed delivery acknowledge nothing.
    let direct: Value = serde_json::from_str(
        &server
            .tool_read_inbox(&json!({ "mark_seen": true }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(direct["ack"], "requested");
    assert!(deliver(&mut BrokenPipe, call(json!({ "mark_seen": true }))).is_err());
    assert_eq!(unread(), vec!["m1"]);

    // Two responses prepared before either is delivered.
    let first = call(json!({ "mark_seen": true }));
    crate::mailbox_store::write_message(&r.main, &mail("m2", "operator", 20, false)).unwrap();
    let second = call(json!({ "mark_seen": true }));
    let mut out = Vec::new();
    deliver(&mut out, first).unwrap();
    assert_eq!(
        unread(),
        vec!["m2"],
        "only the first response's plan committed"
    );
    drop(second);
    assert_eq!(
        unread(),
        vec!["m2"],
        "an undelivered response commits nothing"
    );

    // The mail changes after the plan: the commit refuses after the
    // flush, the client got exactly one line, and the mail stays unread.
    let changed = call(json!({ "mark_seen": true }));
    let mut m2 = mail("m2", "operator", 20, false);
    m2.body = "edited".into();
    crate::mailbox_store::write_message(&r.main, &m2).unwrap();
    let mut out = Vec::new();
    deliver(&mut out, changed).unwrap();
    assert_eq!(out.iter().filter(|b| **b == b'\n').count(), 1);
    let line: Value = serde_json::from_slice(&out).unwrap();
    let text: Value =
        serde_json::from_str(line["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["ack"], "requested");
    assert_eq!(unread(), vec!["m2"]);
    // A retry plans the current bytes and commits.
    server.read_inbox_delivered(json!({ "mark_seen": true }));
    assert!(unread().is_empty());
}

// trace:STORY-585 trace:TASK-1271 | ai:claude+codex
/// read_inbox is a non-marking peek by default; `mark_seen` advances the
/// watermark (the explicit ack) and `unread` filters to the unread slice.
#[test]
fn mcp_read_inbox_peek_unread_and_mark_seen() {
    // STORY-1843: `agent` narrows the server's own verified audience;
    // this server is an unbound `claude` reader whatever session runs
    // the suite.
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // Direct vendor mail now requires a real live registry reader.
    // trace:STORY-1843 | ai:codex
    crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        dir.path().to_path_buf(),
        Some("mail-reader".into()),
        None,
    )
    .unwrap();
    server
        .tool_send_message(&json!({ "to": "claude", "body": "one", "from": "codex" }))
        .unwrap();
    server
        .tool_send_message(&json!({ "to": "claude", "body": "two", "from": "codex" }))
        .unwrap();

    // Default read does NOT mark seen — a second read still sees both unread.
    let peek = server
        .tool_read_inbox(&json!({ "agent": "claude" }))
        .unwrap();
    let peek_p: Value = serde_json::from_str(&peek).unwrap();
    assert_eq!(peek_p["count"], 2);
    let unread1 = server
        .tool_read_inbox(&json!({ "agent": "claude", "unread": true }))
        .unwrap();
    let unread1_p: Value = serde_json::from_str(&unread1).unwrap();
    assert_eq!(unread1_p["count"], 2, "peek did not consume: {unread1}");
    assert_eq!(unread1_p["unread"], true);

    // Explicit ack: mark_seen, once the response is delivered, records
    // per-message latency history.
    server.read_inbox_delivered(json!({ "agent": "claude", "mark_seen": true }));
    let messages = crate::mailbox_store::read_local_messages(dir.path()).unwrap();
    let receipts = crate::mailbox_store::read_receipts(dir.path(), "claude");
    assert_eq!(receipts.len(), 2, "ack records a receipt for each message");
    let max_read_latency_ms = crate::mailbox_cmd::max_read_latency_ms(
        &crate::mailbox_store::ReadEvidence::load(dir.path()).unwrap(),
        "claude",
        &messages,
    );
    assert!(
        max_read_latency_ms.is_some(),
        "latency history is populated after MCP mark_seen"
    );
    let unread2 = server
        .tool_read_inbox(&json!({ "agent": "claude", "unread": true }))
        .unwrap();
    let unread2_p: Value = serde_json::from_str(&unread2).unwrap();
    assert_eq!(
        unread2_p["count"], 0,
        "ack cleared the unread set: {unread2}"
    );
    // The full inbox is still there (ack is a watermark, not a delete).
    let full = server
        .tool_read_inbox(&json!({ "agent": "claude" }))
        .unwrap();
    let full_p: Value = serde_json::from_str(&full).unwrap();
    assert_eq!(full_p["count"], 2);
}

/// An advisor reading its mail through MCP `read_inbox` with `mark_seen`
/// acks a hello exactly as `aida mailbox inbox` does: one ack to the
/// sender, the sender's record marked, and a peek acks nothing.
// trace:STORY-B6B1278890774EF993D2EBC327862470-100.ac5a43a5 | ai:claude
#[test]
fn mcp_read_inbox_mark_seen_acks_a_hello_like_the_cli_inbox() {
    use crate::advisor_handshake::{
        ack_id, hello_message, load_record, save_record, HelloContext, HelloRecord, HomeKind,
    };
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("operator")),
        ("AIDA_SESSION_ROLE", Some("advisor")),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let server = mk_server(&root);
    let handle = "claude-implementer-7";
    let now = chrono::Utc::now();
    let id = uuid::Uuid::new_v4().to_string();
    let mailbox = crate::mailbox_store::mailbox_dir(&root);
    let ctx = HelloContext {
        handle: handle.into(),
        from_source: aida_core::mailbox::SenderSource::Explicit,
        role: "implementer".into(),
        spec: None,
        path: root.clone(),
        session_started: None,
        rotation: None,
    };
    let hello = hello_message(&id, &ctx, "advisor", &mailbox, now);
    crate::mailbox_store::write_message(&root, &hello).unwrap();
    save_record(
        &root,
        &HelloRecord {
            handle: handle.into(),
            role: "implementer".into(),
            spec: None,
            path: root.display().to_string(),
            hello_id: id.clone(),
            sent_at: now,
            mailbox: mailbox.display().to_string(),
            home: HomeKind::Checkout,
            advisor_live: true,
            acked_at: None,
            acked_by: None,
            ack_reported: false,
            rotation: None,
        },
    )
    .unwrap();
    let ack_file = mailbox.join(format!("{}.json", ack_id(&id)));

    // A peek is not a read: no ack.
    let peek: Value = serde_json::from_str(&server.read_inbox_delivered(json!({}))).unwrap();
    assert!(
        peek["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == json!(id)),
        "the advisor sees the hello: {peek}"
    );
    assert!(!ack_file.exists(), "a peek acks nothing");

    server.read_inbox_delivered(json!({ "mark_seen": true }));
    assert!(ack_file.exists(), "mark_seen acks the hello");
    let acks: Vec<_> = crate::mailbox_store::read_local_messages(&root)
        .unwrap()
        .into_iter()
        .filter(|m| m.in_reply_to.as_deref() == Some(id.as_str()))
        .collect();
    assert_eq!(acks.len(), 1, "one ack");
    assert_eq!(
        acks[0].to,
        aida_core::mailbox::Recipient::Agent(handle.into())
    );
    let rec = load_record(&root, handle).unwrap();
    assert!(rec.acked_at.is_some(), "the sender's record is marked");
    assert_eq!(rec.acked_by.as_deref(), Some("operator"));
}

/// STORY-1843 R2-F1: inspecting another inbox counts a receipt (what
/// an inbox read writes, with no watermark move) as a read.
// trace:STORY-1843 | ai:claude
#[test]
fn mcp_inspection_unread_counts_receipts_as_read() {
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let m = crate::mailbox_view::tests::mail("insp-1", "reviewer-seat", 10, false);
    crate::mailbox_store::write_message(dir.path(), &m).unwrap();
    let unread = |server: &McpServer<'static>| -> Value {
        serde_json::from_str(&server.inspect_inbox("reviewer-seat", true, false).unwrap()).unwrap()
    };
    assert_eq!(unread(&server)["count"], 1);
    crate::mailbox_store::record_seen(dir.path(), "reviewer-seat", &[m.id.as_str()]).unwrap();
    assert_eq!(
        crate::mailbox_store::read_watermark(dir.path(), "reviewer-seat"),
        None
    );
    assert_eq!(unread(&server)["count"], 0);
}

// trace:TASK-1271 | ai:codex
#[test]
fn mcp_mark_seen_does_not_backfill_receipts_at_or_below_existing_watermark() {
    // STORY-1843: `agent` narrows the server's own verified audience;
    // this server is an unbound `claude` reader whatever session runs
    // the suite.
    let _env = crate::test_env::AgentSessionIsolation::apply(&[
        ("AIDA_USER", Some("claude")),
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_AGENT_TYPE", None),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    // Direct vendor mail now requires a real live registry reader.
    // trace:STORY-1843 | ai:codex
    crate::agent_registry::register_existing_agent(
        dir.path(),
        "claude",
        std::process::id(),
        "implementer".into(),
        None,
        dir.path().to_path_buf(),
        Some("mail-reader".into()),
        None,
    )
    .unwrap();
    server
        .tool_send_message(&json!({ "to": "claude", "body": "already read", "from": "codex" }))
        .unwrap();

    let first = crate::mailbox_store::read_local_messages(dir.path()).unwrap();
    let old = first[0].timestamp;
    crate::mailbox_store::set_watermark(dir.path(), "claude", old).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));
    server
        .tool_send_message(&json!({ "to": "claude", "body": "new unread", "from": "codex" }))
        .unwrap();
    server.read_inbox_delivered(json!({ "agent": "claude", "mark_seen": true }));

    let messages = crate::mailbox_store::read_local_messages(dir.path()).unwrap();
    let old_id = messages
        .iter()
        .find(|m| m.body == "already read")
        .unwrap()
        .id
        .clone();
    let new_id = messages
        .iter()
        .find(|m| m.body == "new unread")
        .unwrap()
        .id
        .clone();
    let receipts = crate::mailbox_store::read_receipts(dir.path(), "claude");
    assert!(
        !receipts.contains_key(&old_id),
        "a prior watermark must not be converted into a fabricated receipt"
    );
    assert!(
        receipts.contains_key(&new_id),
        "newly acknowledged mail needs a receipt"
    );
}

/// query_graph walks the typed relationship graph: a spec blocked-by
/// another surfaces that blocker in the `blocked-by` mode result, and the
/// JSON carries the count + node id. Regression for the MCP half of the
/// graph-query moat (the CLI half is covered by graph_walk's unit tests).
// trace:STORY-489 | ai:claude
#[test]
fn mcp_query_graph_returns_blocked_by_chain() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let dependent = server
        .tool_add_requirement(&json!({
            "title": "Dependent spec",
            "description": "blocked by the blocker",
            "type": "story",
            "status": "approved",
        }))
        .unwrap();
    let dependent_id = added_spec_id(&dependent).to_string();
    let blocker = server
        .tool_add_requirement(&json!({
            "title": "Blocker spec",
            "description": "blocks the dependent",
            "type": "story",
            "status": "in-progress",
        }))
        .unwrap();
    let blocker_id = added_spec_id(&blocker).to_string();

    server
        .tool_add_relationship(&json!({
            "spec_id": dependent_id,
            "relationship_type": "blocked-by",
            "target_spec_id": blocker_id,
        }))
        .unwrap();

    let out = server
        .tool_query_graph(&json!({
            "spec_id": dependent_id,
            "mode": "blocked-by",
        }))
        .unwrap();
    let parsed: Value = serde_json::from_str(&out).expect("query_graph returns JSON");
    assert_eq!(parsed["mode"], "blocked-by");
    assert_eq!(parsed["count"], 1, "expected one blocker: {out}");
    assert_eq!(parsed["root"], dependent_id, "{out}");
    let ids: Vec<&str> = parsed["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert!(
        ids.contains(&blocker_id.as_str()),
        "blocker in nodes: {out}"
    );

    // BUG-411 parity: impact from the blocker finds what it blocks
    // (the dependent is blocked-by the blocker), via the handler's
    // walk_union legs.
    let impact = server
        .tool_query_graph(&json!({ "spec_id": blocker_id, "mode": "impact" }))
        .unwrap();
    let impact_parsed: Value = serde_json::from_str(&impact).unwrap();
    let impact_ids: Vec<&str> = impact_parsed["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["id"].as_str())
        .collect();
    assert!(
        impact_ids.contains(&dependent_id.as_str()),
        "impact must surface the dependent: {impact}"
    );

    // Unknown mode is a clean error, not a panic.
    let err = server.tool_query_graph(&json!({ "spec_id": dependent_id, "mode": "bogus" }));
    assert!(err.is_err(), "unknown mode must error: {err:?}");

    // Review finding: an underscore alias is accepted but the output must
    // echo the canonical hyphenated form, never `blocked_by`.
    let aliased = server
        .tool_query_graph(&json!({ "spec_id": dependent_id, "mode": "blocked_by" }))
        .unwrap();
    let aliased_parsed: Value = serde_json::from_str(&aliased).unwrap();
    assert_eq!(
        aliased_parsed["mode"], "blocked-by",
        "canonical mode: {aliased}"
    );
}

// trace:BUG-381 | ai:codex
#[test]
fn mcp_list_requirements_descriptor_advertises_priority_filter() {
    let desc = tool_descriptors();
    let tool = desc
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("list_requirements"))
        .expect("list_requirements descriptor must exist");

    assert!(
        tool.pointer("/inputSchema/properties/priority").is_some(),
        "list_requirements should advertise the priority filter it accepts"
    );
}

// trace:BUG-377 TASK-550 | ai:codex
// STORY-82: title / priority / tags are now advertised + persisted (they
// were ignored before). trace:STORY-82 | ai:claude
#[test]
fn mcp_update_requirement_persists_advertised_fields_only() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let response = server
        .tool_add_requirement(&json!({
            "title": "Update target",
            "description": "before",
            "type": "task",
        }))
        .unwrap();
    let spec_id = added_spec_id(&response).to_string();
    // BUG-481: Draft → InProgress via MCP is now advisor-gated; this test is
    // about field persistence, so triage the spec into the pipeline
    // out-of-band first (Approved → InProgress is implementer-legitimate).
    force_status(&server, &spec_id, RequirementStatus::Approved);

    let result = server
        .tool_update_requirement(&json!({
            "id": spec_id,
            // BUG-449/BUG-481: Approved → InProgress is an implementer-
            // legitimate transition; this test is about field persistence.
            "status": "in-progress",
            "description": "after",
            "title": "new title",
            "priority": "high",
            "tags": ["kept"],
        }))
        .unwrap();
    assert!(result.contains("status:"), "{result}");
    assert!(result.contains("description updated"), "{result}");
    assert!(result.contains("title:"), "{result}");
    assert!(result.contains("priority:"), "{result}");

    let store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id(&spec_id)
        .expect("requirement should exist");
    assert_eq!(req.title, "new title");
    assert_eq!(req.description, "after");
    assert_eq!(req.status, RequirementStatus::InProgress);
    assert_eq!(req.priority, RequirementPriority::High);
    assert!(req.tags.contains("kept"));
}

// trace:BUG-449 | ai:claude
#[test]
fn mcp_update_requirement_gates_advisor_and_merge_driven_statuses() {
    let dir = tempdir().unwrap();
    // BUG-1618: pin the ambient project root / TTY / identity so a leased
    // worktree's live roster cannot grant the refusal paths advisor authority.
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None); // trace:BUG-1618 | ai:claude
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Gate target",
            "description": "BUG-449 status gate",
            "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();

    // The add-then-update bypass must be refused: advisor-triage statuses…
    for status in ["approved", "planned"] {
        let err = server
            .tool_update_requirement(&json!({ "id": id, "status": status }))
            .expect_err("approved/planned via MCP must be refused");
        assert!(err.contains("advisor"), "{status}: {err}");
    }
    // …and the merge-driven Completed (the headline self-mark-on-uncommitted bypass).
    let err = server
        .tool_update_requirement(&json!({ "id": id, "status": "completed" }))
        .expect_err("completed via MCP must be refused");
    assert!(err.contains("merge") || err.contains("(SPEC-ID)"), "{err}");

    // BUG-481: Draft → InProgress/Done via MCP is now also refused — a
    // never-triaged Draft can't be pushed into the execution pipeline.
    for status in ["in-progress", "done"] {
        let err = server
            .tool_update_requirement(&json!({ "id": id, "status": status }))
            .expect_err("Draft → in-progress/done via MCP must be refused (BUG-481)");
        assert!(err.contains("advisor"), "{status}: {err}");
    }

    // Once the advisor has triaged the spec into the pipeline (set
    // out-of-band here, as the advisor/CLI would), the in-pipeline
    // execution flips are implementer-legitimate and stay allowed.
    force_status(&server, &id, RequirementStatus::Approved);
    for status in ["in-progress", "done"] {
        server
            .tool_update_requirement(&json!({ "id": id, "status": status }))
            .unwrap_or_else(|e| panic!("{status} should be allowed via MCP: {e}"));
    }
    // Punting (InProgress → NeedsAttention) stays implementer-legitimate;
    // it is the design-fork escape, not a pipeline advance.
    force_status(&server, &id, RequirementStatus::InProgress);
    server
        .tool_update_requirement(&json!({ "id": id, "status": "needs-attention" }))
        .expect("punt (InProgress → NeedsAttention) stays allowed via MCP");
    // …but re-advancing OUT of NeedsAttention into the pipeline is gated
    // (BUG-481 — the same intake-bypass class as Draft).
    let err = server
        .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
        .expect_err("NeedsAttention → in-progress via MCP must be refused (BUG-481)");
    assert!(err.contains("advisor"), "{err}");
    // Reach Done legitimately again so the final assertion holds.
    force_status(&server, &id, RequirementStatus::Done);

    // Nothing slipped the gate: the spec never reached an advisor/merge status.
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(req.status, RequirementStatus::Done);
}

/// TASK-1955 (slice 7 round-1 finding 5): MCP refuses a status write on a
/// deferred spec (flag or legacy `deferred:*` tag) as `aida edit` does —
/// it reads `deferred`, so the write would report success and show
/// nothing. Other fields still update.
// trace:TASK-1955 | ai:claude
#[test]
fn mcp_update_requirement_refuses_a_status_write_on_a_deferred_spec() {
    let dir = tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None);
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Deferred target",
            "description": "deferred status gate",
            "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();
    force_status(&server, &id, RequirementStatus::Approved);
    let set = |f: &dyn Fn(&mut Requirement)| {
        let mut store = server.storage.load().unwrap();
        f(store.get_requirement_by_spec_id_mut(&id).unwrap());
        server.storage.save(&store).unwrap();
    };
    set(&|r| r.deferred = true);
    let err = server
        .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
        .expect_err("a status write on a deferred spec must be refused");
    assert!(err.contains("aida undefer"), "{err}");
    server
        .tool_update_requirement(&json!({ "id": id, "title": "still editable" }))
        .expect("non-status fields stay editable");

    set(&|r| {
        r.deferred = false;
        r.tags.insert("deferred:later".to_string());
    });
    let err = server
        .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
        .expect_err("a legacy deferred:* tag defers too");
    assert!(err.contains("--remove-tag deferred:later"), "{err}");

    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(req.status, RequirementStatus::Approved);
    assert_eq!(req.title, "still editable");
}

/// BUG-481: the update_requirement status gate keyed only on the TARGET
/// status (`Approved|Planned|Completed`), so `add_requirement` (Draft) →
/// `update_requirement{status:in-progress|done}` was a two-tool bypass that
/// pushed a never-triaged Draft into the execution pipeline. The gate is
/// now keyed on the (SOURCE, TARGET) pair: Draft → InProgress/Done is
/// refused, while a legitimately-triaged Approved → InProgress still works.
// trace:BUG-481 | ai:claude
#[test]
fn mcp_update_requirement_gates_draft_into_pipeline() {
    let dir = tempdir().unwrap();
    // BUG-1618: this asserts MCP refusals, which resolve advisor authority
    // from the process-global AIDA_SESSION_ROLE; a leased agent session
    // inherits `advisor`. Pin a hermetic, role-less ambient context.
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None); // trace:BUG-1618 | ai:claude
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Pipeline bypass attempt",
            "description": "BUG-481 (source,target) gate",
            "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();

    // Draft → InProgress and Draft → Done are both refused.
    for status in ["in-progress", "done"] {
        let err = server
            .tool_update_requirement(&json!({ "id": id, "status": status }))
            .expect_err("Draft → in-progress/done via MCP must be refused");
        assert!(err.contains("advisor"), "{status}: {err}");
    }
    // The spec never left Draft.
    let store = server.storage.load().unwrap();
    assert_eq!(
        store.get_requirement_by_spec_id(&id).unwrap().status,
        RequirementStatus::Draft
    );

    // A legitimately-triaged spec (advisor set it Approved out-of-band)
    // flips Approved → InProgress freely — the gate only blocks the
    // un-triaged source, not in-pipeline execution.
    force_status(&server, &id, RequirementStatus::Approved);
    server
        .tool_update_requirement(&json!({ "id": id, "status": "in-progress" }))
        .expect("Approved → in-progress is implementer-legitimate via MCP");
    let store = server.storage.load().unwrap();
    assert_eq!(
        store.get_requirement_by_spec_id(&id).unwrap().status,
        RequirementStatus::InProgress
    );
}

// ===================================================================
// TASK-681 (EPIC-38): authority/lifecycle gate parity audit across
// every mutating MCP tool. The audit found one bypass —
// `triage_finding promote` self-granted Approved — now gated below.
// The remaining mutating tools carry no spec-status/lifecycle
// invariant for an MCP caller to bypass (they either always land a
// safe status, or write coordination state the orchestrator/CLI
// re-derives); these guard tests assert that property so a future
// edit that adds a status mutation trips a red test.
// trace:TASK-681 | ai:claude
// ===================================================================

/// THE FIX: `triage_finding promote` sets a finding `Approved`, which is
/// the advisor's triage decision (same invariant `update_requirement`
/// gates under BUG-449). It must be refused for an MCP caller.
// trace:TASK-681 | ai:claude
#[test]
fn mcp_triage_finding_promote_is_advisor_gated() {
    let dir = tempdir().unwrap();
    // BUG-1618: this asserts MCP refusals, which resolve advisor authority
    // from the process-global AIDA_SESSION_ROLE; a leased agent session
    // inherits `advisor`. Pin a hermetic, role-less ambient context.
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None); // trace:BUG-1618 | ai:claude
    let server = mk_server(dir.path());
    let filed = server
        .tool_file_finding(&json!({
            "title": "Promote bypass attempt",
            "description": "finding body",
            "source": "review",
            "pr": 681,
        }))
        .unwrap();
    let finding_id = filed
        .strip_prefix("Finding filed: ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_string();

    let err = server
        .tool_triage_finding(&json!({
            "id": finding_id,
            "action": "promote",
            "reason": "I want this approved",
        }))
        .expect_err("triage_finding promote must be advisor-gated via MCP");
    assert!(err.contains("advisor"), "{err}");

    // The finding never reached Approved — it stays Draft as filed.
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&finding_id).unwrap();
    assert_eq!(req.status, RequirementStatus::Draft);

    // `dismiss` (Rejected) is implementer-legitimate and still works.
    server
        .tool_triage_finding(&json!({ "id": finding_id, "action": "dismiss" }))
        .expect("dismiss is implementer-legitimate and stays allowed");
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&finding_id).unwrap();
    assert_eq!(req.status, RequirementStatus::Rejected);
}

/// `file_finding` is the findings analogue of `add_requirement`: it must
/// always land `Draft` regardless of any status the caller might try to
/// smuggle in (it accepts no `status` arg today; this pins that).
// trace:TASK-681 | ai:claude
#[test]
fn mcp_file_finding_always_lands_draft() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let filed = server
        .tool_file_finding(&json!({
            "title": "Intake finding",
            "description": "body",
            "source": "implementer",
            "spec_id": "TASK-681",
            // a status arg, if it ever leaks in, must be ignored:
            "status": "approved",
        }))
        .unwrap();
    let id = filed
        .strip_prefix("Finding filed: ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_string();
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(req.status, RequirementStatus::Draft);
}

// trace:STORY-1674 | ai:codex
#[test]
fn story1674_mcp_comment_and_edit_persist_write_context() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[
        // The temporary store test must not append to an inherited live role log.
        ("AIDA_SESSION_ROLE", None),
        ("AIDA_SESSION_ID", Some("mcp-exact-session")),
        ("AIDA_HARNESS_VERSION", Some("mcp-harness")),
    ]);
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(
            &json!({"title":"Provenance target","description":"body","type":"task"}),
        )
        .unwrap();
    let id = added_spec_id(&added).to_string();
    server
        .tool_add_comment(&json!({"id":id,"text":"a note"}))
        .unwrap();
    server
        .tool_update_requirement(&json!({"id":id,"title":"Edited through MCP"}))
        .unwrap();
    let store: aida_core::RequirementsStore = serde_yaml::from_str(
        &std::fs::read_to_string(dir.path().join(".aida/mcp-test-cache.yaml")).unwrap(),
    )
    .unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(
        req.filed_at.as_ref().unwrap().harness_version.as_deref(),
        Some("mcp-harness")
    );
    assert_eq!(
        req.comments.last().unwrap().harness_version.as_deref(),
        Some("mcp-harness")
    );
    assert_eq!(
        req.comments.last().unwrap().session_id.as_deref(),
        Some("mcp-exact-session")
    );
    let edit = req.history.last().unwrap();
    assert_eq!(edit.session_id.as_deref(), Some("mcp-exact-session"));
    assert_eq!(edit.harness_version.as_deref(), Some("mcp-harness"));
}

/// `add_comment` carries no status/lifecycle mutation — confirm it leaves
/// the spec's status untouched no matter what.
// trace:TASK-681 | ai:claude
#[test]
fn mcp_add_comment_does_not_touch_status() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Comment-only target",
            "description": "body",
            "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();
    server
        .tool_add_comment(&json!({ "id": id, "text": "a note" }))
        .unwrap();
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(req.status, RequirementStatus::Draft);
}

/// `add_relationship` links specs and (correctly) guards adding children
/// to a terminal parent, but it must not mutate either spec's status.
// trace:TASK-681 | ai:claude
#[test]
fn mcp_add_relationship_does_not_touch_status() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let parent = server
        .tool_add_requirement(&json!({
            "title": "Parent",
            "description": "p",
            "type": "epic",
        }))
        .unwrap();
    let parent_id = added_spec_id(&parent).to_string();
    let child = server
        .tool_add_requirement(&json!({
            "title": "Child",
            "description": "c",
            "type": "task",
        }))
        .unwrap();
    let child_id = added_spec_id(&child).to_string();
    server
        .tool_add_relationship(&json!({
            "spec_id": child_id,
            "target_spec_id": parent_id,
            "relationship_type": "parent",
        }))
        .unwrap();
    let store = server.storage.load().unwrap();
    for id in [&parent_id, &child_id] {
        assert_eq!(
            store.get_requirement_by_spec_id(id).unwrap().status,
            RequirementStatus::Draft,
            "{id} status must be untouched by add_relationship"
        );
    }
}

/// The coordination-channel writers (`post_punt`/`resolve_punt`/
/// `escalate_punt`/`post_directive`/`ack_brief`/`send_message`) write
/// runtime files the orchestrator/CLI consume; none of them touch spec
/// status in the store. Exercise the punt + directive writers and assert
/// the requirement store is never mutated by them. (Status transitions
/// for punted work are applied by the orchestrator, not the MCP writer.)
// trace:TASK-681 | ai:claude
#[test]
fn mcp_coordination_writers_do_not_touch_spec_status() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let added = server
        .tool_add_requirement(&json!({
            "title": "Coordination target",
            "description": "body",
            "type": "task",
        }))
        .unwrap();
    let id = added_spec_id(&added).to_string();

    server
        .tool_post_punt(&json!({
            "spec_id": id,
            "detail": "which approach?",
            "category": "design-fork",
        }))
        .unwrap();
    server
        .tool_resolve_punt(&json!({
            "spec_id": id,
            "answer": "approach A",
            "reasoning": "simpler",
        }))
        .unwrap();
    server
        .tool_post_directive(&json!({ "verb": "pause" }))
        .unwrap();

    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&id).unwrap();
    assert_eq!(
        req.status,
        RequirementStatus::Draft,
        "coordination writers must not mutate spec status"
    );
}

// trace:BUG-377 TASK-550 | ai:codex
#[test]
fn mcp_add_comment_persists_text_as_comment_content() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let response = server
        .tool_add_requirement(&json!({
            "title": "Comment target",
            "description": "comment roundtrip",
            "type": "task",
        }))
        .unwrap();
    let spec_id = added_spec_id(&response).to_string();

    server
        .tool_add_comment(&json!({
            "id": spec_id,
            "text": "visible comment via MCP",
        }))
        .unwrap();

    let store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id(&spec_id)
        .expect("requirement should exist");
    assert_eq!(req.comments.len(), 1);
    assert_eq!(req.comments[0].author, "mcp");
    assert_eq!(req.comments[0].content, "visible comment via MCP");
}

// trace:BUG-377 TASK-550 | ai:codex
#[test]
fn mcp_triage_finding_persists_reason_as_comment_content() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let filed = server
        .tool_file_finding(&json!({
            "title": "Finding target",
            "description": "finding body",
            "source": "review",
            "pr": 377,
        }))
        .unwrap();
    let finding_id = filed
        .strip_prefix("Finding filed: ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .to_string();

    // TASK-681: `promote` is now advisor-gated (it sets Approved), so the
    // reason-persistence path is exercised via `dismiss` (Rejected stays
    // an implementer-legitimate transition).
    server
        .tool_triage_finding(&json!({
            "id": finding_id,
            "action": "dismiss",
            "reason": "not actually a problem",
        }))
        .unwrap();

    let store = server.storage.load().unwrap();
    let req = store
        .get_requirement_by_spec_id(&finding_id)
        .expect("finding should exist");
    assert_eq!(req.status, RequirementStatus::Rejected);
    assert_eq!(req.comments.len(), 1);
    assert_eq!(req.comments[0].author, "mcp");
    assert_eq!(
        req.comments[0].content,
        "dismissed via MCP: not actually a problem"
    );
}

// trace:TASK-551 | ai:codex
#[test]
fn mcp_add_relationship_roundtrips_to_show_requirement() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let parent = server
        .tool_add_requirement(&json!({
            "title": "Parent epic",
            "description": "relationship target",
            "type": "epic",
        }))
        .unwrap();
    let parent_id = added_spec_id(&parent).to_string();
    let child = server
        .tool_add_requirement(&json!({
            "title": "Child task",
            "description": "relationship source",
            "type": "task",
        }))
        .unwrap();
    let child_id = added_spec_id(&child).to_string();

    let response = server
        .tool_add_relationship(&json!({
            "spec_id": child_id,
            "relationship_type": "child",
            "target_spec_id": parent_id,
            "bidirectional": true,
        }))
        .unwrap();
    assert!(response.contains("Relationship added"), "{response}");
    assert!(response.contains("inverse added as parent"), "{response}");

    let shown_child = server
        .tool_show_requirement(&json!({ "id": child_id }))
        .unwrap();
    assert!(
        shown_child.contains(&format!("- child → {parent_id}")),
        "{shown_child}"
    );
    let shown_parent = server
        .tool_show_requirement(&json!({ "id": parent_id }))
        .unwrap();
    assert!(
        shown_parent.contains(&format!("- parent → {child_id}")),
        "{shown_parent}"
    );
}

// trace:TASK-551 | ai:codex
#[test]
fn mcp_add_relationship_validates_missing_specs_and_empty_type() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let req = server
        .tool_add_requirement(&json!({
            "title": "Source",
            "description": "source exists",
            "type": "task",
        }))
        .unwrap();
    let source_id = added_spec_id(&req).to_string();

    let missing_source = server
        .tool_add_relationship(&json!({
            "spec_id": "TASK-999999",
            "relationship_type": "references",
            "target_spec_id": source_id,
        }))
        .expect_err("missing source should fail");
    assert!(missing_source.contains("Requirement 'TASK-999999' not found"));

    let empty_type = server
        .tool_add_relationship(&json!({
            "spec_id": source_id,
            "relationship_type": " ",
            "target_spec_id": "TASK-999999",
        }))
        .expect_err("empty type should fail before target lookup");
    assert!(empty_type.contains("relationship_type must be non-empty"));
}

// trace:BUG-790 | ai:codex
#[test]
fn mcp_add_relationship_rejects_explicit_cross_store_refs() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let local = server
        .tool_add_requirement(&json!({
            "title": "Local collision",
            "description": "source exists",
            "type": "task",
        }))
        .unwrap();
    let local_id = added_spec_id(&local).to_string();

    let err = server
        .tool_add_relationship(&json!({
            "spec_id": local_id,
            "relationship_type": "references",
            "target_spec_id": "aida-hub#STORY-21",
        }))
        .expect_err("cross-store refs are not local graph edges");
    assert!(err.contains("cross-store requirement reference"), "{err}");
    assert!(
        err.contains("cannot be resolved as a local graph edge"),
        "{err}"
    );
}

// trace:BUG-790 | ai:codex
#[test]
fn mcp_add_relationship_warns_on_open_to_terminal_non_hierarchy_target() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let source = server
        .tool_add_requirement(&json!({
            "title": "Open local source",
            "description": "source exists",
            "type": "story",
        }))
        .unwrap();
    let source_id = added_spec_id(&source).to_string();
    let target = server
        .tool_add_requirement(&json!({
            "title": "Terminal local target",
            "description": "target exists",
            "type": "story",
        }))
        .unwrap();
    let target_id = added_spec_id(&target).to_string();
    force_status(&server, &target_id, RequirementStatus::Rejected);

    let response = server
        .tool_add_relationship(&json!({
            "spec_id": source_id,
            "relationship_type": "references",
            "target_spec_id": target_id,
        }))
        .unwrap();
    assert!(response.contains("Relationship added"), "{response}");
    assert!(response.contains("Warning:"), "{response}");
    assert!(response.contains("another store"), "{response}");
}

// trace:TASK-551 | ai:codex
#[test]
fn mcp_add_relationship_aliases_dependency_vocabulary() {
    assert_eq!(
        parse_mcp_relationship_type("depends-on").unwrap(),
        RelationshipType::BlockedBy
    );
    assert_eq!(
        parse_mcp_relationship_type("related").unwrap(),
        RelationshipType::References
    );
    assert_eq!(
        parse_mcp_relationship_type("subsumes").unwrap(),
        RelationshipType::Custom("subsumes".to_string())
    );
}

// trace:TASK-551 | ai:codex
#[test]
fn mcp_add_relationship_descriptor_is_advertised() {
    let desc = tool_descriptors();
    let names: Vec<&str> = desc
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
        .collect();
    assert!(names.contains(&"add_relationship"));
}

// trace:TASK-551 | ai:codex
#[test]
fn mcp_add_relationship_preserves_terminal_parent_guard() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let parent = server
        .tool_add_requirement(&json!({
            "title": "Closed parent",
            "description": "terminal parent",
            "type": "epic",
        }))
        .unwrap();
    let parent_id = added_spec_id(&parent).to_string();
    // TASK-647 (ADR-3): MCP add lands draft; drive the parent to the
    // terminal `completed` state (the guard's precondition) out-of-band —
    // BUG-449 forbids setting Completed via the MCP update tool itself.
    force_status(&server, &parent_id, RequirementStatus::Completed);
    let child = server
        .tool_add_requirement(&json!({
            "title": "Child task",
            "description": "new child",
            "type": "task",
        }))
        .unwrap();
    let child_id = added_spec_id(&child).to_string();

    let err = server
        .tool_add_relationship(&json!({
            "spec_id": child_id,
            "relationship_type": "child",
            "target_spec_id": parent_id,
        }))
        .expect_err("closed parent should require force_parent");
    assert!(
        err.contains("adding new children to a closed parent"),
        "{err}"
    );
}

// TASK-1468: MCP write tools refuse an id that names two requirements
// (a merge-gate agreed_id colliding with another spec's native id), and
// a refused punt leaves no ledger entry. trace:TASK-1468 | ai:claude
#[test]
fn mcp_write_tools_refuse_an_ambiguous_id() {
    let dir = tempdir().unwrap();
    let server = mk_server(dir.path());
    let a = server
        .tool_add_requirement(
            &json!({ "title": "native owner", "description": "d", "type": "task" }),
        )
        .unwrap();
    let a = added_spec_id(&a).to_string();
    let b = server
        .tool_add_requirement(
            &json!({ "title": "agreed holder", "description": "d", "type": "task" }),
        )
        .unwrap();
    let b = added_spec_id(&b).to_string();
    let mut store = server.storage.load().unwrap();
    let holder = store.get_requirement_by_spec_id_mut(&b).unwrap();
    holder.agreed_id = Some(a.clone());
    holder.tags.insert("from-review".to_string());
    server.storage.save(&store).unwrap();

    let err = server
        .tool_post_punt(&json!({ "spec_id": a, "detail": "fork", "category": "design-fork" }))
        .expect_err("an ambiguous punt target must refuse");
    assert!(err.contains("ambiguous"), "{err}");
    let listed = server.tool_list_punts(&json!({})).unwrap();
    assert!(
        !listed.contains(&a),
        "refused punt must not be recorded: {listed}"
    );

    let err = server
        .tool_triage_finding(&json!({ "id": a, "action": "dismiss" }))
        .expect_err("an ambiguous finding id must refuse");
    assert!(err.contains("ambiguous"), "{err}");

    let err = server
        .tool_add_requirement(&json!({
            "title": "child", "description": "d", "type": "task", "parent": a,
        }))
        .expect_err("an ambiguous parent must refuse");
    assert!(err.contains("ambiguous"), "{err}");

    // The unambiguous handle still reaches the agreed-id holder.
    server
        .tool_post_punt(&json!({ "spec_id": b, "detail": "fork", "category": "design-fork" }))
        .unwrap();
}

#[test]
fn post_punt_then_list_and_read_round_trips() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let r = srv.tool_post_punt(&json!({
        "spec_id": "STORY-X",
        "detail": "two valid auth flows; spec doesn't say",
        "category": "design-fork",
        "lean": "OAuth"
    }));
    assert!(r.is_ok(), "{:?}", r);

    let listed = srv.tool_list_punts(&json!({})).unwrap();
    assert!(listed.contains("STORY-X"), "{}", listed);

    let read = srv.tool_read_punt(&json!({"spec_id": "STORY-X"})).unwrap();
    assert!(read.contains("design-fork"), "{}", read);
    assert!(read.contains("OAuth"), "{}", read);
}

/// BUG-334: post_punt flips an In Progress spec to NeedsAttention (matching
/// the CLI `aida punt`), instead of only recording the ledger entry and
/// telling the operator to flip it by hand.
#[test]
fn post_punt_flips_in_progress_spec_to_needs_attention() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());
    let add = srv
        .tool_add_requirement(&json!({"title":"flip me","description":"d","type":"task"}))
        .unwrap();
    let id = added_spec_id(&add).to_string();
    // TASK-647 (ADR-3): MCP add lands draft; reach In Progress (the punt
    // pre-state). BUG-481: Draft → InProgress via MCP is now advisor-gated,
    // so set the pre-state out-of-band like the other gated statuses.
    force_status(&srv, &id, RequirementStatus::InProgress);

    let r = srv
        .tool_post_punt(&json!({"spec_id": id, "detail":"hit a fork", "category":"design-fork"}))
        .unwrap();
    assert!(
        r.contains("NeedsAttention"),
        "punt response should report the flip: {r}"
    );

    let shown = srv.tool_show_requirement(&json!({"id": id})).unwrap();
    assert!(
        shown.to_lowercase().contains("needs attention") || shown.contains("NeedsAttention"),
        "spec should be NeedsAttention after punt: {shown}"
    );
}

#[test]
fn resolve_punt_writes_response_file() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_resolve_punt(&json!({
        "spec_id": "STORY-Y",
        "answer": "pick option B",
        "reasoning": "consistent with recorded preference"
    }))
    .unwrap();

    let path = punt::punt_response_path(dir.path(), "STORY-Y");
    assert!(path.exists());
    let body = std::fs::read_to_string(&path).unwrap();
    let resp: punt::PuntResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(resp.resolution, punt::PuntResolution::Resolved);
    assert_eq!(resp.answer.as_deref(), Some("pick option B"));
}

#[test]
fn escalate_punt_writes_response_file_with_escalated_resolution() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_escalate_punt(&json!({
        "spec_id": "STORY-Z",
        "reasoning": "irreversible architecture call",
        "escalation_reason": "irreversible"
    }))
    .unwrap();

    let path = punt::punt_response_path(dir.path(), "STORY-Z");
    let body = std::fs::read_to_string(&path).unwrap();
    let resp: punt::PuntResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(resp.resolution, punt::PuntResolution::Escalated);
    assert!(resp.answer.is_none());
    assert_eq!(resp.escalation_reason.as_deref(), Some("irreversible"));
}

#[test]
fn directives_post_list_ack_round_trip() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_post_directive(&json!({"verb": "drain", "args": ["--zen"]}))
        .unwrap();
    srv.tool_post_directive(&json!({"verb": "pause"})).unwrap();

    let listed = srv.tool_list_directives().unwrap();
    assert!(listed.contains("drain"), "{}", listed);
    assert!(listed.contains("pause"), "{}", listed);

    // Ack the first (index 0) — pause should remain.
    srv.tool_ack_directive(&json!({"index": 0})).unwrap();
    let listed_after = srv.tool_list_directives().unwrap();
    assert!(!listed_after.contains("drain --zen"), "{}", listed_after);
    assert!(listed_after.contains("pause"), "{}", listed_after);
}

// trace:TASK-437 | ai:claude
#[test]
fn file_finding_rejects_pr_as_string() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let err = srv
        .tool_file_finding(&json!({
            "title": "x",
            "description": "y",
            "source": "review",
            "pr": "7",
        }))
        .expect_err("string pr should be rejected");
    assert!(err.contains("pr must be an integer"), "{}", err);
}

// trace:TASK-437 | ai:claude
#[test]
fn file_finding_accepts_pr_as_integer_and_uses_canonical_tag() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_file_finding(&json!({
        "title": "broken thing",
        "description": "details",
        "source": "review",
        "pr": 7,
    }))
    .expect("integer pr should be accepted");

    let store = srv.storage.load().unwrap();
    let req = store
        .requirements
        .iter()
        .find(|r| r.title == "broken thing")
        .expect("requirement should have been saved");
    assert!(
        req.tags.contains("from-review:PR-7"),
        "expected canonical from-review:PR-7 tag, got tags: {:?}",
        req.tags,
    );
}

// trace:TASK-437 | ai:claude
#[test]
fn file_finding_rejects_spec_id_as_integer() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let err = srv
        .tool_file_finding(&json!({
            "title": "x",
            "description": "y",
            "source": "implementer",
            "spec_id": 42,
        }))
        .expect_err("integer spec_id should be rejected");
    assert!(err.contains("spec_id must be a string"), "{}", err);
}

// trace:TASK-b6b1278890774ef993d2ebc327862470-269 | ai:claude
#[test]
fn post_directive_rejects_unknown_verb() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let err = srv
        .tool_post_directive(&json!({"verb": "drian"}))
        .expect_err("typo should be rejected");
    assert!(err.contains("invalid directive verb"), "{}", err);
    assert!(err.contains("drian"), "{}", err);

    // The bad verb must NOT have been appended to .aida/worker.cmd.
    let path = crate::worker::worker_cmd_path(dir.path());
    if path.exists() {
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(!body.contains("drian"), "{}", body);
    }
}

#[test]
fn mcp_claim_path_is_case_insensitive_for_session_end_reaping() {
    // BUG-694: `aida session end` reaps the sibling mcp-claim by calling
    // mcp_claim_path(leases_dir, target.scope). The scope on the session
    // lease may differ in case from the spec_id claim_task lowercased into
    // the filename, so the path MUST fold case — else the reap misses and
    // the orphan lingers (the 24-day TASK-702 leak). Lock the contract.
    let dir = std::path::Path::new("/nonexistent/sessions");
    let lower = mcp_claim_path(dir, "task-702");
    let upper = mcp_claim_path(dir, "TASK-702");
    assert_eq!(lower, upper, "scope casing must not change the reaped path");
    assert_eq!(
        upper,
        dir.join("mcp-claim.task-702.toml"),
        "the reaped filename must match what claim_task writes"
    );
}

#[test]
fn claim_task_then_release_round_trips() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let claim = srv
        .tool_claim_task(&json!({"spec_id": "STORY-CLAIM", "role": "implementer"}))
        .unwrap();
    assert!(claim.starts_with("claimed:"), "{}", claim);
    let lease_id = claim.trim_start_matches("claimed: lease_id=").trim();

    // Second claim on same spec → already_claimed
    let again = srv
        .tool_claim_task(&json!({"spec_id": "STORY-CLAIM"}))
        .unwrap();
    assert!(again.contains("already_claimed"), "{}", again);

    // list_active_leases sees it
    let listed = srv.tool_list_active_leases().unwrap();
    assert!(listed.contains("STORY-CLAIM"), "{}", listed);
    assert!(listed.contains("kind=mcp"), "{}", listed);

    // Release deletes the lease file
    srv.tool_release_task(&json!({"lease_id": lease_id}))
        .unwrap();
    let listed_after = srv.tool_list_active_leases().unwrap();
    assert!(!listed_after.contains("STORY-CLAIM"), "{}", listed_after);
}

/// TASK-474: when the agent passes `worktree_path`, the lease records
/// that explicit sibling worktree so consumers (`active_lease_for_cwd`
/// in main.rs) route hints correctly — instead of misattributing the
/// agent's scope to every shell in the parent (which is what happened
/// before the arg existed: `worktree_path` was hardcoded to
/// `self.project_root`). TASK-504 canonicalizes the path at write-time
/// so later cwd comparisons are stable.
/// trace:TASK-474 TASK-504 | ai:claude
#[test]
fn claim_task_records_explicit_worktree_path() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let sibling_dir = dir.path().join("aida-task-310");
    std::fs::create_dir_all(&sibling_dir).unwrap();
    let sibling = sibling_dir.to_string_lossy().to_string();
    let claim = srv
        .tool_claim_task(&json!({
            "spec_id": "TASK-310",
            "role": "implementer",
            "worktree_path": sibling,
        }))
        .unwrap();
    assert!(claim.starts_with("claimed:"), "{}", claim);

    // Round-trip through the on-disk TOML — that's what `aida add`'s
    // hint reads via `list_leases` + `active_lease_for_cwd`. TASK-504
    // canonicalizes the path at write-time, so compare against the
    // canonical form (matches sibling test
    // `claim_task_canonicalizes_explicit_worktree_path`). Without
    // canonicalizing first this fails on Windows (UNC `\\?\` prefix +
    // `RUNNER~1` → `runneradmin` short-name expansion) and macOS
    // (`/var` → `/private/var` symlink resolution). trace:BUG-385 | ai:claude
    let leases = list_leases(dir.path());
    let l = leases
        .iter()
        .find(|l| l.scope == "TASK-310")
        .expect("the claim should have written a lease");
    assert_eq!(
        l.worktree_path,
        sibling_dir.canonicalize().unwrap().to_string_lossy()
    );
}

/// TASK-504: claim_task canonicalizes the recorded `worktree_path` so a
/// non-canonical input path (for example, containing `..`) still matches
/// the canonical cwd used by `lease_covers_cwd`.
/// trace:TASK-504 | ai:codex
#[test]
fn claim_task_canonicalizes_explicit_worktree_path() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());
    let real_worktree = dir.path().join("worktrees").join("task-504");
    std::fs::create_dir_all(&real_worktree).unwrap();
    let raw_worktree = real_worktree
        .parent()
        .unwrap()
        .join("..")
        .join("worktrees")
        .join("task-504");

    srv.tool_claim_task(&json!({
        "spec_id": "TASK-504",
        "role": "implementer",
        "worktree_path": raw_worktree.to_string_lossy(),
    }))
    .unwrap();

    let leases = list_leases(dir.path());
    let l = leases
        .iter()
        .find(|l| l.scope == "TASK-504")
        .expect("the claim should have written a lease");
    assert_eq!(
        l.worktree_path,
        real_worktree.canonicalize().unwrap().to_string_lossy()
    );
}

/// TASK-474: when `worktree_path` is omitted, the lease's worktree field
/// is the empty string — which `lease_covers_cwd` in main.rs treats as
/// "no session context, can't cover any cwd," preventing the misrouted
/// scope hint that was the original symptom.
/// trace:TASK-474 | ai:claude
#[test]
fn claim_task_omits_worktree_when_arg_absent() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_claim_task(&json!({"spec_id": "TASK-474", "role": "implementer"}))
        .unwrap();

    let leases = list_leases(dir.path());
    let l = leases
        .iter()
        .find(|l| l.scope == "TASK-474")
        .expect("the claim should have written a lease");
    assert_eq!(
        l.worktree_path, "",
        "absent worktree_path arg must NOT default to project_root — that was the BUG",
    );
}

/// TASK-474: an empty-string `worktree_path` arg behaves the same as
/// omitting the arg — both signal "no session context, advisory lock
/// only," and both result in an empty `worktree_path` on the lease.
/// trace:TASK-474 | ai:claude
#[test]
fn claim_task_treats_empty_worktree_arg_as_absent() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    srv.tool_claim_task(&json!({
        "spec_id": "TASK-474B",
        "worktree_path": "   ",
    }))
    .unwrap();

    let leases = list_leases(dir.path());
    let l = leases
        .iter()
        .find(|l| l.scope == "TASK-474B")
        .expect("the claim should have written a lease");
    assert_eq!(l.worktree_path, "");
}

#[test]
fn release_task_refuses_non_mcp_lease() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    // Hand-create a non-MCP lease (mcp_claim defaults false).
    let lease = LightLease {
        id: "deadbeef0000".to_string(),
        scope: "STORY-Q".to_string(),
        slug: "STORY-Q".to_string(),
        owner: "alice".to_string(),
        worktree_path: dir.path().to_string_lossy().to_string(),
        branch: "main".to_string(),
        started_at: Utc::now().to_rfc3339(),
        hostname: "imac".to_string(),
        role: Some("implementer".to_string()),
        mcp_claim: false,
    };
    let dir_leases = leases_dir(dir.path());
    std::fs::create_dir_all(&dir_leases).unwrap();
    let path = dir_leases.join("deadbeef0000.toml");
    std::fs::write(&path, toml::to_string(&lease).unwrap()).unwrap();

    let err = srv
        .tool_release_task(&json!({"lease_id": "deadbeef0000"}))
        .unwrap_err();
    assert!(err.contains("aida session end"), "{}", err);
    assert!(path.exists(), "non-mcp lease should not be deleted");
}

fn write_test_brief(root: &Path, agent: &str, name: &str, status: &str) -> PathBuf {
    let dir = root.join(".aida").join("agent-briefs").join(agent);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let spec_id = name.split_once("-20").map(|(s, _)| s).unwrap_or("TASK-X");
    std::fs::write(
            &path,
            format!(
                "---\nspec_id: {spec_id}\nagent: {agent}\ngenerated_at: 2026-05-23T020000Z\nstatus: {status}\n---\n\n## Routing\n\nBrief body for {spec_id}\n"
            ),
        )
        .unwrap();
    path
}

#[test]
fn brief_tools_list_read_ack_roundtrip() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());
    let path = write_test_brief(
        dir.path(),
        "codex",
        "TASK-492-2026-05-23T020000Z.md",
        "pending",
    );
    write_test_brief(
        dir.path(),
        "antigravity",
        "TASK-999-2026-05-23T020001Z.md",
        "pending",
    );

    let listed = srv
        .tool_list_briefs(&json!({"agent": "codex"}))
        .expect("list briefs");
    assert!(listed.contains("Found 1 brief(s):"), "{}", listed);
    assert!(
        listed.contains("path=.aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"),
        "{}",
        listed
    );
    assert!(listed.contains("status=pending"), "{}", listed);
    assert!(!listed.contains("antigravity"), "{}", listed);

    let read = srv
        .tool_read_brief(
            &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}),
        )
        .expect("read brief");
    assert!(read.contains("Brief body for TASK-492"), "{}", read);

    let acked = srv
        .tool_ack_brief(&json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}))
        .expect("ack brief");
    assert!(
        acked.contains("acked: .aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked"),
        "{}",
        acked
    );
    assert!(!path.exists());
    let acked_path = dir
        .path()
        .join(".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked");
    let body = std::fs::read_to_string(&acked_path).unwrap();
    assert!(body.contains("status: acked"), "{}", body);

    let pending = srv
        .tool_list_briefs(&json!({"agent": "codex"}))
        .expect("list pending");
    assert_eq!(pending, "No briefs found.");
    let all = srv
        .tool_list_briefs(&json!({"agent": "codex", "include_acked": true}))
        .expect("list acked");
    assert!(all.contains("status=acked"), "{}", all);
}

#[test]
fn ack_brief_is_idempotent_for_acked_paths_and_original_paths() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());
    write_test_brief(
        dir.path(),
        "codex",
        "TASK-492-2026-05-23T020000Z.md",
        "pending",
    );
    srv.tool_ack_brief(&json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}))
        .unwrap();

    let again_original = srv
        .tool_ack_brief(&json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md"}))
        .unwrap();
    assert!(
        again_original.starts_with("already_acked:"),
        "{}",
        again_original
    );

    let again_acked = srv
        .tool_ack_brief(
            &json!({"path": ".aida/agent-briefs/codex/TASK-492-2026-05-23T020000Z.md.acked"}),
        )
        .unwrap();
    assert!(again_acked.starts_with("already_acked:"), "{}", again_acked);
}

#[test]
fn brief_tools_reject_path_escape_and_missing_files() {
    let dir = tempdir().unwrap();
    let srv = mk_server(dir.path());

    let err = srv
        .tool_read_brief(&json!({"path": "../secret"}))
        .unwrap_err();
    assert!(err.contains("must not contain"), "{}", err);

    let err = srv
        .tool_read_brief(&json!({"path": "CLAUDE.md"}))
        .unwrap_err();
    assert!(err.contains(".aida/agent-briefs"), "{}", err);

    let err = srv
        .tool_read_brief(&json!({"path": ".aida/agent-briefs/codex/missing.md"}))
        .unwrap_err();
    assert!(err.contains("reading brief"), "{}", err);
}

/// Concurrent-write contention test: many threads append punt records
/// in parallel; the ledger must contain exactly N parseable lines.
/// trace:STORY-361 | ai:claude
#[test]
fn concurrent_punt_appends_dont_corrupt_ledger() {
    let dir = tempdir().unwrap();
    let project_root = Arc::new(dir.path().to_path_buf());
    const N: usize = 32;

    let mut handles = Vec::new();
    for i in 0..N {
        let root = Arc::clone(&project_root);
        handles.push(thread::spawn(move || {
            let record = PuntRecord {
                timestamp: Utc::now(),
                spec: format!("STORY-{}", i),
                category: PuntCategory::Other,
                detail: format!("concurrent punt {}", i),
                lean: None,
                raised_by: Some("test".to_string()),
                resolution_path: "punted".to_string(),
                classification: None,
                escalation_reason: None,
                answer: None,
                answered_by: None,
                decision: None,
                principle_link: None,
                calibration_pair: None,
                paused_at: None,
                resolved_at: None,
            };
            append_to_ledger(&root, &record).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let records = read_ledger(&project_root);
    assert_eq!(records.len(), N, "every concurrent append must land");
    let specs: std::collections::HashSet<_> = records.iter().map(|r| r.spec.clone()).collect();
    assert_eq!(specs.len(), N, "each spec must appear exactly once");
}

/// Concurrent directive posts must not interleave / corrupt the worker.cmd
/// file. trace:STORY-361 | ai:claude
#[test]
fn concurrent_directive_posts_dont_corrupt_worker_cmd() {
    let dir = tempdir().unwrap();
    let project_root = Arc::new(dir.path().to_path_buf());
    const N: usize = 16;

    let mut handles = Vec::new();
    for i in 0..N {
        let root = Arc::clone(&project_root);
        handles.push(thread::spawn(move || {
            let path = crate::worker::worker_cmd_path(&root);
            append_line(&path, &format!("drain batch:{}", i)).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    let path = crate::worker::worker_cmd_path(&project_root);
    let directives = crate::worker::parse_directives(&path);
    assert_eq!(directives.len(), N, "every directive must land");
    // Every line is a well-formed "drain batch:K"
    let unique: std::collections::HashSet<_> = directives.iter().map(|d| d.raw.clone()).collect();
    assert_eq!(unique.len(), N);
}

/// Concurrent claim_task calls on the same spec must elect exactly one
/// winner — the others must see `already_claimed`. Regression test for
/// the pre-fix TOCTOU race between the existence check and the lease
/// write. trace:TASK-b6b1278890774ef993d2ebc327862470-271 | ai:claude
#[test]
fn concurrent_claim_task_on_same_spec_elects_one_winner() {
    let dir = tempdir().unwrap();
    let project_root = Arc::new(dir.path().to_path_buf());
    const N: usize = 16;
    const SPEC: &str = "STORY-RACE";

    // Gate every thread on a barrier so the contention window is as
    // tight as possible — maximises the chance the buggy version would
    // flake before the fix lands.
    let barrier = Arc::new(std::sync::Barrier::new(N));

    let mut handles = Vec::new();
    for _ in 0..N {
        let root = Arc::clone(&project_root);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let storage = Box::leak(Box::new(Storage::new(
                root.join(".aida").join("mcp-test-cache.yaml"),
            )));
            std::fs::create_dir_all(root.join(".aida")).unwrap();
            let srv = McpServer::new(storage, (*root).clone());
            barrier.wait();
            srv.tool_claim_task(&json!({"spec_id": SPEC})).unwrap()
        }));
    }
    let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    let claimed: Vec<&String> = results
        .iter()
        .filter(|r| r.starts_with("claimed:"))
        .collect();
    let already: Vec<&String> = results
        .iter()
        .filter(|r| r.contains("already_claimed"))
        .collect();

    assert_eq!(
        claimed.len(),
        1,
        "exactly one thread should win the claim, got {:?}",
        results
    );
    assert_eq!(
        already.len(),
        N - 1,
        "every loser should see already_claimed, got {:?}",
        results
    );

    // And on disk: exactly one MCP claim file for this spec.
    let leases = list_leases(&project_root);
    let matching: Vec<_> = leases
        .iter()
        .filter(|l| l.scope.eq_ignore_ascii_case(SPEC) && l.mcp_claim)
        .collect();
    assert_eq!(matching.len(), 1, "exactly one lease file on disk");
}

/// Crash-mid-write recovery: a punt-ledger truncated mid-line must still
/// yield every valid record before the corruption, and the file must
/// remain appendable after recovery. trace:STORY-361 | ai:claude
#[test]
fn ledger_with_partial_trailing_line_reads_valid_records() {
    let dir = tempdir().unwrap();
    // Two valid records, then a torn third line.
    let valid_a = PuntRecord {
        timestamp: Utc::now(),
        spec: "STORY-A".to_string(),
        category: PuntCategory::Other,
        detail: "a".to_string(),
        lean: None,
        raised_by: None,
        resolution_path: "punted".to_string(),
        classification: None,
        escalation_reason: None,
        answer: None,
        answered_by: None,
        decision: None,
        principle_link: None,
        calibration_pair: None,
        paused_at: None,
        resolved_at: None,
    };
    let valid_b = PuntRecord {
        spec: "STORY-B".to_string(),
        detail: "b".to_string(),
        ..valid_a.clone()
    };
    append_to_ledger(dir.path(), &valid_a).unwrap();
    append_to_ledger(dir.path(), &valid_b).unwrap();
    // Now simulate a crash mid-write by appending half a JSON line.
    let path = ledger_path(dir.path());
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(b"{\"timestamp\":\"2026-05-21T10:00:00")
        .unwrap();
    f.sync_all().unwrap();
    drop(f);

    // read_ledger must skip the torn line and return the two valid records.
    let records = read_ledger(dir.path());
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].spec, "STORY-A");
    assert_eq!(records[1].spec, "STORY-B");

    // After recovery the file is still appendable.
    let valid_c = PuntRecord {
        spec: "STORY-C".to_string(),
        detail: "c".to_string(),
        ..valid_a.clone()
    };
    // Pad the broken half-line with a newline to keep the JSONL invariant,
    // then append — the read_ledger skip means a half-line *between*
    // records is tolerated, but appending after a torn line without a
    // newline would glue STORY-C onto the broken line. write_atomic of a
    // recovery newline is the production recovery path; here we model it
    // inline.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(b"\n").unwrap();
    drop(f);
    append_to_ledger(dir.path(), &valid_c).unwrap();
    let records2 = read_ledger(dir.path());
    assert!(records2.iter().any(|r| r.spec == "STORY-C"));
}

/// Crash-mid-write recovery for a TOML lease: a write_atomic interrupted
/// before rename leaves the original file intact. trace:STORY-361 | ai:claude
#[test]
fn write_atomic_leaves_original_intact_on_failed_rename() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("lease.toml");
    std::fs::write(&target, b"original").unwrap();

    // First successful atomic write
    write_atomic(&target, b"updated").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"updated");

    // Simulate a torn write: an orphan .tmp file beside target should NOT
    // affect future reads of target.
    let orphan = dir.path().join(".lease.toml.tmp-orphan");
    std::fs::write(&orphan, b"junk").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"updated");

    // A subsequent atomic write still works (uses a fresh tmp name).
    write_atomic(&target, b"updated2").unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"updated2");
}

/// BUG-310: an MCP `add_requirement` against a Storage pointing at the
/// git-canonical store directory must be visible to a fresh GitBackend
/// load — that is, to the next CLI invocation. The pre-fix MCP startup
/// wrote to a private YAML snapshot, so this roundtrip silently dropped
/// every MCP write.
/// trace:BUG-310 | ai:claude
#[test]
fn mcp_add_requirement_is_visible_to_cli_via_git_backend() {
    use aida_core::db::{DatabaseBackend, GitBackend};

    let dir = tempdir().unwrap();
    let store_dir = dir.path().join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();

    // Seed an empty git-canonical store so subsequent loads succeed.
    let seed = crate::test_env::home_fixture_backend(&store_dir).unwrap();
    seed.save(&aida_core::RequirementsStore::new()).unwrap();

    // Point the MCP server at the store directory — the same shape the
    // fixed `aida mcp-serve` startup now uses.
    let storage = Box::leak(Box::new(Storage::new(&store_dir)));
    let server = McpServer::new(storage, dir.path().to_path_buf());

    let args = serde_json::json!({
        "title": "Roundtrip via MCP",
        "description": "MCP add must reach the canonical git store",
        "type": "task",
    });
    let result = server.tool_add_requirement(&args).expect("MCP add failed");
    assert!(
        result.contains("Requirement added"),
        "unexpected MCP add response: {result}"
    );

    // A fresh GitBackend simulates the next `aida` CLI invocation. It
    // must see the requirement the MCP server just wrote — that is the
    // contract BUG-310 broke.
    let cli_view = crate::test_env::home_fixture_backend(&store_dir).unwrap();
    let loaded = cli_view.load().unwrap();
    assert!(
        loaded
            .requirements
            .iter()
            .any(|r| r.title == "Roundtrip via MCP"),
        "CLI-equivalent GitBackend did not see the MCP-written requirement"
    );
}

// ========================================================================
// STORY-474: tool profiles + safe-default surface
// ========================================================================

/// Every name in `tool_descriptors()` must be classified explicitly — no
/// tool may fall through to the `Admin` catch-all in `tool_min_profile`. A
/// new tool added without a classification line is a bug (it would silently
/// be admin-only). trace:STORY-474 | ai:claude
#[test]
fn every_tool_is_explicitly_classified() {
    let names: Vec<String> = tool_descriptors()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    // The catch-all returns Admin; an explicit `add_requirement`-style
    // operator/coordination/read classification returns something <= Operator.
    // No *real* tool should be Admin today.
    let unclassified: Vec<&String> = names
        .iter()
        .filter(|n| tool_min_profile(n) == McpProfile::Admin)
        .collect();
    assert!(
        unclassified.is_empty(),
        "tools missing a profile classification (would be admin-only): {:?}",
        unclassified
    );
}

#[test]
fn read_only_profile_excludes_all_write_tools() {
    let names = tool_names_for_profile(McpProfile::ReadOnly);
    // Spot-check the writes are gone.
    for write in [
        "add_requirement",
        "update_requirement",
        "add_comment",
        "add_relationship",
        "post_punt",
        "resolve_punt",
        "escalate_punt",
        "file_finding",
        "triage_finding",
        "claim_task",
        "release_task",
        "post_directive",
        "ack_directive",
        "ack_brief",
        "send_message",
    ] {
        assert!(
            !names.contains(&write.to_string()),
            "read-only profile leaked write tool {write}"
        );
    }
    // Reads are present.
    for read in [
        "list_requirements",
        "show_requirement",
        "search_requirements",
        "query_graph",
        "history",
        "list_punts",
        "read_brief",
    ] {
        assert!(
            names.contains(&read.to_string()),
            "read-only profile dropped read tool {read}"
        );
    }
}

/// Each higher tier is a strict superset of the one below it.
#[test]
fn profiles_are_monotonic_supersets() {
    let ro = tool_names_for_profile(McpProfile::ReadOnly);
    let coord = tool_names_for_profile(McpProfile::Coordination);
    let op = tool_names_for_profile(McpProfile::Operator);
    let full = tool_names_for_profile(McpProfile::Full);
    let admin = tool_names_for_profile(McpProfile::Admin);

    for n in &ro {
        assert!(coord.contains(n), "coordination dropped {n}");
    }
    for n in &coord {
        assert!(op.contains(n), "operator dropped {n}");
    }
    for n in &op {
        assert!(full.contains(n), "full dropped {n}");
    }
    // admin == full today.
    assert_eq!(admin.len(), full.len());
    // full exposes every descriptor.
    assert_eq!(full.len(), tool_descriptors().as_array().unwrap().len());
    // strict growth across tiers.
    assert!(ro.len() < coord.len());
    assert!(coord.len() < op.len());
    assert!(op.len() <= full.len());
}

#[test]
fn profile_token_parsing_round_trips_and_aliases() {
    assert_eq!(
        McpProfile::from_token("read-only"),
        Some(McpProfile::ReadOnly)
    );
    assert_eq!(
        McpProfile::from_token("readonly"),
        Some(McpProfile::ReadOnly)
    );
    assert_eq!(McpProfile::from_token("  RO "), Some(McpProfile::ReadOnly));
    assert_eq!(
        McpProfile::from_token("read_only"),
        Some(McpProfile::ReadOnly)
    );
    assert_eq!(
        McpProfile::from_token("Coordination"),
        Some(McpProfile::Coordination)
    );
    assert_eq!(
        McpProfile::from_token("operator"),
        Some(McpProfile::Operator)
    );
    assert_eq!(McpProfile::from_token("admin"), Some(McpProfile::Admin));
    assert_eq!(McpProfile::from_token("full"), Some(McpProfile::Full));
    assert_eq!(McpProfile::from_token("all"), Some(McpProfile::Full));
    assert_eq!(McpProfile::from_token("bogus"), None);
    // canonical token round-trips
    for p in [
        McpProfile::ReadOnly,
        McpProfile::Coordination,
        McpProfile::Operator,
        McpProfile::Admin,
        McpProfile::Full,
    ] {
        assert_eq!(McpProfile::from_token(p.as_str()), Some(p));
    }
}

#[test]
fn default_profile_is_full_for_backwards_compat() {
    assert_eq!(McpProfile::default(), McpProfile::Full);
}

/// `tools/list` advertises only the in-profile tools and tags each with its
/// minimum tier.
#[test]
fn tools_list_respects_profile() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

    let resp = server.handle_tools_list(&json!(1));
    let result = resp.result.expect("tools/list returns a result");
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"list_requirements"));
    assert!(!names.contains(&"add_requirement"));
    // descriptors carry the profile tier metadata.
    let lr = tools
        .iter()
        .find(|t| t["name"] == json!("list_requirements"))
        .unwrap();
    assert_eq!(lr["profile"], json!("read-only"));
}

/// `tools/call` rejects an out-of-profile (but known) tool with a permission
/// error, even when the client calls it directly. trace:STORY-474 | ai:claude
#[test]
fn tools_call_rejects_out_of_profile_tool() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

    let resp = server.handle_tools_call(
        &json!(1),
        &json!({ "name": "add_requirement", "arguments": { "title": "X" } }),
    );
    let result = resp.result.expect("tools/call returns a result");
    assert_eq!(result["isError"], json!(true));
    let code = result["structuredError"]["code"].as_str().unwrap();
    assert_eq!(code, "permission_denied");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("operator"),
        "should name the required tier: {text}"
    );
    assert!(
        text.contains("read-only"),
        "should name the active tier: {text}"
    );
}

/// A read tool still works under read-only (smoke test the gate doesn't
/// over-block).
#[test]
fn tools_call_allows_in_profile_read() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile(storage, dir.path().to_path_buf(), McpProfile::ReadOnly);

    let resp = server.handle_tools_call(&json!(1), &json!({ "name": "list_features" }));
    let result = resp.result.expect("tools/call returns a result");
    // list_features returns Ok (not a profile rejection).
    assert!(
        result["isError"] != json!(true),
        "in-profile read was rejected: {result}"
    );
}

#[test]
fn guest_persona_lists_and_calls_only_reads() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile_and_role(
        storage,
        dir.path().to_path_buf(),
        McpProfile::Full,
        "guest",
    );

    let listed = server.handle_tools_list(&json!(1)).result.unwrap();
    let names: Vec<&str> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"list_requirements"));
    assert!(names.contains(&"show_requirement"));
    for write in [
        "add_requirement",
        "update_requirement",
        "queue_add",
        "db_sync",
        "session_start",
        "role_enter",
    ] {
        assert!(!names.contains(&write), "guest advertised {write}");
        let result = server
            .handle_tools_call(&json!(2), &json!({"name": write, "arguments": {}}))
            .result
            .unwrap();
        assert_eq!(result["structuredError"]["code"], "permission_denied");
        assert!(result["structuredError"]["message"]
            .as_str()
            .unwrap()
            .starts_with("The 'guest' role is a least-privilege stakeholder role; refusing"));
    }
    let read = server
        .handle_tools_call(&json!(3), &json!({"name": "list_requirements"}))
        .result
        .unwrap();
    assert_ne!(read["isError"], json!(true));
}

#[test]
fn requester_persona_allows_only_constrained_intake_write() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile_and_role(
        storage,
        dir.path().to_path_buf(),
        McpProfile::Full,
        "requester",
    );

    let response = server
        .handle_tools_call(
            &json!(1),
            &json!({
                "name": "add_requirement",
                "arguments": {
                    "title": "Requester report",
                    "description": "Reproduction details",
                    "type": "bug",
                    "status": "completed",
                    "tags": ["external"]
                }
            }),
        )
        .result
        .unwrap();
    assert_ne!(response["isError"], json!(true), "{response}");
    let store = server.storage.load().unwrap();
    let requirement = store.requirements.last().unwrap();
    assert_eq!(requirement.status, RequirementStatus::Draft);
    assert!(requirement.tags.contains("external"));
    assert!(requirement.tags.contains("intake:requester"));

    for write in ["update_requirement", "queue_add", "add_relationship"] {
        let result = server
            .handle_tools_call(&json!(2), &json!({"name": write, "arguments": {}}))
            .result
            .unwrap();
        assert_eq!(result["structuredError"]["code"], "permission_denied");
    }
}

#[test]
fn requester_persona_rejects_non_intake_requirement_types() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile_and_role(
        storage,
        dir.path().to_path_buf(),
        McpProfile::Full,
        "requester",
    );
    let result = server
        .handle_tools_call(
            &json!(1),
            &json!({
                "name": "add_requirement",
                "arguments": {"title": "No", "description": "No", "type": "story"}
            }),
        )
        .result
        .unwrap();
    assert_eq!(result["structuredError"]["code"], "permission_denied");
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("adding anything except change-request, bug, or user specs"));
    assert!(server.storage.load().unwrap().requirements.is_empty());
}

#[test]
fn requester_persona_rejects_relationship_and_grooming_fields_on_intake() {
    let dir = tempdir().unwrap();
    let cache_path = dir.path().join(".aida").join("c.yaml");
    std::fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(cache_path)));
    let server = McpServer::with_profile_and_role(
        storage,
        dir.path().to_path_buf(),
        McpProfile::Full,
        "requester",
    );

    for field in crate::REQUESTER_INTAKE_FORBIDDEN_FIELDS {
        let result = server
            .handle_tools_call(
                &json!(1),
                &json!({
                    "name": "add_requirement",
                    "arguments": {
                        "title": "Requester report",
                        "description": "Reproduction details",
                        (*field): "STORY-1"
                    }
                }),
            )
            .result
            .unwrap();
        assert_eq!(result["structuredError"]["code"], "permission_denied");
        assert!(result["structuredError"]["message"]
            .as_str()
            .unwrap()
            .contains("setting parent, feature, or owner during intake"));
    }
    assert!(server.storage.load().unwrap().requirements.is_empty());
}

#[test]
fn stakeholder_mcp_policy_covers_every_descriptor_via_shared_actions() {
    for tool in tool_descriptors().as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let action = stakeholder_mcp_action(name);
        assert_eq!(
            stakeholder_tool_allowed("guest", name),
            crate::stakeholder_action_allowed("guest", action)
        );
        assert_eq!(
            stakeholder_tool_allowed("requester", name),
            crate::stakeholder_action_allowed("requester", action)
        );
        assert_eq!(
            stakeholder_tool_allowed("requester", name),
            crate::stakeholder_mcp_read_allowed(name) || name == "add_requirement"
        );
    }

    for read in [
        "list_requirements",
        "show_requirement",
        "search_requirements",
        "query_graph",
        "list_features",
        "history",
        "read_inbox",
        "list_punts",
        "read_punt",
        "list_findings",
        "list_active_leases",
        "list_directives",
        "list_briefs",
        "read_brief",
        "queue_list",
        "queue_next",
        "queue_progress",
        "cache_status",
        "schema",
        "status_unified",
        "usage_query",
        "plan_verify",
        "plan_helpers",
        "ultraplan_assemble",
        "goal_derive",
    ] {
        assert!(
            crate::stakeholder_mcp_read_allowed(read),
            "missing read: {read}"
        );
    }
    for write in [
        "db_sync",
        "fetch",
        "pull",
        "queue_work",
        "session_start",
        "session_end",
        "session_manifest",
        "role_enter",
        "role_end",
    ] {
        assert!(
            !crate::stakeholder_mcp_read_allowed(write),
            "unsafe read: {write}"
        );
    }
}

#[test]
fn stakeholder_cli_and_mcp_policy_have_operation_parity() {
    // trace:TASK-1264 | ai:codex
    let cases: &[(&[&str], Option<&str>)] = &[
        (&["aida", "db", "sync"], Some("db_sync")),
        (
            &["aida", "session", "start", "--owns", "X"],
            Some("session_start"),
        ),
        (&["aida", "role", "enter", "advisor"], Some("role_enter")),
        (&["aida", "queue", "add", "X"], Some("queue_add")),
        (
            &["aida", "edit", "X", "--title", "y"],
            Some("update_requirement"),
        ),
        (&["aida", "list"], Some("list_requirements")),
        (&["aida", "mcp-serve"], None),
    ];

    for role in ["guest", "requester"] {
        for (argv, mcp_tool) in cases {
            let mut cli = crate::command_registry::try_parse_from(*argv)
                .unwrap_or_else(|err| panic!("invalid parity argv {argv:?}: {err}"));
            let cli_action = crate::stakeholder_cli_action(&cli.command);
            let cli_result =
                crate::enforce_stakeholder_role_capabilities_for_role(&mut cli.command, role);
            let cli_allowed = cli_result.is_ok();
            let mcp_allowed = mcp_tool
                .map(|tool| stakeholder_tool_allowed(role, tool))
                .unwrap_or(true);

            assert_eq!(
                cli_allowed, mcp_allowed,
                "allow/refuse mismatch for {role}, CLI {argv:?}, MCP {mcp_tool:?}"
            );

            if let (Err(cli_error), Some(tool)) = (cli_result, mcp_tool) {
                let mcp_error = crate::stakeholder_refusal_message(
                    role,
                    crate::stakeholder_action_label(stakeholder_mcp_action(tool)),
                );
                assert_eq!(
                    cli_error.to_string(),
                    mcp_error,
                    "refusal text mismatch for {role}, CLI {argv:?}, MCP {tool}"
                );
                assert_eq!(
                    cli_action,
                    stakeholder_mcp_action(tool),
                    "shared operation mismatch for CLI {argv:?}, MCP {tool}"
                );
            }
        }
    }
}

/// Resolution order for the non-env layers (override > config > default).
/// The `AIDA_MCP_PROFILE` env layer is intentionally NOT exercised here:
/// mutating a process-global env var races with the many parallel tests that
/// build a server via `McpServer::new` (which reads it). The env layer is a
/// thin `std::env::var` read above the config layer in `resolve_mcp_profile`.
#[test]
fn resolve_profile_prefers_override_then_config_then_default() {
    // Guard: a leaked AIDA_MCP_PROFILE from the ambient shell would change
    // the default-layer expectation; skip that one assertion if so.
    let env_clean = std::env::var("AIDA_MCP_PROFILE").is_err();

    let dir = tempdir().unwrap();
    let aida = dir.path().join(".aida");
    std::fs::create_dir_all(&aida).unwrap();

    // No override, no env, no config -> default (full).
    if env_clean {
        assert_eq!(resolve_mcp_profile(dir.path(), None), McpProfile::Full);
    }

    // Config sets coordination (override and env both absent/clean).
    std::fs::write(
        aida.join("config.toml"),
        "[mcp]\nprofile = \"coordination\"\n",
    )
    .unwrap();
    if env_clean {
        assert_eq!(
            resolve_mcp_profile(dir.path(), None),
            McpProfile::Coordination
        );
    }

    // Explicit override beats config (and env) regardless of ambient env.
    assert_eq!(
        resolve_mcp_profile(dir.path(), Some("read-only")),
        McpProfile::ReadOnly
    );

    // An unknown override token is ignored (falls through to config).
    if env_clean {
        assert_eq!(
            resolve_mcp_profile(dir.path(), Some("nonsense")),
            McpProfile::Coordination
        );
    }
}

// =====================================================================
// STORY-532 (EPIC-27): queue MCP tools
// =====================================================================

/// A git-canonical-backed server — the queue layer (`Storage::queue_*`)
/// only works against a SQLite or directory backend, so the queue tests
/// need this rather than the plain-YAML `mk_server`. trace:EPIC-27
fn mk_git_server(dir: &Path) -> McpServer<'static> {
    use aida_core::db::{DatabaseBackend, GitBackend};
    let store_dir = dir.join(".aida-store");
    std::fs::create_dir_all(&store_dir).unwrap();
    let seed = crate::test_env::home_fixture_backend(&store_dir).unwrap();
    seed.save(&aida_core::RequirementsStore::new()).unwrap();
    let storage = Box::leak(Box::new(Storage::new(&store_dir)));
    McpServer::new(storage, dir.to_path_buf())
}

/// Seed a requirement via the MCP add tool (works against the git store)
/// and return its SPEC-ID. trace:EPIC-27
fn seed_req(server: &McpServer<'static>, title: &str) -> String {
    let resp = server
        .tool_add_requirement(&json!({
            "title": title,
            "description": "queue-test seed",
            "type": "task",
        }))
        .expect("seed add_requirement failed");
    added_spec_id(&resp).to_string()
}

// Queue tests pass `user` explicitly (rather than relying on AIDA_USER /
// USER env) so they stay deterministic under cargo's parallel test
// runner — env mutation would race across threads. trace:EPIC-27
const QU: &str = "queue-test-user";

#[test]
fn queue_add_list_remove_roundtrip() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    let spec = seed_req(&server, "Queue roundtrip");

    // Empty to start.
    let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
    assert!(listed.contains("Queue is empty"), "listed: {listed}");

    // Add.
    let added = server
        .tool_queue_add_inner(&json!({ "id": spec, "for": "implementer", "user": QU }))
        .unwrap();
    assert!(added.contains(&spec), "added: {added}");
    assert!(added.contains("for:implementer"), "added: {added}");

    // List shows it.
    let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
    assert!(listed.contains(&spec), "listed: {listed}");
    assert!(listed.contains("Queue roundtrip"), "listed: {listed}");

    // Remove.
    let removed = server
        .tool_queue_remove(&json!({ "id": spec, "user": QU }))
        .unwrap();
    assert!(removed.contains(&spec), "removed: {removed}");

    let listed = server.tool_queue_list(&json!({ "user": QU })).unwrap();
    assert!(listed.contains("Queue is empty"), "listed: {listed}");
}

#[test]
fn queue_add_refuses_terminal_without_force() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let spec = seed_req(&server, "Terminal spec");
    force_status(&server, &spec, RequirementStatus::Completed);

    let err = server
        .tool_queue_add_inner(&json!({ "id": spec }))
        .expect_err("should refuse terminal without force");
    assert!(err.contains("re-queueing closed work"), "err: {err}");

    // force overrides.
    let ok = server
        .tool_queue_add_inner(&json!({ "id": spec, "force": true }))
        .expect("force should allow queueing terminal");
    assert!(ok.contains(&spec), "ok: {ok}");
}

#[test]
fn queue_next_and_work_peek_the_head() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let u = "queue-peek-user";

    // Empty: both report nothing.
    let next = server.tool_queue_next(&json!({ "user": u })).unwrap();
    assert!(next.contains("Queue is empty"), "next: {next}");

    let a = seed_req(&server, "First");
    let b = seed_req(&server, "Second");
    server
        .tool_queue_add_inner(&json!({ "id": a, "for": "any", "user": u }))
        .unwrap();
    server
        .tool_queue_add_inner(&json!({ "id": b, "for": "any", "user": u }))
        .unwrap();

    // `all: true` bypasses any ambient active-role default filter so the
    // unrouted (`for: any`) entries are visible regardless of the test
    // shell's AIDA_SESSION_ROLE. trace:EPIC-27
    let next = server
        .tool_queue_next(&json!({ "user": u, "all": true }))
        .unwrap();
    assert!(next.contains(&a), "head should be first-added: {next}");

    // queue_work with no id mirrors the head peek.
    let work = server
        .tool_queue_work(&json!({ "user": u, "all": true }))
        .unwrap();
    assert!(work.contains(&a), "work peek: {work}");
    assert!(work.contains("aida queue work"), "work hint: {work}");

    // queue_work with an explicit queued id resolves that spec.
    let work_b = server
        .tool_queue_work(&json!({ "id": &b, "user": u }))
        .unwrap();
    assert!(work_b.contains(&b), "work_b: {work_b}");
}

#[test]
fn queue_done_flips_to_done_and_dequeues() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let u = "queue-done-user";

    let spec = seed_req(&server, "To finish");
    server
        .tool_queue_add_inner(&json!({ "id": &spec, "user": u }))
        .unwrap();
    // BUG-1611: `queue_done` finishes approved work; the seed lands Draft,
    // so move it into flight the way the advisor + `queue work` would.
    // trace:BUG-1611 | ai:claude
    force_status(&server, &spec, RequirementStatus::InProgress);

    let done = server
        .tool_queue_done(&json!({ "id": &spec, "user": u }))
        .unwrap();
    assert!(done.contains("marked done"), "done: {done}");

    // Status is Done, queue is empty.
    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&spec).unwrap();
    assert_eq!(req.status, RequirementStatus::Done);
    assert!(req
        .implementation_info
        .as_ref()
        .map(|i| i.implemented)
        .unwrap_or(false));

    let listed = server.tool_queue_list(&json!({ "user": u })).unwrap();
    assert!(listed.contains("Queue is empty"), "listed: {listed}");
}

/// BUG-1611: MCP `queue_done` runs the same lifecycle guard as the CLI. A
/// closed spec is refused whatever the server's role (so this check does
/// not depend on the test shell's AIDA_SESSION_ROLE), with no write and
/// neutral guidance; an In Progress spec passes (covered above).
// trace:BUG-1611 | ai:claude
#[test]
fn queue_done_refuses_a_closed_spec_without_writing() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let u = "queue-done-closed-user";
    let spec = seed_req(&server, "Declined");
    force_status(&server, &spec, RequirementStatus::Rejected);

    let err = server
        .tool_queue_done(&json!({ "id": &spec, "user": u }))
        .expect_err("queue_done on a Rejected spec must refuse");
    assert!(err.contains("queue done refused"), "err: {err}");
    assert!(!err.contains("AIDA_SESSION_ROLE"), "err: {err}");

    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&spec).unwrap();
    assert_eq!(req.status, RequirementStatus::Rejected);
    assert!(req.implementation_info.is_none());

    // The authority half, through the shared decision the tool calls.
    assert!(crate::queue_done_lifecycle_refusal(&RequirementStatus::Draft, false).is_some());
    assert!(crate::queue_done_lifecycle_refusal(&RequirementStatus::Approved, false).is_none());
}

#[test]
fn queue_rework_flips_status_and_requeues() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let u = "queue-rework-user";

    let spec = seed_req(&server, "Reworkable");
    // Done → rework smart-targets InProgress.
    force_status(&server, &spec, RequirementStatus::Done);

    let resp = server
            .tool_queue_rework_inner(&json!({ "id": &spec, "for": "implementer", "reason": "PR review found issues", "user": u }))
            .unwrap();
    // BUG-1470: metadata-only rework must leave the spec CLAIMABLE. In
    // Progress is a claim, and nothing here establishes the lease that
    // would back it, so the MCP path targets Approved instead.
    assert!(resp.contains("Done → Approved"), "resp: {resp}");
    assert!(resp.contains("Queued"), "resp: {resp}");

    let store = server.storage.load().unwrap();
    let req = store.get_requirement_by_spec_id(&spec).unwrap();
    assert_eq!(req.status, RequirementStatus::Approved);
    // Reason captured as a comment.
    assert!(req.comments.iter().any(|c| c.content.contains("PR review")));

    // It's back in the queue.
    let listed = server.tool_queue_list(&json!({ "user": u })).unwrap();
    assert!(listed.contains(&spec), "listed: {listed}");
}

#[test]
fn queue_rework_refuses_terminal_without_force() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let spec = seed_req(&server, "Closed spec");
    force_status(&server, &spec, RequirementStatus::Rejected);

    let err = server
        .tool_queue_rework_inner(&json!({ "id": &spec }))
        .expect_err("should refuse terminal rework without force");
    assert!(err.contains("re-opening closed work"), "err: {err}");
}

/// BUG-480: the public MCP `queue_add` / `queue_rework` tools had NO
/// advisor-authority check — an MCP agent could push a spec straight into
/// the execution pipeline, the exact act the CLI `aida queue add` gates on.
/// Both public tools now refuse unconditionally (MCP is never advisor
/// authority); the queue mechanics still work via the `_inner` methods
/// (used by the advisor-corroborated CLI and by the mechanics tests above).
// trace:BUG-480 | ai:claude
#[test]
fn mcp_queue_add_and_rework_are_advisor_gated() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let spec = seed_req(&server, "Authority-gated queue target");

    // queue_add via the public MCP tool is refused.
    let err = server
        .tool_queue_add(&json!({ "id": &spec, "for": "implementer" }))
        .expect_err("queue_add via MCP must be advisor-gated");
    assert!(err.contains("advisor"), "queue_add err: {err}");

    // queue_rework via the public MCP tool is refused too.
    let err = server
        .tool_queue_rework(&json!({ "id": &spec, "reason": "self-requeue attempt" }))
        .expect_err("queue_rework via MCP must be advisor-gated");
    assert!(err.contains("advisor"), "queue_rework err: {err}");

    // Nothing slipped the gate: the spec is not in any queue and its status
    // is unchanged (the public tools mutated nothing).
    let store = server.storage.load().unwrap();
    assert_eq!(
        store.get_requirement_by_spec_id(&spec).unwrap().status,
        RequirementStatus::Draft
    );
}

/// BUG-486 (THE FIX): the MCP authority gate was role-blind — it refused the
/// advisor-gated status transitions (Draft → Approved/Planned, the
/// Draft/NeedsAttention → InProgress/Done bypass) and the queue_add/rework
/// cases UNCONDITIONALLY, ignoring the caller's role even when the MCP session
/// was a genuine advisor. The gate now routes its authority decision through
/// the SAME predicate the CLI uses (`status_advance_requires_advisor_authority`
/// + `advisor_authority_from`): an advisor caller is permitted, a non-advisor
/// caller is refused.
///
/// This test exercises the pure cores (`mcp_status_gate_message_for` /
/// `mcp_queue_authority_message_for`) so the authority axis is injected, not
/// read from the process-global `AIDA_SESSION_ROLE` env. Against the
/// pre-BUG-486 gate (which took no authority param and refused regardless),
/// the `caller_is_advisor = true` assertions below are red; with the fix they
/// pass.
// trace:BUG-486 | ai:claude
#[test]
fn mcp_authority_gate_honors_advisor_role() {
    use RequirementStatus::*;
    // BUG-1618: the env-resolving wrapper assertions below need a session with
    // no advisor role entered; a leased agent session inherits one
    // (AIDA_SESSION_ROLE=advisor). Pin a hermetic, role-less ambient context.
    let dir = tempdir().unwrap();
    let _ambient = crate::test_env::AmbientGuard::hermetic(dir.path(), None); // trace:BUG-1618 | ai:claude

    // ---- Status gate: advisor-gated transitions ----
    // The advisor-authority transitions the bug names.
    let advisor_gated = [
        (Draft, Approved),
        (Draft, Planned),
        (NeedsAttention, Approved),
        (NeedsAttention, Planned),
        (Draft, InProgress),
        (Draft, Done),
        (NeedsAttention, InProgress),
        (NeedsAttention, Done),
    ];
    for (from, to) in advisor_gated {
        // Non-advisor: refused with the existing guidance message.
        let refused = mcp_status_gate_message_for(&from, &to, false)
            .unwrap_or_else(|| panic!("{from} → {to} must be refused for a non-advisor"));
        assert!(
            refused.contains("advisor"),
            "{from} → {to} refusal should mention advisor authority: {refused}"
        );
        // Advisor (role entered): PERMITTED, exactly as the CLI under
        // AIDA_SESSION_ROLE=advisor. THIS is the BUG-486 regression: the old
        // gate refused here too.
        assert_eq!(
            mcp_status_gate_message_for(&from, &to, true),
            None,
            "{from} → {to} must be PERMITTED for an advisor (BUG-486)"
        );
    }

    // Completed stays merge-driven: refused regardless of role (advisor
    // authority does not unlock it).
    for advisor in [false, true] {
        let msg = mcp_status_gate_message_for(&Draft, &Completed, advisor)
            .expect("Completed via MCP is always refused (merge-driven)");
        assert!(
            msg.contains("merge") || msg.contains("(SPEC-ID)"),
            "Completed refusal should explain merge-driven: {msg}"
        );
    }

    // In-pipeline execution flips are implementer-legitimate for everyone —
    // never gated, with or without advisor authority.
    for (from, to) in [(Approved, InProgress), (InProgress, Done)] {
        for advisor in [false, true] {
            assert_eq!(
                mcp_status_gate_message_for(&from, &to, advisor),
                None,
                "{from} → {to} is an implementer-legitimate flip (advisor={advisor})"
            );
        }
    }

    // ---- Queue authority gate (queue_add / queue_rework) ----
    // Non-dispatch caller refused; dispatch-capable caller permitted.
    let refused = mcp_queue_authority_message_for(false)
        .expect("queue_add/rework via MCP must be refused without dispatch authority");
    assert!(
        refused.contains("dispatch authority"),
        "queue refusal: {refused}"
    );
    assert_eq!(
        mcp_queue_authority_message_for(true),
        None,
        "queue_add/rework must be PERMITTED for an advisor (BUG-486)"
    );

    // ---- The env-resolving wrapper agrees with the pure core ----
    // mcp_caller_has_advisor_authority() reads AIDA_SESSION_ROLE; in the test
    // process no advisor role is entered, so it resolves non-advisor and the
    // public wrappers refuse — matching the headless / non-TTY default
    // (acceptance #5).
    assert!(
        !mcp_caller_has_advisor_authority(),
        "test process has no advisor role entered"
    );
    assert!(
        mcp_status_gate_message(&Draft, &Approved).is_some(),
        "public status gate refuses without an advisor role"
    );
    assert!(
        mcp_queue_authority_message().is_some(),
        "public queue gate refuses without an advisor role"
    );
}

#[test]
fn mcp_authority_refusal_names_roles_and_relaunch_command() {
    use RequirementStatus::*;

    // ADR-66: the server's seat comes from its validated startup grant;
    // the caller's env role is only a hint, and the recovery path is TTY
    // issuance + restart — never an env-var relaunch.
    // trace:STORY-1473 | ai:claude
    let line = mcp_authority_line_for(Some("implementer"), Some("advisor"));
    assert!(line.contains("server seat=implementer"), "{line}");
    assert!(line.contains("caller shell hint=advisor"), "{line}");
    assert!(line.contains("advisor authority: no"), "{line}");
    assert!(
        line.contains("enter advisor at a TTY, then restart the MCP server"),
        "{line}"
    );
    assert!(!line.contains("AIDA_SESSION_ROLE"), "{line}");

    let msg = mcp_status_gate_message_for(&Draft, &Approved, false)
        .expect("non-advisor MCP server must refuse advisor-gated transition");
    assert!(msg.contains("MCP authority:"), "{msg}");
    assert!(msg.contains("server seat="), "{msg}");
    assert!(msg.contains("caller shell hint="), "{msg}");
    assert!(
        msg.contains("Enter advisor at a TTY, restart the MCP server"),
        "{msg}"
    );
    assert!(!msg.contains("AIDA_SESSION_ROLE"), "{msg}");
}

#[test]
fn queue_move_reorders() {
    // STORY-1429: these tools read AIDA_SESSION_ROLE; hold the test env
    // lock (pinned unset) so a sibling test that sets it cannot race.
    // trace:STORY-1429 | ai:claude
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_SESSION_ROLE", None)]);
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let u = "queue-move-user";

    let a = seed_req(&server, "Alpha");
    let b = seed_req(&server, "Bravo");
    let c = seed_req(&server, "Charlie");
    for id in [&a, &b, &c] {
        server
            .tool_queue_add_inner(&json!({ "id": id, "for": "any", "user": u }))
            .unwrap();
    }

    // Move C to the top.
    let moved = server
        .tool_queue_move(&json!({ "id": &c, "top": true, "user": u }))
        .unwrap();
    assert!(moved.contains(&c), "moved: {moved}");

    let next = server
        .tool_queue_next(&json!({ "user": u, "all": true }))
        .unwrap();
    assert!(next.contains(&c), "C should now be the head: {next}");

    // Moving a spec not in the queue errors.
    let stray = seed_req(&server, "Stray");
    let err = server
        .tool_queue_move(&json!({ "id": &stray, "top": true, "user": u }))
        .expect_err("non-queued move should error");
    assert!(err.contains("not in the queue"), "err: {err}");

    // No destination errors.
    let err = server
        .tool_queue_move(&json!({ "id": &a, "user": u }))
        .expect_err("no destination should error");
    assert!(err.contains("specify a destination"), "err: {err}");
}

#[test]
fn queue_progress_buckets_by_status() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    let done = seed_req(&server, "Shipped one");
    let inprog = seed_req(&server, "Working one");
    let todo = seed_req(&server, "Remaining one");
    force_status(&server, &done, RequirementStatus::Completed);
    force_status(&server, &inprog, RequirementStatus::InProgress);
    // todo stays Draft.

    let resp = server
        .tool_queue_progress(&json!({ "specs": [done, inprog, todo] }))
        .unwrap();
    assert!(resp.contains("Progress (3 items)"), "resp: {resp}");
    assert!(resp.contains("Shipped: 1"), "resp: {resp}");
    assert!(resp.contains("Working now: 1"), "resp: {resp}");
    assert!(resp.contains("Remaining: 1"), "resp: {resp}");
}

#[test]
fn queue_progress_needs_a_source() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let err = server
        .tool_queue_progress(&json!({}))
        .expect_err("progress with no source should error");
    assert!(err.contains("batch"), "err: {err}");
}

#[test]
fn queue_tools_are_in_descriptors_and_classified() {
    let names: Vec<String> = tool_descriptors()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    for t in [
        "queue_list",
        "queue_add",
        "queue_work",
        "queue_done",
        "queue_next",
        "queue_progress",
        "queue_rework",
        "queue_move",
        "queue_remove",
    ] {
        assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
        // No queue tool may fall through to the Admin catch-all.
        assert_ne!(
            tool_min_profile(t),
            McpProfile::Admin,
            "{t} is unclassified"
        );
    }
    // Read vs write tier sanity.
    assert_eq!(tool_min_profile("queue_list"), McpProfile::ReadOnly);
    assert_eq!(tool_min_profile("queue_work"), McpProfile::ReadOnly);
    assert_eq!(tool_min_profile("queue_remove"), McpProfile::Coordination);
    assert_eq!(tool_min_profile("queue_add"), McpProfile::Operator);
}

#[test]
fn queue_user_id_honors_explicit_user_arg() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    // Explicit `user` arg wins over any ambient env (deterministic under
    // the parallel test runner). The env-fallback chain itself is covered
    // by `current_user_id_*` tests in main.rs. trace:EPIC-27
    assert_eq!(
        server.queue_user_id(&json!({ "user": "explicit" })),
        "explicit"
    );
    // With no arg, it never returns empty — resolves to *some* identity.
    assert!(!server.queue_user_id(&json!({})).is_empty());
}

// =====================================================================
// STORY-533 (EPIC-27): session MCP tools
// =====================================================================

/// Write a minimal lease TOML under `.aida/sessions/<id>.toml` matching
/// the `LightLease` shape the session tools read. trace:EPIC-27
fn seed_lease(dir: &Path, id: &str, scope: &str, role: &str) {
    let sessions = dir.join(".aida").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let body = format!(
        "id = \"{id}\"\n\
             scope = \"{scope}\"\n\
             slug = \"{scope}\"\n\
             owner = \"tester\"\n\
             worktree_path = \"/nonexistent/wt-{scope}\"\n\
             branch = \"{scope}-branch\"\n\
             started_at = \"2026-06-07T00:00:00Z\"\n\
             hostname = \"testhost\"\n\
             role = \"{role}\"\n\
             mcp_claim = false\n"
    );
    std::fs::write(sessions.join(format!("{id}.toml")), body).unwrap();
}

#[test]
fn session_leases_lists_seeded_leases() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    // Empty to start.
    let empty = server.tool_session_leases(&json!({})).unwrap();
    assert!(empty.contains("no active sessions"), "empty: {empty}");

    seed_lease(dir.path(), "aaaaaaaa1111", "EPIC-27", "implementer");
    let listed = server.tool_session_leases(&json!({})).unwrap();
    assert!(listed.contains("EPIC-27"), "listed: {listed}");
    assert!(listed.contains("role=implementer"), "listed: {listed}");
    assert!(listed.contains("aaaaaaaa"), "listed short id: {listed}");
}

#[test]
fn session_status_shows_a_single_lease_and_errors_on_ambiguity() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    seed_lease(dir.path(), "bbbbbbbb2222", "STORY-1", "implementer");

    // Single lease — id optional.
    let shown = server.tool_session_status(&json!({})).unwrap();
    assert!(shown.contains("STORY-1"), "shown: {shown}");
    assert!(shown.contains("branch: STORY-1-branch"), "shown: {shown}");
    assert!(shown.contains("role: implementer"), "shown: {shown}");

    // Add a second — now id is required.
    seed_lease(dir.path(), "cccccccc3333", "STORY-2", "reviewer");
    let err = server
        .tool_session_status(&json!({}))
        .expect_err("ambiguous without id");
    assert!(err.contains("multiple active sessions"), "err: {err}");

    // id prefix disambiguates.
    let shown2 = server
        .tool_session_status(&json!({ "id": "cccccccc" }))
        .unwrap();
    assert!(shown2.contains("STORY-2"), "shown2: {shown2}");
}

#[test]
fn session_manifest_reads_planned_cluster() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let lease_id = "dddddddd4444";
    seed_lease(dir.path(), lease_id, "EPIC-9", "implementer");

    // No manifest yet.
    let none = server.tool_session_manifest(&json!({})).unwrap();
    assert!(none.contains("no planned-cluster manifest"), "none: {none}");

    // Write a manifest with one completed + one pending item.
    let path = crate::session_manifest::manifest_path(dir.path(), lease_id);
    let manifest = crate::session_manifest::SessionManifest {
        session_id: lease_id.to_string(),
        planned_at: chrono::Utc::now(),
        plan_source: "test".to_string(),
        claude_session_id: None,
        batch_name: None,
        agent_name: None,
        plan: None,
        items: vec![
            crate::session_manifest::ManifestItem {
                spec_id: "TASK-100".to_string(),
                position: 0,
                status_at_plan: "Draft".to_string(),
                started_at: Some(chrono::Utc::now()),
                completed_at: Some(chrono::Utc::now()),
                note: None,
            },
            crate::session_manifest::ManifestItem {
                spec_id: "TASK-101".to_string(),
                position: 1,
                status_at_plan: "Draft".to_string(),
                started_at: None,
                completed_at: None,
                note: None,
            },
        ],
    };
    crate::session_manifest::save(&path, &manifest).unwrap();

    let shown = server.tool_session_manifest(&json!({})).unwrap();
    let check = crate::glyphs::Glyph::Check.render(crate::glyphs::active_profile(None));
    assert!(shown.contains("1/2 done"), "shown: {shown}");
    assert!(
        shown.contains(&format!("{check} TASK-100")),
        "shown: {shown}"
    );
    assert!(shown.contains("○ TASK-101"), "shown: {shown}");
}

#[test]
fn session_start_is_a_peek_with_command() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    let peek = server
        .tool_session_start(&json!({ "owns": "EPIC-27", "role": "implementer" }))
        .unwrap();
    assert!(
        peek.contains("aida session start --owns EPIC-27"),
        "peek: {peek}"
    );
    assert!(peek.contains("--role implementer"), "peek: {peek}");
    // Does NOT actually create a lease.
    assert!(list_leases(&server.project_root).is_empty());

    // Warns when the scope is already held.
    seed_lease(dir.path(), "eeeeeeee5555", "EPIC-27", "implementer");
    let warned = server
        .tool_session_start(&json!({ "owns": "EPIC-27" }))
        .unwrap();
    assert!(warned.contains("already held"), "warned: {warned}");

    // owns is required.
    let err = server
        .tool_session_start(&json!({}))
        .expect_err("owns required");
    assert!(err.contains("owns"), "err: {err}");
}

// trace:TASK-712 — POSIX single-quote rule.
#[test]
fn shell_quote_arg_quotes_metachars_and_passes_safe_words() {
    // Safe words pass through bare for readability.
    assert_eq!(shell_quote_arg("EPIC-27"), "EPIC-27");
    assert_eq!(shell_quote_arg("feature/foo_bar.v2"), "feature/foo_bar.v2");
    // Spaces / metacharacters get single-quoted.
    assert_eq!(shell_quote_arg("a b"), "'a b'");
    assert_eq!(shell_quote_arg("a;rm -rf b"), "'a;rm -rf b'");
    assert_eq!(shell_quote_arg("$(whoami)"), "'$(whoami)'");
    // Embedded single quote uses the '\'' escape.
    assert_eq!(shell_quote_arg("it's"), "'it'\\''s'");
    // Empty is quoted (not a safe word) so it stays a single empty arg.
    assert_eq!(shell_quote_arg(""), "''");
}

// trace:TASK-712 — the session_start peek must shell-quote a scope/branch
// containing spaces so the pasted command is a single correct argument.
#[test]
fn session_start_peek_quotes_unsafe_args() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let peek = server
        .tool_session_start(&json!({ "owns": "my scope", "branch": "a;b" }))
        .unwrap();
    assert!(
        peek.contains("--owns 'my scope'"),
        "scope must be quoted; peek: {peek}"
    );
    assert!(
        peek.contains("--branch 'a;b'"),
        "branch must be quoted; peek: {peek}"
    );
}

#[test]
fn session_end_is_a_peek_with_command() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    // No sessions.
    let none = server.tool_session_end(&json!({})).unwrap();
    assert!(none.contains("nothing to end"), "none: {none}");

    seed_lease(dir.path(), "ffffffff6666", "EPIC-27", "implementer");
    let peek = server.tool_session_end(&json!({})).unwrap();
    assert!(peek.contains("aida session end ffffffff"), "peek: {peek}");
    // Lease still present (peek did not remove it).
    assert!(!list_leases(&server.project_root).is_empty());

    // Resolve by spec/scope.
    let by_spec = server
        .tool_session_end(&json!({ "spec": "EPIC-27" }))
        .unwrap();
    assert!(by_spec.contains("ffffffff"), "by_spec: {by_spec}");

    // Ambiguous without selector once a second lease exists.
    seed_lease(dir.path(), "99999999aaaa", "EPIC-99", "implementer");
    let err = server
        .tool_session_end(&json!({}))
        .expect_err("ambiguous end");
    assert!(err.contains("multiple active sessions"), "err: {err}");
}

#[test]
fn session_tools_are_in_descriptors_and_classified() {
    let names: Vec<String> = tool_descriptors()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    for t in [
        "session_leases",
        "session_status",
        "session_manifest",
        "session_start",
        "session_end",
    ] {
        assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
        // No session tool may fall through to the Admin catch-all.
        assert_ne!(
            tool_min_profile(t),
            McpProfile::Admin,
            "{t} is unclassified"
        );
        // All session tools are read tier (the two peeks never mutate).
        assert_eq!(
            tool_min_profile(t),
            McpProfile::ReadOnly,
            "{t} should be read-only"
        );
    }
}

// =====================================================================
// STORY-534 (EPIC-27): role MCP tools
// =====================================================================

/// Write a minimal role TOML under `.aida/roles/<name>.toml` matching the
/// `LightRole` shape the role tools read. trace:EPIC-27
fn seed_role(dir: &Path, name: &str, purpose: &str) {
    let roles = dir.join(".aida").join("roles");
    std::fs::create_dir_all(&roles).unwrap();
    let body = format!(
        "name = \"{name}\"\n\
             purpose = \"{purpose}\"\n\
             created_at = \"2026-06-01T00:00:00Z\"\n\
             last_active_at = \"2026-06-07T00:00:00Z\"\n\
             working_directory = \"/nonexistent/wt-{name}\"\n\
             global = false\n\
             scope_tags = [\"inbox\"]\n\
             scope_status = \"draft\"\n"
    );
    std::fs::write(roles.join(format!("{name}.toml")), body).unwrap();
}

#[test]
fn role_list_lists_seeded_roles() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    // NOTE: list_light_roles also reads `~/.aida/roles/` (global roles),
    // which may be populated on the developer/CI machine, so we don't
    // assert emptiness — we assert the seeded *project* roles surface. The
    // empty-state branch is covered by the help text path, not here.
    // trace:EPIC-27
    seed_role(dir.path(), "proj-impl", "ships code");
    seed_role(dir.path(), "proj-adv", "strategy partner");
    let listed = server.tool_role_list(&json!({})).unwrap();
    assert!(listed.contains("proj-impl"), "listed: {listed}");
    assert!(listed.contains("proj-adv"), "listed: {listed}");
    assert!(listed.contains("ships code"), "listed: {listed}");
}

#[test]
fn role_show_renders_a_role_and_errors_on_unknown() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    seed_role(dir.path(), "reviewer", "guards the merge");

    let shown = server
        .tool_role_show(&json!({ "name": "reviewer" }))
        .unwrap();
    assert!(shown.contains("Role:        reviewer"), "shown: {shown}");
    assert!(shown.contains("guards the merge"), "shown: {shown}");
    assert!(shown.contains("tags=inbox"), "shown: {shown}");
    assert!(shown.contains("status=draft"), "shown: {shown}");

    // Unknown role errors.
    let err = server
        .tool_role_show(&json!({ "name": "nope" }))
        .expect_err("unknown role");
    assert!(err.contains("No such role"), "err: {err}");
}

#[test]
fn role_show_canonicalizes_dialog_to_advisor() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    // Legacy `dialog.toml` should resolve as the advisor role.
    let roles = dir.path().join(".aida").join("roles");
    std::fs::create_dir_all(&roles).unwrap();
    std::fs::write(
        roles.join("dialog.toml"),
        "name = \"dialog\"\nlast_active_at = \"2026-06-07T00:00:00Z\"\n",
    )
    .unwrap();

    let shown = server
        .tool_role_show(&json!({ "name": "advisor" }))
        .unwrap();
    assert!(shown.contains("Role:        advisor"), "shown: {shown}");
}

#[test]
fn role_enter_is_a_peek_with_command() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    seed_role(dir.path(), "implementer", "ships code");

    let peek = server
        .tool_role_enter(&json!({ "name": "implementer", "cd": true }))
        .unwrap();
    assert!(
        peek.contains("aida role enter implementer --cd"),
        "peek: {peek}"
    );
    assert!(peek.contains("ships code"), "peek: {peek}");

    // name is required.
    let err = server
        .tool_role_enter(&json!({}))
        .expect_err("name required");
    assert!(err.contains("name"), "err: {err}");

    // Unknown role errors.
    let unknown = server
        .tool_role_enter(&json!({ "name": "ghost" }))
        .expect_err("unknown role");
    assert!(unknown.contains("No such role"), "unknown: {unknown}");
}

#[test]
fn role_end_is_a_peek_with_command() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let peek = server.tool_role_end(&json!({})).unwrap();
    assert!(peek.contains("aida role end"), "peek: {peek}");
}

#[test]
fn role_tools_are_in_descriptors_and_classified() {
    let names: Vec<String> = tool_descriptors()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    for t in ["role_list", "role_show", "role_enter", "role_end"] {
        assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
        // No role tool may fall through to the Admin catch-all.
        assert_ne!(
            tool_min_profile(t),
            McpProfile::Admin,
            "{t} is unclassified"
        );
        // All role tools are read tier (the two peeks never mutate).
        assert_eq!(
            tool_min_profile(t),
            McpProfile::ReadOnly,
            "{t} should be read-only"
        );
    }
}

// =====================================================================
// STORY-536 (EPIC-27): workflow MCP tools
// =====================================================================

#[test]
fn workflow_tools_are_in_descriptors_and_classified() {
    let names: Vec<String> = tool_descriptors()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.get("name").unwrap().as_str().unwrap().to_string())
        .collect();
    for t in [
        "cache_status",
        "plan_verify",
        "plan_helpers",
        "ultraplan_assemble",
        "goal_derive",
        "status_unified",
        "usage_query",
        "db_sync",
        "fetch",
        "pull",
    ] {
        assert!(names.contains(&t.to_string()), "missing descriptor: {t}");
        // No workflow tool may fall through to the Admin catch-all, and
        // every one is read tier (the library mirrors are pure reads; the
        // three git verbs are metadata-only PEEKs). trace:EPIC-27
        assert_eq!(
            tool_min_profile(t),
            McpProfile::ReadOnly,
            "{t} should be read-only"
        );
    }
}

#[test]
fn cache_status_reports_freshness() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let out = server.tool_cache_status(&json!({})).unwrap();
    assert!(out.contains("Cache path:"), "out: {out}");
    assert!(out.contains("Cached requirements:"), "out: {out}");
    assert!(
        out.contains("Status:") && (out.contains("FRESH") || out.contains("STALE")),
        "out: {out}"
    );
}

#[test]
fn plan_verify_lints_a_plan_file() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    // Minimal plan missing required sections — must report and verdict.
    let plan = dir.path().join("plan.md");
    std::fs::write(&plan, "# Plan: STORY-X\n\nSome prose, no real sections.\n").unwrap();
    let out = server
        .tool_plan_verify(&json!({ "file": plan.to_str().unwrap() }))
        .unwrap();
    assert!(out.contains("Verifying plan:"), "out: {out}");
    assert!(out.contains("Verdict:"), "out: {out}");
    // A missing file is an error, not a panic.
    let err = server
        .tool_plan_verify(&json!({ "file": "does-not-exist.md" }))
        .unwrap_err();
    assert!(err.contains("could not read plan file"), "err: {err}");
}

#[test]
fn plan_helpers_handles_no_helpers() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let spec = seed_req(&server, "Helpers target");
    let out = server.tool_plan_helpers(&json!({ "spec": spec })).unwrap();
    // A lone seeded spec has no related helper-bearing specs.
    assert!(out.contains("No reusable helpers derived"), "out: {out}");
    // Unknown spec errors cleanly.
    let err = server
        .tool_plan_helpers(&json!({ "spec": "NOPE-999" }))
        .unwrap_err();
    assert!(!err.is_empty(), "err: {err}");
}

#[test]
fn ultraplan_assemble_builds_a_prompt() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    let spec = seed_req(&server, "Ultraplan target");
    let out = server
        .tool_ultraplan_assemble(&json!({ "spec": spec }))
        .unwrap();
    assert!(out.contains("Plan the implementation of"), "out: {out}");
    assert!(out.contains("## Acceptance criteria"), "out: {out}");
}

#[test]
fn goal_derive_composes_clauses_and_requires_an_axis() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    crate::forge::write_forge_config_provider(dir.path(), crate::forge::ForgeKind::GitHub).unwrap();
    let server = mk_git_server(dir.path());
    let out = server
        .tool_goal_derive(&json!({ "spec": "STORY-7", "pr": 42 }))
        .unwrap();
    assert!(out.starts_with("/goal "), "out: {out}");
    assert!(out.contains("verify each clause:"), "out: {out}");
    assert!(out.contains("aida show STORY-7"), "out: {out}");
    assert!(out.contains("gh pr view 42"), "out: {out}");
    // No axis → error.
    let err = server.tool_goal_derive(&json!({})).unwrap_err();
    assert!(err.contains("no condition flags"), "err: {err}");
}

#[test]
fn goal_derive_uses_gitlab_forge_vocabulary() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aida")).unwrap();
    std::fs::write(
        dir.path().join(".aida").join("config.toml"),
        "[forge]\nprovider = \"pure-git\"\n",
    )
    .unwrap();
    crate::forge::write_forge_config_provider(dir.path(), crate::forge::ForgeKind::GitLab).unwrap();
    let server = mk_git_server(dir.path());
    let out = server
        .tool_goal_derive(&json!({ "spec": "STORY-7", "pr": 42 }))
        .unwrap();
    assert!(out.contains("MR #42 is merged"), "out: {out}");
    assert!(out.contains("glab mr view 42"), "out: {out}");
    assert!(!out.contains("gh pr"), "out: {out}");
}

#[test]
fn status_unified_reports_counts_and_queue_depth() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    seed_req(&server, "A status spec");
    let out = server
        .tool_status_unified(&json!({ "user": "su-test" }))
        .unwrap();
    assert!(out.contains("Project status"), "out: {out}");
    assert!(out.contains("By status:"), "out: {out}");
    assert!(out.contains("Active sessions:"), "out: {out}");
    assert!(out.contains("Queue depth ('su-test')"), "out: {out}");
}

#[test]
fn status_unified_excludes_meta_from_count_matching_cli() {
    // BUG-717: the MCP status count must exclude standing-artifact /
    // stateless types (meta, folder, ...) exactly as `aida status` / `aida
    // list` do — otherwise the seeded META prompt-templates inflate the
    // "Total requirements" the MCP surface reports vs the CLI.
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    seed_req(&server, "A real work spec"); // type=task → counted
    server
        .tool_add_requirement(&json!({
            "title": "A META prompt template",
            "description": "standing artifact, not work",
            "type": "meta",
        }))
        .expect("seed meta add_requirement failed");
    let out = server
        .tool_status_unified(&json!({ "user": "su-test" }))
        .unwrap();
    // Only the task counts; the META spec is excluded (the BUG-717 bug was
    // it reading "Total requirements: 2").
    assert!(
        out.contains("Total requirements: 1"),
        "META must be excluded from the MCP status count: {out}"
    );
}

#[test]
fn usage_query_handles_empty_log_gracefully() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());
    // The local log may or may not exist in CI; the tool must never panic
    // and must return a string in both the empty and populated cases.
    let out = server.tool_usage_query(&json!({})).unwrap();
    assert!(out.contains("Usage:"), "out: {out}");
}

#[test]
fn git_verbs_peek_and_return_commands() {
    let dir = tempdir().unwrap();
    let server = mk_git_server(dir.path());

    let sync = server
        .tool_db_sync(&json!({ "pull": true, "push": true }))
        .unwrap();
    assert!(sync.contains("Run:"), "sync: {sync}");
    assert!(sync.contains("aida db sync --pull --push"), "sync: {sync}");
    // No legs → defaults to both.
    let sync2 = server.tool_db_sync(&json!({})).unwrap();
    assert!(
        sync2.contains("aida db sync --pull --push"),
        "sync2: {sync2}"
    );

    let fetch = server.tool_fetch(&json!({ "code_only": true })).unwrap();
    assert!(fetch.contains("aida fetch --code-only"), "fetch: {fetch}");

    let pull = server.tool_pull(&json!({ "store_only": true })).unwrap();
    assert!(pull.contains("aida pull --store-only"), "pull: {pull}");
}
